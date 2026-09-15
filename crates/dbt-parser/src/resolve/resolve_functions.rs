use std::collections::{HashMap, HashSet};
use std::{collections::BTreeMap, sync::Arc};

use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::path::DbtPath;
use dbt_common::static_analysis::{
    StaticAnalysisDeprecationOrigin, check_deprecated_static_analysis_kind,
};
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_common::{ErrorCode, FsResult, error::AbstractLocation, fs_err};
use dbt_jinja_utils::listener::DefaultJinjaTypeCheckEventListenerFactory;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_jinja_utils::{jinja_environment::JinjaEnv, node_resolver::NodeResolver};
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::common::{Access, DbtMaterialization, DbtQuoting};
use dbt_schemas::schemas::project::FunctionConfig;
use dbt_schemas::schemas::project::ResolvedConfig;
use dbt_schemas::schemas::{AbsorbedOverload, DbtFunctionAttr};
use dbt_schemas::{
    schemas::{
        CommonAttributes, DbtFunction, NodeBaseAttributes,
        common::NodeDependsOn,
        properties::{
            FUNCTION_LANGUAGE_JAVASCRIPT, FUNCTION_LANGUAGE_PYTHON, FunctionKind, FunctionOverload,
            FunctionProperties,
        },
        ref_and_source::{DbtRef, DbtSourceWrapper},
    },
    state::{DbtPackage, DbtRuntimeConfig, NodeResolverTracker},
};
use indexmap::IndexMap;
use minijinja::MacroSpans;

use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags, init_project_config,
};
use crate::renderer::{RenderCtx, RenderCtxInner};
use crate::resolve::resolve_utils::{
    build_unrendered_config, extract_config_map, validate_node_adapter,
};
use crate::utils::{
    RelationComponents, extract_resource_config_from_raw_project, parse_unrendered_config,
    update_node_relation_components,
};
use crate::{
    args::ResolveArgs,
    renderer::{SqlFileRenderResult, render_unresolved_sql_files},
    utils::{get_node_fqn, get_original_file_path, get_unique_id},
};
use dbt_common::tokiofs::read_to_string;

use super::resolve_properties::MinimalPropertiesEntry;

/// Body + dependencies of an overload-only SQL file, captured during the build
/// loop and merged into its root function during absorption. Overload SQL can
/// contain its own `ref()`/`source()`/`function()` calls; those dependencies
/// must be carried onto the root node so the DAG stays complete.
struct CapturedOverload {
    raw_body: String,
    compiled_body: String,
    refs: Vec<DbtRef>,
    functions: Vec<DbtRef>,
    sources: Vec<DbtSourceWrapper>,
}

