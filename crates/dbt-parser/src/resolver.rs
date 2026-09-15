//! Module containing the entrypoint for the resolve phase.
use dbt_adapter_core::AdapterType;
#[allow(unused_imports)]
use dbt_common::FsError;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::DBT_GENERIC_TESTS_DIR_NAME;
use dbt_common::io_args::FsCommand;
use dbt_common::once_cell_vars::DISPATCH_CONFIG;
use dbt_common::path::DbtPath;
use dbt_common::stdfs;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::tracing::event_info::store_event_attributes;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_jinja_utils::JinjaFactory;
use dbt_jinja_utils::invocation_args::InvocationArgs;
use dbt_jinja_utils::invocation_graph::reset_invocation_graph;
use dbt_jinja_utils::listener::JinjaTypeCheckingEventListenerFactory;
use dbt_jinja_utils::node_resolver::{
    NodeResolver, PackageSearchOrder, check_for_model_deprecations, resolve_dependencies,
};
use dbt_jinja_utils::phases::parse::{
    build_docs_jinja_environment, build_docs_resolve_context, build_resolve_context,
};
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::common::{Access, DbtIncrementalStrategy, DbtMaterialization};
use dbt_schemas::schemas::macros::{DbtDocsMacro, build_macro_units};
use dbt_schemas::schemas::properties::{
    FUNCTION_LANGUAGE_JAVASCRIPT, FUNCTION_LANGUAGE_PYTHON, FUNCTION_LANGUAGE_SQL, FunctionKind,
    ModelProperties,
};
use dbt_schemas::schemas::{DbtModel, DbtSeed, InternalDbtNode, Nodes};

use dbt_schemas::schemas::dbt_catalogs::DbtCatalogs;

use crate::args::ResolveArgs;
use crate::dbt_project_config::{RootProjectConfigs, build_root_project_configs};
use crate::resolve::resolve_groups::resolve_groups;
use crate::resolve::resolve_operations::resolve_operations;
use crate::resolve::resolve_query_comment::resolve_query_comment;
use crate::resolver_hooks::ResolverHooks;
use crate::utils::{self, clear_package_diagnostics};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::common::DbtQuoting;
use dbt_schemas::schemas::telemetry::{ExecutionPhase, NodeType, PhaseExecuted};
use dbt_schemas::state::{
    DbtPackage, GenericTestAsset, GetColumnsInRelationCalls, GetRelationCalls, Macros,
    ManifestPathConfig, PatternedDanglingSources, RenderResults,
};
use dbt_schemas::state::{DbtRuntimeConfig, Operations};
use dbt_schemas::state::{DbtState, ResolverState};
use minijinja::constants::CURRENT_PATH;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::resolve::resolve_analyses::resolve_analyses;
use crate::resolve::resolve_checks::resolve_checks;
use crate::resolve::resolve_exposures::resolve_exposures;
use crate::resolve::resolve_functions::resolve_functions;
use crate::resolve::resolve_macros::apply_macro_patches;
use crate::resolve::resolve_macros::resolve_docs_macros;
use crate::resolve::resolve_macros::resolve_macros;
use crate::resolve::resolve_macros::typecheck_macros;
use crate::resolve::resolve_metrics::resolve_metrics;
use crate::resolve::resolve_models::resolve_models;
use crate::resolve::resolve_properties;
use crate::resolve::resolve_properties::resolve_minimal_properties;
use crate::resolve::resolve_saved_queries::resolve_saved_queries;
use crate::resolve::resolve_seeds::resolve_seeds;
use crate::resolve::resolve_semantic_models::resolve_semantic_models;
use crate::resolve::resolve_snapshots::resolve_snapshots;
use crate::resolve::resolve_sources::resolve_sources;
use crate::resolve::resolve_tests::resolve_data_tests::resolve_data_tests;
use crate::resolve::resolve_tests::resolve_unit_tests::resolve_unit_tests;
use crate::resolve::resolve_utils::validate_adapter_project_configs;

use crate::resolve::primary_key_inference::infer_and_apply_primary_keys;
use crate::resolve::resolve_selectors::{
    resolve_final_selectors, resolve_manifest_selectors, resolve_selectors_from_yaml,
};
use crate::unused_config_paths::check_unused_resource_config_paths;

use crate::constants::DEFAULT_OVERVIEW_CONTENTS;

