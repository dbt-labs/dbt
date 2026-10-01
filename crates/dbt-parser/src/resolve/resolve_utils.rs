use std::collections::BTreeMap;
use std::path::Path;

use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::error::FsError;
use dbt_common::fs_err;
use dbt_common::io_args::{
    ComputeArg, LOCAL_UNIT_TESTS_ENV, MULTI_ADAPTER_ENV, local_unit_tests_enabled,
    multi_adapter_enabled,
};
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_schemas::schemas::project::AdapterProjectConfig;
use dbt_schemas::state::ProfileAdapter;
use indexmap::IndexMap;

/// Validate the root project's `adapters:` block. Called once per run.
///
/// Keying by adapter type removes two checks by construction: a duplicate entry is
/// impossible in a map, and a key that is not an adapter type at all is rejected at
/// deserialization. What is left is an entry for an adapter *this* target does not
/// declare, which is only a warning — one project is commonly run against several
/// targets, so such an entry is not a mistake, but a stray one would otherwise do
/// nothing at all.
pub(crate) fn validate_adapter_project_configs(
    adapters: Option<&IndexMap<AdapterType, AdapterProjectConfig>>,
    target_adapters: &IndexMap<AdapterType, ProfileAdapter>,
) {
    let Some(adapters) = adapters else {
        return;
    };

    for adapter_type in adapters.keys() {
        if !target_adapters.contains_key(adapter_type) {
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!(
                    "dbt_project.yml configures adapter '{adapter_type}' under `adapters:`, but \
                     the active target does not declare it; the entry has no effect. Declared \
                     adapters are: {}",
                    target_adapters
                        .keys()
                        .map(|t| t.as_ref())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
    }
}

/// Normalizes hook key names in an unrendered config map, matching dbt-core's
/// `translate_hook_names` behavior (`context/context_config.py:235`):
/// `post_hook` → `post-hook`, `pre_hook` → `pre-hook`.
/// This applies to both project config and inline SQL config, since users may write
/// either spelling in `{{ config(post_hook=...) }}` calls.
pub(crate) fn normalize_hook_names(
    mut config: BTreeMap<String, dbt_yaml::Value>,
) -> BTreeMap<String, dbt_yaml::Value> {
    if let Some(v) = config.remove("post_hook") {
        config.insert("post-hook".to_string(), v);
    }
    if let Some(v) = config.remove("pre_hook") {
        config.insert("pre-hook".to_string(), v);
    }
    config
}

/// Extracts the `config:` subtree from a `dbt_yaml::Value` node into a flat
/// `BTreeMap`, returning `None` if the key is absent or not a mapping.
pub(crate) fn extract_config_map(
    value: &dbt_yaml::Value,
) -> Option<BTreeMap<String, dbt_yaml::Value>> {
    value
        .get("config")
        .and_then(|v| v.as_mapping())
        .map(|mapping| {
            mapping
                .iter()
                .filter_map(|(k, v)| k.as_str().map(|k| (k.to_string(), v.clone())))
                .collect()
        })
}

/// Coerces a hook config value into its flat list of entries: a sequence's items as-is, or a
/// single non-sequence value (e.g. a lone hook string) as a one-element list.
fn hook_value_entries(value: dbt_yaml::Value) -> Vec<dbt_yaml::Value> {
    match value {
        dbt_yaml::Value::Sequence(seq, _) => seq,
        other => vec![other],
    }
}

/// Combines two hook values into one, concatenating their entries (`existing` first). Hooks
/// accumulate across config sources rather than the later source replacing the earlier one —
/// matching the same "extend, don't replace" semantics [`default_hooks`] already applies on the
/// rendered-config path, so `unrendered_config` reflects every configured hook, not just the
/// most specific source's.
fn merge_hook_values(existing: dbt_yaml::Value, incoming: dbt_yaml::Value) -> dbt_yaml::Value {
    let mut entries = hook_value_entries(existing);
    entries.extend(hook_value_entries(incoming));
    dbt_yaml::Value::Sequence(entries, Default::default())
}

/// Merges `source`'s keys into `unrendered`. Ordinary keys overwrite (most specific source
/// wins); `pre-hook`/`post-hook` accumulate instead, since a resource can configure hooks at
/// more than one level (e.g. a schema.yml `post_hook` plus an inline SQL
/// `{{ config(post_hook=...) }}`) and dbt expects to run all of them, not just the last one
/// configured.
fn merge_config_source(
    unrendered: &mut BTreeMap<String, dbt_yaml::Value>,
    source: BTreeMap<String, dbt_yaml::Value>,
    normalize_hooks: bool,
) {
    for (key, value) in source {
        if normalize_hooks && (key == "pre-hook" || key == "post-hook") {
            if let Some(existing) = unrendered.remove(&key) {
                unrendered.insert(key, merge_hook_values(existing, value));
                continue;
            }
        }
        unrendered.insert(key, value);
    }
}

/// Deep-merges `source` into `destination`, transcribing `deep_merge_item` in dbt_common
/// `utils/dict.py:69-79`: mappings recurse, sequences concatenate with `source` first, everything
/// else `source` replaces. Spans come from whichever side supplied the value.
///
/// Where dbt-core raises or splats a string into characters, `source` wins instead — except a
/// sequence `source` over a scalar `destination`, which keeps the scalar as a one-element tail.
pub(crate) fn deep_merge_yaml(destination: &mut dbt_yaml::Value, source: &dbt_yaml::Value) {
    match source {
        dbt_yaml::Value::Mapping(source_map, _) if destination.is_mapping() => {
            let destination_map = destination
                .as_mapping_mut()
                .expect("destination is a mapping");
            for (key, source_value) in source_map.iter() {
                match destination_map.get_mut(key) {
                    Some(destination_value) => deep_merge_yaml(destination_value, source_value),
                    None => {
                        destination_map.insert(key.clone(), source_value.clone());
                    }
                }
            }
        }
        dbt_yaml::Value::Sequence(source_items, _) => {
            let mut merged = source_items.clone();
            match destination {
                dbt_yaml::Value::Sequence(destination_items, _) => {
                    merged.append(destination_items);
                    *destination_items = merged;
                }
                other => {
                    merged.push(other.clone());
                    *other = dbt_yaml::Value::Sequence(merged, source.span().clone());
                }
            }
        }
        _ => *destination = source.clone(),
    }
}

/// Applies `adapter_type`'s config-key alias map to one config source.
/// (`core/dbt/utils/utils.py:185-192`)
pub(crate) fn canonicalize_source_config_keys(
    adapter_type: AdapterType,
    cfg: BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
    dbt_adapter_core::config_aliases::canonicalize_config_keys(adapter_type, cfg).map_err(|dup| {
        let location = dbt_common::CodeLocationWithFile::from(dup.value_b.span().clone());
        fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => location,
            "Config keys `{}` and `{}` both resolve to `{}` for adapter '{}'; a project cannot \
             set the same underlying config key two different ways in the same place.",
            dup.key_a,
            dup.key_b,
            dup.canonical,
            adapter_type,
        )
    })
}

/// Builds `unrendered_config` by merging config sources in hierarchical order:
/// project < root < schema.yml < inline. Each source is canonicalized independently, before merging.
/// Ordinary keys use overwrite semantics (most specific source wins);
/// `pre-hook`/`post-hook` accumulate across sources instead (see [`merge_config_source`]).
///
/// Sources not applicable to a resource type should be passed as `None`.
/// `normalize_hooks` should be `true` only for resource types that support
/// `pre_hook`/`post_hook` (models, seeds, snapshots, tests).
pub(crate) fn build_unrendered_config(
    fqn: &[String],
    local: &crate::utils::RawProjectConfig,
    root: Option<&crate::utils::RawProjectConfig>,
    schema: Option<&BTreeMap<String, dbt_yaml::Value>>,
    inline: Option<&BTreeMap<String, dbt_yaml::Value>>,
    normalize_hooks: bool,
    adapter_type: AdapterType,
) -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
    let canonicalize =
        |cfg: BTreeMap<String, dbt_yaml::Value>| -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
            let cfg = canonicalize_source_config_keys(adapter_type, cfg)?;
            Ok(if normalize_hooks {
                normalize_hook_names(cfg)
            } else {
                cfg
            })
        };

    let mut unrendered = canonicalize(local.get_config_for_fqn(fqn).clone())?;

    if let Some(root_cfg) = root {
        merge_config_source(
            &mut unrendered,
            canonicalize(root_cfg.get_config_for_fqn(fqn).clone())?,
            normalize_hooks,
        );
    }
    if let Some(schema_cfg) = schema {
        merge_config_source(
            &mut unrendered,
            canonicalize(schema_cfg.clone())?,
            normalize_hooks,
        );
    }
    if let Some(inline_cfg) = inline {
        merge_config_source(
            &mut unrendered,
            canonicalize(inline_cfg.clone())?,
            normalize_hooks,
        );
    }

    Ok(unrendered)
}

