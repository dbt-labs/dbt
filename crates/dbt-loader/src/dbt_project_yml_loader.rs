use dbt_common::FsResult;
use dbt_common::constants::DBT_PROJECT_YML;
use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;
use dbt_common::{ErrorCode, fs_err};
use dbt_jinja_utils::serde::{into_typed_with_jinja, value_from_file};
use dbt_jinja_utils::{Var, jinja_environment::JinjaEnv, phases::parse::build_resolve_context};
use dbt_schemas::schemas::project::DbtProject;
use dbt_schemas::schemas::project::{
    ConfigKeys, DataTestConfig, FunctionConfig, ModelConfig, ProjectAnalysisConfig,
    ProjectDataTestConfig, ProjectExposureConfig, ProjectFunctionConfig, ProjectModelConfig,
    ProjectSeedConfig, ProjectSemanticModelConfig, ProjectSkillConfig, ProjectSnapshotConfig,
    ProjectSourceConfig, ProjectUnitTestConfig, SeedConfig, SnapshotConfig, SourceConfig,
    UnitTestConfig,
};
use dbt_yaml::{ShouldBe, Value as YmlValue};
use indexmap::IndexMap;
use minijinja::Value;
use minijinja::constants::CURRENT_PATH;
use std::{
    collections::{BTreeMap, HashSet},
    path::Path,
};

macro_rules! prune_section {
    ($proj:expr, $field:ident, $name:expr, $ty:ty, $valid_field_names:expr) => {
        if let Some(cfg) = $proj.$field.as_mut() {
            prune_unexpected_nulls_in_section($name, cfg, &$valid_field_names, |c: &mut $ty| {
                &mut c.__additional_properties__
            });
        }
    };
}

fn prune_sections(dbt_project: &mut DbtProject) {
    prune_section!(
        dbt_project,
        models,
        "models",
        ProjectModelConfig,
        ModelConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        seeds,
        "seeds",
        ProjectSeedConfig,
        SeedConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        snapshots,
        "snapshots",
        ProjectSnapshotConfig,
        SnapshotConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        sources,
        "sources",
        ProjectSourceConfig,
        SourceConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        tests,
        "tests",
        ProjectDataTestConfig,
        DataTestConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        unit_tests,
        "unit_tests",
        ProjectUnitTestConfig,
        UnitTestConfig::valid_field_names()
    );
    prune_section!(
        dbt_project,
        functions,
        "functions",
        ProjectFunctionConfig,
        FunctionConfig::valid_field_names()
    );
    // TODO: Do we need to implement ConfigKeys for ExposureConfig?
    prune_section!(
        dbt_project,
        exposures,
        "exposures",
        ProjectExposureConfig,
        HashSet::<String>::new()
    );
    // TODO: Do we need to implement ConfigKeys for AnalysisConfig?
    prune_section!(
        dbt_project,
        analyses,
        "analyses",
        ProjectAnalysisConfig,
        HashSet::<String>::new()
    );
    // TODO: Do we need to implement ConfigKeys for SemanticModelConfig?
    prune_section!(
        dbt_project,
        semantic_models,
        "semantic-models",
        ProjectSemanticModelConfig,
        HashSet::<String>::new()
    );
    // TODO: Do we need to implement ConfigKeys for SkillConfig?
    prune_section!(
        dbt_project,
        skills,
        "skills",
        ProjectSkillConfig,
        HashSet::<String>::new()
    );
}

