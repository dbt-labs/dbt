//! Module containing the entrypoint for the resolve phase.
use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::resolve::resolve_utils::{canonicalize_source_config_keys, extract_config_map};
use crate::utils::{extract_resource_config_from_raw_project, get_node_fqn};
use crate::validation::check_node_static_analysis;

use dbt_adapter::load_catalogs;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, FsResult, err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::serde::{Omissible, into_typed_with_jinja};
use dbt_jinja_utils::utils::generate_relation_name;
use dbt_schemas::schemas::common::{
    DbtChecksum, DbtMaterialization, DbtQuoting, FreshnessDefinition, FreshnessRules,
    NodeDependsOn, normalize_quoting,
};
use dbt_schemas::schemas::dbt_catalogs::LoadedCatalogs;
use dbt_schemas::schemas::dbt_column::process_columns;
use dbt_schemas::schemas::project::{ResolvableConfig, SourceConfig, Tags};
use dbt_schemas::schemas::properties::{SourceProperties, Tables, TablesConfig};
use dbt_schemas::schemas::relations::default_dbt_quoting_for;
use dbt_schemas::schemas::serde::strip_one_trailing_newline_at_keys;
use dbt_schemas::schemas::{CommonAttributes, DbtSource, DbtSourceAttr, NodeBaseAttributes};
use dbt_schemas::state::{DbtPackage, GenericTestAsset, ModelStatus, NodeResolverTracker};
use minijinja::Value as MinijinjaValue;
use regex::Regex;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;
use super::resolve_tests::persist_generic_data_tests::{
    TestableNodeTrait, TestableTable, extract_test_unrendered_configs,
};

fn merge_raw_time_threshold(
    merged_freshness: &mut dbt_yaml::Mapping,
    field: &str,
    freshness: &dbt_yaml::Mapping,
) {
    let str_to_yml = |s: &str| dbt_yaml::Value::string(s.to_string());

    let default_threshold = || {
        dbt_yaml::Mapping::from_iter([
            (str_to_yml("count"), dbt_yaml::Value::null()),
            (str_to_yml("period"), dbt_yaml::Value::null()),
        ])
    };

    let merge_yml = |dst: &mut dbt_yaml::Mapping, src: &dbt_yaml::Mapping| {
        for (key, value) in src.iter() {
            dst.insert(key.clone(), value.clone());
        }
    };

    match (merged_freshness.get(field), freshness.get(field)) {
        (None, None) => {
            merged_freshness.insert(
                str_to_yml(field),
                dbt_yaml::Value::mapping(default_threshold()),
            );
        }
        // TODO: This might be a state:modified bug in general. Core uses Python truthiness to determine
        // whether a Time threshold is "set" — Time(count=None, period=None).__bool__() returns False,
        // so explicit null values are treated identically to not setting the field at all, and the
        // base value is preserved. We currently treat explicit nulls as overrides, which diverges.
        // It does appear that explicitly setting freshness to null -- not a dict of nulls -- clears the freshness data.
        // Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/sources.py#L518-L526
        // Carry original freshness through.
        (Some(_), None) => (),
        // We can always assume that the values are mappings. Fusion and Core both fail if the freshness config isn't a dict.
        (None, Some(new)) => {
            let mut threshold = default_threshold();
            if let Some(new_mapping) = new.as_mapping() {
                merge_yml(&mut threshold, new_mapping);
            }
            merged_freshness.insert(str_to_yml(field), dbt_yaml::Value::mapping(threshold));
        }
        (Some(old), Some(new)) => {
            let mut threshold = default_threshold();
            if let Some(old_mapping) = old.as_mapping() {
                merge_yml(&mut threshold, old_mapping);
            }
            if let Some(new_mapping) = new.as_mapping() {
                merge_yml(&mut threshold, new_mapping);
            }
            merged_freshness.insert(str_to_yml(field), dbt_yaml::Value::mapping(threshold));
        }
    };
}

