use dbt_adapter::adapter::AdapterFactory;
use dbt_adapter::sql_types::TypeOpsFactory;
use dbt_clap_core::{Cli, Command, CoreCommand};
use dbt_common::{FsResult, io_args::EvalArgs};
use dbt_compilation::config::CompilationConfig;
use dbt_compilation::core::DbtLoadedProject;
use dbt_jinja_utils::JinjaFactory;
use dbt_jinja_utils::node_resolver::{NodeResolver, PackageSearchOrder};
use dbt_metadata::{
    file_registry::CompleteStateWithKind,
    partial_parse::{
        dbt_packages_have_no_file_changes, load_parse_state_filtered_with_unique_ids,
        payload_kinds_for_command, reconstruct_package_metadata, reconstruct_relation_calls,
        unique_id_filter_for_dirty, unique_id_filter_for_selector,
    },
};
use dbt_schemas::{
    schemas::common::ResolvedQuoting,
    state::{DbtRuntimeConfig, DbtState, NodeResolverTracker, ResolverState},
};

use std::sync::{Arc, OnceLock};

use crate::compilation::DbtProjectCompilation;

pub enum PrevCompilationResult {
    /// Pass to initialize() for incremental parse
    Incremental(Arc<DbtProjectCompilation>),
    /// Non-incremental change detected — caller should do full parse without prev
    FullParse,
    /// No state file, corrupted, or validation failed
    None,
}