/// Entrypoint for the resolve phase.
///
/// It is responsible for resolving all project source files (i.e. models, seeds, tests,
/// macros etc.) and propagating all configuration properties.
///
/// The final product is the parsed [DbtManifest], along with the collected
/// macros to be used during compilation.
#[tracing::instrument(
    skip_all,
    fields(
        _e = ?store_event_attributes(PhaseExecuted::start_general(ExecutionPhase::Parse)),
    )
)]
#[allow(clippy::too_many_arguments)]
pub async fn resolve(
    arg: &ResolveArgs,
    invocation_args: &InvocationArgs,
    dbt_state: Arc<DbtState>,
    macros: Macros,
    nodes: Nodes,
    disabled_nodes: Nodes,
    get_relation_calls: GetRelationCalls,
    get_columns_in_relation_calls: GetColumnsInRelationCalls,
    patterned_dangling_sources: PatternedDanglingSources,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    resolver_hooks: Arc<dyn ResolverHooks>,
    jinja_factory: Arc<dyn JinjaFactory>,
) -> FsResult<(ResolverState, Arc<JinjaEnv>)> {
    // Hand this invocation a fresh `graph` mapping before any Jinja renders.
    // Macros use `graph` as scratch state (Elementary's `set_cache`), and in a
    // long-lived process (LSP, service) one invocation's scratch state must not
    // be visible to the next. dbt-labs/fs#13454.
    reset_invocation_graph();

    // Get the root project name
    let root_project_name = dbt_state.root_project_name();

    let mut macros = macros;
    let mut nodes = nodes;
    let mut get_relation_calls = get_relation_calls;
    let mut get_columns_in_relation_calls = get_columns_in_relation_calls;
    let mut patterned_dangling_sources = patterned_dangling_sources;

    // First, resolve all of the macros from each package
    for package in &dbt_state.packages {
        token.check_cancellation()?;

        let macro_files = package.macro_files.iter().chain(&package.snapshot_files);
        let resolved_macros = resolve_macros(
            macro_files.collect::<Vec<_>>().as_slice(),
            package.embedded_file_contents.as_ref(),
        )?;
        macros.macros.extend(resolved_macros);
        let docs_macros = resolve_docs_macros(
            &arg.io,
            &package.docs_files,
            package.embedded_file_contents.as_ref(),
        )?;
        macros.docs_macros.extend(docs_macros);
    }

    inject_default_overview(&mut macros.docs_macros);

    let adapter_type = dbt_state.dbt_profile.default_db_config().adapter_type();

    // Build the root project config
    let root_project_quoting =
        resolve_package_quoting(*dbt_state.root_project().quoting, adapter_type);

    // The factory builds the environment (and registers any extra functions it
    // provides) before any model SQL is rendered for static analysis.
    let jinja_env = Arc::new(
        jinja_factory.create_parse_jinja_environment(
            root_project_name,
            &dbt_state.dbt_profile.profile,
            &dbt_state.dbt_profile.target,
            adapter_type,
            dbt_state.dbt_profile.default_db_config().clone(),
            dbt_state.dbt_profile.adapter_types(),
            root_project_quoting,
            build_macro_units(&macros.macros, &arg.io.in_dir),
            dbt_state.vars.clone(),
            dbt_state.cli_vars.clone(),
            dbt_state.root_project_flags(),
            dbt_state.run_started_at,
            invocation_args,
            macros
                .macros
                .values()
                .map(|m| m.package_name.clone())
                .collect(),
            arg.io.clone(),
            dbt_state.catalogs.clone(),
        )?,
    );

    // Load and resolve selectors
    let resolved_selectors_map = resolve_selectors_from_yaml(arg, root_project_name, &jinja_env)?;
    let manifest_selectors = resolve_manifest_selectors(resolved_selectors_map.clone())?;
    let resolved_selectors = resolve_final_selectors(resolved_selectors_map, arg)?;

    // let mut nodes = Nodes::default();
    let mut disabled_nodes = disabled_nodes;
    resolver_hooks.pre_resolve(&arg.io, adapter_type, &mut nodes, root_project_quoting)?;
    // The root project's `adapters:` list is validated once per run, not per
    // package: it is a root-only key, so re-checking it per package would repeat
    // the same warning for every dependency.
    validate_adapter_project_configs(
        dbt_state.root_project().adapters.as_ref(),
        &dbt_state.dbt_profile.adapters,
    );
    let root_project_configs = build_root_project_configs(
        dbt_state.root_project(),
        &dbt_state.dbt_profile.adapters,
        dbt_state.dbt_profile.default_adapter,
    )?;
    let root_project_configs = Arc::new(root_project_configs);
    // Process packages in topological order

    let mut node_resolver = NodeResolver::from_dbt_nodes(
        &nodes,
        adapter_type,
        root_project_name.to_string(),
        None,
        arg.sample_config.clone(),
        arg.sample_renaming.clone(),
        arg.command == FsCommand::Compile || arg.command == FsCommand::Test,
        PackageSearchOrder::resolve(dbt_state.root_project().flags.as_ref()),
    )?;
    let mut collector = RenderResults {
        rendering_results: BTreeMap::new(),
    };

    let package_waves = utils::prepare_package_dependency_levels(dbt_state.clone());

    // Build every package's runtime config up front. `DbtRuntimeConfig::new` only reads
    // `dbt_state`/`arg.io.in_dir`, never parse output, so it doesn't need to wait on the
    // waves below. This is what lets a *dependency* package's naming macros and model
    // contexts see the ROOT project's config, matching dbt Core's `self.root_project`.
    let all_runtime_configs: Arc<BTreeMap<String, Arc<DbtRuntimeConfig>>> = {
        let mut configs: BTreeMap<String, Arc<DbtRuntimeConfig>> = BTreeMap::new();
        for package_name in package_waves.iter().flatten() {
            let package = dbt_state
                .packages
                .iter()
                .find(|p| &p.dbt_project.name == package_name)
                .ok_or_else(|| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Encountered unexpected package not found in project: {}",
                        package_name
                    )
                })?;
            let vars = dbt_state
                .vars
                .get(package_name)
                .expect("All packages should have vars initialized");
            let runtime_config = Arc::new(DbtRuntimeConfig::new(
                &arg.io.in_dir,
                package,
                &dbt_state.dbt_profile,
                &configs,
                vars,
                &dbt_state.cli_vars,
            ));
            dbt_schemas::state::register_global_runtime_config(
                package_name.clone(),
                runtime_config.clone(),
            );
            configs.insert(package_name.clone(), runtime_config);
        }
        Arc::new(configs)
    };
    let root_runtime_config = all_runtime_configs
        .get(root_project_name)
        .expect("root project runtime config must be built by the pre-pass above")
        .clone();

    let mut semantic_layer_spec_is_legacy = false;
    let mut test_name_truncations: HashMap<String, String> = HashMap::new();
    let all_macro_properties: BTreeMap<
        String,
        BTreeMap<String, resolve_properties::MinimalPropertiesEntry>,
    >;

    let (
        resolved_nodes,
        resolved_disabled_nodes,
        resolved_collector,
        resolved_semantic_layer_spec_is_legacy,
        resolved_test_name_truncations,
        resolved_macro_properties,
    ) = resolve_package_waves(
        package_waves,
        arg,
        dbt_state.clone(),
        root_project_name,
        root_project_configs.clone(),
        adapter_type,
        &macros,
        jinja_env.clone(),
        &mut node_resolver,
        all_runtime_configs.clone(),
        root_runtime_config.clone(),
        token,
        jinja_type_checking_event_listener_factory.clone(),
    )
    .await?;
    nodes.extend(resolved_nodes);
    disabled_nodes.extend(resolved_disabled_nodes);
    collector
        .rendering_results
        .extend(resolved_collector.rendering_results);
    semantic_layer_spec_is_legacy |= resolved_semantic_layer_spec_is_legacy;
    test_name_truncations.extend(resolved_test_name_truncations);
    all_macro_properties = resolved_macro_properties;

    // Read the validate_macro_args flag from dbt_project.yml (defaults to true)
    let validate_macro_args = dbt_state
        .root_project_flags()
        .get("validate_macro_args")
        .map(|v| v.is_true())
        .unwrap_or(true);

    // Macro properties render in the restricted documentation context, so a project macro
    // cannot execute from a `description:`. From dbt-labs/dbt-core#14494.
    let docs_jinja_env = build_docs_jinja_environment(&jinja_env);
    for (package_name, macro_properties) in all_macro_properties {
        // Get the base context for this package
        let package = dbt_state
            .packages
            .iter()
            .find(|p| p.dbt_project.name == package_name);
        if let Some(package) = package {
            let docs_ctx = build_docs_resolve_context(
                root_project_name,
                package.dbt_project.name.as_str(),
                &macros.docs_macros,
                &jinja_env,
            )?;
            apply_macro_patches(
                &mut macros.macros,
                &macro_properties,
                &package_name,
                &docs_jinja_env,
                &docs_ctx,
                (package_name != root_project_name).then_some(package_name.as_str()),
                validate_macro_args,
            )?;
        }
    }

    typecheck_macros(
        &arg.io,
        &mut macros.macros,
        jinja_env.clone(),
        adapter_type,
        root_project_name,
        minijinja::Value::from_dyn_object(jinja_env.env.get_dbt_and_adapters_namespaces()),
    )?;

    // Ensure that there are no duplicate relations
    check_relation_uniqueness(&nodes)?;

    match nodes.warn_on_microbatch(adapter_type) {
        Ok(_) => {}
        Err(e) => {
            emit_warn_log_from_fs_error(*e);
        }
    }

    let parse_adapter = jinja_env
        .get_adapter()
        .expect("parse adapter must be initialized");
    let parse_adapter_state = parse_adapter
        .parse_adapter_state()
        .expect("adapter must be configured for the parse phase");
    let (
        get_relation_calls_from_parse,
        get_columns_in_relation_calls_from_parse,
        patterned_dangling_sources_from_parse,
    ) = parse_adapter_state.relations_to_fetch();
    get_relation_calls.extend(get_relation_calls_from_parse?);
    get_columns_in_relation_calls.extend(get_columns_in_relation_calls_from_parse?);
    patterned_dangling_sources.extend(patterned_dangling_sources_from_parse);

    // Resolve operations (on_run_start and on_run_end) with rendering and dependency extraction
    let mut operations = Operations::default();
    for package in &dbt_state.packages {
        // Get the package-specific runtime config so operations can access package vars
        let package_runtime_config = all_runtime_configs
            .get(&package.dbt_project.name)
            .unwrap_or(&root_runtime_config);

        let (on_run_start, on_run_end) = resolve_operations(
            &package.dbt_project,
            &package.package_root_path,
            &arg.io.in_dir,
            &jinja_env,
            arg.static_analysis,
            adapter_type,
            &dbt_state.dbt_profile.database,
            &dbt_state.dbt_profile.schema,
            DbtQuoting {
                database: root_project_quoting.database,
                schema: root_project_quoting.schema,
                identifier: root_project_quoting.identifier,
                snowflake_ignore_case: None,
            },
            package_runtime_config.clone(),
            root_runtime_config.clone(),
        )?;
        operations.on_run_start.extend(on_run_start);
        operations.on_run_end.extend(on_run_end);
    }

    // take refs and sources, resolve them to a unique_id and put in depends_on
    // This returns a set of node IDs that had resolution errors (unresolved refs/sources)
    let nodes_with_resolution_errors = resolve_dependencies(
        &mut nodes,
        &mut disabled_nodes,
        &mut operations,
        &node_resolver,
    );
    for warning in microbatch_model_no_event_time_inputs_warnings(&nodes) {
        emit_warn_log_from_fs_error(warning);
    }

    // Check for model deprecation warnings
    check_for_model_deprecations(&nodes);

    check_unused_resource_config_paths(
        &dbt_state.root_package().package_root_path,
        &nodes,
        &disabled_nodes,
        arg.skip_creating_generic_tests,
    )?;

    // A model on a non-`default` compute platform requires each of its upstreams
    // to be reachable through a catalog (see `check_compute_platform_upstreams`).
    check_compute_platform_upstreams(&nodes, dbt_state.catalogs.as_deref())?;

    // Check access
    let nodes_with_access_errors = check_access(&nodes, &all_runtime_configs);

    // Validate function configuration against per-adapter capabilities:
    // JS UDF language support, JS-aggregate restrictions, default arguments.
    validate_function_config(&nodes, adapter_type);

    resolver_hooks.post_resolve(
        &arg.io,
        &mut nodes,
        root_project_name,
        root_project_quoting,
        &dbt_state.cloud_config,
    )?;

    // Set the project name on nodes so that `package:this` selectors can resolve
    nodes.project_name = Some(root_project_name.to_string());

    // Store macros in nodes.macros so that they can be accessed by
    // state:modified for checking macro modifications
    // TODO: Instead of cloning macro_node into an Arc, implement
    //       macro_node as an Arc from the outset. Note that this
    //.      has the potential for a huge blast radius,
    //.      hence why we leave it as a TODO for when we
    //.      have the bandwidth to do it.
    //       See: https://github.com/dbt-labs/fs/pull/8760#discussion_r2965959119
    for (uid, macro_node) in &macros.macros {
        nodes
            .macros
            .insert(uid.clone(), Arc::new(macro_node.clone()));
    }

    let user_defined_schema_registry =
        dbt_schemas::state::hydrate_user_defined_schema_registry(&nodes, adapter_type);

    Ok((
        ResolverState {
            root_project_name: root_project_name.to_string(),
            adapter_type,
            nodes,
            disabled_nodes,
            macros,
            operations,
            dbt_profile: dbt_state.dbt_profile.clone(),
            cloud_config: dbt_state.cloud_config.clone(),
            render_results: collector,
            run_started_at: dbt_state.run_started_at,
            nodes_with_resolution_errors,
            nodes_with_access_errors,
            node_resolver: Arc::new(node_resolver),
            get_relation_calls,
            get_columns_in_relation_calls,
            patterned_dangling_sources,
            runtime_config: root_runtime_config.clone(),
            manifest_path_configs: ManifestPathConfig::for_packages(&dbt_state.packages),
            manifest_selectors,
            resolved_selectors,
            root_project_quoting: root_project_quoting.try_into()?,
            defer_nodes: None,
            semantic_layer_spec_is_legacy,
            test_name_truncations,
            user_defined_schema_registry,
        },
        jinja_env,
    ))
}