fn build_source_unrendered_config(
    fqn: &[String],
    raw_local_project_config: &crate::utils::RawProjectConfig,
    raw_root_project_models_cfg: Option<&crate::utils::RawProjectConfig>,
    raw_schema_yml_config: Option<BTreeMap<String, dbt_yaml::Value>>,
    raw_table_yml_config: Option<BTreeMap<String, dbt_yaml::Value>>,
    adapter_type: AdapterType,
) -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
    let mut unrendered = BTreeMap::new();

    // Core unconditionally pre-populates these fields before merging schema.yml config,
    // so they always appear in unrendered_config even when not set.
    // Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/sources.py#L292
    unrendered.insert("loaded_at_field".to_string(), dbt_yaml::Value::null());
    unrendered.insert("loaded_at_query".to_string(), dbt_yaml::Value::null());

    // Merge configs in hierarchical order: project < root < schema.config < table.config
    // For most keys, we completely overwrite the previous value. Each source's config-key
    // aliases are canonicalized, to be consistent during merging.
    unrendered.extend(canonicalize_source_config_keys(
        adapter_type,
        raw_local_project_config.get_config_for_fqn(fqn).clone(),
    )?);

    if let Some(root_cfg) = raw_root_project_models_cfg {
        unrendered.extend(canonicalize_source_config_keys(
            adapter_type,
            root_cfg.get_config_for_fqn(fqn).clone(),
        )?);
    }
    if let Some(schema_cfg) = raw_schema_yml_config.as_ref() {
        unrendered.extend(canonicalize_source_config_keys(
            adapter_type,
            schema_cfg.clone(),
        )?);
    }
    if let Some(table_cfg) = raw_table_yml_config.as_ref() {
        unrendered.extend(canonicalize_source_config_keys(
            adapter_type,
            table_cfg.clone(),
        )?);
    }

    // Special precedence config: meta, tags, and freshness are merged across source and table levels,
    // not simply overwritten. See `calculate_*_from_raw_target`.
    // Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/sources.py#L317-L322
    let mut merged_meta = dbt_yaml::Mapping::new();
    let mut merged_tags = std::collections::BTreeSet::new();
    let mut merged_freshness = dbt_yaml::Mapping::new();
    for cfg in [
        raw_schema_yml_config.as_ref(),
        raw_table_yml_config.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(meta) = cfg.get("meta").and_then(|v| v.as_mapping()) {
            for (k, v) in meta.iter() {
                merged_meta.insert(k.clone(), v.clone());
            }
        }
        if let Some(tags) = cfg.get("tags").and_then(|v| v.as_sequence()) {
            for t in tags {
                if let Some(s) = t.as_str() {
                    merged_tags.insert(s.to_string());
                }
            }
        }
        // We can't just call merge_freshness here because these are unrendered values, so
        // we just have to hardcode the fields of the freshness definition.
        // Reference: https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/sources.py#L383
        if let Some(freshness) = cfg.get("freshness").and_then(|v| v.as_mapping()) {
            merge_raw_time_threshold(&mut merged_freshness, "warn_after", freshness);
            merge_raw_time_threshold(&mut merged_freshness, "error_after", freshness);
            // filter: last non-null wins
            if let Some(f) = freshness.get(dbt_yaml::Value::string("filter".to_string())) {
                if !f.is_null() {
                    merged_freshness
                        .insert(dbt_yaml::Value::string("filter".to_string()), f.clone());
                }
            } else {
                merged_freshness.insert(
                    dbt_yaml::Value::string("filter".to_string()),
                    dbt_yaml::Value::null(),
                );
            }
        }
    }
    let tags_seq = merged_tags
        .into_iter()
        .map(dbt_yaml::Value::string)
        .collect::<Vec<_>>();
    unrendered.insert("meta".to_string(), dbt_yaml::Value::mapping(merged_meta));
    unrendered.insert("tags".to_string(), dbt_yaml::Value::sequence(tags_seq));
    if !merged_freshness.is_empty() {
        unrendered.insert(
            "freshness".to_string(),
            dbt_yaml::Value::mapping(merged_freshness),
        );
    }

    Ok(unrendered)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn resolve_sources(
    arg: &ResolveArgs,
    package: &DbtPackage,
    root_package_name: &str,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    source_properties: BTreeMap<(String, String), MinimalPropertiesEntry>,
    database: &str,
    adapter_type: AdapterType,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    jinja_env: &JinjaEnv,
    collected_generic_tests: &mut Vec<GenericTestAsset>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    node_resolver: &mut NodeResolver,
) -> FsResult<(
    HashMap<String, Arc<DbtSource>>,
    HashMap<String, Arc<DbtSource>>,
)> {
    let io_args = &arg.io;
    let mut sources: HashMap<String, Arc<DbtSource>> = HashMap::new();
    let mut disabled_sources: HashMap<String, Arc<DbtSource>> = HashMap::new();
    let package_name = package.dbt_project.name.as_ref();

    let dependency_package_name = if package.dbt_project.name != root_package_name {
        Some(package.dbt_project.name.as_str())
    } else {
        None
    };

    let is_dependency = dependency_package_name.is_some();
    // Best-effort raw parse of the root project's `sources:` subtree, used only to hydrate
    // dependency package nodes' `unrendered_config` with root overrides (preserving Jinja).
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "sources",
        adapter_type,
    )?;
    let raw_root_project_models_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "sources",
            adapter_type,
        )?)
    } else {
        None
    };

    let special_chars = Regex::new(r"[^a-zA-Z0-9_]").unwrap();

    // Sources use adapter-specific quoting defaults, NOT project-level quoting.
    // The defaults are only folded in below to drive SQL generation; the manifest
    // serializes the raw user-supplied merge via `user_quoting`.
    // https://docs.getdbt.com/reference/resource-properties/quoting
    let source_default_quoting = default_dbt_quoting_for(adapter_type);

    // See the equivalent model-side check in resolve_models.rs: a catalog_name
    // with no match would otherwise silently materialize/read against the
    // default catalog and fail much later, inside the query, with a
    // confusing backend error instead of a clear one here.
    let catalogs = load_catalogs::fetch_catalogs();
    let use_catalogs_v2 = load_catalogs::fetch_use_catalogs_v2();
    let catalogs_state = match catalogs.as_deref() {
        Some(c) if use_catalogs_v2 => LoadedCatalogs::V2(c),
        Some(c) => LoadedCatalogs::V1(c),
        None => LoadedCatalogs::None,
    };

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.sources.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.sources,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?
    .with_resolve_defaults((
        arg.static_analysis.unwrap_or_default(),
        root_package.dbt_project.sync.clone(),
    ));
    for ((source_name, table_name), mpe) in source_properties.into_iter() {
        // Extract raw (unrendered) database and schema from the YAML before Jinja rendering.
        // These preserve Jinja templates like `{{ env_var('DBT_ENV') }}` for state comparisons.
        let unrendered_database = mpe
            .schema_value
            .get("database")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let unrendered_schema = mpe
            .schema_value
            .get("schema")
            .and_then(|v| v.as_str())
            .map(str::to_string);

        // Extract the rest of the raw properties YAML for merging into the unrendered config.
        // This comes in two places: nested under each source, and nested under `tables:`
        let raw_schema_yml_config = extract_config_map(&mpe.schema_value);
        let raw_table_yml_config = mpe.table_value.as_ref().and_then(extract_config_map);
        // Capture raw (unrendered) schema.yml test config before `table_value` is moved into
        // the typed `table` below; consumed by `persist` for generic tests' unrendered_config.
        let raw_table_test_configs = mpe
            .table_value
            .as_ref()
            .map(extract_test_unrendered_configs)
            .unwrap_or_default();

        // Strip before Jinja renders, not after; `unrendered_*` above deliberately keeps the
        // raw newline, matching a Core-produced state manifest.
        let mut schema_value = mpe.schema_value;
        strip_one_trailing_newline_at_keys(
            &mut schema_value,
            &["name", "schema", "database", "catalog"],
        );

        let mut source: SourceProperties = into_typed_with_jinja(
            schema_value,
            false,
            jinja_env,
            base_ctx,
            &[],
            dependency_package_name,
            true,
        )?;
        // Canonicalize Databricks' `catalog` alias into `database`, mirroring dbt-core's
        // `credentials.translate_aliases` (D1: gated on adapter type, unlike the ungated fold
        // this replaces). `database` -- an explicit value already set on this same source --
        // takes precedence, matching ordinary same-source-dict alias precedence.
        //
        // TODO: resove divergence with Mantle, depending on [https://github.com/dbt-labs/dbt-core/issues/16108].
        // Mantle recurses into every nested mapping and sequence, possibly unintentionally.
        // We here only canonicalize the top-level keys of the source.
        if matches!(adapter_type, AdapterType::Databricks)
            && source.database.is_none()
            && let Some(catalog) = source.catalog.take()
        {
            source.database = Some(catalog);
        }

        let mut table_value = mpe.table_value.unwrap();
        strip_one_trailing_newline_at_keys(&mut table_value, &["name", "identifier"]);

        let table: Tables = into_typed_with_jinja(
            table_value,
            false,
            jinja_env,
            base_ctx,
            &[],
            dependency_package_name,
            true,
        )?;

        let fqn = get_node_fqn(
            package_name,
            mpe.relative_path.clone(),
            vec![source_name.to_owned(), table_name.to_owned()],
            &package.dbt_project.all_source_paths(),
        );

        let normalized_table_name = special_chars.replace_all(&table_name, "__");
        let unique_id = format!(
            "source.{}.{}.{}",
            &package_name, source_name, &normalized_table_name
        );

        let table_config = table.config.clone().unwrap_or_default();

        // Pre-merge source-level and table-level schema.yml configs before passing to the
        // resolver. This ensures root overlay (applied inside try_resolve_with_overrides) runs
        // AFTER both schema.yml layers are folded together — matching dbt-core's order where
        // project/root config always wins over schema.yml config.
        // See: https://github.com/dbt-labs/dbt-fusion/issues/767
        let merged_schema_config = merge_table_into_schema_config(
            source.config.as_ref(),
            &table_config,
            &source_name,
            &table_name,
        )?;

        let source_config = config_resolver.try_resolve_with_overrides(
            &fqn,
            &fqn,
            &[Some(&merged_schema_config)],
            |c: &mut SourceConfig| -> FsResult<()> {
                apply_freshness_loaded_at_override(c, &source_name, &table_name)?;
                Ok(())
            },
        )?;

        check_node_static_analysis(
            &source_config,
            arg.static_analysis,
            &unique_id,
            dependency_package_name,
        );

        // catalog check: a catalog_name with no match would otherwise silently read against the default catalog
        if source_config.enabled
            && let Some(catalog_name) = source_config
                .__warehouse_specific_config__
                .catalog_name
                .as_deref()
            && !catalogs_state.has_catalog_name(catalog_name)?
        {
            let err = match catalogs_state.catalog_names()? {
                None => dbt_common::fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => mpe.relative_path.clone(),
                    "Source specifies catalog_name '{catalog_name}', but no catalogs.yml was found in the project. \
                     Add a catalog named '{catalog_name}' to catalogs.yml, or remove the catalog_name config."
                ),
                Some(names) => dbt_common::fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => mpe.relative_path.clone(),
                    "Source specifies catalog_name '{catalog_name}', which is not defined in catalogs.yml. \
                     Defined catalogs: [{}].", names.join(", ")
                ),
            };
            emit_error_log_from_fs_error(*err);
            continue;
        }

        // `user_quoting` is the raw source+table YAML merge (no defaults). It is
        // serialized as `ManifestSource.quoting` and matches dbt-core's
        // `source.quoting.merged(table.quoting)`. The resolved `table_quoting`
        // below folds in project + adapter defaults and drives SQL generation.
        let user_quoting = DbtQuoting::merge_user(source.quoting.as_ref(), table.quoting.as_ref());

        let mut table_quoting = user_quoting.unwrap_or_default();
        table_quoting.default_to(&source_default_quoting);
        let quoting_ignore_case = table_quoting.snowflake_ignore_case.unwrap_or(false);

        let (database, schema, raw_identifier) =
            resolve_relation_parts(&source, &table, &table_name, database);
        let (database, schema, identifier, quoting) = normalize_quoting(
            &table_quoting.try_into()?,
            adapter_type,
            &database,
            &schema,
            &raw_identifier,
        );

        let parse_adapter = jinja_env
            .get_adapter()
            .expect("Failed to get parse adapter");

        let relation_name =
            generate_relation_name(parse_adapter, &database, &schema, &identifier, quoting)?;

        let columns = if let Some(ref cols) = table.columns {
            process_columns(
                Some(cols),
                source_config.tags.inner().clone().map(|tags| tags.into()),
            )?
        } else {
            vec![]
        };

        // Validate local sources have data types defined
        use dbt_schemas::schemas::common::SchemaOrigin;
        if source_config.schema_origin == SchemaOrigin::Local {
            if columns.is_empty() {
                return err!(
                    ErrorCode::InvalidConfig,
                    "source:{}.{} has schema_origin: local but no columns defined. \
                     Local sources must have column definitions with data_type specified.",
                    source_name,
                    table_name
                );
            }

            for col in &columns {
                if col.data_type.is_none() {
                    return err!(
                        ErrorCode::InvalidConfig,
                        "Column '{}' in source:{}.{} has schema_origin: local but no data_type defined. \
                         All columns must have data_type specified for local sources.",
                        col.name,
                        source_name,
                        table_name
                    );
                }
            }
        }

        if let Omissible::Present(Some(freshness)) = &source_config.freshness {
            // F2: a partially-populated freshness rule (only one of
            // {count, period}) is tolerated by Mantle at parse time and only
            // enforced when `dbt source freshness` actually consumes the rule.
            // Match that behavior here — demote the validation failure to a
            // warning so `parse` / `run` / `build` are not aborted, while the
            // safety net inside `dbt-freshness` still produces a hard error
            // when the rule is consumed. F1 (fully-empty rule) is already
            // accepted silently by `FreshnessRules::validate`.
            if let Err(err) = FreshnessRules::validate(freshness.error_after.as_ref()) {
                emit_warn_log_from_fs_error(*err);
            }
            if let Err(err) = FreshnessRules::validate(freshness.warn_after.as_ref()) {
                emit_warn_log_from_fs_error(*err);
            }
        }

        let static_analysis = source_config.static_analysis.clone();

        let unrendered_config = build_source_unrendered_config(
            &fqn,
            &raw_local_project_config,
            raw_root_project_models_cfg.as_ref(),
            raw_schema_yml_config,
            raw_table_yml_config,
            adapter_type,
        )?;

        let dbt_source = DbtSource {
            __common_attr__: CommonAttributes {
                name: table_name.to_owned(),
                package_name: package_name.to_owned(),
                original_file_path: DbtPath::from(&mpe.relative_path),
                path: DbtPath::from(&mpe.relative_path),
                name_span: dbt_common::Span::from_serde_span(
                    mpe.name_span,
                    mpe.relative_path.clone(),
                ),
                unique_id: unique_id.to_owned(),
                fqn,
                description: Some(table.description.clone().unwrap_or_default()),
                // Core only sets patch_path for sources in the rare cross-package
                // source-override case, which Fusion doesn't implement:
                // https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/parser/sources.py#L116
                // For the normal (un-overridden) case it defaults to None.
                patch_path: None,
                meta: source_config.meta.clone().unwrap_or_default(),
                tags: source_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                raw_code: None,
                checksum: DbtChecksum::default(),
                language: None,
            },
            __base_attr__: NodeBaseAttributes {
                adapter: adapter_type,
                // This node type has no `+propagate` config; nothing is published.
                propagate: Vec::new(),
                effective_propagation_target: None,
                database: database.to_owned(),
                schema: schema.to_owned(),
                alias: identifier.to_owned(),
                relation_name: Some(relation_name),
                quoting,
                quoting_ignore_case,
                enabled: source_config.enabled,
                extended_model: false,
                persist_docs: None,
                materialized: DbtMaterialization::External,
                static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                    .then_some(StaticAnalysisOffReason::ConfiguredOff),
                static_analysis,
                compute: None,
                columns,
                refs: vec![],
                sources: vec![],
                functions: vec![],
                depends_on: NodeDependsOn::default(),
                metrics: vec![],
                unrendered_config,
            },
            __source_attr__: DbtSourceAttr {
                freshness: match &source_config.freshness {
                    Omissible::Present(f) => f.clone(),
                    Omissible::Omitted => None,
                },
                identifier: raw_identifier,
                source_name: source_name.to_owned(),
                source_description: source.description.clone().unwrap_or_default(), // needs to be some or empty string per dbt spec
                loader: source.loader.clone().unwrap_or_default(),
                loaded_at_field: source_config.loaded_at_field.clone(),
                loaded_at_query: source_config.loaded_at_query.0.clone(),
                user_quoting,
                schema_origin: source_config.schema_origin,
                sync: source_config.sync.clone(),
                unrendered_database,
                unrendered_schema,
                external: table.external.clone(),
                catalog_name: source_config
                    .__warehouse_specific_config__
                    .catalog_name
                    .clone(),
            },
            deprecated_config: source_config.clone().into(),
            __other__: BTreeMap::new(),
        };
        let status = if source_config.enabled {
            ModelStatus::Enabled
        } else {
            ModelStatus::Disabled
        };

        match node_resolver.insert_source(package_name, &dbt_source, adapter_type, status) {
            Ok(_) => (),
            Err(e) => {
                let err_with_loc = e.with_location(mpe.relative_path.clone());
                emit_error_log_from_fs_error(err_with_loc);
            }
        }

        match status {
            ModelStatus::Enabled => {
                sources.insert(unique_id, Arc::new(dbt_source));

                if !arg.skip_creating_generic_tests {
                    TestableTable {
                        source_name: source_name.clone(),
                        table: &table.clone(),
                    }
                    .as_testable()
                    .persist(
                        package_name,
                        root_package_name,
                        collected_generic_tests,
                        test_name_truncations,
                        seen_generic_test_paths,
                        adapter_type,
                        io_args,
                        &mpe.relative_path,
                        false,
                        &raw_table_test_configs,
                    )?;
                }
            }
            ModelStatus::Disabled => {
                disabled_sources.insert(unique_id, Arc::new(dbt_source));
            }
            ModelStatus::ParsingFailed => {}
        }
    }
    Ok((sources, disabled_sources))
}

