//! Unified compilation pipeline abstraction for dbt

use dbt_adapter_core::AdapterType;
use dbt_common::{
    ErrorCode, FsResult,
    cancellation::CancellationToken,
    fs_err,
    io_args::LocalExecutionBackendKind,
    io_utils::{CSV_EXT, SQL_EXT},
    node_selector::{IndirectSelection, MethodName, SelectExpression, SelectionCriteria},
    tracing::emit::create_info_span,
    tracing::span_info::SpanStatusRecorder as _,
};
use dbt_compilation::core::DbtLoadedProject;
use dbt_dag::schedule::Schedule;
use dbt_scheduler::{
    args::SchedulerArgs,
    node_selector::StateSelectorResults,
    schedule::{
        build_schedule_with_state_selector_results, modify_schedule_for_sidecar_compute_boundaries,
    },
};
use dbt_schemas::IndexMap;
use dbt_schemas::state::ProfileAdapter;
use dbt_schemas::{
    schemas::{
        InternalDbtNodeAttributes, Nodes, StateArtifacts,
        common::DbtMaterialization,
        profiles::Execute,
        selectors::{ResolvedSelector, SelectorEntry},
    },
    state::{CacheState, ResolverState},
};
use dbt_state::selector::{RunCacheStateSelectorArgs, evaluate_state_selector};
use dbt_telemetry::{ExecutionPhase, PhaseExecuted};
use std::collections::{BTreeSet, HashMap};

use tracing::Instrument as _;

use crate::debug::DebugArgs;

/// Common compilation pipeline phases
struct CompilationPipeline;

impl CompilationPipeline {
    /// Phase 3: Schedule
    /// Note: Phases 1 (Load) + 2 (Resolve) live in dbt-compilation/src/core.rs.
    pub async fn schedule_phase(
        schedule_args: SchedulerArgs,
        resolved_state: &ResolverState,
        previous_state: Option<&StateArtifacts>,
        run_cache_state_selector_args: Option<&RunCacheStateSelectorArgs>,
        atoms: Option<Vec<SelectExpression>>,
        local_execution_backend: LocalExecutionBackendKind,
        token: &CancellationToken,
    ) -> FsResult<Schedule<String>> {
        // Build selectors for incremental or use existing
        let mut resolved_selectors = resolved_state.resolved_selectors.clone();

        // Check if inline model exists
        let maybe_inline_model_name = resolved_state
            .nodes
            .models
            .values()
            .find(|model| model.materialized() == DbtMaterialization::Inline)
            .map(|model| model.__common_attr__.name.clone());

        // Handle inline model selection - override all selectors to select only the inline model
        if let Some(inline_model) = maybe_inline_model_name {
            // Create a selector that exactly matches the inline model name
            resolved_selectors.include = Some(SelectExpression::Atom(SelectionCriteria {
                method: MethodName::Fqn,
                method_args: vec![],
                value: inline_model.into(),
                childrens_parents: false,
                parents_depth: None,
                children_depth: None,
                indirect: None,
                exclude: None,
            }));
            // Clear any excludes to ensure the inline model is selected
            resolved_selectors.exclude = None;
        }

        // Includes new selectors if given
        if let Some(ref atoms) = atoms {
            let expr = SelectExpression::Or(atoms.clone());

            // Merge with existing selectors if any
            resolved_selectors.include = match resolved_selectors.include {
                Some(existing) => Some(SelectExpression::And(vec![expr, existing])),
                None => Some(expr),
            };
        }

        // Create schedule
        let state_selector_results = state_selector_results(
            &resolved_state.nodes,
            &resolved_selectors,
            previous_state,
            run_cache_state_selector_args,
        )
        .await?;
        let mut schedule = build_schedule_with_state_selector_results(
            &schedule_args,
            &resolved_state.nodes,
            previous_state,
            &resolved_selectors,
            state_selector_results.as_ref(),
            token,
            resolved_state.adapter_type,
        )?;
        Self::check_scheduled_adapters_are_declared(
            &resolved_state.nodes,
            &schedule,
            &resolved_state.dbt_profile.adapters,
            &resolved_state.dbt_profile.target,
        )?;
        let execute = Execute::from_compute_flag(local_execution_backend);
        if matches!(execute, Execute::Sidecar | Execute::Service) {
            modify_schedule_for_sidecar_compute_boundaries(&mut schedule, &resolved_state.nodes);
        }
        Ok(schedule)
    }