/// Returns an error for resource names derived from filenames that contain spaces.
/// dbt does not allow spaces in resource names — this mirrors dbt-core's
/// `check_for_spaces_in_resource_names` validation.
pub(crate) fn err_resource_name_has_spaces(name: &str, path: &Path) -> Box<FsError> {
    fs_err!(
        code => ErrorCode::DbtYamlValidationError,
        loc => path.to_path_buf(),
        "Resource name '{}' contains spaces. Resource names cannot contain spaces. \
         Rename '{}' to remove any spaces.",
        name,
        path.display()
    )
}

/// Validates the merged `compute` config on a model / data_test / snapshot node. Currently
/// only `Remote` is supported for those node types; other variants are rejected at parse time
/// rather than mid-build. The set of accepted values will widen as local-compute support
/// for additional node types stabilizes. Unit test nodes use [`validate_unit_test_compute`]
/// instead.
pub(crate) fn validate_compute(compute: Option<ComputeArg>, path: &Path) -> FsResult<()> {
    match compute {
        None | Some(ComputeArg::Remote) => Ok(()),
        Some(other) => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "compute config currently only accepts 'remote'; got '{other}'",
        )),
    }
}

/// Unit tests can run on either on the `remote` warehouse or `sidecar`.
///
/// `sidecar` is gated behind [`LOCAL_UNIT_TESTS_ENV`]. The run-wide `--compute` flag is a
/// separate, older knob and is not covered by that gate.
pub(crate) fn validate_unit_test_compute(compute: Option<ComputeArg>, path: &Path) -> FsResult<()> {
    match compute {
        None | Some(ComputeArg::Remote) => Ok(()),
        Some(ComputeArg::Sidecar) if local_unit_tests_enabled() => Ok(()),
        Some(ComputeArg::Sidecar) => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "Running an individual unit test on local compute is experimental and not yet \
             supported. To use it, set the environment variable {LOCAL_UNIT_TESTS_ENV}=true. \
             Note that experimental features may be unstable and are not yet recommended for \
             production use.",
        )),
        Some(other) => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "unit_test compute config accepts 'remote', 'sidecar', or 'local'; got '{other}'",
        )),
    }
}