/// Merge source-level and table-level schema.yml configs into a single `SourceConfig`.
///
/// This pre-merge must happen before the config is handed to `try_resolve_with_overrides`
/// so that the root overlay (applied inside that call) runs on top of the fully-merged
/// schema.yml patch — not below it. dbt-core merges source+table first, then applies
/// project/root config on top.
fn merge_table_into_schema_config(
    source_config: Option<&SourceConfig>,
    table_config: &TablesConfig,
    source_name: &str,
    table_name: &str,
) -> FsResult<SourceConfig> {
    let default_source = SourceConfig::default();
    let source = source_config.unwrap_or(&default_source);

    // Build a SourceConfig from the table-level fields, then inherit source-level values
    // for any field the table didn't set (via `default_to`).
    let mut table_as_source = SourceConfig {
        enabled: table_config.enabled,
        event_time: table_config.event_time.clone(),
        meta: table_config.meta.clone(),
        freshness: table_config.freshness.clone(),
        tags: Tags(table_config.tags.clone()),
        loaded_at_field: table_config.loaded_at_field.clone(),
        loaded_at_query: table_config.loaded_at_query.clone(),
        schema_origin: table_config.schema_origin,
        sync: table_config.sync.clone(),
        external_location: table_config.external_location.clone(),
        formatter: table_config.formatter.clone(),
        static_analysis: None,
        __warehouse_specific_config__: Default::default(),
    };

    // `default_to` fills unset fields from source-level config and uses additive semantics
    // for meta/tags (union merge). freshness uses `handle_omissible_override` which does
    // whole-value replacement — we fix that below.
    table_as_source.default_to(source);

    // Re-merge freshness at the field level: table may only set one sub-field (e.g.
    // `error_after`) and should inherit the other (e.g. `warn_after`) from source-level.
    let source_freshness = &source.freshness;
    table_as_source.freshness =
        Omissible::Present(merge_freshness(source_freshness, &table_config.freshness));

    // Merge loaded_at_field / loaded_at_query with mutual-exclusion peer-clearing.
    let merged_loaded_at = merge_loaded_at_pair(
        source.loaded_at_field.as_deref(),
        source.loaded_at_query.0.as_deref(),
        table_config.loaded_at_field.as_deref(),
        table_config.loaded_at_query.0.as_deref(),
    )
    .map_err(|msg| {
        dbt_common::fs_err!(
            ErrorCode::Unexpected,
            "{} on source `{}.{}`",
            msg,
            source_name,
            table_name
        )
    })?;
    table_as_source.loaded_at_field = Some(merged_loaded_at.field).filter(|s| !s.is_empty());
    table_as_source.loaded_at_query = Some(merged_loaded_at.query)
        .filter(|s| !s.is_empty())
        .into();

    Ok(table_as_source)
}