/// Attempt to load and reconstruct a previous compilation from the parquet parse cache.
///
/// Returns:
/// - `Incremental(prev)` — cache is valid; `prev` can be passed to `initialize()` or
///   returned directly (fast-path).
/// - `FullParse` — cache exists but a non-incremental change was detected (e.g. yaml
///   modified); caller must do a full parse *without* passing `prev`.
/// - `None` — no cache, corrupted, or validation failed; caller does a normal full parse.
///
/// The returned bool is `use_lazy_filter`: true when a unique-id filter was applied to the
/// parquet load (via `--partial-load` OR `--dirty`). Callers use it in place of
/// `effective_partial_load()` to enable the fast path and track `partial_load_filter_applied`.
#[allow(clippy::cognitive_complexity)]
pub fn try_load_prev_compilation(
    eval: &EvalArgs,
    config: &CompilationConfig,
    cli: &Cli,
    type_ops_factory: Arc<dyn TypeOpsFactory>,
    adapter_factory: Arc<dyn AdapterFactory>,
    jinja_factory: Arc<dyn JinjaFactory>,
) -> (PrevCompilationResult, bool) {
    let io = &eval.io;
    let has_dirty = cli.common_args.dirty;

    // --dirty requires the parse cache. Fail early with a clear error rather than
    // silently selecting nothing (which would happen if the cache doesn't exist).
    if has_dirty {
        let cache_dir = eval.metadata_dir().join("parse");
        if !cache_dir.exists() {
            tracing::error!(
                "`--dirty` requires a parse cache (none found at {}). \
                 Run once with `--partial-parse` to build the cache.",
                cache_dir.display()
            );
            return (PrevCompilationResult::None, false);
        }
    }

    // --dirty implies partial-load: it narrows the loaded set to dirty nodes via
    // unique_id_filter, so all partial-load machinery (fast path, dep-closure check,
    // filter_applied tracking) must fire for it too.
    let use_lazy_filter = cli.common_args.effective_partial_load() || has_dirty;
    let allowed_kinds = if use_lazy_filter {
        payload_kinds_for_command(eval, io)
    } else {
        None
    };
    let allowed_unique_ids = if use_lazy_filter {
        if has_dirty {
            // --dirty: compute the dirty set from file mtimes, then expand downstream.
            // This replaces the old state:dirty selector atom.
            unique_id_filter_for_dirty(io)
        } else {
            unique_id_filter_for_selector(eval, io)
        }
    } else {
        None
    };

    // When a --select is present but the unique_id filter resolved to None (selector
    // matched nothing in the cached index — possibly because the index is stale and the
    // selected node is brand-new), disable the lazy-filter fast path.  The fast path
    // only checks mtimes of files it already knows about and cannot detect new files;
    // without this guard it would silently return an empty result instead of letting
    // initialize() run WalkDir and discover the new node.
    let use_lazy_filter = if use_lazy_filter
        && eval.select.is_some()
        && !has_dirty
        && allowed_unique_ids.is_none()
    {
        tracing::debug!(
            "Partial parse: selector present but not resolvable from index — disabling fast path"
        );
        false
    } else {
        use_lazy_filter
    };

    tracing::debug!(
        "Partial parse: kind filter = {:?}, unique_id filter = {:?}",
        allowed_kinds
            .as_ref()
            .map(|s| s.iter().copied().collect::<Vec<_>>()),
        allowed_unique_ids.as_ref().map(|s| s.len()),
    );

    let Some(state) = load_parse_state_filtered_with_unique_ids(
        io,
        allowed_kinds.as_ref(),
        allowed_unique_ids.as_ref(),
    ) else {
        return (PrevCompilationResult::None, use_lazy_filter);
    };

    if let Some(reason) = state.validate(&cli.common_args.vars) {
        tracing::debug!("Partial parse: {reason}, invalidating cache");
        return (PrevCompilationResult::None, use_lazy_filter);
    }

    if let Some(reason) = state.needs_full_parse() {
        tracing::debug!("Partial parse: {reason}, falling back to full parse");
        return (PrevCompilationResult::FullParse, use_lazy_filter);
    }

    // Check dependency closure whenever filtering was applied — either via --partial-load
    // or via --dirty (which also narrows the loaded set via unique_id_filter).
    // Without this check, a corrupted or incomplete index could produce a node set that
    // is missing dependencies, causing a scheduler error instead of a clean fallback.
    if use_lazy_filter {
        if let Some(reason) = state.all_deps_present() {
            tracing::debug!("Partial parse: {reason}, falling back to full parse");
            return (PrevCompilationResult::FullParse, use_lazy_filter);
        }
    }

    let manifest_path_configs = state
        .packages
        .iter()
        .map(|package| {
            (
                package.package_name.clone(),
                package.manifest_path_config.clone(),
            )
        })
        .collect();

    let Some(packages) = state
        .packages
        .iter()
        .map(reconstruct_package_metadata)
        .collect::<FsResult<Vec<_>>>()
        .ok()
    else {
        return (PrevCompilationResult::None, use_lazy_filter);
    };

    let dbt_state = Arc::new(DbtState {
        dbt_profile: state.dbt_profile.clone(),
        run_started_at: chrono::Utc::now().with_timezone(&chrono_tz::UTC),
        packages,
        vars: state.vars,
        cli_vars: Default::default(),
        catalogs: None,
        cloud_config: None,
        warn_error: false,
        warn_error_options: Default::default(),
    });

    let adapter_type = dbt_state.dbt_profile.default_db_config().adapter_type();
    let dbt_quoting = dbt_schemas::dbt_utils::resolve_package_quoting(
        *dbt_state.root_project().quoting,
        adapter_type,
    );
    let Some(root_project_quoting): Option<ResolvedQuoting> = dbt_quoting.try_into().ok() else {
        return (PrevCompilationResult::None, use_lazy_filter);
    };

    let compile_or_test = matches!(
        cli.command,
        Command::Core(CoreCommand::Compile(_)) | Command::Core(CoreCommand::Test(_))
    );
    let node_resolver = match NodeResolver::from_dbt_nodes(
        &state.nodes,
        adapter_type,
        dbt_state.root_project_name().to_string(),
        None,
        Default::default(),
        Default::default(),
        compile_or_test,
        PackageSearchOrder::resolve(dbt_state.root_project().flags.as_ref()),
    ) {
        Ok(r) => Arc::new(r) as Arc<dyn NodeResolverTracker>,
        Err(_) => return (PrevCompilationResult::None, use_lazy_filter),
    };

    let user_defined_schema_registry =
        dbt_schemas::state::hydrate_user_defined_schema_registry(&state.nodes, adapter_type);

    let resolved_state = ResolverState {
        root_project_name: dbt_state.root_project_name().to_string(),
        adapter_type,
        nodes: state.nodes,
        disabled_nodes: state.disabled_nodes,
        macros: state.macros,
        operations: state.operations,
        dbt_profile: state.dbt_profile,
        cloud_config: dbt_state.cloud_config.clone(),
        render_results: Default::default(),
        node_resolver,
        get_relation_calls: reconstruct_relation_calls(
            &state.get_relation_calls,
            adapter_type,
            root_project_quoting,
        ),
        get_columns_in_relation_calls: reconstruct_relation_calls(
            &state.get_columns_in_relation_calls,
            adapter_type,
            root_project_quoting,
        ),
        patterned_dangling_sources: state.patterned_dangling_sources.clone(),
        run_started_at: chrono::Utc::now().with_timezone(&chrono_tz::UTC),
        runtime_config: Arc::new(DbtRuntimeConfig::default()),
        manifest_path_configs,
        manifest_selectors: serde_json::from_str(&state.manifest_selectors_json)
            .unwrap_or_default(),
        resolved_selectors: {
            use dbt_common::node_selector::IndirectSelection;
            use dbt_schemas::schemas::selectors::ResolvedSelector;
            // --dirty was fully resolved at load time via unique_id_filter_for_dirty.
            // The scheduler sees only the user's --select (if any); --dirty is not a
            // selector atom and never reaches the scheduler.
            let mut resolved = ResolvedSelector {
                include: eval.select.clone(),
                exclude: eval.exclude.clone(),
                // Empty on the partial-parse path: selectors.yml is not re-parsed here, so
                // `selector:` references in --select/--exclude cannot be resolved.
                selector_definitions: Default::default(),
            };
            let default_mode: IndirectSelection = eval.indirect_selection.unwrap_or_default();
            if let Some(ref mut include) = resolved.include {
                include.apply_default_indirect_selection(default_mode);
            }
            if let Some(ref mut exclude) = resolved.exclude {
                exclude.apply_default_indirect_selection(default_mode);
            }
            resolved
        },
        root_project_quoting,
        defer_nodes: None,
        nodes_with_resolution_errors: state.nodes_with_resolution_errors,
        nodes_with_access_errors: state.nodes_with_access_errors,
        semantic_layer_spec_is_legacy: false,
        test_name_truncations: Default::default(),
        user_defined_schema_registry,
    };

    let loaded_project = DbtLoadedProject::from_parts(
        config.clone(),
        type_ops_factory,
        adapter_factory,
        dbt_state,
        jinja_factory,
    );

    let compilation = Arc::new(DbtProjectCompilation {
        loaded_project: Arc::new(loaded_project),
        resolved_state,
        lazy_dbt_manifest: OnceLock::new(),
        file_kind_registry: CompleteStateWithKind::new(),
        metricflow_server_client: None,
        catalog_artifact: None,
        previous_state: None,
        run_cache_state_selector_args: None,
        invocation_id: uuid::Uuid::new_v4().to_string(),
        partial_load_filter_applied: false,
    });

    (
        PrevCompilationResult::Incremental(compilation),
        use_lazy_filter,
    )
}