// Check that models accessing other models (dependecies) can do so.
// Returns the set of unique_ids that have access violations.
fn check_access(
    nodes: &Nodes,
    all_runtime_configs: &BTreeMap<String, Arc<DbtRuntimeConfig>>,
) -> HashSet<String> {
    let mut violations = HashSet::new();

    // Check access for models
    for (unique_id, node) in nodes.models.iter() {
        if check_node_access(
            unique_id,
            &node.base().depends_on.nodes_with_ref_location,
            &node.common().package_name,
            nodes,
            all_runtime_configs,
            |target_node, diffent_packages| {
                // Models can access private models if they're in the same group and same package
                node.__model_attr__.group != target_node.__model_attr__.group || diffent_packages
            },
        ) {
            violations.insert(unique_id.clone());
        }
    }

    // Check access for exposures
    for (unique_id, node) in nodes.exposures.iter() {
        if check_node_access(
            unique_id,
            &node.base().depends_on.nodes_with_ref_location,
            &node.common().package_name,
            nodes,
            all_runtime_configs,
            |target_node, diffent_packages| {
                // Exposures don't have groups, so they can't access private models
                // unless the private model has no group and they're in the same package
                target_node.__model_attr__.group.is_some() || diffent_packages
            },
        ) {
            violations.insert(unique_id.clone());
        }
    }

    violations
}

/// Validate function configuration against per-adapter capabilities:
/// - JavaScript UDFs are only supported on BigQuery and Snowflake.
/// - JavaScript aggregate UDFs are not supported on Snowflake.
/// - `default_value` on arguments is only supported on Snowflake, and defaulted
///   arguments must form a trailing suffix of the argument list (mirrors
///   `expand_default_fields` in the SQL binder, which only peels trailing
///   defaults when expanding `CREATE FUNCTION` into candidate arities).
fn validate_function_config(nodes: &Nodes, adapter_type: AdapterType) {
    use AdapterType::*;
    for function in nodes.functions.values() {
        let name = &function.__common_attr__.name;
        let language = function.__function_attr__.language.as_deref();
        let function_kind = function.deprecated_config.function_kind.as_ref();

        // JavaScript language + function-kind support
        match (adapter_type, language) {
            (Bigquery, Some(FUNCTION_LANGUAGE_JAVASCRIPT)) => {}
            (Snowflake, Some(FUNCTION_LANGUAGE_JAVASCRIPT)) => {
                if function_kind == Some(&FunctionKind::Aggregate) {
                    let err = fs_err!(
                        ErrorCode::InvalidConfig,
                        "Function '{}' is a JavaScript aggregate function and not supported on '{}'.",
                        name,
                        adapter_type,
                    );
                    emit_error_log_from_fs_error(*err);
                    continue;
                }
            }
            (_, Some(FUNCTION_LANGUAGE_JAVASCRIPT)) => {
                let err = fs_err!(
                    ErrorCode::InvalidConfig,
                    "Function '{}' uses JavaScript, which is not supported on '{}'.",
                    name,
                    adapter_type,
                );
                emit_error_log_from_fs_error(*err);
                continue;
            }
            // SQL / Python / unspecified: no per-adapter restrictions today.
            (_, Some(FUNCTION_LANGUAGE_SQL)) | (_, Some(FUNCTION_LANGUAGE_PYTHON)) | (_, None) => {}
            (_, Some(other)) => {
                unimplemented!("No per-adapter validation defined for function language '{other}'")
            }
        }

        // Default-value argument support
        if let Some(arguments) = function.__function_attr__.arguments.as_ref()
            && let Some(first_default) = arguments.iter().position(|a| a.default_value.is_some())
        {
            match adapter_type {
                Snowflake => {
                    if arguments[first_default..]
                        .iter()
                        .any(|a| a.default_value.is_none())
                    {
                        let err = fs_err!(
                            ErrorCode::InvalidConfig,
                            "Function '{}' has arguments with 'default_value' that are not at the end of the argument list. Defaulted arguments must form a trailing suffix.",
                            name,
                        );
                        emit_error_log_from_fs_error(*err);
                    }
                }
                _ => {
                    let err = fs_err!(
                        ErrorCode::InvalidConfig,
                        "Function '{}' declares an argument with 'default_value', which is not supported on '{}'.",
                        name,
                        adapter_type,
                    );
                    emit_error_log_from_fs_error(*err);
                }
            }
        }
    }
}

fn microbatch_model_no_event_time_inputs_warnings(nodes: &Nodes) -> Vec<FsError> {
    nodes
        .models
        .values()
        .filter(|model| {
            model.__model_attr__.incremental_strategy == Some(DbtIncrementalStrategy::Microbatch)
                && model.__model_attr__.event_time.is_some()
                && !has_event_time_input(nodes, model.as_ref())
        })
        .map(|model| {
            FsError::new(
                ErrorCode::MicrobatchModelNoEventTimeInputs,
                format!(
                    "The microbatch model '{}' has no 'ref' or 'source' input with an 'event_time' configuration. \nThis means no filtering can be applied and can result in unexpected duplicate records in the resulting microbatch model.",
                    model.common().name
                ),
            )
        })
        .collect()
}

fn has_event_time_input(nodes: &Nodes, model: &dyn InternalDbtNode) -> bool {
    model.base().depends_on.nodes.iter().any(|unique_id| {
        nodes
            .get_node(unique_id)
            .and_then(|node| node.event_time())
            .is_some()
    })
}

/// Helper function to check access for a node referencing other models.
/// Returns true if any access violation was found.
fn check_node_access<F>(
    unique_id: &str,
    node_dependencies: &[(String, dbt_common::CodeLocationWithFile)],
    node_package_name: &str,
    nodes: &Nodes,
    all_runtime_configs: &BTreeMap<String, Arc<DbtRuntimeConfig>>,
    should_deny_private_access: F,
) -> bool
where
    F: Fn(&DbtModel, bool) -> bool,
{
    let mut had_violation = false;
    for (target_unique_id, location) in node_dependencies {
        if let Some(target_node) = nodes.models.get(target_unique_id) {
            let restricted_access = all_runtime_configs
                .get(&target_node.common().package_name)
                .is_some_and(|config| config.inner.restrict_access.unwrap_or(false));

            let diffent_packages =
                target_node.common().package_name != node_package_name && restricted_access;

            if target_node.__model_attr__.access == Access::Private
                && should_deny_private_access(target_node, diffent_packages)
            {
                let err = fs_err!(
                    code => ErrorCode::AccessDenied,
                    loc => location.clone(),
                    "Node '{}' attempted to reference node '{}', which is not allowed because the referenced node is private to the '{}' group",
                    unique_id,
                    target_unique_id,
                    target_node.__model_attr__.group.as_deref().unwrap_or(""),
                );
                emit_error_log_from_fs_error(*err);
                had_violation = true;
            } else if target_node.__model_attr__.access == Access::Protected && diffent_packages {
                let err = fs_err!(
                    code => ErrorCode::AccessDenied,
                    loc => location.clone(),
                    "Node '{}' attempted to reference node '{}', which is not allowed because the referenced node is protected to the '{}' package",
                    unique_id,
                    target_unique_id,
                    target_node.common().package_name,
                );
                emit_error_log_from_fs_error(*err);
                had_violation = true;
            }
        }
    }
    had_violation
}