fn resolve_relation_parts(
    source: &SourceProperties,
    table: &Tables,
    table_name: &str,
    default_database: &str,
) -> (String, String, String) {
    let database: String = source
        .database
        .clone()
        .unwrap_or_else(|| default_database.to_owned());
    let schema = source
        .schema
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| source.name.clone());

    // Preserve the raw user-provided identifier (including any embedded quote
    // characters) for `__source_attr__.identifier`. dbt-core stores this verbatim
    // and packages such as `zendesk` rely on `source(...).identifier` round-tripping
    // through Jinja — see `union_zendesk_connections` calling
    // `adapter.get_relation(identifier=source(...).identifier)`. Stripping the
    // quotes here would turn `"GROUP"` into `GROUP` and produce SQL that fails on
    // reserved-keyword tables in Snowflake.
    let raw_identifier = table
        .identifier
        .clone()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| table_name.to_owned());
    (database, schema, raw_identifier)
}

/// Resolved (`loaded_at_field`, `loaded_at_query`) pair after merging
/// table-level config over source-level config. Either value may be empty
/// (downstream treats `""` and `None` as "no freshness on this dimension").
#[derive(Debug)]
struct MergedLoadedAt {
    field: String,
    query: String,
}

/// Merge `loaded_at_field` and `loaded_at_query` across source-level and
/// table-level config.
///
/// `loaded_at_field` and `loaded_at_query` are mutually exclusive peers.
/// dbt-core treats a table-level override of either as implicitly clearing
/// the inherited *other* — that's how patterns like a source-wide
/// `loaded_at_query` plus a per-table `loaded_at_field` override (e.g. the
/// merge-log table the query references) resolve cleanly. Without this,
/// the per-key `or_else` chain would leak the source-level value of the
/// un-overridden key and falsely trip the same-block validation.
///
/// Returns `Err` only when, after merging, BOTH peers are non-empty —
/// which happens in two cases:
///   1. Both peers set in the same `config:` block at table level, OR
///   2. Both peers set at the source level with no table-level override
///      to clear them.
fn merge_loaded_at_pair(
    source_field: Option<&str>,
    source_query: Option<&str>,
    table_field: Option<&str>,
    table_query: Option<&str>,
) -> Result<MergedLoadedAt, &'static str> {
    let table_has_field = table_field.is_some();
    let table_has_query = table_query.is_some();

    let merged_field = if table_has_query {
        // Table override of `loaded_at_query` clears any inherited
        // `loaded_at_field` from the source level.
        table_field.unwrap_or("").to_string()
    } else {
        table_field.or(source_field).unwrap_or("").to_string()
    };

    let merged_query = if table_has_field {
        // Table override of `loaded_at_field` clears any inherited
        // `loaded_at_query` from the source level.
        table_query.unwrap_or("").to_string()
    } else {
        table_query.or(source_query).unwrap_or("").to_string()
    };

    if !merged_field.is_empty() && !merged_query.is_empty() {
        return Err("loaded_at_field and loaded_at_query cannot be set at the same time");
    }
    Ok(MergedLoadedAt {
        field: merged_field,
        query: merged_query,
    })
}