/// --partial-load fast path: if all file mtimes are unchanged since the last parse, skip
/// the expensive WalkDir scan inside DbtLoadedProject::load entirely.
///
/// Returns `Some(compilation)` if the fast path applies, `None` to fall through.
/// On `None`, `maybe_prev` is left untouched so the caller can still pass it to `initialize()`.
pub fn try_lazy_load_fast_path(
    maybe_prev: &mut Option<Arc<DbtProjectCompilation>>,
) -> Option<DbtProjectCompilation> {
    let should_fast_path = maybe_prev.as_ref().is_some_and(|prev| {
        let packages = prev.loaded_project.dbt_state().packages.clone();
        dbt_packages_have_no_file_changes(&packages)
    });
    if !should_fast_path {
        return None;
    }
    let prev_arc = maybe_prev.take().expect("should_fast_path implies Some");
    match Arc::try_unwrap(prev_arc) {
        Ok(compilation) => Some(compilation),
        Err(arc) => {
            // Another Arc ref exists (e.g. server holds a reference).
            // Put it back so the caller can still pass it to initialize().
            tracing::debug!(
                "Partial parse: fast-path aborted — Arc has {} strong refs, falling through",
                Arc::strong_count(&arc)
            );
            *maybe_prev = Some(arc);
            None
        }
    }
}