fn prune_unexpected_nulls_in_children<T>(
    section_name: &str,
    current_path: &str,
    cfg: &mut T,
    valid_field_names: &HashSet<String>,
    get_children_map: fn(&mut T) -> &mut BTreeMap<String, ShouldBe<T>>,
) {
    let children = get_children_map(cfg);

    // Collect keys to remove to avoid mutable iteration issues
    let mut keys_to_remove: Vec<String> = Vec::new();

    for (child_key, child_val) in children.iter_mut() {
        match child_val {
            ShouldBe::AndIs(child_cfg) => {
                let next_path = if current_path.is_empty() {
                    child_key.clone()
                } else {
                    format!("{}.{}", current_path, child_key)
                };
                prune_unexpected_nulls_in_children::<T>(
                    section_name,
                    &next_path,
                    child_cfg,
                    valid_field_names,
                    get_children_map,
                );
            }
            ShouldBe::ButIsnt(..) => {
                // FIXME: We should always emit the original error from the
                // ShouldBe::ButIsnt, instead of making up a new one here
                if let Some(YmlValue::Null(span)) = child_val.as_ref_raw() {
                    let trimmed_key = child_key.trim();
                    let yaml_path = if current_path.is_empty() {
                        format!("{}.{}", section_name, trimmed_key)
                    } else {
                        format!("{}.{}.{}", section_name, current_path, trimmed_key)
                    };
                    // An empty set means the section has no ConfigKeys impl to
                    // gate on, so fall back to always suggesting.
                    let suggestion = if !trimmed_key.starts_with('+')
                        && (valid_field_names.is_empty() || valid_field_names.contains(trimmed_key))
                    {
                        format!(" Try '+{}' instead.", trimmed_key)
                    } else {
                        String::new()
                    };
                    let err = fs_err!(
                        code => ErrorCode::UnusedConfigKey,
                        loc => span.clone(),
                        "Ignored unexpected key '{}'.{} YAML path: '{}'.",
                        trimmed_key,
                        suggestion,
                        yaml_path
                    );
                    emit_warn_log_from_fs_error(*err);
                    keys_to_remove.push(child_key.clone());
                }
            }
        }
    }

    for key in keys_to_remove {
        children.remove(&key);
    }
}

fn prune_unexpected_nulls_in_section<T>(
    section_name: &str,
    section_cfg: &mut T,
    valid_field_names: &HashSet<String>,
    get_children_map: fn(&mut T) -> &mut BTreeMap<String, ShouldBe<T>>,
) {
    prune_unexpected_nulls_in_children(
        section_name,
        "",
        section_cfg,
        valid_field_names,
        get_children_map,
    );
}

/// Renders a single Yaml value tree with Jinja on a "best effort" basis: if rendering the
/// whole subtree in one shot fails (e.g. an unknown macro call nested somewhere within it),
/// recurses into its children (mapping values / sequence elements) and renders each
/// independently, so a broken descendant only leaves its own leaf/subtree unrendered -- with
/// a warning -- instead of poisoning unrelated siblings the way a single combined `into_typed`
/// pass over the whole tree would.
fn render_yml_value_tolerantly(
    value: YmlValue,
    env: &JinjaEnv,
    ctx: &BTreeMap<String, Value>,
    dependency_package_name: Option<&str>,
) -> YmlValue {
    match into_typed_with_jinja::<YmlValue, _>(
        value.clone(),
        false,
        env,
        ctx,
        &[],
        dependency_package_name,
        true,
    ) {
        Ok(rendered) => rendered,
        Err(e) => match value {
            YmlValue::Mapping(mapping, span) => YmlValue::Mapping(
                mapping
                    .into_iter()
                    .map(|(k, v)| {
                        (
                            k,
                            render_yml_value_tolerantly(v, env, ctx, dependency_package_name),
                        )
                    })
                    .collect(),
                span,
            ),
            YmlValue::Sequence(seq, span) => YmlValue::Sequence(
                seq.into_iter()
                    .map(|v| render_yml_value_tolerantly(v, env, ctx, dependency_package_name))
                    .collect(),
                span,
            ),
            leaf => {
                emit_warn_log_from_fs_error(*e);
                leaf
            }
        },
    }
}