/// Gates a node's authored `+adapter` behind [`MULTI_ADAPTER_ENV`]. Any value is refused, not
/// just `lakecompute`; selecting a node's adapter at all is what is unreleased.
///
/// Pass the **authored** config field, never the resolved adapter. Data tests inherit from
/// their attached node and unit tests from their subject, so the resolved value is `Some` for
/// essentially every node in every project; checking it would reject them all.
pub(crate) fn validate_node_adapter(adapter: Option<AdapterType>, path: &Path) -> FsResult<()> {
    match adapter {
        None => Ok(()),
        Some(_) if multi_adapter_enabled() => Ok(()),
        Some(adapter) => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "Selecting a node's adapter with `adapter: {adapter}` is experimental and not yet \
             supported. To use it, set the environment variable {MULTI_ADAPTER_ENV}=true. \
             Note that experimental features may be unstable and are not yet recommended for \
             production use.",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::RawProjectConfig;
    use std::sync::Mutex;

    /// The gate predicates read process-wide state, so the gate tests below cannot run
    /// concurrently under a plain `cargo test` (nextest gives each test its own process).
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    /// Run `f` with `var` set, restoring the previous value afterwards.
    fn with_env_var<T>(var: &str, value: &str, f: impl FnOnce() -> T) -> T {
        let _lock = ENV_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var(var).ok();
        unsafe {
            #[allow(clippy::disallowed_methods)]
            std::env::set_var(var, value);
        }
        let result = f();
        unsafe {
            #[allow(clippy::disallowed_methods)]
            match previous {
                Some(previous) => std::env::set_var(var, previous),
                None => std::env::remove_var(var),
            }
        }
        result
    }

    #[test]
    fn an_authored_adapter_is_refused_unless_the_gate_is_set() {
        let path = Path::new("models/m.sql");

        // Not just `lakecompute`: selecting *any* adapter is what is unreleased.
        let err = validate_node_adapter(Some(AdapterType::Bigquery), path).unwrap_err();
        assert!(
            err.to_string().contains(MULTI_ADAPTER_ENV),
            "the refusal must name the variable that lifts it: {err}"
        );

        with_env_var(MULTI_ADAPTER_ENV, "true", || {
            assert!(validate_node_adapter(Some(AdapterType::LakeCompute), path).is_ok());
        });
    }

    #[test]
    fn a_node_without_an_authored_adapter_is_always_accepted() {
        assert!(validate_node_adapter(None, Path::new("models/m.sql")).is_ok());
    }

    #[test]
    fn a_unit_test_selecting_local_compute_is_refused_unless_the_gate_is_set() {
        let path = Path::new("models/unit_tests.yml");

        assert!(validate_unit_test_compute(None, path).is_ok());
        assert!(validate_unit_test_compute(Some(ComputeArg::Remote), path).is_ok());

        let err = validate_unit_test_compute(Some(ComputeArg::Sidecar), path).unwrap_err();
        assert!(
            err.to_string().contains(LOCAL_UNIT_TESTS_ENV),
            "the refusal must name the variable that lifts it: {err}"
        );

        with_env_var(LOCAL_UNIT_TESTS_ENV, "true", || {
            assert!(validate_unit_test_compute(Some(ComputeArg::Sidecar), path).is_ok());
        });
    }

    #[test]
    fn the_gate_does_not_widen_the_accepted_compute_values() {
        // The gate governs `sidecar` only; `inline` and `service` stay rejected.
        let path = Path::new("models/unit_tests.yml");
        with_env_var(LOCAL_UNIT_TESTS_ENV, "true", || {
            assert!(validate_unit_test_compute(Some(ComputeArg::Inline), path).is_err());
            assert!(validate_unit_test_compute(Some(ComputeArg::Service), path).is_err());
        });
    }

    fn config_map(pairs: &[(&str, &str)]) -> BTreeMap<String, dbt_yaml::Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), dbt_yaml::from_str(v).unwrap()))
            .collect()
    }

    fn hook_strings(value: &dbt_yaml::Value) -> Vec<String> {
        match value {
            dbt_yaml::Value::Sequence(seq, _) => seq
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect(),
            dbt_yaml::Value::String(s, _) => vec![s.clone()],
            other => panic!("expected a hook string or sequence, got {other:?}"),
        }
    }

    #[test]
    fn hooks_accumulate_across_config_sources_instead_of_overwriting() {
        // A resource can configure a `post_hook` at both the schema.yml level and the inline
        // SQL `{{ config(...) }}` level. Both should run (matching the rendered-config path's
        // `default_hooks`, which already extends rather than replaces), so `unrendered_config`
        // must reflect both, not just the more specific (inline) source.
        let local = RawProjectConfig::empty();
        let schema = config_map(&[("post_hook", "\"apply masking\"")]);
        let inline = config_map(&[("post_hook", "\"delete rows\"")]);

        let unrendered = build_unrendered_config(
            &[],
            &local,
            None,
            Some(&schema),
            Some(&inline),
            true,
            AdapterType::Snowflake,
        )
        .unwrap();

        let post_hook = unrendered.get("post-hook").expect("expected post-hook key");
        assert_eq!(
            hook_strings(post_hook),
            vec!["apply masking".to_string(), "delete rows".to_string()]
        );
    }

    #[test]
    fn non_hook_keys_still_use_overwrite_semantics() {
        // Ordinary (non-hook) keys keep "most specific source wins" — only hooks accumulate.
        let local = RawProjectConfig::empty();
        let schema = config_map(&[("materialized", "\"view\"")]);
        let inline = config_map(&[("materialized", "\"table\"")]);

        let unrendered = build_unrendered_config(
            &[],
            &local,
            None,
            Some(&schema),
            Some(&inline),
            true,
            AdapterType::Snowflake,
        )
        .unwrap();

        assert_eq!(
            unrendered.get("materialized").and_then(|v| v.as_str()),
            Some("table")
        );
    }

    #[test]
    fn single_source_hook_is_not_wrapped_unnecessarily() {
        // When only one source configures a hook, no merge occurs -- confirms the accumulation
        // path doesn't kick in (and doesn't panic) when there's nothing to merge with.
        let local = RawProjectConfig::empty();
        let inline = config_map(&[("post_hook", "\"delete rows\"")]);

        let unrendered = build_unrendered_config(
            &[],
            &local,
            None,
            None,
            Some(&inline),
            true,
            AdapterType::Snowflake,
        )
        .unwrap();

        assert_eq!(
            unrendered.get("post-hook").and_then(|v| v.as_str()),
            Some("delete rows")
        );
    }

    #[test]
    fn hook_accumulation_does_not_apply_when_normalize_hooks_is_false() {
        // `normalize_hooks=false` is used for resource types that don't support hooks (e.g.
        // exposures); a `post_hook`-named key there is just an ordinary key, not special-cased.
        let local = RawProjectConfig::empty();
        let schema = config_map(&[("post_hook", "\"a\"")]);
        let inline = config_map(&[("post_hook", "\"b\"")]);

        let unrendered = build_unrendered_config(
            &[],
            &local,
            None,
            Some(&schema),
            Some(&inline),
            false,
            AdapterType::Snowflake,
        )
        .unwrap();

        assert_eq!(
            unrendered.get("post_hook").and_then(|v| v.as_str()),
            Some("b")
        );
        assert!(!unrendered.contains_key("post-hook"));
    }

    fn raw_project_config(pairs: &[(&str, &str)]) -> RawProjectConfig {
        RawProjectConfig {
            config: config_map(pairs),
            children: BTreeMap::new(),
        }
    }

    #[test]
    fn databricks_catalog_alias_is_canonicalized_to_database() {
        let local = raw_project_config(&[("catalog", "\"my_catalog\"")]);

        let unrendered =
            build_unrendered_config(&[], &local, None, None, None, true, AdapterType::Databricks)
                .unwrap();

        assert_eq!(
            unrendered.get("database").and_then(|v| v.as_str()),
            Some("my_catalog")
        );
        assert!(!unrendered.contains_key("catalog"));
    }

    /// The alias map is gated on adapter type, so the same `+catalog:` is an inert extra
    /// config key on an adapter with no such alias.
    #[test]
    fn catalog_key_is_untouched_on_an_adapter_with_no_alias_map() {
        let local = raw_project_config(&[("catalog", "\"my_catalog\"")]);

        let unrendered =
            build_unrendered_config(&[], &local, None, None, None, true, AdapterType::Snowflake)
                .unwrap();

        assert_eq!(
            unrendered.get("catalog").and_then(|v| v.as_str()),
            Some("my_catalog")
        );
        assert!(!unrendered.contains_key("database"));
    }

    /// `catalog_name` is a distinct, real config with its own dbt-core counterpart and must
    /// never be swept up by the `catalog` → `database` alias.
    #[test]
    fn catalog_name_is_never_aliased() {
        for adapter_type in [AdapterType::Databricks, AdapterType::Snowflake] {
            let local = raw_project_config(&[("catalog_name", "\"cat\"")]);

            let unrendered =
                build_unrendered_config(&[], &local, None, None, None, true, adapter_type).unwrap();

            assert_eq!(
                unrendered.get("catalog_name").and_then(|v| v.as_str()),
                Some("cat"),
                "{adapter_type:?}"
            );
        }
    }

    /// Two keys in the same config source resolving to the same canonical key is an error,
    /// mirroring dbt-core's `DuplicateAliasError`.
    #[test]
    fn duplicate_alias_and_canonical_key_in_one_source_errors() {
        let local = raw_project_config(&[("catalog", "\"a\""), ("database", "\"b\"")]);

        let err =
            build_unrendered_config(&[], &local, None, None, None, true, AdapterType::Databricks)
                .expect_err(
                    "catalog and database both resolve to database and must not silently pick one",
                );

        let message = err.to_string();
        assert!(message.contains("catalog"), "{message}");
        assert!(message.contains("database"), "{message}");
    }

    /// Canonicalization runs per source, before layering, so a less specific source's
    /// alias spelling does not shadow ordinary precedence -- a model-level `database:` still
    /// wins over a project-level `+catalog:`.
    #[test]
    fn model_level_canonical_key_wins_over_project_level_alias() {
        let local = raw_project_config(&[("catalog", "\"project_catalog\"")]);
        let inline = config_map(&[("database", "\"model_database\"")]);

        let unrendered = build_unrendered_config(
            &[],
            &local,
            None,
            None,
            Some(&inline),
            true,
            AdapterType::Databricks,
        )
        .unwrap();

        assert_eq!(
            unrendered.get("database").and_then(|v| v.as_str()),
            Some("model_database")
        );
        assert!(!unrendered.contains_key("catalog"));
    }

    /// https://github.com/dbt-labs/fs/pull/13752#discussion_r3872848766 -- a `+catalog:` alias
    /// at a parent `dbt_project.yml` subtree level and a `+database:` canonical spelling at a
    /// nested child level. dbt-mantle (`078260e46`, `context/context_config.py:120-127,222`,
    /// `utils/utils.py:258` `fqn_search`) translates each hierarchy level's own dict through
    /// `translate_aliases` *before* folding it into the accumulating typed result one level at a
    /// time, so this is ordinary override precedence (child's `database` wins) and never a
    /// `DuplicateAliasError` -- that error only fires when the *same* level dict has two
    /// colliding keys.
    ///
    /// `RawProjectConfig` (`crate::utils::recur_raw_project_config`) used to pre-merge every
    /// `dbt_project.yml` subtree level into one raw dict via plain key overwrite
    /// (`merge_raw_config_mappings`), before `build_unrendered_config` ever canonicalized
    /// anything. So the leaf's merged dict carried both `catalog` (from the parent level) and
    /// `database` (from this level) as two distinct keys, and canonicalizing that combined dict
    /// as if it were one config source raised a spurious `DuplicateAliasKey` -- exactly the
    /// "duplicate issue" the review comment predicted, and not one of D3's documented axes
    /// (cross-*source* canonicalize-before-merge, not cross-*level* within one source).
    /// `merge_raw_config_mappings` now canonicalizes each level's own keys before folding them
    /// into the (already-canonical) accumulated parent, closing that gap.
    #[test]
    fn alias_at_parent_level_and_canonical_key_at_child_level_does_not_error() {
        let mapping = yaml(
            r#"
my_project:
  "+catalog": parent_catalog
  staging:
    "+database": staging_database
"#,
        );
        let tree = crate::utils::recur_raw_project_config(
            mapping.as_mapping().unwrap(),
            &BTreeMap::new(),
            AdapterType::Databricks,
        )
        .expect(
            "dbt-core translates per dbt_project.yml level before merging, so a parent-level \
             alias and a child-level canonical spelling never collide",
        );

        let fqn = vec!["my_project".to_string(), "staging".to_string()];

        // Each level is canonicalized before merging, so the leaf's merged dict already carries
        // only the canonical spelling -- the parent's `catalog` was renamed to `database` before
        // the child's own `database` overwrote it, never surviving as a second distinct key.
        let merged = tree.get_config_for_fqn(&fqn);
        assert_eq!(
            merged.get("database").and_then(|v| v.as_str()),
            Some("staging_database")
        );
        assert!(!merged.contains_key("catalog"));

        let unrendered =
            build_unrendered_config(&fqn, &tree, None, None, None, true, AdapterType::Databricks)
                .unwrap();

        assert_eq!(
            unrendered.get("database").and_then(|v| v.as_str()),
            Some("staging_database")
        );
        assert!(!unrendered.contains_key("catalog"));
    }

    /// A genuine same-level duplicate (one `dbt_project.yml` subtree writes both spellings at
    /// once) must still error -- the per-level canonicalization the fix above adds must check at
    /// the same granularity dbt-core does, not disable the check entirely.
    #[test]
    fn alias_and_canonical_key_in_the_same_dbt_project_level_still_errors() {
        let mapping = yaml(
            r#"
my_project:
  "+catalog": a
  "+database": b
"#,
        );

        let err = crate::utils::recur_raw_project_config(
            mapping.as_mapping().unwrap(),
            &BTreeMap::new(),
            AdapterType::Databricks,
        )
        .expect_err("catalog and database both resolve to database at the same level");

        let message = err.to_string();
        assert!(message.contains("catalog"), "{message}");
        assert!(message.contains("database"), "{message}");
    }

    fn yaml(text: &str) -> dbt_yaml::Value {
        dbt_yaml::from_str(text).unwrap()
    }

    /// Deep-merges two YAML documents given as source text. Parsing rather than hand-constructing
    /// the values keeps the spans real.
    fn deep_merged(destination: &str, source: &str) -> dbt_yaml::Value {
        let mut destination = yaml(destination);
        deep_merge_yaml(&mut destination, &yaml(source));
        destination
    }

    #[test]
    fn deep_merge_recurses_into_nested_mappings() {
        // A mapping set at both levels merges key-wise instead of the source replacing it.
        assert_eq!(
            deep_merged(
                "persist_docs:\n  relation: true\n",
                "persist_docs:\n  columns: true\n",
            ),
            yaml("persist_docs:\n  relation: true\n  columns: true\n"),
        );
    }

    #[test]
    fn deep_merge_concatenates_sequences_with_source_first() {
        // `list(source) + list(destination)`: source items first. Not a union, not a replace.
        assert_eq!(
            deep_merged("tags: [model_tag]\n", "tags: [version_tag]\n"),
            yaml("tags: [version_tag, model_tag]\n"),
        );
    }

    #[test]
    fn deep_merge_replaces_other_values_with_source() {
        // Note the asymmetry: a scalar source over a sequence destination replaces, not appends.
        assert_eq!(
            deep_merged("alias: model_alias\n", "alias: version_alias\n"),
            yaml("alias: version_alias\n"),
        );
        assert_eq!(
            deep_merged("tags: [model_tag]\n", "tags: version_tag\n"),
            yaml("tags: version_tag\n"),
        );
    }

    #[test]
    fn deep_merge_handles_keys_present_on_only_one_side() {
        // Source-only keys are added; destination-only keys survive untouched.
        assert_eq!(
            deep_merged("alias: model_alias\n", "materialized: table\n"),
            yaml("alias: model_alias\nmaterialized: table\n"),
        );
        // ... including when the destination has no sub-mapping to recurse into.
        assert_eq!(
            deep_merged("alias: model_alias\n", "persist_docs:\n  columns: true\n"),
            yaml("alias: model_alias\npersist_docs:\n  columns: true\n"),
        );
    }

    #[test]
    fn deep_merge_lets_source_win_over_a_type_mismatch() {
        // dbt-core raises on mapping-over-scalar; we let the source win in both directions.
        assert_eq!(
            deep_merged("persist_docs: true\n", "persist_docs:\n  columns: true\n"),
            yaml("persist_docs:\n  columns: true\n"),
        );
        assert_eq!(
            deep_merged("persist_docs:\n  relation: true\n", "persist_docs: false\n"),
            yaml("persist_docs: false\n"),
        );
    }

    #[test]
    fn deep_merge_coerces_a_scalar_destination_under_a_sequence_source() {
        // A lone hook string plus a hook list: keep both, source first.
        assert_eq!(
            deep_merged("pre-hook: model_hook\n", "pre-hook: [version_hook]\n"),
            yaml("pre-hook: [version_hook, model_hook]\n"),
        );
    }

    #[test]
    fn deep_merge_preserves_spans_from_the_supplying_side() {
        // Parse errors must still point at the line the user authored.
        let mut destination = yaml("alias: model_alias\npersist_docs:\n  relation: true\n");
        let source = yaml("persist_docs:\n  columns: true\ntags: [version_tag]\n");
        deep_merge_yaml(&mut destination, &source);

        let line_of = |path: &[&str]| {
            let mut value = &destination;
            for key in path {
                value = value.get(*key).expect("expected key to be present");
            }
            value.span().start.line
        };

        // Untouched destination values keep their line; source-supplied values carry the source's.
        assert_eq!(line_of(&["persist_docs", "relation"]), 3);
        assert_eq!(line_of(&["persist_docs", "columns"]), 2);
        assert_eq!(line_of(&["tags"]), 3);
        // The merged container keeps the destination's line (a mapping's span starts at its
        // first entry, hence 3 rather than the key's 2).
        assert_eq!(line_of(&["persist_docs"]), 3);
    }
}

