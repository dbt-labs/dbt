//! Utility functions for the resolver
use crate::args::ResolveArgs;
use crate::dbt_namespace::DbtNamespace;
use crate::dbt_project_config::ProjectConfigResolver;
use crate::resolve::resolve_properties::MinimalPropertiesEntry;
use crate::sql_file_info::SqlFileInfo;
use crate::utils::{
    extract_unrendered_config_from_ast, get_node_fqn, get_snapshot_fqn, parse_unrendered_config,
    register_duplicate_resource, trigger_duplicate_errors,
};
use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::{DBT_SNAPSHOTS_DIR_NAME, DBT_TARGET_DIR_NAME, PARSING};
use dbt_common::io_args::{FsCommand, IoArgs, StaticAnalysisKind};
use dbt_common::tokiofs::read_to_string;
use dbt_common::tracing::dbt_emit::{
    emit_debug_log_message, emit_error_log_from_fs_error, emit_warn_log_from_fs_error,
};
use dbt_common::tracing::span_info::SpanStatusRecorder as _;
use dbt_common::{ErrorCode, FsError, FsResult, create_debug_span, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::{
    DefaultJinjaTypeCheckEventListenerFactory, DefaultRenderingEventListenerFactory,
    JinjaTypeCheckingEventListenerFactory, RenderingEventListenerFactory,
};
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::phases::build_operation_context_btreemap;
use dbt_jinja_utils::phases::compile::{
    DependencyValidationConfig, build_compile_node_context_inner,
};
use dbt_jinja_utils::phases::parse::build_resolve_model_context;
use dbt_jinja_utils::serde::into_typed_with_jinja_error_context;
use dbt_jinja_utils::silence_base_context;
use dbt_jinja_utils::utils::{render_sql, render_sql_with_listeners};
use dbt_schemas::schemas::common::{DbtChecksum, DbtQuoting, Hooks, normalize_sql};
use dbt_schemas::schemas::project::{ResolvableConfig, ResolvedConfig};
use dbt_schemas::schemas::properties::GetConfig;
use dbt_schemas::schemas::telemetry::NodeType;
use dbt_schemas::schemas::{InternalDbtNodeAttributes, IntrospectionKind, Nodes};
use dbt_schemas::state::{DbtAsset, DbtRuntimeConfig, ModelStatus};
use dbt_telemetry::AssetParsed;
use std::cell::RefCell;
use std::fmt::Debug;
use std::rc::Rc;
use tracing::Instrument as _;

use dbt_frontend_common::error::CodeLocation;
use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
use minijinja::constants::{TARGET_PACKAGE_NAME, TARGET_UNIQUE_ID};
use minijinja::{MacroSpans, Value as MinijinjaValue};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{self, AtomicBool};

/// Represents the result of rendering a single SQL file
#[derive(Debug)]
pub struct SqlFileRenderResult<T: ResolvableConfig<T>, S> {
    /// The asset that was rendered
    pub asset: DbtAsset,
    /// The status of the model
    pub status: ModelStatus,
    /// Whether a render failure was intentionally deferred until command selection.
    pub render_error_deferred: bool,
    /// The file info for the rendered SQL file
    pub sql_file_info: SqlFileInfo<T>,
    /// Fully resolved config for this node (project + properties + inline + root overlay).
    pub config: T::Resolved,
    /// The verbatim, unrendered source contents (raw bytes from disk for SQL/Python,
    /// or the synthesized `select * from {{ ... }}` for YAML-defined snapshots).
    /// Used to populate `raw_code` in the manifest — see dbt-core/core/dbt/parser/base.py:249.
    pub raw_code: String,
    /// The rendered SQL
    pub rendered_sql: String,
    /// The macro spans for the rendered SQL
    pub macro_spans: MacroSpans,
    /// The properties for the model
    pub properties: Option<S>,
    /// The path to the properties file that defines this model
    pub patch_path: Option<PathBuf>,
    /// Macro unique_ids that were invoked during rendering (for `depends_on.macros`).
    pub macro_dependencies: Vec<String>,
    /// Raw (unrendered) kwargs from any `{{ config(...) }}` call(s) in the rendered source,
    /// extracted from the same Jinja parse used to render it (see [`extract_unrendered_config_from_ast`]).
    /// `None` for sources that were never rendered from their own file's raw text (e.g. the
    /// rewritten block-style snapshot stub, whose caller re-derives this from the original file).
    pub raw_config_call_dict: Option<BTreeMap<String, dbt_yaml::Value>>,
}

/// Whether `command` is one of the commands that defer a render error to
/// compile time (see `defer_render_errors_to_compile` on [`RenderCtxInner`])
/// instead of failing eagerly during parse.
fn command_defers_render_errors(command: FsCommand) -> bool {
    matches!(
        command,
        FsCommand::Compile
            | FsCommand::Run
            | FsCommand::RunOperation
            | FsCommand::Test
            | FsCommand::Seed
            | FsCommand::Snapshot
            | FsCommand::Show
            | FsCommand::Build
            | FsCommand::Source
            | FsCommand::Freshness
            | FsCommand::Clone
            | FsCommand::Retry
    )
}

/// Extracts the typed properties (including the `config:` block) from a node's schema.yml entry.
/// For a versioned model this is the only properties-file config layer: the version's own
/// `config:` is already merged into `mpe.schema_value` (see
/// `merge_version_config_into_schema_value`), so typing it again here would apply it twice.
fn extract_model_config<T: ResolvableConfig<T>, S: GetConfig<T> + Debug>(
    mpe: &mut MinimalPropertiesEntry,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    dependency_package_name: Option<&str>,
) -> FsResult<Option<S>> {
    // Note: Duplicate checking is deferred until after we determine if the model is enabled.
    // This matches dbt-core behavior which only checks for duplicates among enabled models.
    // Can occur if a model asset is duplicated, but does not have duplicate property.yml definitions.
    if mpe.schema_value.is_null() {
        return Ok(None);
    }

    // Swap the schema value for Null - we are doing this so that we don't have to clone
    let schema_value = std::mem::replace(&mut mpe.schema_value, dbt_yaml::Value::null());

    let maybe_model = into_typed_with_jinja_error_context::<S, _>(
        schema_value,
        false,
        jinja_env,
        base_ctx,
        &[],
        |error| format!("While parsing config: {}", error.context),
        dependency_package_name,
    )?;

    Ok(Some(maybe_model))
}

/// Appends `source()` calls discovered by static AST analysis to
/// `sql_resources`, keyed by `(source_name, table_name)`.
///
/// The `execute=false` discovery render only observes `source()` calls in
/// branches that evaluate to true, so a source guarded by
/// `{% if is_incremental() %}` or `{% if execute %}` would otherwise be missing
/// from the model's dependencies and never have its schema fetched
/// (dbt-fusion #1660). Sources with non-literal arguments cannot be resolved
/// statically and are left to render-driven discovery.
fn augment_sql_resources_with_static_sources<T: ResolvableConfig<T>>(
    sql: &str,
    display_path: &Path,
    jinja_env: &JinjaEnv,
    sql_resources: &Arc<Mutex<Vec<SqlResource<T>>>>,
) {
    let filename = display_path.to_string_lossy();
    // Parse failure here is non-fatal: the render above already succeeded.
    let Ok(discovered) = jinja_env
        .env
        .find_static_string_arg_calls(&filename, sql, &["source"])
    else {
        return;
    };
    if discovered.is_empty() {
        return;
    }

    let mut resources = sql_resources.lock().unwrap();
    let existing: HashSet<(String, String)> = resources
        .iter()
        .filter_map(|r| match r {
            SqlResource::Source((name, table, _)) | SqlResource::StaticSource((name, table, _)) => {
                Some((name.clone(), table.clone()))
            }
            _ => None,
        })
        .collect();

    for call in discovered {
        let [name, table] = call.args.as_slice() else {
            continue;
        };
        let key = (name.clone(), table.clone());
        if existing.contains(&key) {
            continue;
        }
        let (name, table) = key;
        let span = call.span;
        resources.push(SqlResource::StaticSource((
            name,
            table,
            CodeLocation::new(span.start_line, span.start_col, span.start_offset),
        )));
    }
}

async fn render_sql_file<T, S>(
    render_ctx: &RenderCtx<T>,
    dbt_asset: &DbtAsset,
    node_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    duplicate_errors: &mut Vec<FsError>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<Option<SqlFileRenderResult<T, S>>>
where
    T: ResolvableConfig<T> + 'static,
    S: GetConfig<T> + Debug,
{
    let RenderCtx { inner, .. } = render_ctx;

    let RenderCtxInner {
        args, package_name, ..
    } = &**inner;

    let ref_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();

    let display_path = if dbt_asset.base_path == args.io.out_dir
        && dbt_asset.path.starts_with(DBT_SNAPSHOTS_DIR_NAME)
    {
        dbt_asset.original_path.clone()
    } else if dbt_asset.base_path == args.io.out_dir {
        PathBuf::from(DBT_TARGET_DIR_NAME).join(dbt_asset.to_display_path(&args.io.out_dir))
    } else {
        dbt_asset.to_display_path(&args.io.in_dir)
    };
    let display_path_str = if dbt_asset.base_path == args.io.out_dir
        && dbt_asset.path.starts_with(DBT_SNAPSHOTS_DIR_NAME)
        && let Some(mpe) = node_properties.get(ref_name)
        && mpe.name_span.is_valid()
    {
        format!(
            "{}:{}:{} ({})",
            display_path.display(),
            mpe.name_span.start.line,
            mpe.name_span.start.column,
            ref_name
        )
    } else {
        display_path.display().to_string()
    };

    let span = create_debug_span(AssetParsed::new_with_phase_from_context(
        package_name.clone(),
        ref_name.to_string(),
        dbt_asset.path.display().to_string(),
        display_path_str.clone(),
        None,
    ));

    render_sql_file_inner(
        render_ctx,
        dbt_asset,
        node_properties,
        duplicate_errors,
        token,
        jinja_type_checking_event_listener_factory,
        display_path,
        display_path_str,
        ref_name,
    )
    .instrument(span.clone())
    .await
    .record_status(&span)
}

#[allow(clippy::too_many_arguments)]
async fn render_sql_file_inner<T, S>(
    render_ctx: &RenderCtx<T>,
    dbt_asset: &DbtAsset,
    node_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    duplicate_errors: &mut Vec<FsError>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    display_path: PathBuf,
    display_path_str: String,
    ref_name: &str,
) -> FsResult<Option<SqlFileRenderResult<T, S>>>
where
    T: ResolvableConfig<T> + 'static,
    S: GetConfig<T> + Debug,
{
    let RenderCtx {
        inner,
        jinja_env,
        runtime_config,
        root_runtime_config,
    } = render_ctx;

    let RenderCtxInner {
        args,
        base_ctx,
        root_project_name,
        package_name,
        adapter_type,
        database,
        schema,
        config_resolver,
        resource_paths,
        package_quoting,
        uses_snapshot_fqn,
        defer_render_errors_to_compile,
    } = &**inner;

    token.check_cancellation()?;

    let dependency_package_name = if package_name != root_project_name {
        Some(package_name.as_str())
    } else {
        None
    };

    let has_duplicate_paths = node_properties
        .get(ref_name)
        .map(|mpe| !mpe.duplicate_paths.is_empty())
        .unwrap_or(false);

    let maybe_model = {
        if let Some(mpe) = node_properties.get_mut(ref_name) {
            extract_model_config::<T, S>(mpe, jinja_env, base_ctx, dependency_package_name)
                .map_err(|e| *e)?
        } else {
            None::<S>
        }
    };

    let model_name = ref_name.to_string();

    let (fqn, original_fqn) = if *uses_snapshot_fqn {
        // Snapshots have a single, definition-style-aware fqn; it already reads
        // from the original path where that matters, so both roles use it.
        let fqn = get_snapshot_fqn(
            package_name,
            &dbt_asset.path,
            &dbt_asset.original_path,
            &model_name,
            resource_paths,
        );
        (fqn.clone(), fqn)
    } else {
        (
            get_node_fqn(
                package_name,
                dbt_asset.path.clone(),
                vec![model_name.clone()],
                resource_paths,
            ),
            get_node_fqn(
                package_name,
                dbt_asset.original_path.clone(),
                vec![model_name.clone()],
                resource_paths,
            ),
        )
    };

    let model_properties_config = maybe_model.as_ref().and_then(|m| m.get_config());
    let properties_configs: &[Option<&T>] = &[model_properties_config];
    let properties_config = config_resolver.with_configs(&original_fqn, properties_configs);

    // Early exit: the root overlay has the highest precedence for dependency packages, so if it
    // explicitly disables this node no inline `{{ config(...) }}` call can re-enable it.
    // Skip Jinja rendering entirely, but still statically extract unrendered_config from the raw
    // file (a single, cheap parse) so a disabled node's manifest entry stays accurate.
    if config_resolver.is_disabled_by_root_overlay(&fqn) {
        let raw_config_call_dict = read_to_string(dbt_asset.base_path.join(&dbt_asset.path))
            .await
            .ok()
            .and_then(|sql| parse_unrendered_config(&sql, *uses_snapshot_fqn));
        return Ok(Some(SqlFileRenderResult {
            asset: dbt_asset.clone(),
            sql_file_info: SqlFileInfo::default(),
            config: config_resolver.resolve_with_configs(&original_fqn, &fqn, properties_configs),
            raw_code: String::new(),
            rendered_sql: String::new(),
            macro_spans: MacroSpans::default(),
            properties: maybe_model,
            status: ModelStatus::Disabled,
            render_error_deferred: false,
            patch_path: node_properties
                .get(ref_name)
                .map(|mpe| mpe.relative_path.clone()),
            macro_dependencies: Vec::new(),
            raw_config_call_dict,
        }));
    }

    let absolute_path = dbt_asset.base_path.join(&dbt_asset.path);
    let sql = read_to_string(&absolute_path).await.map_err(|e| *e)?;

    let sql_resources = Arc::new(Mutex::new(Vec::new()));
    let execute_exists = Arc::new(AtomicBool::new(false));

    // dbt-core exposes `model.path` relative to the resource root (e.g.
    // `staging/orders.sql`); strip the configured model-paths prefix from the
    // package-relative asset path so config-block reads of `model.path` match.
    let model_path = dbt_common::path::strip_resource_paths(&dbt_asset.path, resource_paths);

    // A dependency package's inline `config(enabled=false)` must not abort rendering when the
    // root project re-enables the node; the overlay outranks it.
    let root_overlay_forces_enabled = config_resolver.is_enabled_by_root_overlay(&fqn);

    let mut resolve_model_context = base_ctx.clone();
    resolve_model_context.extend(build_resolve_model_context(
        &properties_config,
        root_overlay_forces_enabled,
        *adapter_type,
        database,
        schema,
        &model_name,
        fqn.clone(),
        package_name,
        root_project_name,
        *package_quoting,
        runtime_config.clone(),
        root_runtime_config.clone(),
        sql_resources.clone(),
        execute_exists.clone(),
        &display_path,
        &model_path,
        args.static_analysis,
    ));

    if let Some(status_reporter) = &args.io.status_reporter {
        status_reporter.show_progress(PARSING, &display_path_str, None);
    }

    // Check if mangled ref checking is enabled via global static_analysis settings.
    // Per node suppression is applied later based on the final config for the node.
    let check_mangled_refs = args.static_analysis.unwrap_or_default() != StaticAnalysisKind::Off;

    // Run typecheck
    if let Some(model) = resolve_model_context.get("model")
        && let Ok(unique_id) = model.get_attr("unique_id")
        && let Some(unique_id) = unique_id.as_str()
    {
        let _ = dbt_jinja_utils::typecheck::typecheck(
            &args.io,
            jinja_env.clone(),
            &HashMap::new(),
            jinja_type_checking_event_listener_factory.clone(),
            None,
            &jinja_env.env.get_root_package_name(),
            MinijinjaValue::from_dyn_object(jinja_env.env.get_dbt_and_adapters_namespaces()),
            &display_path,
            &sql,
            &dbt_common::CodeLocationWithFile::new(1, 1, 0, display_path.clone()),
            unique_id,
            *adapter_type,
            true,
        );
    }

    // Create listeners for rendering via factory
    let listener_factory = if check_mangled_refs {
        DefaultRenderingEventListenerFactory::with_mangled_ref_checking(true, args.io.clone())
    } else {
        DefaultRenderingEventListenerFactory::new(true)
    };
    let mut listeners = listener_factory.create_listener_bundle(
        &display_path,
        &CodeLocation::start_of_file(),
        &sql,
    );
    let macro_dep_listener = Rc::new(dbt_jinja_utils::listener::MacroDependencyListener::new());
    listeners.push(macro_dep_listener.clone());

    // Populated by `ast_visitor` below, reusing the AST from the compile it's passed into.
    let raw_config_call_dict_cell: RefCell<Option<BTreeMap<String, dbt_yaml::Value>>> =
        RefCell::new(None);
    let mut ast_visitor = |ast: &minijinja::compiler::ast::Stmt<'_>| {
        *raw_config_call_dict_cell.borrow_mut() =
            extract_unrendered_config_from_ast(ast, *uses_snapshot_fqn, sql.as_bytes());
    };

    let (sql_file_info, resolved_config, status, rendered_sql, macro_spans, macro_dependencies) =
        match render_sql_with_listeners(
            &sql,
            jinja_env.as_ref(),
            &resolve_model_context,
            &listeners,
            &[],
            &display_path,
            Some(&mut ast_visitor),
        ) {
            Ok(rendered_sql) => {
                // A `source()` inside a Jinja branch that evaluates to false
                // during this `execute=false` render is never observed by the
                // render-driven `SqlResource` collection above. Walk the
                // template AST to recover any such sources so their schemas
                // are still fetched (dbt-fusion #1660).
                augment_sql_resources_with_static_sources(
                    &sql,
                    &display_path,
                    jinja_env,
                    &sql_resources,
                );

                let normalized_sql = normalize_sql(&sql);
                // Get config from current resources to use for hook rendering
                let temp_config = {
                    let sql_resources_locked = sql_resources.lock().unwrap().clone();
                    let temp_info = SqlFileInfo::from_sql_resources(
                        sql_resources_locked,
                        DbtChecksum::hash(normalized_sql.as_bytes()),
                        execute_exists.load(atomic::Ordering::Relaxed),
                    );
                    config_resolver.resolve_with_configs(
                        &original_fqn,
                        &fqn,
                        &[
                            model_properties_config,
                            temp_info.explicit_config.as_deref(),
                        ],
                    )
                };

                // Collect dependencies from pre and post hooks (adds to same sql_resources)
                collect_hook_dependencies_from_config(
                    &temp_config,
                    jinja_env.clone(),
                    *adapter_type,
                    &display_path,
                    args.io.clone(),
                    &resolve_model_context,
                    jinja_type_checking_event_listener_factory.clone(),
                )?;

                // Create normalized SQL strings (remove all whitespace and convert to lowercase)
                // These transformations make state:modified stable in the face of whitespace
                // Create final sql_file_info with all dependencies (main SQL + hooks)
                // and case differences. See https://github.com/dbt-labs/dbt-fusion/issues/768
                let normalized_sql = normalize_sql(&sql);

                let (sql_file_info, resolved_config) = {
                    let sql_resources_locked = sql_resources.lock().unwrap().clone();
                    let info = SqlFileInfo::from_sql_resources(
                        sql_resources_locked,
                        DbtChecksum::hash(normalized_sql.as_bytes()),
                        execute_exists.load(atomic::Ordering::Relaxed),
                    );
                    let cfg = config_resolver.resolve_with_configs(
                        &original_fqn,
                        &fqn,
                        &[model_properties_config, info.explicit_config.as_deref()],
                    );
                    (info, cfg)
                };

                let status = if resolved_config.enabled() {
                    ModelStatus::Enabled
                } else {
                    ModelStatus::Disabled
                };

                // Destroy DefaultRenderingEventListener first to transfer macro_spans to factory
                // Keep other listeners for later
                let (default_listeners, other_listeners): (Vec<_>, Vec<_>) =
                    listeners.into_iter().partition(|l| {
                        l.as_any()
                        .downcast_ref::<dbt_jinja_utils::listener::DefaultRenderingEventListener>()
                        .is_some()
                    });
                for listener in default_listeners {
                    listener_factory.destroy_listener(&display_path, listener);
                }

                // Now drain macro_spans from factory
                let macro_spans = listener_factory.drain_macro_spans(&display_path);

                // Emit mangled ref warnings based on the final config's static_analysis setting
                // This allows {{ config(static_analysis='off') }} to suppress warnings
                if resolved_config
                    .get_static_analysis()
                    .map(|s| s.into_inner())
                    != Some(StaticAnalysisKind::Off)
                {
                    for listener in &other_listeners {
                        listener
                            .check_and_emit_mangled_ref_warnings(&rendered_sql, &macro_spans.items);
                    }
                }

                // Destroy remaining listeners
                for listener in other_listeners {
                    listener_factory.destroy_listener(&display_path, listener);
                }

                let macro_dependencies = macro_dep_listener.drain_macro_unique_ids();
                (
                    sql_file_info,
                    resolved_config,
                    (status, false),
                    rendered_sql,
                    macro_spans,
                    macro_dependencies,
                )
            }
            Err(err) => {
                // Keep the partially resolved node available to selection. Executable
                // commands render selected nodes again during compilation, so a render
                // failure in an unselected node must not abort the parse phase.
                let (sql_file_info, resolved_config) = {
                    let sql_resources_locked = sql_resources.lock().unwrap().clone();
                    let normalized_sql = normalize_sql(&sql);
                    let info = SqlFileInfo::from_sql_resources(
                        sql_resources_locked,
                        DbtChecksum::hash(normalized_sql.as_bytes()),
                        execute_exists.load(atomic::Ordering::Relaxed),
                    );
                    let cfg = config_resolver.resolve_with_configs(
                        &original_fqn,
                        &fqn,
                        &[model_properties_config, info.explicit_config.as_deref()],
                    );
                    (info, cfg)
                };

                let (status, render_error_deferred) = match err.code {
                    ErrorCode::DisabledModel => (ModelStatus::Disabled, false),
                    ErrorCode::MacroSyntaxInvalid => {
                        let err_with_loc = err.with_location(display_path.clone());
                        emit_error_log_from_fs_error(err_with_loc);
                        (ModelStatus::ParsingFailed, false)
                    }
                    _ => {
                        if resolved_config.enabled() {
                            let err_with_loc = err.with_location(display_path.clone());
                            if *defer_render_errors_to_compile
                                && command_defers_render_errors(args.command)
                            {
                                emit_debug_log_message(err_with_loc.message());
                                (ModelStatus::ParsingFailed, true)
                            } else {
                                emit_error_log_from_fs_error(err_with_loc);
                                (ModelStatus::ParsingFailed, false)
                            }
                        } else {
                            (ModelStatus::Disabled, false)
                        }
                    }
                };

                (
                    sql_file_info,
                    resolved_config,
                    (status, render_error_deferred),
                    String::new(),
                    MacroSpans::default(),
                    Vec::new(),
                )
            }
        };

    let (status, render_error_deferred) = status;
    let raw_config_call_dict = raw_config_call_dict_cell.into_inner();

    // Only check for duplicate resource definitions for enabled models to match dbt-core behavior.
    if status == ModelStatus::Enabled
        && has_duplicate_paths
        && node_properties.get(ref_name).is_some()
    {
        register_duplicate_resource(
            node_properties.get(ref_name).unwrap(),
            ref_name,
            "model",
            duplicate_errors,
        );
        return Ok(None);
    }

    Ok(Some(SqlFileRenderResult {
        asset: dbt_asset.clone(),
        sql_file_info,
        config: resolved_config,
        // Match dbt-core: file contents are loaded via `load_file_contents(strip=True)`,
        // which trims leading/trailing whitespace before storing as raw_code.
        raw_code: sql.trim().to_owned(),
        rendered_sql,
        macro_spans,
        properties: maybe_model,
        status,
        render_error_deferred,
        patch_path: node_properties
            .get(ref_name)
            .map(|mpe| mpe.relative_path.clone()),
        macro_dependencies,
        raw_config_call_dict,
    }))
}

/// Inner context for rendering sql files
#[derive(Clone)]
pub struct RenderCtxInner<T: ResolvableConfig<T>> {
    /// The arguments for the resolve
    pub args: ResolveArgs,
    /// The base context for the jinja environment
    pub base_ctx: BTreeMap<String, MinijinjaValue>,
    /// The name of the root project
    pub root_project_name: String,
    /// The name of the package
    pub package_name: String,
    /// The type of the adapter
    pub adapter_type: AdapterType,
    /// The database name
    pub database: String,
    /// The schema name
    pub schema: String,
    /// The config resolver for this package
    pub config_resolver: ProjectConfigResolver<T>,
    /// The resource paths
    pub resource_paths: Vec<String>,
    /// The quoting for the package
    pub package_quoting: DbtQuoting,
    /// Derive fqns with [`get_snapshot_fqn`] instead of the generic path + node
    /// name rule. Only snapshots do this: dbt-core keeps the source filename stem
    /// as an fqn segment for block-style snapshots, and `dbt_project.yml` config
    /// is applied off that fqn. Without this the renderer would resolve config
    /// from the rewritten `{snapshot_name}.sql` stub path and disagree with both
    /// dbt-core and the fqn stored on the node.
    pub uses_snapshot_fqn: bool,
    /// Keep parse-failed runnable nodes available for selection so compilation
    /// reports the error only when the command selects that node.
    pub defer_render_errors_to_compile: bool,
}

/// Outer context for rendering sql files
#[derive(Clone)]
pub struct RenderCtx<T: ResolvableConfig<T>> {
    /// The inner context for rendering sql files
    pub inner: Arc<RenderCtxInner<T>>,
    /// The jinja environment
    pub jinja_env: Arc<JinjaEnv>,
    /// This package's own runtime config — used only for the `MacroLookupContext`
    /// package allow-list. See [`build_resolve_model_context`]'s doc comment.
    pub runtime_config: Arc<DbtRuntimeConfig>,
    /// The root project's runtime config — backs `ref`/`source`/`metric`'s
    /// `.config`, matching dbt Core's `self.root_project`. See
    /// [`build_resolve_model_context`]'s doc comment.
    pub root_runtime_config: Arc<DbtRuntimeConfig>,
}

/// Iterate over all the sql files passed in, generate the local config, initialize the sql render env, and render the sql
/// and return the sql resources (deps) found while rendering the files
pub async fn render_unresolved_sql_files<
    T: ResolvableConfig<T> + 'static,
    S: GetConfig<T> + 'static + Debug,
>(
    render_ctx: &RenderCtx<T>,
    model_sql_files: &[DbtAsset],
    node_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<Vec<SqlFileRenderResult<T, S>>> {
    if model_sql_files.is_empty() {
        return Ok(Vec::new());
    }

    let mut max_concurrency =
        crate::parallel::effective_parallelism(render_ctx.inner.args.no_parallel);
    // TODO: why do we have this override?
    if model_sql_files.len() < 50 {
        max_concurrency = 1;
    }
    let chunk_size = model_sql_files.len().div_ceil(max_concurrency);

    // Split node_properties into per-chunk subsets
    let chunks: Vec<(Vec<DbtAsset>, BTreeMap<String, MinimalPropertiesEntry>)> = model_sql_files
        .chunks(chunk_size)
        .map(|chunk| {
            let chunk_vec = chunk.to_vec();
            let mut chunk_props = BTreeMap::new();
            for dbt_asset in &chunk_vec {
                let ref_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();
                if let Some(entry) = node_properties.get(ref_name) {
                    chunk_props.insert(ref_name.to_string(), entry.clone());
                }
            }
            (chunk_vec, chunk_props)
        })
        .collect();

    let render_ctx = Arc::new(render_ctx.clone());
    let token = token.clone();

    let chunk_results = crate::parallel::dispatch_maybe_parallel(
        chunks,
        max_concurrency > 1,
        move |(chunk, mut chunk_node_properties)| {
            let render_ctx = render_ctx.clone();
            let token = token.clone();
            let jinja_type_checking_event_listener_factory =
                jinja_type_checking_event_listener_factory.clone();
            async move {
                let mut local_results: Vec<SqlFileRenderResult<T, S>> = Vec::new();
                let mut local_duplicate_errors: Vec<FsError> = Vec::new();

                for dbt_asset in chunk {
                    token.check_cancellation()?;
                    if let Some(res) = render_sql_file::<T, S>(
                        &render_ctx,
                        &dbt_asset,
                        &mut chunk_node_properties,
                        &mut local_duplicate_errors,
                        &token,
                        jinja_type_checking_event_listener_factory.clone(),
                    )
                    .await?
                    {
                        local_results.push(res);
                    }
                }

                Ok((local_results, local_duplicate_errors, chunk_node_properties))
            }
        },
    )
    .await?;

    let mut results = Vec::new();
    let mut duplicate_errors: Vec<FsError> = Vec::new();
    for (task_results, errors, chunk_node_properties) in chunk_results {
        results.extend(task_results);
        duplicate_errors.extend(errors);
        // Merge back node_properties — update entries that were processed while preserving
        // entries that weren't (e.g., Python models that go through a different code path)
        node_properties.extend(chunk_node_properties);
    }

    trigger_duplicate_errors(&mut duplicate_errors)?;
    Ok(results)
}

/// Collect the adapter identifiers for the given nodes and check if they are detected as unsafe
#[allow(clippy::too_many_arguments)]
pub async fn collect_adapter_identifiers_detect_unsafe<T: InternalDbtNodeAttributes + 'static>(
    arg: &ResolveArgs,
    node_map: HashMap<String, T>,
    node_resolver: &NodeResolver,
    jinja_env: Arc<JinjaEnv>,
    adapter_type: AdapterType,
    package_name: &str,
    root_project_name: &str,
    runtime_config: Arc<DbtRuntimeConfig>,
    token: &CancellationToken,
) -> FsResult<Vec<(T, bool)>> {
    if node_map.is_empty() {
        return Ok(Vec::new());
    }

    let max_concurrency = crate::parallel::effective_parallelism(arg.no_parallel);
    let model_vec: Vec<(String, T)> = node_map.into_iter().collect();
    let chunk_size = model_vec.len().div_ceil(max_concurrency);

    let parse_adapter = jinja_env
        .get_adapter()
        .expect("Adapter should be available during parse phase");

    let chunks = chunk_vec(model_vec, chunk_size);

    let arg = Arc::new(arg.clone());
    let node_resolver = Arc::new(node_resolver.clone());
    let package_name = package_name.to_string();
    let root_project_name = root_project_name.to_string();
    let token = token.clone();

    let chunk_results =
        crate::parallel::dispatch_maybe_parallel(chunks, max_concurrency > 1, move |chunk| {
            let arg = arg.clone();
            let node_resolver = node_resolver.clone();
            let jinja_env = jinja_env.clone();
            let package_name = package_name.clone();
            let root_project_name = root_project_name.clone();
            let runtime_config = runtime_config.clone();
            let parse_adapter = parse_adapter.clone();
            let token = token.clone();
            async move {
                process_model_chunk_for_unsafe_detection(
                    chunk,
                    arg,
                    node_resolver,
                    &jinja_env,
                    adapter_type,
                    package_name,
                    root_project_name,
                    runtime_config,
                    parse_adapter,
                    &token,
                )
                .await
            }
        })
        .await?;

    let mut dbt_nodes: Vec<(T, bool)> = chunk_results.into_iter().flatten().collect();

    for (node, is_unsafe) in dbt_nodes.iter_mut() {
        if *is_unsafe {
            node.set_detected_introspection(IntrospectionKind::Execute);
        }
    }

    Ok(dbt_nodes)
}

/// Processes a chunk of models to detect unsafe identifiers
#[allow(clippy::too_many_arguments)]
async fn process_model_chunk_for_unsafe_detection<T: InternalDbtNodeAttributes + 'static>(
    chunk: Vec<(String, T)>,
    arg: Arc<ResolveArgs>,
    node_resolver: Arc<NodeResolver>,
    jinja_env: &JinjaEnv,
    adapter_type: AdapterType,
    package_name: String,
    root_project_name: String,
    runtime_config: Arc<DbtRuntimeConfig>,
    parse_adapter: Arc<dbt_adapter::Adapter>,
    token: &CancellationToken,
) -> FsResult<Vec<(T, bool)>> {
    let mut nodes = Vec::new();
    let namespace_keys: Vec<String> = jinja_env
        .env
        .get_macro_namespace_registry()
        .map(|r| r.keys().map(|k| k.to_string()).collect())
        .unwrap_or_default();
    let mut render_base_context = build_operation_context_btreemap(
        node_resolver.clone(),
        &package_name,
        &Nodes::default(),
        None,
        runtime_config.clone(),
        namespace_keys,
        None,
    );
    silence_base_context(&mut render_base_context);

    for (_key, model) in chunk {
        token.check_cancellation()?;
        // For snapshots, use path (generated file in target) instead of original_file_path (source file)
        // because the generated file may have a different name than the source file
        let absolute_path = if model.resource_type() == NodeType::Snapshot {
            arg.io.out_dir.join(&model.common().path)
        } else {
            arg.io.in_dir.join(&model.common().original_file_path)
        };
        let sql = read_to_string(&absolute_path).await?;

        render_base_context.insert(
            TARGET_PACKAGE_NAME.to_string(),
            MinijinjaValue::from(model.common().package_name.clone()),
        );
        render_base_context.insert(
            TARGET_UNIQUE_ID.to_string(),
            MinijinjaValue::from(model.common().unique_id.clone()),
        );

        let (mut render_resolved_context, _) = build_compile_node_context_inner(
            &model,
            adapter_type,
            &render_base_context,
            &root_project_name,
            node_resolver.clone(),
            runtime_config.clone(),
            DependencyValidationConfig::new_for_node(&model).skip_validation(),
        )?;

        // Inject the DbtNamespace to intercept dbt macro calls
        let dbt_namespace = DbtNamespace::new(parse_adapter.clone());
        render_resolved_context.insert(
            "dbt".to_string(),
            MinijinjaValue::from_object(dbt_namespace),
        );

        let display_path = if arg
            .io
            .out_dir
            .join(&model.common().original_file_path)
            .exists()
        {
            PathBuf::from(DBT_TARGET_DIR_NAME).join(&model.common().original_file_path)
        } else {
            arg.io.in_dir.join(&model.common().original_file_path)
        };
        // TODO: Potentially catch rendering warning on second pass and notify user / add file as unsafe by default
        let _res = render_sql(
            &sql,
            jinja_env,
            &render_resolved_context,
            &DefaultRenderingEventListenerFactory::default(),
            &display_path,
        );
        let is_unsafe = parse_adapter
            .parse_adapter_state()
            .expect("Adapter must be configured for the parse phase")
            .unsafe_nodes()
            .contains(&model.common().unique_id);
        nodes.push((model, is_unsafe));
    }
    Ok(nodes)
}