fn merge_freshness(
    base: &Omissible<Option<FreshnessDefinition>>,
    update: &Omissible<Option<FreshnessDefinition>>,
) -> Option<FreshnessDefinition> {
    match update {
        // Table-level `freshness: null` inhibits inheritance.
        Omissible::Present(None) => None,
        Omissible::Present(Some(t)) => {
            let base_inner = match base {
                Omissible::Present(b) => b.as_ref(),
                Omissible::Omitted => None,
            };
            merge_freshness_unwrapped(base_inner, Some(t))
        }
        // Table-level omitted: defer to project-level.
        Omissible::Omitted => match base {
            // Project-level `+freshness: null` inhibits (META-7188).
            Omissible::Present(None) => None,
            Omissible::Present(Some(b)) => merge_freshness_unwrapped(Some(b), None),
            Omissible::Omitted => merge_freshness_unwrapped(None, None),
        },
    }
}

fn merge_freshness_unwrapped(
    base: Option<&FreshnessDefinition>,
    update: Option<&FreshnessDefinition>,
) -> Option<FreshnessDefinition> {
    match (base, update) {
        (_, Some(update)) => {
            // Mantle uses field-level merging: each field uses the update value if set,
            // otherwise inherits from base.
            // https://github.com/dbt-labs/dbt-mantle/blob/6bcac392d653a5c8a35da01bc94d93a45b882629/core/dbt/parser/sources.py#L545-L555
            let update_has_loaded_at_field = update.loaded_at_field.is_some();
            let update_has_loaded_at_query = update.loaded_at_query.is_some();
            Some(FreshnessDefinition {
                error_after: update
                    .error_after
                    .clone()
                    .or_else(|| base.and_then(|b| b.error_after.clone())),
                warn_after: update
                    .warn_after
                    .clone()
                    .or_else(|| base.and_then(|b| b.warn_after.clone())),
                filter: update
                    .filter
                    .clone()
                    .or_else(|| base.and_then(|b| b.filter.clone())),
                loaded_at_field: if update_has_loaded_at_query {
                    update.loaded_at_field.clone()
                } else {
                    update
                        .loaded_at_field
                        .clone()
                        .or_else(|| base.and_then(|b| b.loaded_at_field.clone()))
                },
                loaded_at_query: if update_has_loaded_at_field {
                    update.loaded_at_query.clone()
                } else {
                    update
                        .loaded_at_query
                        .clone()
                        .or_else(|| base.and_then(|b| b.loaded_at_query.clone()))
                },
            })
        }
        (Some(base), None) => Some(base.clone()),
        (None, None) => Some(FreshnessDefinition::default()), // Provide default value if user never defined freshness https://dbtlabs.atlassian.net/browse/META-5461
    }
}