#[cfg(test)]
mod adapter_quoting_tests {
    use super::*;
    // Lives in dbt-schemas now, alongside its only caller
    // (`build_root_project_configs`); these tests still cover it from here.
    use dbt_schemas::schemas::common::DbtQuoting;
    use dbt_schemas::schemas::project::authored_quoting_per_adapter;

    fn quoting(database: bool, schema: bool, identifier: bool) -> DbtQuoting {
        DbtQuoting {
            database: Some(database),
            schema: Some(schema),
            identifier: Some(identifier),
            snowflake_ignore_case: None,
        }
    }

    fn adapters_fixture() -> IndexMap<AdapterType, ProfileAdapter> {
        use dbt_schemas::schemas::profiles::DbConfig;
        IndexMap::from(
            [
                DbConfig::Snowflake(
                    Box::<dbt_schemas::schemas::profiles::SnowflakeDbConfig>::default(),
                ),
                DbConfig::LakeCompute(
                    Box::<dbt_schemas::schemas::profiles::LakeComputeConfig>::default(),
                ),
            ]
            .map(|config| (config.adapter_type(), ProfileAdapter::single(config))),
        )
    }

    /// One `adapters:` entry, keyed by type.
    fn entry(
        adapter_type: AdapterType,
        quoting: Option<DbtQuoting>,
    ) -> IndexMap<AdapterType, AdapterProjectConfig> {
        IndexMap::from([(adapter_type, AdapterProjectConfig { quoting })])
    }