    /// Every node that will actually execute must run on an adapter the active
    /// target declares.
    ///
    /// Deliberately after scheduling, not at parse. A project may carry
    /// `+adapter: bigquery` and be run against a Snowflake-only target -- perfectly
    /// legitimate so long as selection excludes those nodes -- and parse cannot
    /// know what selection will do. The schedule is the first point where the
    /// executing set is known, so it is the first point this can be an error
    /// rather than a guess.
    ///
    /// Only `selected_nodes` is checked. Frontier nodes are pulled in for schema
    /// hydration rather than execution; a frontier node on an undeclared adapter is
    /// a separate concern and would surface from hydration itself.
    fn check_scheduled_adapters_are_declared(
        nodes: &Nodes,
        schedule: &Schedule<String>,
        declared: &IndexMap<AdapterType, ProfileAdapter>,
        target: &str,
    ) -> FsResult<()> {
        // First offending node per adapter, so the diagnostic names an example
        // without listing a whole subgraph. A Vec rather than a map because
        // `AdapterType` is not `Ord` and the set is at most a handful wide.
        let mut undeclared: Vec<(AdapterType, &String)> = Vec::new();
        for unique_id in &schedule.selected_nodes {
            if let Some(node) = nodes.get_node(unique_id) {
                let adapter = node.node_adapter();
                if !declared.contains_key(&adapter)
                    && !undeclared.iter().any(|(seen, _)| *seen == adapter)
                {
                    undeclared.push((adapter, unique_id));
                }
            }
        }

        if undeclared.is_empty() {
            return Ok(());
        }

        let offenders = undeclared
            .iter()
            .map(|(adapter, unique_id)| format!("'{adapter}' (e.g. {unique_id})"))
            .collect::<Vec<_>>()
            .join(", ");
        let declared_list = declared
            .keys()
            .map(|t| t.as_ref())
            .collect::<Vec<_>>()
            .join(", ");
        Err(fs_err!(
            ErrorCode::InvalidConfig,
            "selected nodes need adapters the target '{}' does not declare: {offenders}. \
             It declares: {declared_list}. Either configure the missing adapter in profiles.yml \
             or narrow your selection so those nodes are excluded.",
            target
        ))
    }

    fn build_atoms_from_cache_state(cache_state: &CacheState) -> Vec<SelectExpression> {
        let mut atoms = Vec::new();
        for path in cache_state
            .file_changes
            .new_files
            .iter()
            .chain(cache_state.file_changes.impacted_files.iter())
            .filter(|p| p.has_extension(SQL_EXT) || p.has_extension(CSV_EXT))
        {
            let criteria = SelectionCriteria {
                method: MethodName::Path,
                method_args: vec![],
                value: path.to_str().unwrap_or_default().into(),
                childrens_parents: false,
                parents_depth: Some(u32::MAX),
                children_depth: Some(u32::MAX),
                indirect: Some(IndirectSelection::Eager),
                exclude: None,
            };
            atoms.push(SelectExpression::Atom(criteria));
        }
        atoms
    }