/// Inner resolve function that resolves a single package.
#[allow(clippy::too_many_arguments)]
pub async fn resolve_inner(
    arg: &ResolveArgs,
    package: &DbtPackage,
    dbt_state: Arc<DbtState>,
    root_package_name: &str,
    root_project_configs: &RootProjectConfigs,
    adapter_type: AdapterType,
    macros: &Macros,
    jinja_env: Arc<JinjaEnv>,
    mut node_resolver: NodeResolver,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<(
    Nodes,
    Nodes,
    RenderResults,
    NodeResolver,
    bool,
    BTreeMap<String, resolve_properties::MinimalPropertiesEntry>,
)> {
    let mut nodes = Nodes::default();
    let mut disabled_nodes = Nodes::default();

    let database: &String = &dbt_state.dbt_profile.database;

    let schema = &dbt_state.dbt_profile.schema;

    let package_quoting = resolve_package_quoting(*package.dbt_project.quoting, adapter_type);

    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let base_ctx = build_resolve_context(
        root_package_name,
        package.dbt_project.name.as_str(),
        &macros.docs_macros,
        DISPATCH_CONFIG.get().unwrap().read().unwrap().clone(),
        namespace_keys,
        Some(root_runtime_config.clone()),
    );
    // Resolve the dbt properties (schema.yml) files
    let mut min_properties = resolve_minimal_properties(
        arg,
        package,
        dbt_state.root_package(),
        root_package_name,
        root_project_configs,
        &jinja_env,
        &base_ctx,
        token,
        adapter_type,
    )?;

    let package_name = package.dbt_project.name.as_str();

    // Collect macro properties for patching later (after all packages resolved)
    let macro_properties = std::mem::take(&mut min_properties.macros);

    let mut collected_generic_tests: Vec<GenericTestAsset> = Vec::new();

    let dbt_tests_dir = arg.io.out_dir.join(DBT_GENERIC_TESTS_DIR_NAME);
    stdfs::create_dir_all(&dbt_tests_dir)?;

    let dependency_package_name = dependency_package_name_from_ctx(&jinja_env, &base_ctx);
    let mut typed_models_properties: BTreeMap<String, ModelProperties> = BTreeMap::new();

    let semantic_layer_spec_is_legacy = min_properties.semantic_layer_spec_is_legacy;

    for (model_name, minimal_model_props) in &min_properties.models {
        // Update base context with the relative yaml file path.
        // We do this for accurate error reporting.
        let base_ctx = {
            let mut base_ctx = base_ctx.clone();
            base_ctx.insert(
                CURRENT_PATH.to_string(),
                minijinja::Value::from(minimal_model_props.relative_path.to_string_lossy()),
            );
            base_ctx
        };

        // Legacy semantic layer projects use a different `metrics` spec, so drop the key
        // rather than deserializing it against the current schema.
        let mut model_yml = minimal_model_props.clone().schema_value;
        if semantic_layer_spec_is_legacy && let Some(m) = model_yml.as_mapping_mut() {
            m.remove("metrics");
        }

        let typed_model_props: ModelProperties = into_typed_with_jinja(
            model_yml,
            false,
            &jinja_env,
            &base_ctx,
            &[],
            dependency_package_name,
            // HACK: to avoid duplicate errors due to multiple parses of model yaml properties (once here and once in resolve_models)
            // do not show_errors_or_warnings because that will be done by resolve_models
            false,
        )?;

        typed_models_properties.insert(model_name.clone(), typed_model_props);
    }

    // Resolve sources based on the dbt_state, database, schema, and project name
    let (sources, disabled_sources) = resolve_sources(
        arg,
        package,
        root_package_name,
        dbt_state.root_package(),
        root_project_configs,
        min_properties.source_tables,
        database,
        adapter_type,
        &base_ctx,
        &jinja_env,
        &mut collected_generic_tests,
        test_name_truncations,
        seen_generic_test_paths,
        &mut node_resolver,
    )
    .await?;
    nodes.sources.extend(sources);
    disabled_nodes.sources.extend(disabled_sources);

    // Resolve seeds based on the dbt_state, database, schema, and project name
    let (seeds, disabled_seeds) = resolve_seeds(
        arg,
        min_properties.seeds,
        package,
        &root_project_configs.adapter_quoting,
        dbt_state.root_package(),
        root_project_configs,
        database,
        schema,
        adapter_type,
        package_name,
        &jinja_env,
        &base_ctx,
        &mut collected_generic_tests,
        test_name_truncations,
        seen_generic_test_paths,
        &mut node_resolver,
    )
    .await?;
    nodes.seeds.extend(seeds);
    disabled_nodes.seeds.extend(disabled_seeds);

    // TODO: resolve_snapshots still creates its own local JinjaTypeCheckingEventListenerFactory
    // instead of receiving the top-level one, so snapshot macro dependencies are populated
    // locally rather than via update_manifest_with_macro_depends_on. Out of scope for now.
    // Resolve snapshots based on the dbt_state, database, schema, and project name
    let (snapshots, disabled_snapshots) = resolve_snapshots(
        arg,
        package,
        package_quoting,
        dbt_state.root_package(),
        root_project_configs,
        min_properties.snapshots,
        &macros.macros,
        database,
        schema,
        adapter_type,
        &root_project_configs.adapter_quoting,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        &mut node_resolver,
        &mut collected_generic_tests,
        test_name_truncations,
        seen_generic_test_paths,
        token,
    )
    .await?;
    nodes.snapshots.extend(snapshots);
    disabled_nodes.snapshots.extend(disabled_snapshots);

    let (groups, disabled_groups) = resolve_groups(
        adapter_type,
        &mut min_properties.groups,
        package_name,
        &jinja_env,
        &base_ctx,
    )
    .await?;

    nodes.groups.extend(groups);
    disabled_nodes.groups.extend(disabled_groups);

    // Resolve SQLs and get nodes and rendered SQLs except refs and sources
    let (models, rendering_results, disabled_models) = resolve_models(
        arg,
        package,
        package_quoting,
        &root_project_configs.adapter_quoting,
        dbt_state.root_package(),
        root_project_configs,
        &min_properties.models,
        // TODO: pass in typed_models_properties
        database,
        schema,
        adapter_type,
        package_name,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        &mut collected_generic_tests,
        test_name_truncations,
        seen_generic_test_paths,
        &mut node_resolver,
        token,
        jinja_type_checking_event_listener_factory.clone(),
    )
    .await?;
    nodes.models.extend(models);
    disabled_nodes.models.extend(disabled_models);

    // TODO: resolve_analyses still creates its own local JinjaTypeCheckingEventListenerFactory
    // instead of receiving the top-level one — same issue as resolve_snapshots. Out of scope for now.
    let (analyses, analyses_rendering_results) = resolve_analyses(
        arg,
        package,
        package_quoting,
        dbt_state.root_package(),
        root_project_configs,
        &mut min_properties.analyses,
        database,
        schema,
        adapter_type,
        package_name,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        token,
    )
    .await?;
    nodes.analyses.extend(analyses);

    // Resolve checks
    let (checks, disabled_checks, checks_rendering_results) = resolve_checks(
        arg,
        package,
        package_quoting,
        dbt_state.root_package(),
        root_project_configs,
        &mut min_properties.checks,
        database,
        schema,
        adapter_type,
        package_name,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        token,
    )
    .await?;
    nodes.checks.extend(checks);
    disabled_nodes.checks.extend(disabled_checks);

    // Resolve functions
    let (functions, functions_rendering_results) = resolve_functions(
        arg,
        package,
        package_quoting,
        dbt_state.root_package(),
        root_project_configs,
        &mut min_properties.functions,
        database,
        schema,
        adapter_type,
        &root_project_configs.adapter_quoting,
        package_name,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        &mut node_resolver,
        token,
    )
    .await?;
    nodes.functions.extend(functions);

    let (exposures, disabled_exposures) = resolve_exposures(
        arg,
        &mut min_properties.exposures,
        package,
        dbt_state.root_package(),
        root_project_configs,
        database,
        schema,
        adapter_type,
        package_name,
        &jinja_env,
        &base_ctx,
    )
    .await?;
    nodes.exposures.extend(exposures);
    disabled_nodes.exposures.extend(disabled_exposures);

    if !semantic_layer_spec_is_legacy {
        let (semantic_models, disabled_semantic_models) = resolve_semantic_models(
            adapter_type,
            arg,
            package,
            dbt_state.root_package(),
            root_project_configs,
            &min_properties.models,
            &typed_models_properties,
            &nodes.models,
            package_name,
            &jinja_env,
            &base_ctx,
        )
        .await?;
        nodes.semantic_models.extend(semantic_models);
        disabled_nodes
            .semantic_models
            .extend(disabled_semantic_models);

        let (metrics, disabled_metrics) = resolve_metrics(
            adapter_type,
            arg,
            package,
            dbt_state.root_package(),
            root_project_configs,
            &min_properties.models,
            &min_properties.metrics,
            &typed_models_properties,
            package_name,
            &jinja_env,
            &base_ctx,
        )
        .await?;
        nodes.metrics.extend(metrics);
        disabled_nodes.metrics.extend(disabled_metrics);

        let (saved_queries, disabled_saved_queries) = resolve_saved_queries(
            adapter_type,
            arg,
            package,
            dbt_state.root_package(),
            root_package_name,
            root_project_configs,
            &mut min_properties.saved_queries,
            database,
            schema,
            package_name,
            jinja_env.clone(),
            &base_ctx,
        )
        .await?;
        nodes.saved_queries.extend(saved_queries);
        disabled_nodes.saved_queries.extend(disabled_saved_queries);
    }

    let (data_tests, disabled_tests) = resolve_data_tests(
        arg,
        package,
        package_quoting,
        dbt_state.root_package(),
        root_project_configs,
        &mut min_properties.tests,
        database,
        schema,
        adapter_type,
        jinja_env.clone(),
        &base_ctx,
        runtime_config.clone(),
        root_runtime_config.clone(),
        &collected_generic_tests,
        &node_resolver,
        token,
        jinja_type_checking_event_listener_factory.clone(),
        &root_project_configs.adapter_quoting,
        &nodes.models,
        &disabled_nodes.models,
        &nodes.seeds,
        &nodes.snapshots,
    )
    .await?;
    nodes.tests.extend(data_tests);
    disabled_nodes.tests.extend(disabled_tests);

    infer_and_apply_primary_keys(&mut nodes, &disabled_nodes);

    let (unit_tests, disabled_unit_tests) = resolve_unit_tests(
        arg,
        min_properties.unit_tests,
        package,
        dbt_state.root_package(),
        package_quoting,
        root_project_configs,
        package_name,
        &jinja_env,
        &base_ctx,
        &min_properties.models,
        &nodes.models,
        &dbt_state.packages,
        adapter_type,
        &node_resolver,
    )?;

    nodes.unit_tests.extend(unit_tests);
    disabled_nodes.unit_tests.extend(disabled_unit_tests);

    if let Some(query_comment) = package.dbt_project.query_comment.as_ref() {
        resolve_query_comment(query_comment, &jinja_env, &base_ctx)?;
    }

    let collector = RenderResults {
        rendering_results: rendering_results
            .into_iter()
            .chain(analyses_rendering_results)
            .chain(checks_rendering_results)
            .chain(functions_rendering_results)
            .collect(),
    };

    clear_package_diagnostics(&arg.io, package);

    Ok((
        nodes,
        disabled_nodes,
        collector,
        node_resolver,
        semantic_layer_spec_is_legacy,
        macro_properties,
    ))
}

/// Returns `true` if `upstream` is readable as an input for a node running on
/// `lake_compute`.
///
/// The question is about the *catalog*, never about which adapter wrote the
/// upstream: a BigQuery model landing in a lake-compute-readable catalog is a legal input,
/// while a Snowflake model in warehouse-native storage is not.
///
/// Where the catalog type is known, it decides. Where it cannot be resolved — no
/// `catalogs.yml`, or a name that is not a v2 catalog — a named catalog stays
/// permissive, so this only ever tightens on positive knowledge.
fn upstream_is_catalog_reachable(upstream: &DbtModel, catalogs: Option<&DbtCatalogs>) -> bool {
    let attr = &upstream.__model_attr__;

    // An upstream on `lake_compute` itself writes somewhere `lake_compute` can read, by definition.
    if upstream.node_adapter() == AdapterType::LakeCompute {
        return true;
    }

    // A `view`/`ephemeral` upstream has no physical storage format on any
    // adapter: `catalog_name` and `table_format` are both silently ignored
    // by every warehouse's view materialization (e.g. Snowflake's `view.sql`
    // never consults either), so no config value can make one Iceberg- or
    // catalog-reachable. Reject outright rather than trusting the config.
    if matches!(
        upstream.base().materialized,
        DbtMaterialization::View | DbtMaterialization::Ephemeral
    ) {
        return false;
    }

    match attr.catalog_name.as_deref() {
        Some(name) => match catalogs.and_then(|c| c.v2_catalog_type(name).ok().flatten()) {
            Some(catalog_type) => catalog_type.lake_compute_can_read(),
            None => true,
        },
        // Iceberg without a named catalog is still an open format.
        None => attr
            .table_format
            .as_deref()
            .is_some_and(|f| f.eq_ignore_ascii_case("iceberg")),
    }
}

/// Same question as [`upstream_is_catalog_reachable`], for a seed upstream.
///
/// Seeds have no `table_format` config, so a seed with no `catalog_name` is
/// reachable only by being placed on `lake_compute` itself — there is no
/// "landed in Iceberg with no named catalog" escape hatch for seeds the way
/// there is for models.
fn seed_upstream_is_catalog_reachable(upstream: &DbtSeed, catalogs: Option<&DbtCatalogs>) -> bool {
    if upstream.node_adapter() == AdapterType::LakeCompute {
        return true;
    }

    match upstream.__seed_attr__.catalog_name.as_deref() {
        Some(name) => match catalogs.and_then(|c| c.v2_catalog_type(name).ok().flatten()) {
            Some(catalog_type) => catalog_type.lake_compute_can_read(),
            None => true,
        },
        None => false,
    }
}

/// WS1 rule 5: every `ref`/`source` upstream of a model on a non-`default` compute
/// platform must be reachable through a catalog. A plain warehouse-native upstream
/// is rejected with a precise error naming both nodes, since the compute target
/// reads its inputs through attached catalogs rather than the warehouse.
///
/// Sources are external tables whose catalog reachability is validated elsewhere,
/// so they are skipped here. Model and seed upstreams are looked up in their own
/// `Nodes` maps (`unique_id`'s `model.`/`seed.` prefix never overlaps), since a
/// seed has no `__model_attr__` to check the model-shaped helper against.
pub fn check_compute_platform_upstreams(
    nodes: &Nodes,
    catalogs: Option<&DbtCatalogs>,
) -> FsResult<()> {
    for (unique_id, model) in nodes.models.iter() {
        if model.node_adapter() != AdapterType::LakeCompute {
            continue;
        }
        for upstream_id in &model.__base_attr__.depends_on.nodes {
            if upstream_id.starts_with("source.") {
                continue;
            }
            let upstream_model = nodes.models.get(upstream_id);
            let reachable = if let Some(up) = upstream_model {
                upstream_is_catalog_reachable(up, catalogs)
            } else if let Some(seed) = nodes.seeds.get(upstream_id) {
                seed_upstream_is_catalog_reachable(seed, catalogs)
            } else {
                false
            };
            if !reachable {
                // A `view`/`ephemeral` upstream is unreachable no matter what
                // `catalog_name`/`table_format` it declares (see
                // `upstream_is_catalog_reachable`), so the generic "set
                // catalog_name/table_format" advice below would be actively
                // misleading if that config is already set and just being
                // ignored. Name the real fix instead.
                if let Some(up) = upstream_model {
                    if matches!(
                        up.base().materialized,
                        DbtMaterialization::View | DbtMaterialization::Ephemeral
                    ) {
                        return err!(
                            ErrorCode::InvalidConfig,
                            "Model '{}' runs on adapter: '{}' but its upstream '{}' \
                             materializes as '{}', which has no physical storage format \
                             and so it can never be catalog-reachable. \
                             Materialize '{}' as 'table' or 'incremental' (with 'catalog_name' \
                             or 'table_format: iceberg') or place it on adapter: '{}'.",
                            unique_id,
                            AdapterType::LakeCompute.as_ref(),
                            upstream_id,
                            up.base().materialized,
                            upstream_id,
                            AdapterType::LakeCompute.as_ref()
                        );
                    }
                }
                return err!(
                    ErrorCode::InvalidConfig,
                    "Model '{}' runs on adapter: '{}' but its upstream '{}' is not \
                     reachable through a catalog. Materialize '{}' into an open table \
                     format (set 'catalog_name', or set 'table_format: iceberg' to land \
                     it in Iceberg without a named catalog) or place it on adapter: '{}'.",
                    unique_id,
                    AdapterType::LakeCompute.as_ref(),
                    upstream_id,
                    upstream_id,
                    AdapterType::LakeCompute.as_ref()
                );
            }
        }
    }
    Ok(())
}

/// Function to check models, seeds, and snapshots for relation uniqueness
pub fn check_relation_uniqueness(nodes: &Nodes) -> FsResult<()> {
    let mut alias_resources: HashMap<String, &dyn InternalDbtNode> = HashMap::new();

    for (_, node) in nodes.iter() {
        // We only check models, seeds and snapshots
        if ![NodeType::Model, NodeType::Seed, NodeType::Snapshot].contains(&node.resource_type()) {
            continue;
        }
        if let Some(node_relation_name) = node.base().relation_name.clone() {
            // Check for alias conflicts
            if let std::collections::hash_map::Entry::Vacant(e) =
                alias_resources.entry(node_relation_name.clone())
            {
                e.insert(node);
            } else {
                // Get node that's already stored
                let existing_node = alias_resources.get(&node_relation_name).unwrap();
                return err!(
                    ErrorCode::InvalidConfig,
                    "dbt found two resources with the database relation {}. Nodes: {}, {}",
                    node_relation_name,
                    node.common().unique_id,
                    existing_node.common().unique_id
                );
            }
        }
    }

    Ok(())
}

/// Resolves a single package asynchronously.
#[allow(clippy::too_many_arguments)]
async fn resolve_package(
    package_name: String,
    arg: &ResolveArgs,
    dbt_state: Arc<DbtState>,
    root_project_name: String,
    root_project_configs: Arc<RootProjectConfigs>,
    adapter_type: AdapterType,
    macros: &Macros,
    jinja_env: Arc<JinjaEnv>,
    node_resolver: NodeResolver,
    all_runtime_configs: &BTreeMap<String, Arc<DbtRuntimeConfig>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<(
    String,
    Nodes,
    Nodes,
    RenderResults,
    NodeResolver,
    bool,
    HashMap<String, String>,
    BTreeMap<String, resolve_properties::MinimalPropertiesEntry>,
)> {
    let package = dbt_state
        .packages
        .iter()
        .find(|p| p.dbt_project.name == package_name)
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::InvalidConfig,
                "Encountered unexpected package not found in project: {}",
                package_name
            )
        })?;

    // Built up front for every package in `resolve()`'s pre-pass, independent of wave order —
    // see the comment there for why the config can't be (re)built lazily here anymore.
    let runtime_config = all_runtime_configs
        .get(&package_name)
        .expect("runtime config must be pre-built for every package before waves run")
        .clone();

    let mut test_name_truncations: HashMap<String, String> = HashMap::new();
    let mut seen_generic_test_paths: HashMap<PathBuf, String> = HashMap::new();
    let (
        new_nodes,
        new_disabled_nodes,
        rendering_results,
        updated_node_resolver,
        semantic_layer_spec_is_legacy,
        macro_properties,
    ) = resolve_inner(
        arg,
        package,
        dbt_state.clone(),
        &root_project_name,
        &root_project_configs,
        adapter_type,
        macros,
        jinja_env.clone(),
        node_resolver,
        runtime_config.clone(),
        root_runtime_config.clone(),
        &mut test_name_truncations,
        &mut seen_generic_test_paths,
        token,
        jinja_type_checking_event_listener_factory.clone(),
    )
    .await?;

    // Return everything needed for merging
    Ok((
        package_name,
        new_nodes,
        new_disabled_nodes,
        rendering_results,
        updated_node_resolver,
        semantic_layer_spec_is_legacy,
        test_name_truncations,
        macro_properties,
    ))
}