fn chunk_vec<T>(mut v: Vec<T>, chunk_size: usize) -> Vec<Vec<T>> {
    let mut chunks = Vec::new();
    while !v.is_empty() {
        let chunk: Vec<T> = v.drain(..chunk_size.min(v.len())).collect();
        chunks.push(chunk);
    }
    chunks
}

/// Collect refs and sources from pre and post hooks in any resource config
/// by rendering them into the existing sql_resources collection
///
/// This function works generically for all resource types (models, snapshots, seeds, etc.)
/// and should only be called when the main resource has been successfully rendered to ensure
/// we have a reliable config and context.
///
/// Uses the real file path for error reporting rather than virtual paths.
#[allow(clippy::too_many_arguments)]
pub fn collect_hook_dependencies_from_config(
    config: &dyn ResolvedConfig,
    jinja_env: Arc<JinjaEnv>,
    adapter_type: AdapterType,
    resource_path: &Path,
    io: IoArgs,
    hook_context: &BTreeMap<String, MinijinjaValue>,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<()> {
    // Helper function to extract SQL strings from hooks
    // Note: YAML span information is available in the original Verbatim<Option<Hooks>> wrapper
    // but is not accessible once converted to DbtConfig. To preserve spans, we would need to:
    // 1. Pass the original Verbatim wrappers to this function
    // 2. Use dbt_yaml APIs to extract span information from the Value objects
    // 3. Update the schema definitions to expose span access methods
    let extract_hook_sqls = |hooks: &Hooks| -> Vec<String> {
        match hooks {
            Hooks::String(sql) => vec![sql.clone()],
            Hooks::ArrayOfStrings(sqls) => sqls.clone(),
            Hooks::HookConfig(hook_config) => {
                if let Some(sql) = &hook_config.sql {
                    vec![sql.clone()]
                } else {
                    vec![]
                }
            }
            Hooks::HookConfigArray(hook_configs) => hook_configs
                .iter()
                .filter_map(|config| config.sql.clone())
                .collect(),
        }
    };

    // Helper function to render hook SQL and collect dependencies into the shared sql_resources
    let render_hook_for_deps = |sql: &str, adapter_type: AdapterType| -> FsResult<()> {
        if let Some(model) = hook_context.get("model")
            && let Ok(unique_id) = model.get_attr("unique_id")
            && let Some(unique_id) = unique_id.as_str()
        {
            let jinja_type_checking_event_listener_factory =
                if jinja_type_checking_event_listener_factory.can_listen_on_hooks() {
                    jinja_type_checking_event_listener_factory.clone()
                } else {
                    Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default())
                };
            Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default());
            let _ = dbt_jinja_utils::typecheck::typecheck(
                &io,
                jinja_env.clone(),
                &HashMap::new(),
                jinja_type_checking_event_listener_factory,
                None,
                &jinja_env.env.get_root_package_name(),
                MinijinjaValue::from_dyn_object(jinja_env.env.get_dbt_and_adapters_namespaces()),
                resource_path,
                sql,
                &dbt_common::CodeLocationWithFile::new(1, 1, 0, resource_path.to_path_buf()),
                unique_id,
                adapter_type,
                true,
            );
        }
        let listener_factory = DefaultRenderingEventListenerFactory::default();
        match render_sql(
            sql,
            jinja_env.as_ref(),
            hook_context,
            &listener_factory,
            resource_path,
        ) {
            Ok(_) => Ok(()),
            Err(err) => {
                // Log hook rendering error with clear context but don't fail the build
                // Question (Ani): What should we do if a hook fails to render?
                let err = fs_err!(
                    ErrorCode::JinjaError,
                    "Hook failed to render: {}",
                    err.to_string()
                )
                .with_location(resource_path.to_path_buf());
                emit_warn_log_from_fs_error(err);

                Ok(()) // Return Ok to avoid breaking the build
            }
        }
    };

    // Process pre-hooks
    if let Some(pre_hooks) = config.get_pre_hook() {
        let hook_sqls = extract_hook_sqls(pre_hooks);
        for sql in hook_sqls.iter() {
            if sql.trim().is_empty() {
                continue;
            }

            render_hook_for_deps(sql, adapter_type)?;
        }
    }

    // Process post-hooks
    if let Some(post_hooks) = config.get_post_hook() {
        let hook_sqls = extract_hook_sqls(post_hooks);
        for sql in hook_sqls.iter() {
            if sql.trim().is_empty() {
                continue;
            }

            render_hook_for_deps(sql, adapter_type)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_defers_render_errors_covers_freshness_alongside_source() {
        // Freshness must defer the same way `dbt source freshness` already does,
        // so a render error in an unselected node can't abort `dbt freshness`.
        assert!(command_defers_render_errors(FsCommand::Source));
        assert!(command_defers_render_errors(FsCommand::Freshness));
    }

    #[test]
    fn command_defers_render_errors_covers_every_documented_command() {
        for command in [
            FsCommand::Compile,
            FsCommand::Run,
            FsCommand::RunOperation,
            FsCommand::Test,
            FsCommand::Seed,
            FsCommand::Snapshot,
            FsCommand::Show,
            FsCommand::Build,
            FsCommand::Source,
            FsCommand::Freshness,
            FsCommand::Clone,
            FsCommand::Retry,
        ] {
            assert!(
                command_defers_render_errors(command),
                "{command:?} should defer render errors to compile"
            );
        }
    }

    #[test]
    fn command_defers_render_errors_excludes_non_executable_commands() {
        for command in [
            FsCommand::List,
            FsCommand::Parse,
            FsCommand::Deps,
            FsCommand::Debug,
            FsCommand::Clean,
        ] {
            assert!(
                !command_defers_render_errors(command),
                "{command:?} should not defer render errors to compile"
            );
        }
    }
}