/// Re-renders `+meta` on its own, after the whole-document parse in [`load_project_yml`] already
/// skipped it via `Verbatim` (fs#14217). Each value is rendered tolerantly (see
/// [`render_yml_value_tolerantly`]), so a render failure -- e.g. an unknown macro call nested
/// inside a customer's misplaced `+meta` block -- is contained to a warning and the original,
/// unrendered value for just that node, instead of failing the entire enclosing directory-path
/// node (and silently dropping unrelated sibling directories' `+schema`/etc. overrides, or even
/// unrelated sibling `+meta` keys, with it) the way it would if `meta` were rendered as part of
/// the single combined `into_typed` pass.
fn render_meta_tolerantly(
    config: &mut ProjectModelConfig,
    env: &JinjaEnv,
    ctx: &BTreeMap<String, Value>,
    dependency_package_name: Option<&str>,
) {
    if let Some(raw_meta) = config.meta.0.clone() {
        let rendered: IndexMap<String, YmlValue> = raw_meta
            .into_iter()
            .map(|(k, v)| {
                (
                    k,
                    render_yml_value_tolerantly(v, env, ctx, dependency_package_name),
                )
            })
            .collect();
        config.meta = Some(rendered).into();
    }

    for child in config.__additional_properties__.values_mut() {
        if let ShouldBe::AndIs(child_config) = child {
            render_meta_tolerantly(child_config, env, ctx, dependency_package_name);
        }
    }
}

pub fn load_project_yml(
    env: &JinjaEnv,
    dbt_project_path: &Path,
    dependency_package_name: Option<&str>,
    cli_vars: BTreeMap<String, dbt_yaml::Value>,
) -> FsResult<(DbtProject, dbt_yaml::Value)> {
    let namespace_keys: Vec<String> = env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let mut context = build_resolve_context(
        DBT_PROJECT_YML,
        DBT_PROJECT_YML,
        &BTreeMap::new(),
        BTreeMap::new(),
        namespace_keys,
        None,
    );

    context.insert("var".to_string(), Value::from_object(Var::new(cli_vars)));
    context.insert(CURRENT_PATH.to_string(), Value::from(DBT_PROJECT_YML));

    let raw_yml = value_from_file(dbt_project_path, true, dependency_package_name)?;

    // Parse the template without vars using Jinja
    let mut dbt_project: DbtProject = into_typed_with_jinja(
        raw_yml.clone(),
        false,
        env,
        &context,
        &[],
        dependency_package_name,
        true,
    )?;

    if let Some(models) = dbt_project.models.as_mut() {
        render_meta_tolerantly(models, env, &context, dependency_package_name);
    }

    if dbt_project.name.contains(' ') {
        return Err(fs_err!(
            code => ErrorCode::DbtYamlValidationError,
            loc => dbt_project_path.to_path_buf(),
            "Project name '{}' in {} contains spaces. Project names cannot contain spaces.",
            dbt_project.name,
            DBT_PROJECT_YML
        ));
    }

    // Prune unexpected null keys (e.g. empty keys) early and emit warnings
    prune_sections(&mut dbt_project);

    Ok((
        crate::load_packages::build_internal_dbt_project(dbt_project)?,
        raw_yml,
    ))
}