/// Resolves packages in waves (inter-wave sequential, intra-wave parallel via `dispatch_maybe_parallel`).
#[allow(clippy::too_many_arguments)]
async fn resolve_package_waves(
    package_waves: Vec<Vec<String>>,
    arg: &ResolveArgs,
    dbt_state: Arc<DbtState>,
    root_project_name: &str,
    root_project_configs: Arc<RootProjectConfigs>,
    adapter_type: AdapterType,
    macros: &Macros,
    jinja_env: Arc<JinjaEnv>,
    node_resolver: &mut NodeResolver,
    all_runtime_configs: Arc<BTreeMap<String, Arc<DbtRuntimeConfig>>>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<(
    Nodes,
    Nodes,
    RenderResults,
    bool,
    HashMap<String, String>,
    BTreeMap<String, BTreeMap<String, resolve_properties::MinimalPropertiesEntry>>,
)> {
    let max_concurrency = crate::parallel::effective_parallelism(arg.no_parallel);
    let arg = Arc::new(arg.clone());
    let macros = Arc::new(macros.clone());

    let mut nodes = Nodes::default();
    let mut disabled_nodes = Nodes::default();
    let mut collector = RenderResults {
        rendering_results: BTreeMap::new(),
    };
    let mut semantic_layer_spec_is_legacy = false;
    let mut test_name_truncations: HashMap<String, String> = HashMap::new();
    let mut all_macro_properties: BTreeMap<
        String,
        BTreeMap<String, resolve_properties::MinimalPropertiesEntry>,
    > = BTreeMap::new();

    for package_wave in package_waves {
        token.check_cancellation()?;

        // Snapshot per-wave state for parallel tasks. Runtime configs for every package
        // (including root) were already built in `resolve()`'s pre-pass, so this is just a
        // cheap Arc clone now, not a map clone.
        let all_runtime_configs = all_runtime_configs.clone();
        let root_runtime_config = root_runtime_config.clone();
        let node_resolver_snapshot = node_resolver.clone();

        let arg = arg.clone();
        let dbt_state = dbt_state.clone();
        let root_project_name = root_project_name.to_string();
        let root_project_configs = root_project_configs.clone();
        let macros = macros.clone();
        let jinja_env = jinja_env.clone();
        let token = token.clone();
        let jinja_type_checking_event_listener_factory =
            jinja_type_checking_event_listener_factory.clone();

        let results = crate::parallel::dispatch_maybe_parallel(
            package_wave,
            max_concurrency > 1,
            move |package_name: String| {
                let arg = arg.clone();
                let dbt_state = dbt_state.clone();
                let root_project_name = root_project_name.clone();
                let root_project_configs = root_project_configs.clone();
                let macros = macros.clone();
                let jinja_env = jinja_env.clone();
                let node_resolver = node_resolver_snapshot.clone();
                let all_runtime_configs = all_runtime_configs.clone();
                let root_runtime_config = root_runtime_config.clone();
                let token = token.clone();
                let jinja_type_checking_event_listener_factory =
                    jinja_type_checking_event_listener_factory.clone();

                async move {
                    resolve_package(
                        package_name,
                        &arg,
                        dbt_state,
                        root_project_name,
                        root_project_configs,
                        adapter_type,
                        &macros,
                        jinja_env,
                        node_resolver,
                        &all_runtime_configs,
                        root_runtime_config,
                        &token,
                        jinja_type_checking_event_listener_factory,
                    )
                    .await
                }
            },
        )
        .await?;

        // Merge wave results back into accumulators
        for result in results {
            let (
                package_name,
                new_nodes,
                new_disabled_nodes,
                rendering_results,
                updated_node_resolver,
                resolved_semantic_layer_spec_is_legacy,
                resolved_test_name_truncations,
                macro_properties,
            ) = result;

            semantic_layer_spec_is_legacy |= resolved_semantic_layer_spec_is_legacy;

            if !macro_properties.is_empty() {
                all_macro_properties.insert(package_name.clone(), macro_properties);
            }

            nodes.extend(new_nodes);
            disabled_nodes.extend(new_disabled_nodes);
            collector
                .rendering_results
                .extend(rendering_results.rendering_results);
            test_name_truncations.extend(resolved_test_name_truncations);
            node_resolver.merge(updated_node_resolver);
        }
    }

    Ok((
        nodes,
        disabled_nodes,
        collector,
        semantic_layer_spec_is_legacy,
        test_name_truncations,
        all_macro_properties,
    ))
}

/// Add the `dbt` global project's `__overview__` doc block if it isn't there.
///
/// dbt Core always ships a global project with an overview.md that produces
/// `doc.dbt.__overview__`, and the dbt Docs HTML reads that key unconditionally
/// (overview controller: `i = n.docs["doc.dbt.__overview__"]`), crashing with a
/// TypeError if it is absent.
///
/// This entry is *always* present, exactly as in dbt Core — a user's own
/// `{% docs __overview__ %}` is keyed `doc.<their_package>.__overview__` and so
/// coexists with it rather than replacing it. Both dbt Docs and the docs-v2
/// Overview page pick the winner at read time, preferring any non-`dbt` package.
/// Do not "optimize" this into skipping injection when a user overview exists:
/// `block_contents` here is compared byte-for-byte against dbt Core's manifest
/// by the conformance regression suite, and the docs-v2 page relies on the row
/// as its fallback.
fn inject_default_overview(docs: &mut BTreeMap<String, DbtDocsMacro>) {
    let overview_uid = "doc.dbt.__overview__".to_string();
    docs.entry(overview_uid.clone())
        .or_insert_with(|| DbtDocsMacro {
            name: "__overview__".to_string(),
            package_name: "dbt".to_string(),
            path: DbtPath::from("overview.md"),
            original_file_path: DbtPath::from("docs/overview.md"),
            unique_id: overview_uid,
            block_contents: DEFAULT_OVERVIEW_CONTENTS.to_string(),
        });
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use dbt_common::path::DbtPath;
    use dbt_schemas::schemas::macros::DbtDocsMacro;

    use super::inject_default_overview;
    use crate::constants::DEFAULT_OVERVIEW_CONTENTS;

    /// When a project defines no {% docs %} blocks, `doc.dbt.__overview__`
    /// must be present in the manifest so the dbt Docs HTML doesn't crash.
    #[test]
    fn test_default_overview_injected_when_no_docs_defined() {
        let mut docs: BTreeMap<String, DbtDocsMacro> = BTreeMap::new();
        inject_default_overview(&mut docs);

        let entry = docs
            .get("doc.dbt.__overview__")
            .expect("doc.dbt.__overview__ must be injected");

        assert_eq!(entry.name, "__overview__");
        assert_eq!(entry.package_name, "dbt");
        assert_eq!(entry.unique_id, "doc.dbt.__overview__");
        assert!(
            !entry.block_contents.is_empty(),
            "block_contents must not be empty"
        );
    }

    /// A user-defined {% docs __overview__ %} and the injected default coexist,
    /// under different keys — which is exactly dbt Core's manifest shape.
    ///
    /// `resolve_docs_macros` keys docs as `doc.{package_name}.{name}`, so a
    /// block in `my_project` is `doc.my_project.__overview__` and never collides
    /// with the injected `doc.dbt.__overview__`. Readers pick the winner; see
    /// [`inject_default_overview`].
    #[test]
    fn test_user_overview_coexists_with_default() {
        let uid = "doc.my_project.__overview__".to_string();
        let user_doc = DbtDocsMacro {
            name: "__overview__".to_string(),
            package_name: "my_project".to_string(),
            path: DbtPath::from("models/overview.md"),
            original_file_path: DbtPath::from("models/overview.md"),
            unique_id: uid.clone(),
            block_contents: "# My custom overview".to_string(),
        };

        let mut docs: BTreeMap<String, DbtDocsMacro> = BTreeMap::new();
        docs.insert(uid.clone(), user_doc);
        inject_default_overview(&mut docs);

        assert_eq!(
            docs.get(&uid)
                .expect("user overview must survive")
                .block_contents,
            "# My custom overview",
        );
        assert_eq!(
            docs.get("doc.dbt.__overview__")
                .expect("the default must still be injected alongside it")
                .block_contents,
            DEFAULT_OVERVIEW_CONTENTS,
        );
    }

    /// A microbatch model whose only upstream is a seed with `event_time`
    /// configured counts as having a valid event_time input (matching dbt-core),
    /// so no `MicrobatchModelNoEventTimeInputs` warning is raised. Seeds, like
    /// sources, store `event_time` in `deprecated_config`.
    #[test]
    fn test_has_event_time_input_resolves_seed() {
        use std::sync::Arc;

        use dbt_schemas::schemas::project::SeedConfig;
        use dbt_schemas::schemas::{CommonAttributes, DbtModel, DbtSeed, Nodes};

        use super::has_event_time_input;

        let seed_uid = "seed.test.raw_events".to_string();
        let make_seed = |event_time: Option<&str>| DbtSeed {
            __common_attr__: CommonAttributes {
                unique_id: seed_uid.clone(),
                name: "raw_events".to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            deprecated_config: SeedConfig {
                event_time: event_time.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut model = DbtModel::default();
        model.__base_attr__.depends_on.nodes = vec![seed_uid.clone()];

        // Seed declares event_time -> valid input.
        let mut nodes = Nodes::default();
        nodes
            .seeds
            .insert(seed_uid.clone(), Arc::new(make_seed(Some("event_time"))));
        assert!(has_event_time_input(&nodes, &model));

        let mut nodes = Nodes::default();
        nodes
            .seeds
            .insert(seed_uid.clone(), Arc::new(make_seed(None)));
        assert!(!has_event_time_input(&nodes, &model));
    }

    /// A microbatch model whose only upstream is a snapshot with `event_time`
    /// configured also counts as having a valid event_time input. Snapshots, like
    /// seeds and sources, store `event_time` in `deprecated_config`.
    #[test]
    fn test_has_event_time_input_resolves_snapshot() {
        use std::sync::Arc;

        use dbt_schemas::schemas::project::SnapshotConfig;
        use dbt_schemas::schemas::{CommonAttributes, DbtModel, DbtSnapshot, Nodes};

        use super::has_event_time_input;

        let snapshot_uid = "snapshot.test.raw_events".to_string();
        let make_snapshot = |event_time: Option<&str>| DbtSnapshot {
            __common_attr__: CommonAttributes {
                unique_id: snapshot_uid.clone(),
                name: "raw_events".to_string(),
                package_name: "test".to_string(),
                ..Default::default()
            },
            deprecated_config: SnapshotConfig {
                event_time: event_time.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        };

        let mut model = DbtModel::default();
        model.__base_attr__.depends_on.nodes = vec![snapshot_uid.clone()];

        let mut nodes = Nodes::default();
        nodes.snapshots.insert(
            snapshot_uid.clone(),
            Arc::new(make_snapshot(Some("event_time"))),
        );
        assert!(has_event_time_input(&nodes, &model));

        let mut nodes = Nodes::default();
        nodes
            .snapshots
            .insert(snapshot_uid.clone(), Arc::new(make_snapshot(None)));
        assert!(!has_event_time_input(&nodes, &model));
    }

    /// `lake_compute_can_read` is the whole of the capability question, and it is a property
    /// of the catalog type rather than of anything a project declares. Pinned
    /// exhaustively so that adding a `CatalogType` has to answer it.
    #[test]
    fn lake_compute_reads_open_catalogs_and_not_engine_owned_ones() {
        use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;

        for readable in [
            CatalogType::Horizon,
            CatalogType::Glue,
            CatalogType::IcebergRest,
        ] {
            assert!(
                readable.lake_compute_can_read(),
                "lake compute should read {readable:?}"
            );
        }
        for unreadable in [
            CatalogType::DuckLake,
            CatalogType::LocalFilesystem,
            CatalogType::BiglakeMetastore,
            CatalogType::Unity,
            CatalogType::HiveMetastore,
        ] {
            assert!(
                !unreadable.lake_compute_can_read(),
                "lake compute does not support {unreadable:?} today"
            );
        }
    }

    /// WS1 rule 5: an `adapter: lake_compute` model requires each of its model upstreams to
    /// be reachable through a catalog; a plain warehouse-native upstream is
    /// rejected, while catalog-backed / Iceberg / lake compute upstreams pass.
    #[test]
    fn test_check_compute_platform_upstreams() {
        use std::sync::Arc;

        use dbt_adapter_core::AdapterType;
        use dbt_schemas::schemas::CommonAttributes;
        use dbt_schemas::schemas::{DbtModel, Nodes};

        use super::check_compute_platform_upstreams;

        // Build a model with the given placement and upstream config knobs.
        let make_model = |uid: &str,
                          adapter: AdapterType,
                          catalog_name: Option<&str>,
                          table_format: Option<&str>,
                          upstreams: &[&str]| {
            let mut model = DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: uid.to_string(),
                    name: uid.rsplit('.').next().unwrap_or(uid).to_string(),
                    package_name: "test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            model.__base_attr__.adapter = adapter;
            model.__model_attr__.catalog_name = catalog_name.map(str::to_string);
            model.__model_attr__.table_format = table_format.map(str::to_string);
            model.__base_attr__.depends_on.nodes =
                upstreams.iter().map(|s| (*s).to_string()).collect();
            model
        };

        let consumer_uid = "model.test.consumer";
        let upstream_uid = "model.test.upstream";

        // Helper: run the check with a `dbt` consumer reading `upstream`.
        let run = |upstream: DbtModel| {
            let consumer = make_model(
                consumer_uid,
                AdapterType::LakeCompute,
                Some("horizon"),
                None,
                &[upstream_uid],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert(consumer_uid.to_string(), Arc::new(consumer));
            nodes
                .models
                .insert(upstream_uid.to_string(), Arc::new(upstream));
            check_compute_platform_upstreams(&nodes, None)
        };

        // Catalog-backed / Iceberg / dbt upstreams are reachable.
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::Snowflake,
                Some("horizon"),
                None,
                &[]
            ))
            .is_ok()
        );
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::Snowflake,
                None,
                Some("iceberg"),
                &[]
            ))
            .is_ok()
        );
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::LakeCompute,
                Some("horizon"),
                None,
                &[]
            ))
            .is_ok()
        );

        // A plain warehouse-native upstream is rejected.
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::Snowflake,
                None,
                None,
                &[]
            ))
            .is_err()
        );

        // A `default` consumer is unconstrained even with a warehouse-native upstream.
        let consumer = make_model(
            consumer_uid,
            AdapterType::Snowflake,
            None,
            None,
            &[upstream_uid],
        );
        let upstream = make_model(upstream_uid, AdapterType::Snowflake, None, None, &[]);
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert(consumer_uid.to_string(), Arc::new(consumer));
        nodes
            .models
            .insert(upstream_uid.to_string(), Arc::new(upstream));
        assert!(check_compute_platform_upstreams(&nodes, None).is_ok());

        // A `source.*` upstream is skipped (validated elsewhere), so no error even
        // though it is not present in `nodes.models`.
        let consumer = make_model(
            consumer_uid,
            AdapterType::LakeCompute,
            Some("horizon"),
            None,
            &["source.test.raw"],
        );
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert(consumer_uid.to_string(), Arc::new(consumer));
        assert!(check_compute_platform_upstreams(&nodes, None).is_ok());
    }

    /// WS1 rule 5 for a `view`/`ephemeral` upstream: `catalog_name` and
    /// `table_format` are both silently ignored by every adapter's view
    /// materialization, so a `view` upstream must be rejected even when it
    /// declares `table_format: iceberg` -- the config value is never actually
    /// honored, so trusting it (as a plain `lands_in_iceberg`-style check
    /// would) lets a guaranteed-to-fail model through parse/compile and only
    /// fails at Lakecompute execution time, deep in an adapter-specific error.
    #[test]
    fn test_check_compute_platform_upstreams_view_ignores_table_format() {
        use std::sync::Arc;

        use dbt_adapter_core::AdapterType;
        use dbt_schemas::schemas::CommonAttributes;
        use dbt_schemas::schemas::common::DbtMaterialization;
        use dbt_schemas::schemas::{DbtModel, Nodes};

        use super::check_compute_platform_upstreams;

        let consumer_uid = "model.test.consumer";
        let upstream_uid = "model.test.upstream";

        let make_model = |uid: &str,
                          adapter: AdapterType,
                          materialized: DbtMaterialization,
                          table_format: Option<&str>,
                          upstreams: &[&str]| {
            let mut model = DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: uid.to_string(),
                    name: uid.rsplit('.').next().unwrap_or(uid).to_string(),
                    package_name: "test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            model.__base_attr__.adapter = adapter;
            model.__base_attr__.materialized = materialized;
            model.__model_attr__.table_format = table_format.map(str::to_string);
            model.__base_attr__.depends_on.nodes =
                upstreams.iter().map(|s| (*s).to_string()).collect();
            model
        };

        let run = |upstream: DbtModel| {
            let consumer = make_model(
                consumer_uid,
                AdapterType::LakeCompute,
                DbtMaterialization::Table,
                None,
                &[upstream_uid],
            );
            let mut nodes = Nodes::default();
            nodes
                .models
                .insert(consumer_uid.to_string(), Arc::new(consumer));
            nodes
                .models
                .insert(upstream_uid.to_string(), Arc::new(upstream));
            check_compute_platform_upstreams(&nodes, None)
        };

        // A `view` upstream declaring `table_format: iceberg` is still
        // rejected -- Snowflake's `view.sql` never consults `table_format`,
        // so the config is a no-op and the model never actually lands in
        // Iceberg. The error should name the real fix (change `materialized`),
        // not repeat advice to set config that is already set.
        let err = run(make_model(
            upstream_uid,
            AdapterType::Snowflake,
            DbtMaterialization::View,
            Some("iceberg"),
            &[],
        ))
        .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("materializes as 'view'"),
            "expected the view-specific error, got: {msg}"
        );

        // Same for `ephemeral`, which has no physical relation at all.
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::Snowflake,
                DbtMaterialization::Ephemeral,
                Some("iceberg"),
                &[],
            ))
            .is_err()
        );

        // Sanity check: the same upstream materialized as `table` (still with
        // `table_format: iceberg`) is accepted -- confirms the rejection above
        // is specific to `view`/`ephemeral`, not to the table_format value.
        assert!(
            run(make_model(
                upstream_uid,
                AdapterType::Snowflake,
                DbtMaterialization::Table,
                Some("iceberg"),
                &[],
            ))
            .is_ok()
        );
    }

    /// WS1 rule 5 for a seed upstream: a seed lives in `nodes.seeds`, not
    /// `nodes.models`, so it needs its own lookup branch in
    /// `check_compute_platform_upstreams` -- otherwise every seed upstream
    /// looks unreachable regardless of where it's actually placed.
    #[test]
    fn test_check_compute_platform_upstreams_seed() {
        use std::sync::Arc;

        use dbt_adapter_core::AdapterType;
        use dbt_schemas::schemas::{CommonAttributes, DbtModel, DbtSeed, Nodes};

        use super::check_compute_platform_upstreams;

        let consumer_uid = "model.test.consumer";
        let seed_uid = "seed.test.raw_customers";

        let make_consumer = |upstream: &str| {
            let mut model = DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: consumer_uid.to_string(),
                    name: "consumer".to_string(),
                    package_name: "test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            model.__base_attr__.adapter = AdapterType::LakeCompute;
            model.__base_attr__.depends_on.nodes = vec![upstream.to_string()];
            model
        };

        let make_seed = |adapter: AdapterType, catalog_name: Option<&str>| {
            let mut seed = DbtSeed {
                __common_attr__: CommonAttributes {
                    unique_id: seed_uid.to_string(),
                    name: "raw_customers".to_string(),
                    package_name: "test".to_string(),
                    ..Default::default()
                },
                ..Default::default()
            };
            seed.__base_attr__.adapter = adapter;
            seed.__seed_attr__.catalog_name = catalog_name.map(str::to_string);
            seed
        };

        // A seed placed on `lake_compute` itself is a reachable upstream.
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert(consumer_uid.to_string(), Arc::new(make_consumer(seed_uid)));
        nodes.seeds.insert(
            seed_uid.to_string(),
            Arc::new(make_seed(AdapterType::LakeCompute, None)),
        );
        assert!(check_compute_platform_upstreams(&nodes, None).is_ok());

        // A plain warehouse-native seed (no `lake_compute`, no `catalog_name`) is
        // rejected, same as a plain warehouse-native model would be.
        let mut nodes = Nodes::default();
        nodes
            .models
            .insert(consumer_uid.to_string(), Arc::new(make_consumer(seed_uid)));
        nodes.seeds.insert(
            seed_uid.to_string(),
            Arc::new(make_seed(AdapterType::Snowflake, None)),
        );
        assert!(check_compute_platform_upstreams(&nodes, None).is_err());
    }
}