    /// The rule the chain exists to express: the top-level `quoting:` block
    /// configures the *default* adapter and nothing else. A node on `lake_compute` gets
    /// only `lake_compute`'s own entry, so it is free to differ without every model
    /// having to say so.
    #[test]
    fn top_level_quoting_reaches_only_the_default_adapter() {
        let top_level = Some(quoting(false, false, false));
        let adapters = entry(AdapterType::LakeCompute, Some(quoting(true, true, true)));

        let per_adapter = authored_quoting_per_adapter(
            Some(&adapters),
            &adapters_fixture(),
            AdapterType::Snowflake,
            top_level,
        );

        assert_eq!(
            per_adapter[&AdapterType::Snowflake],
            quoting(false, false, false),
            "the default adapter takes the top-level block"
        );
        assert_eq!(
            per_adapter[&AdapterType::LakeCompute],
            quoting(true, true, true),
            "a non-default adapter takes only its own entry"
        );
    }

    /// A declared adapter with no `adapters:` entry contributes nothing, so the
    /// node falls straight through to its adapter type's default. Without this
    /// the map would be missing the key and the layer would be skipped silently
    /// either way -- the test pins that they agree.
    #[test]
    fn a_declared_adapter_without_an_entry_contributes_nothing() {
        let per_adapter =
            authored_quoting_per_adapter(None, &adapters_fixture(), AdapterType::Snowflake, None);

        assert_eq!(per_adapter.len(), 2, "every declared adapter gets a key");
        assert_eq!(
            per_adapter[&AdapterType::LakeCompute],
            DbtQuoting::default()
        );
        assert_eq!(per_adapter[&AdapterType::Snowflake], DbtQuoting::default());
    }