#[allow(clippy::too_many_arguments)]
pub async fn resolve_functions(
    arg: &ResolveArgs,
    package: &DbtPackage,
    package_quoting: DbtQuoting,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    function_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    database: &str,
    schema: &str,
    default_adapter: AdapterType,
    adapter_quoting: &IndexMap<AdapterType, DbtQuoting>,
    package_name: &str,
    env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    node_resolver: &mut NodeResolver,
    token: &CancellationToken,
) -> FsResult<(
    HashMap<String, Arc<DbtFunction>>,
    HashMap<String, (String, MacroSpans)>,
)> {
    let mut functions: HashMap<String, Arc<DbtFunction>> = HashMap::new();
    let mut rendering_results: HashMap<String, (String, MacroSpans)> = HashMap::new();
    let dependency_package_name = dependency_package_name_from_ctx(&env, base_ctx);

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.functions.clone(),
        dependency_package_name.is_some(),
        || {
            init_project_config(
                &package.dbt_project.functions,
                DbtQuoting::default(),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                default_adapter,
            )
        },
        default_adapter,
    )?
    .with_resolve_defaults(arg.static_analysis.unwrap_or_default());

    let render_ctx = RenderCtx {
        inner: Arc::new(RenderCtxInner {
            args: arg.clone(),
            root_project_name: root_package.dbt_project.name.clone(),
            config_resolver,
            package_quoting,
            uses_snapshot_fqn: false,
            defer_render_errors_to_compile: false,
            base_ctx: base_ctx.clone(),
            package_name: package_name.to_string(),
            adapter_type: default_adapter,
            database: database.to_string(),
            schema: schema.to_string(),
            resource_paths: package
                .dbt_project
                .function_paths
                .as_ref()
                .unwrap_or(&vec![])
                .clone(),
        }),
        jinja_env: env.clone(),
        runtime_config: runtime_config.clone(),
        root_runtime_config: root_runtime_config.clone(),
    };

    // Raw config sources for `unrendered_config`, mirroring resolve_models.rs. These preserve
    // Jinja as raw strings so that env-aware configs (identical authored Jinja that renders
    // differently per target) are not flagged as `state:modified` (Approach A soundness
    // prerequisite — the Stage-1 wholesale comparison requires a fully populated
    // `unrendered_config`).
    let is_dependency = dependency_package_name.is_some();
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "functions",
        default_adapter,
    )?;
    let raw_root_project_functions_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "functions",
            default_adapter,
        )?)
    } else {
        None
    };

    // Snapshot raw schema.yml `config:` blocks before render_unresolved_sql_files nulls out
    // schema_value entries via std::mem::replace. Keyed by function name.
    let raw_schema_yml_configs: BTreeMap<String, BTreeMap<String, dbt_yaml::Value>> =
        function_properties
            .iter()
            .filter_map(|(key, mpe)| {
                let config_map = extract_config_map(&mpe.schema_value)?;
                Some((key.clone(), config_map))
            })
            .collect();

    let mut function_sql_resources_map =
        render_unresolved_sql_files::<FunctionConfig, FunctionProperties>(
            &render_ctx,
            &package.function_sql_files,
            function_properties,
            token,
            Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default()),
        )
        .await?;
    // make deterministic
    function_sql_resources_map.sort_by(|a, b| {
        a.asset
            .path
            .file_name()
            .cmp(&b.asset.path.file_name())
            .then(a.asset.path.cmp(&b.asset.path))
    });

    let mut pending_overloads: Vec<(String, Vec<FunctionOverload>)> = Vec::new();

    // Discovery pass: walk the parsed function properties once to learn, before
    // any nodes are built:
    //   - overload_stems: every file claimed via `defined_in`
    //   - root_stems:     every file that declares an `overloads:` block
    //   - pending_overloads: (root unique_id, overload defs) to absorb post-loop
    // SQL files can be listed in any order, so we must know the full overload set
    // up front — that way overload files never become standalone function nodes
    // (no register-then-delete). Root files are always built, even if they
    // (incorrectly) claim themselves, so the self-reference validation still fires.
    let mut overload_stems: HashSet<String> = HashSet::new();
    let mut root_stems: HashSet<String> = HashSet::new();
    for render_result in &function_sql_resources_map {
        let Some(overloads) = render_result
            .properties
            .as_ref()
            .and_then(|p| p.overloads.as_ref())
        else {
            continue;
        };
        if overloads.is_empty() {
            continue;
        }
        overload_stems.extend(overloads.iter().map(|o| o.defined_in.clone()));
        if let Some(stem) = render_result
            .asset
            .path
            .file_stem()
            .and_then(|s| s.to_str())
        {
            root_stems.insert(stem.to_string());
            let root_uid = get_unique_id(stem, package_name, None, "function");
            pending_overloads.push((root_uid, overloads.clone()));
        }
    }

    // Rendered bodies + dependencies of overload files, keyed by file stem,
    // captured during the build loop and absorbed into their root function
    // afterwards.
    let mut overload_bodies: HashMap<String, CapturedOverload> = HashMap::new();

    for SqlFileRenderResult {
        asset: dbt_asset,
        sql_file_info,
        config: mut model_config,
        raw_code,
        rendered_sql,
        macro_spans,
        properties: maybe_properties,
        status,
        patch_path,
        ..
    } in function_sql_resources_map.into_iter()
    {
        let function_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();

        // Overload-only SQL files are absorbed into their root function below;
        // they must never become standalone nodes in the manifest or node
        // resolver. Capture the rendered body for absorption and skip node
        // construction. Files that are themselves roots (even self-referencing
        // ones) are still built so absorption validations can run.
        if overload_stems.contains(function_name) && !root_stems.contains(function_name) {
            // Overload SQL may contain its own ref()/source()/function() calls.
            // Capture those dependencies (built the same way as for a standalone
            // node) so they can be merged onto the root function during absorption.
            let refs = sql_file_info
                .refs
                .iter()
                .map(|(model, project, version, location)| DbtRef {
                    name: model.to_owned(),
                    package: project.to_owned(),
                    version: version.clone(),
                    location: Some(location.with_file(&dbt_asset.path)),
                })
                .collect();
            let function_refs = sql_file_info
                .functions
                .iter()
                .map(|(name, package, location)| DbtRef {
                    name: name.to_owned(),
                    package: package.to_owned(),
                    version: None, // Functions don't have versions
                    location: Some(location.with_file(&dbt_asset.path)),
                })
                .collect();
            let sources = sql_file_info
                .sources
                .iter()
                .map(|(source, table, location)| DbtSourceWrapper {
                    source: vec![source.to_owned(), table.to_owned()],
                    location: Some(location.with_file(&dbt_asset.path)),
                })
                .collect();
            overload_bodies.insert(
                function_name.to_string(),
                CapturedOverload {
                    raw_body: raw_code,
                    compiled_body: rendered_sql,
                    refs,
                    functions: function_refs,
                    sources,
                },
            );
            continue;
        }

        let original_file_path =
            get_original_file_path(&dbt_asset.base_path, &arg.io.in_dir, &dbt_asset.path);

        let unique_id = get_unique_id(function_name, package_name, None, "function");
        // See `resolve_models`: the flag overrides the config. A function selects
        // explicitly; it has no attached node to inherit from.
        validate_node_adapter(model_config.adapter, &dbt_asset.path)?;
        let resolved_node_adapter = arg.adapter_override.or(model_config.adapter);
        // See `resolve_models`: both remaining quoting layers depend on which
        // adapter the node runs on, which is only known after the config merge.
        let selected_adapter = resolved_node_adapter.unwrap_or(default_adapter);
        model_config.quoting = resolve_package_quoting(
            Some(match adapter_quoting.get(&selected_adapter) {
                Some(authored) => model_config.quoting.filled_from(authored),
                None => model_config.quoting,
            }),
            selected_adapter,
        );
        let static_analysis = model_config.static_analysis.clone();
        if let Some(spanned) = model_config.get_static_analysis() {
            let kind = spanned.into_inner();
            if kind != arg.static_analysis.unwrap_or_default() {
                check_deprecated_static_analysis_kind(
                    kind,
                    StaticAnalysisDeprecationOrigin::NodeConfig {
                        unique_id: unique_id.as_str(),
                    },
                    dependency_package_name,
                );
                if dbt_asset.is_python() {
                    crate::validation::warn_python_static_analysis(kind, unique_id.as_str());
                }
            }
        }

        let fqn = get_node_fqn(
            package_name,
            dbt_asset.path.to_owned(),
            vec![function_name.to_owned()],
            package
                .dbt_project
                .function_paths
                .as_ref()
                .unwrap_or(&vec![]),
        );

        // Capture inline `{{ config(...) }}` overrides from the function SQL file, Jinja preserved.
        let raw_config_call_dict = read_to_string(dbt_asset.base_path.join(&dbt_asset.path))
            .await
            .ok()
            .and_then(|sql| parse_unrendered_config(&sql, false));

        // Merge the four raw sources (project < root < schema.yml < inline) into the node's
        // `unrendered_config`. Functions do not support pre_hook/post_hook, so hook-name
        // normalization is disabled.
        let unrendered_config = build_unrendered_config(
            &fqn,
            &raw_local_project_config,
            raw_root_project_functions_cfg.as_ref(),
            raw_schema_yml_configs.get(function_name),
            raw_config_call_dict.as_ref(),
            false,
            default_adapter,
        )?;

        let properties = if let Some(properties) = maybe_properties {
            properties
        } else {
            FunctionProperties::empty(function_name.to_owned())
        };

        let depends_on = NodeDependsOn {
            macros: vec![],
            nodes: vec![],
            nodes_with_ref_location: vec![],
        };

        rendering_results.insert(unique_id.clone(), (rendered_sql.clone(), macro_spans));

        let mut function = DbtFunction {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.clone(),
                name: function_name.to_owned(),
                name_span: Default::default(),
                package_name: package_name.to_owned(),
                path: DbtPath::from(&dbt_asset.path),
                original_file_path,
                patch_path: patch_path.map(DbtPath::from),
                fqn,
                description: properties.description,
                raw_code: Some(raw_code),
                checksum: sql_file_info.checksum,
                language: if dbt_asset.is_python() {
                    Some(FUNCTION_LANGUAGE_PYTHON.to_string())
                } else if dbt_asset.is_javascript() {
                    Some(FUNCTION_LANGUAGE_JAVASCRIPT.to_string())
                } else {
                    properties.language.clone()
                },
                tags: model_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: Default::default(),
                meta: model_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: selected_adapter,
                // A function is not a relation, so there is nothing to bind into another
                // platform's catalog: no `+propagate` config exists for this node type.
                propagate: Vec::new(),
                database: database.to_string(), // will be updated below
                schema: schema.to_string(),     // will be updated below
                alias: "".to_owned(),           // will be updated below
                relation_name: None,            // will be updated below
                materialized: DbtMaterialization::Function,
                static_analysis,
                static_analysis_off_reason: None,
                compute: None,
                quoting: model_config
                    .quoting
                    .try_into()
                    .expect("DbtQuoting should be set"),
                quoting_ignore_case: false,
                enabled: model_config.enabled,
                extended_model: false,
                persist_docs: None,
                columns: vec![],
                depends_on,
                refs: sql_file_info
                    .refs
                    .iter()
                    .map(|(model, project, version, location)| DbtRef {
                        name: model.to_owned(),
                        package: project.to_owned(),
                        version: version.clone(),
                        location: Some(location.with_file(&dbt_asset.path)),
                    })
                    .collect(),
                functions: sql_file_info
                    .functions
                    .iter()
                    .map(|(function_name, package, location)| DbtRef {
                        name: function_name.to_owned(),
                        package: package.to_owned(),
                        version: None, // Functions don't have versions
                        location: Some(location.with_file(&dbt_asset.path)),
                    })
                    .collect(),
                sources: sql_file_info
                    .sources
                    .iter()
                    .map(|(source, table, location)| DbtSourceWrapper {
                        source: vec![source.to_owned(), table.to_owned()],
                        location: Some(location.with_file(&dbt_asset.path)),
                    })
                    .collect(),
                metrics: vec![],
                unrendered_config,
            },
            __function_attr__: DbtFunctionAttr {
                access: properties
                    .config
                    .as_ref()
                    .and_then(|c| c.access.clone())
                    .unwrap_or(Access::Private),
                group: properties.config.as_ref().and_then(|c| c.group.clone()),
                language: if dbt_asset.is_python() {
                    Some(FUNCTION_LANGUAGE_PYTHON.to_string())
                } else if dbt_asset.is_javascript() {
                    Some(FUNCTION_LANGUAGE_JAVASCRIPT.to_string())
                } else {
                    properties.language.clone()
                },
                on_configuration_change: properties
                    .config
                    .as_ref()
                    .and_then(|c| c.on_configuration_change.clone()),
                returns: properties.returns.clone(),
                arguments: properties.arguments.clone(),
                overloads: Vec::new(),
            },
            // TODO: can we just take model_config and apply function_kind default elsewhere?
            deprecated_config: FunctionConfig {
                enabled: Some(model_config.enabled),
                group: model_config.group.clone(),
                tags: model_config.tags.clone(),
                meta: model_config.meta.clone(),
                function_kind: model_config
                    .function_kind
                    .clone()
                    .or(Some(FunctionKind::Scalar)),
                volatility: model_config.volatility.clone(),
                runtime_version: model_config.runtime_version.clone(),
                entry_point: model_config.entry_point.clone(),
                packages: model_config.packages.clone(),
                snowflake: model_config.snowflake.clone(),
                pre_hook: model_config.pre_hook.clone(),
                post_hook: model_config.post_hook.clone(),
                ..Default::default()
            },
            __other__: BTreeMap::new(),
        };

        let components = RelationComponents {
            database: model_config.database.clone().into_inner().unwrap_or(None),
            schema: model_config.schema.clone().into_inner().unwrap_or(None),
            alias: model_config.alias.clone(),
            store_failures: None,
        };

        // update model components using the generate_relation_components function
        update_node_relation_components(
            &mut function,
            &env,
            &root_package.dbt_project.name,
            package_name,
            base_ctx,
            &components,
            default_adapter,
        )?;

        match node_resolver.insert_function(&function, default_adapter, status) {
            Ok(_) => (),
            Err(e) => {
                let err_with_loc = e.with_location(dbt_asset.path.clone());
                emit_error_log_from_fs_error(err_with_loc);
            }
        }

        functions.insert(unique_id, Arc::new(function));
    }

    // Post-loop: absorb overloads into root functions
    let mut claimed_overloads: HashMap<String, String> = Default::default();
    for (root_uid, overload_defs) in pending_overloads {
        let mut absorbed = Vec::new();
        let mut merged_refs: Vec<DbtRef> = Vec::new();
        let mut merged_functions: Vec<DbtRef> = Vec::new();
        let mut merged_sources: Vec<DbtSourceWrapper> = Vec::new();
        let root_name = functions
            .get(&root_uid)
            .map(|f| f.__common_attr__.name.clone())
            .unwrap_or_default();

        for overload_def in &overload_defs {
            let overload_name = &overload_def.defined_in;

            if overload_name == &root_name {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Function '{}' cannot list itself in overloads",
                    root_name
                ));
            }

            if let Some(claimed_by) = claimed_overloads.get(overload_name) {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Function overload '{}' (defined_in) is already used by '{}'. \
                     Each overload file can only belong to one root function.",
                    overload_name,
                    claimed_by
                ));
            }

            // A file that is itself a root (declares its own `overloads:`) is
            // built as a node and never captured into overload_bodies, so guard
            // this case explicitly — otherwise it would surface as a misleading
            // "file not found" below even though the file exists.
            if root_stems.contains(overload_name) {
                return Err(fs_err!(
                    ErrorCode::InvalidConfig,
                    "Function overload '{}' (defined_in) is itself a root function with its own \
                     `overloads` block and cannot also be used as an overload of '{}'.",
                    overload_name,
                    root_name
                ));
            }

            let captured = overload_bodies.get(overload_name).ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Function overload defined_in '{}' not found for root function '{}'. \
                     Expected a SQL file at functions/{}.sql",
                    overload_name,
                    root_name,
                    overload_name
                )
            })?;

            claimed_overloads.insert(overload_name.clone(), root_uid.clone());

            merged_refs.extend(captured.refs.iter().cloned());
            merged_functions.extend(captured.functions.iter().cloned());
            merged_sources.extend(captured.sources.iter().cloned());

            absorbed.push(AbsorbedOverload {
                defined_in: overload_def.defined_in.clone(),
                arguments: overload_def.arguments.clone(),
                returns: overload_def.returns.clone(),
                description: overload_def.description.clone(),
                raw_body: Some(captured.raw_body.clone()),
                compiled_body: Some(captured.compiled_body.clone()),
            });
        }

        // Add absorbed overloads to the root function, along with the deps their
        // SQL pulls in. Those edges must live on the root so the DAG is complete;
        // resolve_dependencies dedups against the root's own refs/sources later.
        let root_func = functions.get_mut(&root_uid).unwrap();
        let root_mut = Arc::make_mut(root_func);
        root_mut.__function_attr__.overloads = absorbed;
        root_mut.__base_attr__.refs.extend(merged_refs);
        root_mut.__base_attr__.functions.extend(merged_functions);
        root_mut.__base_attr__.sources.extend(merged_sources);
    }

    Ok((functions, rendering_results))
}