    /// Helper: Build selector atoms from unique ids
    pub fn build_atoms_from_unique_ids(
        unique_ids: &[String],
        include_parents: bool,
        include_children: bool,
        indirect_selection: IndirectSelection,
    ) -> Vec<SelectExpression> {
        let mut atoms = Vec::new();

        for unique_id in unique_ids {
            // Determine the appropriate selector method based on node type.
            // Sources must use source: selector, not FQN (matches dbt-core behavior
            // where QualifiedNameSelectorMethod uses non_source_nodes()).
            let (method, value) = if unique_id.starts_with("source.") {
                // Convert source unique_id to source selector pattern
                // source.package.source_name.table_name -> package.source_name.table_name
                let pattern = unique_id.strip_prefix("source.").unwrap_or(unique_id);
                (MethodName::Source, pattern.to_string())
            } else {
                (MethodName::Fqn, unique_id.to_string())
            };

            let criteria = SelectionCriteria {
                method,
                method_args: vec![],
                value: value.into(),
                childrens_parents: false,
                parents_depth: {
                    if include_parents {
                        Some(u32::MAX)
                    } else {
                        None
                    }
                },
                children_depth: {
                    if include_children {
                        Some(u32::MAX)
                    } else {
                        None
                    }
                },
                indirect: Some(indirect_selection),
                exclude: None,
            };
            atoms.push(SelectExpression::Atom(criteria));
        }
        atoms
    }
}

async fn state_selector_results(
    nodes: &Nodes,
    selectors: &ResolvedSelector,
    previous_state: Option<&StateArtifacts>,
    args: Option<&RunCacheStateSelectorArgs>,
) -> FsResult<Option<StateSelectorResults>> {
    let Some(args) = args else {
        return Ok(None);
    };
    if previous_state.is_some() {
        return Ok(None);
    }

    let mut values = BTreeSet::new();
    // Traverse include expression
    if let Some(include) = &selectors.include {
        collect_service_state_selector_values(
            include,
            &mut values,
            &selectors.selector_definitions,
        );
    }
    // Also traverse exclude expression - state selectors there need service results too
    if let Some(exclude) = &selectors.exclude {
        collect_service_state_selector_values(
            exclude,
            &mut values,
            &selectors.selector_definitions,
        );
    }

    let mut results = StateSelectorResults::new();
    for value in values {
        let selected = evaluate_state_selector(nodes, args, &value)
            .await
            .map_err(|err| {
                fs_err!(
                    ErrorCode::SelectorError,
                    "Failed to evaluate state:{value}: {err}"
                )
            })?;
        results.insert(value, selected);
    }
    Ok(Some(results))
}

fn collect_service_state_selector_values(
    expr: &SelectExpression,
    values: &mut BTreeSet<String>,
    selector_definitions: &HashMap<String, SelectorEntry>,
) {
    match expr {
        SelectExpression::Atom(criteria) => {
            if criteria.method == MethodName::State {
                if let Some(value) = criteria.value.as_str() {
                    values.insert(value.to_string());
                }
            } else if criteria.method == MethodName::Selector {
                // Handle selector:name references by looking up the named selector
                if let Some(entry) = criteria
                    .value
                    .as_str()
                    .and_then(|name| selector_definitions.get(name))
                {
                    collect_service_state_selector_values(
                        &entry.include,
                        values,
                        selector_definitions,
                    );
                }
            }
            // Also traverse nested excludes within the atom
            if let Some(ref exclude) = criteria.exclude {
                collect_service_state_selector_values(exclude, values, selector_definitions);
            }
        }
        SelectExpression::And(children) | SelectExpression::Or(children) => {
            for child in children {
                collect_service_state_selector_values(child, values, selector_definitions);
            }
        }
        SelectExpression::Exclude(inner) => {
            collect_service_state_selector_values(inner, values, selector_definitions);
        }
    }
}

pub async fn schedule(
    resolved_state: &ResolverState,
    schedule_args: SchedulerArgs,
    previous_state: Option<&StateArtifacts>,
    run_cache_state_selector_args: Option<&RunCacheStateSelectorArgs>,
    local_execution_backend: LocalExecutionBackendKind,
    token: &CancellationToken,
) -> FsResult<Schedule<String>> {
    CompilationPipeline::schedule_phase(
        schedule_args,
        resolved_state,
        previous_state,
        run_cache_state_selector_args,
        None,
        local_execution_backend,
        token,
    )
    .await
}