pub fn collect_protected_paths(dbt_project: &DbtProject) -> Vec<String> {
    let mut result: Vec<String> = vec![];

    result.extend_from_slice(dbt_project.analysis_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.asset_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.macro_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.model_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.seed_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.snapshot_paths.as_deref().unwrap_or_default());
    result.extend_from_slice(dbt_project.test_paths.as_deref().unwrap_or_default());

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for fs#14217: a render failure nested inside a `+meta` value (here, a
    /// misplaced sibling directory's config with an unknown macro in its `+post-hook`) must not
    /// drop unrelated sibling directories' own config -- only the containing struct as a whole
    /// used to fail before `+meta` was made `Verbatim` and re-rendered tolerantly afterward.
    #[test]
    fn test_meta_render_failure_does_not_drop_sibling_config() {
        let yaml = r#"
        INT:
          +schema: INT_SCHEMA
        CNT:
          +schema: CNT_SCHEMA
        +meta:
          PRF:
            +schema: PRF_SCHEMA
            +post-hook: "{{ some_totally_unknown_macro() }}"
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let env = JinjaEnv::new(minijinja::Environment::new());
        let ctx: BTreeMap<String, Value> = BTreeMap::new();

        // With `+meta` protected by `Verbatim`, this must succeed even though the value nested
        // inside `+meta.PRF.+post-hook` could never render (the macro doesn't exist).
        let mut cfg: ProjectModelConfig =
            into_typed_with_jinja(val, false, &env, &ctx, &[], None, true)
                .expect("a render failure inside +meta must not fail the whole node");

        render_meta_tolerantly(&mut cfg, &env, &ctx, None);

        for (dir, expected_schema) in [("INT", "INT_SCHEMA"), ("CNT", "CNT_SCHEMA")] {
            let child = cfg
                .__additional_properties__
                .get(dir)
                .unwrap_or_else(|| panic!("{dir} should still be present in the config tree"));
            let ShouldBe::AndIs(child_config) = child else {
                panic!("{dir}'s own config should have parsed fine");
            };
            assert_eq!(
                child_config.schema,
                dbt_common::serde_utils::Omissible::Present(Some(expected_schema.to_string())),
                "{dir}'s own +schema override must survive the sibling +meta failure"
            );
        }
    }

    /// Regression for the review discussion on fs#14227's fix: a render failure nested inside
    /// one `+meta` key (here, `PRF.+post-hook`) must not also leave an unrelated sibling `+meta`
    /// key (`owner`) unrendered -- only the failing subtree should fall back to its raw value.
    /// <https://github.com/dbt-labs/fs/pull/14227#discussion_r3927327837>
    #[test]
    fn test_meta_render_failure_does_not_poison_sibling_meta_key() {
        let yaml = r#"
        +meta:
          owner: "{{ 1 + 1 }}"
          PRF:
            +post-hook: "{{ some_totally_unknown_macro() }}"
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let env = JinjaEnv::new(minijinja::Environment::new());
        let ctx: BTreeMap<String, Value> = BTreeMap::new();

        let mut cfg: ProjectModelConfig =
            into_typed_with_jinja(val, false, &env, &ctx, &[], None, true)
                .expect("a render failure inside +meta must not fail the whole node");

        render_meta_tolerantly(&mut cfg, &env, &ctx, None);

        let meta = cfg.meta.0.as_ref().expect("+meta should be present");
        match meta.get("owner").expect("owner key in +meta") {
            dbt_yaml::Value::Number(n, _) => assert_eq!(
                n.as_i64(),
                Some(2),
                "owner must still render even though a sibling +meta key failed"
            ),
            other => panic!("expected owner to render to a number, got {other:?}"),
        }

        match meta.get("PRF").expect("PRF key in +meta") {
            dbt_yaml::Value::Mapping(m, _) => {
                let post_hook = m
                    .get(dbt_yaml::Value::string("+post-hook".to_string()))
                    .expect("+post-hook should still be present, unrendered");
                match post_hook {
                    dbt_yaml::Value::String(s, _) => {
                        assert_eq!(s, "{{ some_totally_unknown_macro() }}")
                    }
                    other => panic!("expected +post-hook to stay a raw string, got {other:?}"),
                }
            }
            other => panic!("expected PRF to stay a mapping, got {other:?}"),
        }
    }

    /// The success path still renders `+meta`'s Jinja, matching dbt-core -- the fix only changes
    /// *when* that happens (after the main parse, tolerating failure) not *whether* it happens.
    #[test]
    fn test_meta_renders_successfully_when_valid() {
        let yaml = r#"
        +meta:
          demo: "{{ 1 + 2 }}"
        "#;

        let val: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let env = JinjaEnv::new(minijinja::Environment::new());
        let ctx: BTreeMap<String, Value> = BTreeMap::new();

        let mut cfg: ProjectModelConfig =
            into_typed_with_jinja(val, false, &env, &ctx, &[], None, true).unwrap();

        render_meta_tolerantly(&mut cfg, &env, &ctx, None);

        let meta = cfg.meta.0.as_ref().expect("+meta should be present");
        match meta.get("demo").expect("demo key in +meta") {
            dbt_yaml::Value::Number(n, _) => assert_eq!(n.as_i64(), Some(3)),
            other => panic!("expected number in +meta.demo, got {other:?}"),
        }
    }
}
