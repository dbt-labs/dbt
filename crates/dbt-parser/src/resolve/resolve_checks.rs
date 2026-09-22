use std::collections::HashMap;
use std::{collections::BTreeMap, sync::Arc};

use crate::resolve::resolve_utils::err_resource_name_has_spaces;

use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::path::DbtPath;
use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultJinjaTypeCheckEventListenerFactory;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::common::{DbtMaterialization, DbtQuoting, ResolvedQuoting};
use dbt_schemas::schemas::project::{CheckConfig, SUPPORTED_INFO_SCHEMA_VERSIONS};
use dbt_schemas::schemas::properties::CheckProperties;
use dbt_schemas::state::ModelStatus;
use dbt_schemas::{
    schemas::{
        CommonAttributes, DbtCheck, DbtCheckAttr, NodeBaseAttributes, common::NodeDependsOn,
    },
    state::{DbtPackage, DbtRuntimeConfig},
};
use minijinja::MacroSpans;

use super::resolve_properties::MinimalPropertiesEntry;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::renderer::{RenderCtx, RenderCtxInner};
use crate::{
    args::ResolveArgs,
    renderer::{SqlFileRenderResult, render_unresolved_sql_files},
    utils::{get_node_fqn, get_original_file_path, get_unique_id},
};

/// Resolve check resources for a package into [`DbtCheck`] nodes and rendered SQL.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_checks(
    arg: &ResolveArgs,
    package: &DbtPackage,
    package_quoting: DbtQuoting,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    check_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    database: &str,
    schema: &str,
    adapter_type: AdapterType,
    package_name: &str,
    env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    token: &CancellationToken,
) -> FsResult<(
    HashMap<String, Arc<DbtCheck>>,
    HashMap<String, Arc<DbtCheck>>,
    HashMap<String, (String, MacroSpans)>,
)> {
    let mut checks: HashMap<String, Arc<DbtCheck>> = HashMap::new();
    let mut disabled_checks: HashMap<String, Arc<DbtCheck>> = HashMap::new();
    let mut rendering_results: HashMap<String, (String, MacroSpans)> = HashMap::new();
    let jinja_type_checking_event_listener_factory =
        Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default());

    let dependency_package_name = dependency_package_name_from_ctx(&env, base_ctx);

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.checks.clone(),
        dependency_package_name.is_some(),
        || {
            init_project_config(
                &package.dbt_project.checks,
                (),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                adapter_type,
            )
        },
        adapter_type,
    )?;

    let render_ctx = RenderCtx {
        inner: Arc::new(RenderCtxInner {
            args: arg.clone(),
            root_project_name: root_package.dbt_project.name.clone(),
            config_resolver,
            package_quoting,
            uses_snapshot_fqn: false,
            base_ctx: base_ctx.clone(),
            package_name: package_name.to_string(),
            adapter_type,
            database: database.to_string(),
            schema: schema.to_string(),
            resource_paths: package
                .dbt_project
                .check_paths
                .as_ref()
                .unwrap_or(&vec![])
                .clone(),
            // Checks execute entirely within the parse-time gate; there is no later compile
            // phase to defer a render error to, so a render failure must surface immediately
            // (matches resolve_functions.rs, the other resource type with no compile-time
            // re-render).
            defer_render_errors_to_compile: false,
        }),
        jinja_env: env.clone(),
        runtime_config: runtime_config.clone(),
        root_runtime_config: root_runtime_config.clone(),
    };

    let mut check_sql_resources_map = render_unresolved_sql_files::<CheckConfig, CheckProperties>(
        &render_ctx,
        &package.check_files,
        check_properties,
        token,
        jinja_type_checking_event_listener_factory.clone(),
    )
    .await?;
    // make deterministic
    check_sql_resources_map.sort_by(|a, b| {
        a.asset
            .path
            .file_name()
            .cmp(&b.asset.path.file_name())
            .then(a.asset.path.cmp(&b.asset.path))
    });

    // `info_schema` is package-scoped, not per-check (see `InfoSchemaConfig`), so this package's
    // declared version is validated once, up front, rather than once per check file.
    if !check_sql_resources_map.is_empty() {
        let version = package
            .dbt_project
            .info_schema
            .as_ref()
            .and_then(|c| c.version);
        match version {
            Some(v) if SUPPORTED_INFO_SCHEMA_VERSIONS.contains(&v) => {}
            other => {
                return Err(err_invalid_info_schema_version(
                    package_name,
                    &package.package_root_path.join("dbt_project.yml"),
                    other,
                ));
            }
        }
    }

    for SqlFileRenderResult {
        asset: dbt_asset,
        sql_file_info,
        config: check_config,
        raw_code,
        rendered_sql,
        macro_spans,
        properties: maybe_properties,
        status,
        patch_path,
        ..
    } in check_sql_resources_map.into_iter()
    {
        let check_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();

        if check_name.contains(' ') {
            return Err(err_resource_name_has_spaces(check_name, &dbt_asset.path));
        }

        let original_file_path =
            get_original_file_path(&dbt_asset.base_path, &arg.io.in_dir, &dbt_asset.path);

        let unique_id = get_unique_id(check_name, package_name, None, "check");

        let fqn = get_node_fqn(
            package_name,
            dbt_asset.path.to_owned(),
            vec![check_name.to_owned()],
            package.dbt_project.check_paths.as_ref().unwrap_or(&vec![]),
        );

        let properties = if let Some(properties) = maybe_properties {
            properties
        } else {
            CheckProperties::empty(check_name.to_owned())
        };

        let is_enabled = matches!(status, ModelStatus::Enabled);

        let dbt_check = DbtCheck {
            __common_attr__: CommonAttributes {
                name: check_name.to_owned(),
                package_name: package_name.to_owned(),
                path: DbtPath::from(dbt_asset.path.to_owned()),
                name_span: dbt_common::Span::default(),
                original_file_path,
                unique_id: unique_id.clone(),
                fqn,
                description: properties.description.clone(),
                patch_path: patch_path.map(DbtPath::from),
                checksum: sql_file_info.checksum.clone(),
                language: Some("sql".to_string()),
                raw_code: Some(raw_code),
                tags: check_config
                    .tags
                    .inner()
                    .clone()
                    .map(|tags| tags.into())
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: check_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: adapter_type,
                // This node type has no `+propagate` config; nothing is published.
                propagate: Vec::new(),
                effective_propagation_target: None,
                // A check is never materialized, so it has no relation of its own. These stay
                // empty rather than being run through `update_node_relation_components`, which
                // would invent a database/schema/alias for something that is never written.
                database: String::new(),
                schema: String::new(),
                alias: String::new(),
                relation_name: None,
                enabled: is_enabled,
                extended_model: false,
                persist_docs: None,
                materialized: DbtMaterialization::Analysis,
                quoting: ResolvedQuoting::trues(),
                quoting_ignore_case: false,
                static_analysis: Default::default(),
                static_analysis_off_reason: None,
                compute: None,
                columns: Default::default(),
                // A check queries project metadata, not relations: it has no refs, sources,
                // function calls or metric dependencies, and therefore no DAG parents. Ordering
                // comes from `__check_attr__.phase`.
                depends_on: NodeDependsOn::default(),
                refs: vec![],
                sources: vec![],
                functions: vec![],
                metrics: vec![],
                unrendered_config: Default::default(),
            },
            __check_attr__: DbtCheckAttr {
                // Carried to execution: the parse-time runner executes exactly this text.
                // inferred from; checks have no render task to compile them later.
                compiled_sql: Some(rendered_sql.clone()),
            },
            deprecated_config: check_config.into(),
        };

        if is_enabled {
            checks.insert(unique_id.to_owned(), Arc::new(dbt_check));
            rendering_results.insert(
                unique_id.to_owned(),
                (rendered_sql.clone(), macro_spans.clone()),
            );
        } else {
            // Recorded rather than dropped so callers can tell "this check is turned off" from
            // "no such check" — `dbt check <name>` must stay a no-op success for the former and
            // report an unknown name for the latter.
            disabled_checks.insert(unique_id.to_owned(), Arc::new(dbt_check));
        }
    }

    for (check_name, mpe) in check_properties.iter() {
        if !mpe.schema_value.is_null() {
            let err = fs_err!(
                code => ErrorCode::NoNodeForYamlKey,
                loc => mpe.relative_path.clone(),
                "Unused schema.yml entry for check '{}'",
                check_name,
            );
            emit_warn_log_from_fs_error(*err);
        }
    }

    Ok((checks, disabled_checks, rendering_results))
}