/// Schedule with explicit select expressions (used by Pull command)
/// This REPLACES the resolved_selectors.include instead of merging,
/// making Pull behave like Run/Build with the given select.
pub async fn schedule_with_select(
    resolved_state: &ResolverState,
    mut schedule_args: SchedulerArgs,
    previous_state: Option<&StateArtifacts>,
    run_cache_state_selector_args: Option<&RunCacheStateSelectorArgs>,
    select_expr: SelectExpression,
    exclude_expr: Option<SelectExpression>,
    local_execution_backend: LocalExecutionBackendKind,
    token: &CancellationToken,
) -> FsResult<Schedule<String>> {
    // Clear resource_types to allow all node types (like Build does)
    // Pull's --select should work on models, not just sources
    schedule_args.resource_types.clear();
    schedule_args.exclude_resource_types.clear();

    // Create a modified resolved_state with the select expression as the selector
    let resolved_selectors = ResolvedSelector {
        include: Some(select_expr),
        exclude: exclude_expr,
        ..Default::default()
    };

    // Call build_schedule directly with the overridden selectors
    let state_selector_results = state_selector_results(
        &resolved_state.nodes,
        &resolved_selectors,
        previous_state,
        run_cache_state_selector_args,
    )
    .await?;
    let mut schedule = build_schedule_with_state_selector_results(
        &schedule_args,
        &resolved_state.nodes,
        previous_state,
        &resolved_selectors,
        state_selector_results.as_ref(),
        token,
        resolved_state.adapter_type,
    )?;
    let execute = Execute::from_compute_flag(local_execution_backend);
    if matches!(execute, Execute::Sidecar | Execute::Service) {
        modify_schedule_for_sidecar_compute_boundaries(&mut schedule, &resolved_state.nodes);
    }
    Ok(schedule)
}

#[allow(clippy::too_many_arguments)]
pub async fn schedule_with_unique_ids(
    resolved_state: &ResolverState,
    schedule_args: SchedulerArgs,
    previous_state: Option<&StateArtifacts>,
    run_cache_state_selector_args: Option<&RunCacheStateSelectorArgs>,
    unique_ids: &[String],
    include_parents: bool,
    include_children: bool,
    indirect_selection: IndirectSelection,
    local_execution_backend: LocalExecutionBackendKind,
    token: &CancellationToken,
) -> FsResult<Schedule<String>> {
    let atoms = CompilationPipeline::build_atoms_from_unique_ids(
        unique_ids,
        include_parents,
        include_children,
        indirect_selection,
    );
    CompilationPipeline::schedule_phase(
        schedule_args,
        resolved_state,
        previous_state,
        run_cache_state_selector_args,
        Some(atoms),
        local_execution_backend,
        token,
    )
    .await
}

pub async fn schedule_with_cache_state(
    resolved_state: &ResolverState,
    schedule_args: SchedulerArgs,
    previous_state: Option<&StateArtifacts>,
    run_cache_state_selector_args: Option<&RunCacheStateSelectorArgs>,
    cache_state: &CacheState,
    local_execution_backend: LocalExecutionBackendKind,
    token: &CancellationToken,
) -> FsResult<Schedule<String>> {
    let atoms = CompilationPipeline::build_atoms_from_cache_state(cache_state);
    CompilationPipeline::schedule_phase(
        schedule_args,
        resolved_state,
        previous_state,
        run_cache_state_selector_args,
        Some(atoms),
        local_execution_backend,
        token,
    )
    .await
}

pub mod loaded_project {
    use crate::debug;

    use super::*;

    pub async fn debug(
        loaded_project: &DbtLoadedProject,
        debug_args: DebugArgs,
        token: &CancellationToken,
    ) -> FsResult<()> {
        let span = create_info_span(PhaseExecuted::start_general(ExecutionPhase::Debug));

        debug::debug(&debug_args, loaded_project, token.clone())
            .instrument(span.clone())
            .await
            .record_status(&span)
    }
}

#[cfg(test)]
mod scheduled_adapter_tests {
    use super::*;
    use dbt_common::cancellation::never_cancels;
    use dbt_schemas::schemas::profiles::{DbConfig, SnowflakeDbConfig};
    use dbt_schemas::schemas::{CommonAttributes, DbtModel};
    use std::sync::Arc;