    /// The default adapter may carry its own entry, which wins over the
    /// top-level block field-wise -- more specific, same file.
    #[test]
    fn the_default_adapters_own_entry_beats_the_top_level_block() {
        let top_level = Some(quoting(false, false, false));
        let adapters = entry(
            AdapterType::Snowflake,
            Some(DbtQuoting {
                identifier: Some(true),
                ..Default::default()
            }),
        );

        let per_adapter = authored_quoting_per_adapter(
            Some(&adapters),
            &adapters_fixture(),
            AdapterType::Snowflake,
            top_level,
        );

        let resolved = per_adapter[&AdapterType::Snowflake];
        assert_eq!(resolved.identifier, Some(true), "the entry wins");
        assert_eq!(
            resolved.database,
            Some(false),
            "fields the entry leaves unset still come from the top-level block"
        );
    }

    /// Only `snowflake_ignore_case` set on a layer must survive. `default_to`
    /// drops that field, which is why the layering uses `filled_from`.
    #[test]
    fn snowflake_ignore_case_survives_layering() {
        let adapters = entry(
            AdapterType::LakeCompute,
            Some(DbtQuoting {
                snowflake_ignore_case: Some(true),
                ..Default::default()
            }),
        );

        let per_adapter = authored_quoting_per_adapter(
            Some(&adapters),
            &adapters_fixture(),
            AdapterType::Snowflake,
            None,
        );

        assert_eq!(
            per_adapter[&AdapterType::LakeCompute].snowflake_ignore_case,
            Some(true)
        );
    }

    /// A target the project was not written for is a normal thing to run against,
    /// so an entry this target cannot use warns rather than failing. Duplicate
    /// entries need no test: the block is a map, so they cannot be expressed.
    #[test]
    fn an_entry_for_an_undeclared_adapter_is_accepted() {
        let adapters = entry(AdapterType::Redshift, Some(quoting(true, true, true)));
        validate_adapter_project_configs(Some(&adapters), &adapters_fixture());
    }
}