/// `info_schema.version` has no default (see `InfoSchemaConfig`), so a package with checks but no
/// version set, or an unsupported one, is a hard config error rather than a silent pass-through to
/// whichever version the checks happened to be written against.
fn err_invalid_info_schema_version(
    package_name: &str,
    path: &std::path::Path,
    got: Option<u32>,
) -> Box<dbt_common::FsError> {
    let supported = SUPPORTED_INFO_SCHEMA_VERSIONS
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    match got {
        None => fs_err!(
            code => ErrorCode::DbtYamlValidationError,
            loc => path.to_path_buf(),
            "Package '{}' has checks but no `info_schema.version` set. This is required -- set \
             it in the package's own `dbt_project.yml`:\n\ninfo_schema:\n  version: 1\n\n\
             Supported: {}.",
            package_name,
            supported,
        ),
        Some(v) => fs_err!(
            code => ErrorCode::DbtYamlValidationError,
            loc => path.to_path_buf(),
            "Package '{}' sets `info_schema.version: {}`, which is not supported. Supported: {}.",
            package_name,
            v,
            supported,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn missing_info_schema_version_names_what_to_set_and_where() {
        let err =
            err_invalid_info_schema_version("my_package", &PathBuf::from("dbt_project.yml"), None);
        let msg = format!("{err}");
        assert!(msg.contains("my_package"), "should name the package: {msg}");
        assert!(
            msg.contains("no `info_schema.version` set"),
            "should say it is unset: {msg}"
        );
        assert!(
            msg.contains("dbt_project.yml") && msg.contains("info_schema:\n  version: 1"),
            "should show the project-level fix: {msg}"
        );
    }

    #[test]
    fn unsupported_info_schema_version_names_the_value_and_the_supported_set() {
        let err = err_invalid_info_schema_version(
            "my_package",
            &PathBuf::from("dbt_project.yml"),
            Some(2),
        );
        let msg = format!("{err}");
        assert!(msg.contains("my_package"), "should name the package: {msg}");
        assert!(
            msg.contains("info_schema.version: 2"),
            "should name the offending value: {msg}"
        );
        assert!(
            msg.contains("Supported: 1"),
            "should name what is actually supported: {msg}"
        );
    }

    #[test]
    fn supported_version_is_accepted_by_the_match_arm_used_at_the_call_site() {
        // Mirrors the exact guard in the resolution loop: `Some(v) if
        // SUPPORTED_INFO_SCHEMA_VERSIONS.contains(&v)`.
        let accepted =
            |v: Option<u32>| matches!(v, Some(v) if SUPPORTED_INFO_SCHEMA_VERSIONS.contains(&v));
        assert!(accepted(Some(1)));
        assert!(!accepted(Some(2)));
        assert!(!accepted(None));
    }
}