fn apply_freshness_loaded_at_override(
    config: &mut SourceConfig,
    source_name: &str,
    table_name: &str,
) -> FsResult<()> {
    if let Omissible::Present(Some(freshness)) = &config.freshness {
        match (
            freshness.loaded_at_field.clone(),
            freshness.loaded_at_query.clone(),
        ) {
            (Some(_), Some(_)) => {
                return Err(dbt_common::fs_err!(
                    ErrorCode::InvalidConfig,
                    "loaded_at_field and loaded_at_query cannot be set at the same time on source `{}.{}`",
                    source_name,
                    table_name
                ));
            }
            (Some(field), None) => {
                config.loaded_at_field = Some(field);
                config.loaded_at_query = Some(String::new()).into();
            }
            (None, Some(query)) => {
                config.loaded_at_field = Some(String::new());
                config.loaded_at_query = Some(query).into();
            }
            (None, None) => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_jinja_utils::serde::Omissible;
    use dbt_schemas::schemas::common::{FreshnessDefinition, FreshnessPeriod, FreshnessRules};

    fn source_with(name: &str, schema: Option<&str>) -> SourceProperties {
        SourceProperties {
            config: None,
            database: None,
            schema: schema.map(str::to_string),
            catalog: None,
            description: None,
            loader: None,
            name: name.to_string(),
            quoting: None,
            tables: None,
        }
    }

    fn table_with(name: &str, identifier: Option<&str>) -> Tables {
        Tables {
            columns: None,
            config: None,
            data_tests: None,
            description: None,
            external: None,
            identifier: identifier.map(str::to_string),
            loader: None,
            name: name.to_string(),
            quoting: None,
            tests: None,
        }
    }

    #[test]
    fn test_resolve_relation_parts_schema_omitted_defaults_to_source_name() {
        let source = source_with("dummy_src", None);
        let table = table_with("src", None);
        let (_, schema, _) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(schema, "dummy_src");
    }

    #[test]
    fn test_resolve_relation_parts_schema_explicit_empty_defaults_to_source_name() {
        let source = source_with("dummy_src", Some(""));
        let table = table_with("src", None);
        let (_, schema, _) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(schema, "dummy_src");
    }

    #[test]
    fn test_resolve_relation_parts_schema_explicit_non_empty_is_kept() {
        let source = source_with("dummy_src", Some("custom_schema"));
        let table = table_with("src", None);
        let (_, schema, _) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(schema, "custom_schema");
    }

    #[test]
    fn test_resolve_relation_parts_identifier_omitted_defaults_to_table_name() {
        let source = source_with("dummy_src", None);
        let table = table_with("src", None);
        let (_, _, identifier) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(identifier, "src");
    }

    #[test]
    fn test_resolve_relation_parts_identifier_explicit_empty_defaults_to_table_name() {
        let source = source_with("dummy_src", None);
        let table = table_with("src", Some(""));
        let (_, _, identifier) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(identifier, "src");
    }

    #[test]
    fn test_resolve_relation_parts_identifier_explicit_non_empty_is_kept() {
        let source = source_with("dummy_src", None);
        let table = table_with("src", Some("custom_identifier"));
        let (_, _, identifier) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(identifier, "custom_identifier");
    }

    #[test]
    fn test_resolve_relation_parts_database_omitted_defaults_to_default_database() {
        let source = source_with("dummy_src", None);
        let table = table_with("src", None);
        let (database, _, _) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(database, "default_db");
    }

    #[test]
    fn test_resolve_relation_parts_database_explicit_empty_is_kept() {
        let mut source = source_with("dummy_src", None);
        source.database = Some(String::new());
        let table = table_with("src", None);
        let (database, _, _) = resolve_relation_parts(&source, &table, "src", "default_db");
        assert_eq!(database, "");
    }

    #[test]
    fn test_merge_freshness_unwrapped_update_overrides_base() {
        // When both base and update have values, update fields win; unset fields inherit from base.
        let base = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: Some(FreshnessRules {
                count: Some(3),
                period: Some(FreshnessPeriod::hour),
            }),
            filter: Some("base_filter".to_string()),
            ..Default::default()
        };
        let update = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(10),
                period: Some(FreshnessPeriod::day),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(Some(&base), Some(&update)).unwrap();
        // error_after comes from update
        assert_eq!(result.error_after.as_ref().unwrap().count, Some(10));
        // warn_after and filter inherit from base
        assert_eq!(result.warn_after.as_ref().unwrap().count, Some(3));
        assert_eq!(result.filter.as_deref(), Some("base_filter"));
    }

    #[test]
    fn test_merge_freshness_unwrapped_inherit_from_base() {
        // When update is None, base should be inherited
        let base = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: Some(FreshnessRules {
                count: Some(3),
                period: Some(FreshnessPeriod::hour),
            }),
            filter: None,
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(Some(&base), None);
        assert_eq!(result, Some(base));
    }

    #[test]
    fn test_merge_freshness_unwrapped_no_base() {
        // When base is None but update has value, use update
        let update = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(10),
                period: Some(FreshnessPeriod::day),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(None, Some(&update));
        assert_eq!(result, Some(update));
    }

    #[test]
    fn test_merge_freshness_unwrapped_both_none() {
        // When both are None, result should be None
        let result = merge_freshness_unwrapped(None, None);
        assert_eq!(result, Some(FreshnessDefinition::default()));
    }

    #[test]
    fn test_merge_freshness_present_null_inhibits() {
        // Present but null freshness should return None (inhibits freshness)
        let base = Omissible::Present(Some(FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        }));

        let update = Omissible::Present(None);
        let result = merge_freshness(&base, &update);
        assert_eq!(result, None);
    }

    #[test]
    fn test_merge_freshness_present_with_value() {
        // Present with value should use the value
        let base = Omissible::Present(Some(FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        }));

        let update_value = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(10),
                period: Some(FreshnessPeriod::day),
            }),
            warn_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::day),
            }),
            filter: None,
            ..Default::default()
        };

        let update = Omissible::Present(Some(update_value.clone()));
        let result = merge_freshness(&base, &update);
        assert_eq!(result, Some(update_value));
    }

    #[test]
    fn test_merge_freshness_omitted_inherits_base() {
        // Omitted freshness should inherit from base
        let base_value = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        };
        let base = Omissible::Present(Some(base_value.clone()));

        let update = Omissible::Omitted;
        let result = merge_freshness(&base, &update);
        assert_eq!(result, Some(base_value));
    }

    #[test]
    fn test_merge_freshness_omitted_no_base() {
        // Omitted freshness with no base should return default (META-5461)
        let base = Omissible::Omitted;
        let update = Omissible::Omitted;
        let result = merge_freshness(&base, &update);
        assert_eq!(result, Some(FreshnessDefinition::default()));
    }

    #[test]
    fn test_merge_freshness_project_null_inhibits_when_table_omitted() {
        // Project-level `+freshness: null` + table omitted → None (META-7188)
        let base = Omissible::Present(None);
        let update = Omissible::Omitted;
        assert_eq!(merge_freshness(&base, &update), None);
    }

    #[test]
    fn test_merge_freshness_table_null_opts_out() {
        // Table-level `config: freshness: null` → None, even when the source level sets rules.
        //
        // Together with `test_merge_freshness_omitted_no_base` this is what makes `None` mean
        // "the user explicitly opted out" and nothing else — `dbt-freshness`'s
        // `collects_freshness_during_build` relies on that distinction to skip the metadata
        // query for opted-out sources only (dbt-labs/dbt-core#14534).
        let base = Omissible::Present(Some(FreshnessDefinition {
            warn_after: Some(FreshnessRules {
                count: Some(12),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        }));
        assert_eq!(merge_freshness(&base, &Omissible::Present(None)), None);
        assert_eq!(
            merge_freshness(&Omissible::Omitted, &Omissible::Present(None)),
            None
        );
    }

    #[test]
    fn test_merge_freshness_partial_update_overrides_completely() {
        // Test that partial updates in the update completely override base
        // This validates the comment about mantle logic
        let base = Omissible::Present(Some(FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(5),
                period: Some(FreshnessPeriod::hour),
            }),
            warn_after: Some(FreshnessRules {
                count: Some(3),
                period: Some(FreshnessPeriod::hour),
            }),
            filter: Some("base_filter".to_string()),
            ..Default::default()
        }));

        // Update only has error_after; warn_after and filter should be inherited from base.
        let update_value = FreshnessDefinition {
            error_after: Some(FreshnessRules {
                count: Some(10),
                period: Some(FreshnessPeriod::day),
            }),
            warn_after: None,
            filter: None,
            ..Default::default()
        };

        let update = Omissible::Present(Some(update_value));
        let result = merge_freshness(&base, &update);

        let merged = result.unwrap();
        // error_after comes from update
        assert_eq!(merged.error_after.as_ref().unwrap().count, Some(10));
        // warn_after and filter are inherited from base
        assert_eq!(merged.warn_after.as_ref().unwrap().count, Some(3));
        assert_eq!(merged.filter.as_deref(), Some("base_filter"));
    }

    #[test]
    fn test_merge_freshness_inherits_nested_loaded_at_metadata() {
        let base = FreshnessDefinition {
            loaded_at_field: Some("SRC_LOADED_AT".to_string()),
            ..Default::default()
        };
        let update = FreshnessDefinition {
            warn_after: Some(FreshnessRules {
                count: Some(3),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(Some(&base), Some(&update)).unwrap();
        assert_eq!(result.loaded_at_field.as_deref(), Some("SRC_LOADED_AT"));
        assert_eq!(result.loaded_at_query, None);
        assert_eq!(result.warn_after.as_ref().unwrap().count, Some(3));
    }

    #[test]
    fn test_merge_freshness_loaded_at_query_clears_inherited_field() {
        let base = FreshnessDefinition {
            loaded_at_field: Some("SRC_LOADED_AT".to_string()),
            ..Default::default()
        };
        let update = FreshnessDefinition {
            loaded_at_query: Some("select max(loaded_at) from source_table".to_string()),
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(Some(&base), Some(&update)).unwrap();
        assert_eq!(result.loaded_at_field, None);
        assert_eq!(
            result.loaded_at_query.as_deref(),
            Some("select max(loaded_at) from source_table")
        );
    }

    #[test]
    fn test_merge_freshness_loaded_at_field_clears_inherited_query() {
        let base = FreshnessDefinition {
            loaded_at_query: Some("select max(loaded_at) from source_table".to_string()),
            ..Default::default()
        };
        let update = FreshnessDefinition {
            loaded_at_field: Some("TABLE_LOADED_AT".to_string()),
            ..Default::default()
        };

        let result = merge_freshness_unwrapped(Some(&base), Some(&update)).unwrap();
        assert_eq!(result.loaded_at_field.as_deref(), Some("TABLE_LOADED_AT"));
        assert_eq!(result.loaded_at_query, None);
    }

    #[test]
    fn test_freshness_loaded_at_field_overrides_top_level_query() {
        let mut config = SourceConfig {
            loaded_at_field: Some(String::new()),
            loaded_at_query: Some("select max(src_loaded_at) from source_table".to_string()).into(),
            freshness: Omissible::Present(Some(FreshnessDefinition {
                loaded_at_field: Some("FRESHNESS_LOADED_AT".to_string()),
                ..Default::default()
            })),
            ..Default::default()
        };

        apply_freshness_loaded_at_override(&mut config, "src", "table").unwrap();

        assert_eq!(
            config.loaded_at_field.as_deref(),
            Some("FRESHNESS_LOADED_AT")
        );
        assert_eq!(config.loaded_at_query.0.as_deref(), Some(""));
    }

    #[test]
    fn test_freshness_loaded_at_query_overrides_top_level_field() {
        let mut config = SourceConfig {
            loaded_at_field: Some("SRC_LOADED_AT".to_string()),
            loaded_at_query: Some(String::new()).into(),
            freshness: Omissible::Present(Some(FreshnessDefinition {
                loaded_at_query: Some(
                    "select max(freshness_loaded_at) from source_table".to_string(),
                ),
                ..Default::default()
            })),
            ..Default::default()
        };

        apply_freshness_loaded_at_override(&mut config, "src", "table").unwrap();

        assert_eq!(config.loaded_at_field.as_deref(), Some(""));
        assert_eq!(
            config.loaded_at_query.0.as_deref(),
            Some("select max(freshness_loaded_at) from source_table")
        );
    }

    #[test]
    fn test_freshness_loaded_at_field_and_query_conflict_errors() {
        let mut config = SourceConfig {
            freshness: Omissible::Present(Some(FreshnessDefinition {
                loaded_at_field: Some("LOADED_AT".to_string()),
                loaded_at_query: Some("select max(loaded_at) from source_table".to_string()),
                ..Default::default()
            })),
            ..Default::default()
        };

        let err = apply_freshness_loaded_at_override(&mut config, "src", "table")
            .expect_err("nested freshness peers should be mutually exclusive");
        assert!(
            err.to_string()
                .contains("loaded_at_field and loaded_at_query cannot be set at the same time"),
            "error must name the conflict; got: {err}"
        );
    }

    // ── merge_loaded_at_pair ──────────────────────────────────────────────
    //
    // These unit tests pin the RIGHT (`field`, `query`) pair after merge for
    // every state-relevant input combination. The e2e tests in
    // `crates/dbt-cli/tests/dbt_conformance/regression.rs` cannot make this
    // assertion because parse-success/parse-failure goldens only see the CLI
    // summary, not the per-row resolved config. This block addresses that
    // exactly: any merge-logic regression that produces a different result
    // for any of these inputs trips one or more of these tests.

    /// Table-level `loaded_at_field` overrides source-level `loaded_at_query`.
    /// The *inherited* peer must be cleared on the override row.
    #[test]
    fn test_merge_loaded_at_pair_table_field_clears_inherited_query() {
        let result = merge_loaded_at_pair(
            None,                               // source_field
            Some("select max(load_ts) from x"), // source_query
            Some("LOAD_TIMESTAMP"),             // table_field — override
            None,                               // table_query
        )
        .expect("must not error: cross-level peer-clearing is valid");
        assert_eq!(
            result.field, "LOAD_TIMESTAMP",
            "table-level loaded_at_field override must survive the merge"
        );
        assert_eq!(
            result.query, "",
            "inherited source-level loaded_at_query must be cleared by table-level field override"
        );
    }

    /// Mirror direction: table-level `loaded_at_query` overrides source-level
    /// `loaded_at_field`. Guards against an asymmetric fix that handles only
    /// one direction.
    #[test]
    fn test_merge_loaded_at_pair_table_query_clears_inherited_field() {
        let result = merge_loaded_at_pair(
            Some("SRC_LOADED_AT"),                // source_field
            None,                                 // source_query
            None,                                 // table_field
            Some("select max(custom_ts) from y"), // table_query — override
        )
        .expect("must not error: cross-level peer-clearing is valid");
        assert_eq!(
            result.field, "",
            "inherited source-level loaded_at_field must be cleared by table-level query override"
        );
        assert_eq!(
            result.query, "select max(custom_ts) from y",
            "table-level loaded_at_query override must survive the merge"
        );
    }

    /// Sibling table with NO table-level override on the SAME source: must
    /// inherit source-level values untouched. Together with the override
    /// tests above, proves the peer-clearing is selective to override rows
    /// rather than a broad clear.
    #[test]
    fn test_merge_loaded_at_pair_no_table_override_inherits_source() {
        let result = merge_loaded_at_pair(
            Some("SRC_LOADED_AT"), // source_field
            None,                  // source_query
            None,                  // table_field
            None,                  // table_query
        )
        .expect("must not error: only source-level field is set");
        assert_eq!(
            result.field, "SRC_LOADED_AT",
            "non-override table must inherit source-level loaded_at_field"
        );
        assert_eq!(result.query, "", "no query anywhere → empty");
    }

    /// Genuine misuse — both peers set in the SAME (table) `config:` block.
    /// Must error with the exact validation message; the merge cannot
    /// silently swallow the conflict.
    #[test]
    fn test_merge_loaded_at_pair_both_in_same_table_block_errors() {
        let err = merge_loaded_at_pair(
            None,
            None,
            Some("TS"),                             // table_field
            Some("select max(ts) from {{ this }}"), // table_query — same block!
        )
        .expect_err("both peers in the same table block must error");
        assert!(
            err.contains("loaded_at_field and loaded_at_query cannot be set at the same time"),
            "error must name the conflict; got: {err}"
        );
    }

    /// Same-block conflict at the SOURCE level (no table-level override).
    /// Even without table-level config the validation must fire if the
    /// inherited pair conflicts. Discriminates against a buggy fix that
    /// only checks for table-level conflicts.
    #[test]
    fn test_merge_loaded_at_pair_both_at_source_level_errors() {
        let err = merge_loaded_at_pair(
            Some("SRC_LOADED_AT"),         // source_field
            Some("select max(ts) from x"), // source_query
            None,
            None,
        )
        .expect_err("both peers inherited at the source level must error");
        assert!(
            err.contains("loaded_at_field and loaded_at_query cannot be set at the same time"),
            "error must name the conflict; got: {err}"
        );
    }

    /// Empty everywhere → both empty, no error. Pin the no-op case so a
    /// future bug that spuriously errors on absent freshness is caught.
    #[test]
    fn test_merge_loaded_at_pair_all_none() {
        let result =
            merge_loaded_at_pair(None, None, None, None).expect("absent peers must not error");
        assert_eq!(result.field, "");
        assert_eq!(result.query, "");
    }
}