    /// A target declaring `declared`, with `scheduled` selected for execution.
    fn check(declared: &[AdapterType], scheduled: &[(&str, AdapterType)]) -> FsResult<()> {
        let mut nodes = Nodes::default();
        let mut schedule = Schedule::<String>::default();
        for (unique_id, adapter) in scheduled {
            let mut model = DbtModel::default();
            model.__base_attr__.adapter = *adapter;
            nodes
                .models
                .insert((*unique_id).to_string(), Arc::new(model));
            schedule.selected_nodes.insert((*unique_id).to_string());
        }

        let declared: IndexMap<AdapterType, ProfileAdapter> = declared
            .iter()
            .map(|t| {
                (
                    *t,
                    ProfileAdapter::single(
                        DbConfig::Snowflake(Box::<SnowflakeDbConfig>::default()),
                    ),
                )
            })
            .collect();

        CompilationPipeline::check_scheduled_adapters_are_declared(
            &nodes, &schedule, &declared, "prod",
        )
    }

    #[test]
    fn scheduled_nodes_on_declared_adapters_pass() {
        check(
            &[AdapterType::Snowflake, AdapterType::Bigquery],
            &[
                ("model.p.a", AdapterType::Snowflake),
                ("model.p.b", AdapterType::Bigquery),
            ],
        )
        .expect("both adapters are declared");
    }

    /// The whole reason this is not a parse-time check: a project may carry nodes
    /// on an adapter the target lacks, so long as selection leaves them out.
    #[test]
    fn an_undeclared_adapter_is_fine_when_selection_excludes_it() {
        check(
            &[AdapterType::Snowflake],
            &[("model.p.a", AdapterType::Snowflake)],
        )
        .expect("nothing scheduled needs the missing adapter");
    }

    #[test]
    fn a_scheduled_node_on_an_undeclared_adapter_fails_and_names_it() {
        let err = check(
            &[AdapterType::Snowflake],
            &[("model.p.needs_bq", AdapterType::Bigquery)],
        )
        .expect_err("bigquery is scheduled but not declared");
        let msg = err.to_string();
        assert!(msg.contains("bigquery"), "must name the adapter: {msg}");
        assert!(
            msg.contains("model.p.needs_bq"),
            "must name an offending node: {msg}"
        );
        assert!(
            msg.contains("snowflake"),
            "must say what the target does declare: {msg}"
        );
    }

    /// One example per adapter, not one line per node.
    #[test]
    fn several_nodes_on_one_undeclared_adapter_report_once() {
        let err = check(
            &[AdapterType::Snowflake],
            &[
                ("model.p.a", AdapterType::Bigquery),
                ("model.p.b", AdapterType::Bigquery),
            ],
        )
        .expect_err("bigquery is not declared");
        let msg = err.to_string();
        assert_eq!(msg.matches("bigquery").count(), 1, "reported twice: {msg}");
    }

    #[test]
    fn state_selector_results_are_used_when_building_the_schedule() {
        let unique_id = "model.project.selected".to_string();
        let mut nodes = Nodes::default();
        nodes.models.insert(
            unique_id.clone(),
            Arc::new(DbtModel {
                __common_attr__: CommonAttributes {
                    unique_id: unique_id.clone(),
                    ..Default::default()
                },
                ..Default::default()
            }),
        );
        let selectors = ResolvedSelector {
            include: Some(SelectExpression::Atom(SelectionCriteria {
                method: MethodName::State,
                method_args: vec![],
                value: "modified".into(),
                childrens_parents: false,
                parents_depth: None,
                children_depth: None,
                indirect: None,
                exclude: None,
            })),
            ..Default::default()
        };
        let state_selector_results = StateSelectorResults::from([(
            "modified".to_string(),
            BTreeSet::from([unique_id.clone()]),
        )]);

        let schedule = build_schedule_with_state_selector_results(
            &SchedulerArgs::default(),
            &nodes,
            None,
            &selectors,
            Some(&state_selector_results),
            &never_cancels(),
            AdapterType::Bigquery,
        )
        .expect("the externally evaluated state selector result is scheduled");

        assert_eq!(schedule.selected_nodes, BTreeSet::from([unique_id]));
    }
}
