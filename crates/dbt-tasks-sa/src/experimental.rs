//! Experimentals: their own kind of node, resolved by the core and run in the task graph.
//!
//! An experimental never implements [`Task`]. The core wraps each selected experimental node in a
//! [`ExperimentalTask`], which calls the experimental's [`ExperimentalRun::run`] on the async pool
//! and turns its outcome into the node's result. Experimentals see only their own nodes, their own
//! data and their nodes' dependencies, and reach the warehouse only through [`ExperimentalRunCtx`].

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use arrow::array::RecordBatch;
use async_trait::async_trait;
use dbt_adapter::adapter::AdapterStore;
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::FsCommand;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::tracing::dbt_metrics::{FusionMetricKey, InvocationMetricKey};
use dbt_common::tracing::metrics::increment_metric;
use dbt_common::{ErrorCode, FsResult};
use dbt_dag::schedule::Schedule;
use dbt_flags::{FlagDef, Stage};
use dbt_schemas::schemas::{
    DbtExperimentalNode, ExperimentalData, InternalDbtNodeAttributes, Nodes,
};
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::{TP, Task, run_blocking_task_operation};
use dbt_telemetry::NodeType;

pub use dbt_parser::experimental::{
    ExperimentalDiagnostics, ExperimentalEntry, ExperimentalError, ExperimentalFile,
    ExperimentalNodeDef, ExperimentalPackage, ResolveExperimental, ResolveExperimentals,
};

/// Adds its own kind of node to a project ([`ResolveExperimental`]) and runs the ones selected.
///
/// An experimental node runs only when it is selected directly, and only in its Run phase: it has
/// nothing to render or analyze.
pub trait ExperimentalNode: ResolveExperimental {
    /// Prepares to run this experimental's selected nodes. Called once per invocation, after nodes
    /// are selected and before the task graph is built, so an error stops the invocation before any
    /// node runs. `None` runs none of them.
    fn start(
        &self,
        args: &ExperimentalRunArgs<'_>,
    ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError>;

    /// The extension command whose task graph runs only this experimental's nodes, if the
    /// experimental adds one.
    fn command(&self) -> Option<&'static str> {
        None
    }
}

/// What an experimental's run starts from.
pub struct ExperimentalRunArgs<'a> {
    pub command: FsCommand,
    pub invocation_id: &'a str,
    pub project_dir: &'a Path,
    /// Where the invocation writes its artifacts.
    pub target_dir: &'a Path,
    /// This experimental's selected nodes, in schedule order.
    pub selected: &'a [ExperimentalNodeRef],
}

/// One invocation's run of an experimental's selected nodes.
#[async_trait]
pub trait ExperimentalRun: Send + Sync {
    /// Runs one of the selected nodes, on the async pool.
    async fn run(
        &self,
        node: &ExperimentalNodeRef,
        ctx: &ExperimentalRunCtx,
    ) -> Result<ExperimentalOutcome, ExperimentalError>;

    /// Called once the task graph has run, whether or not it succeeded.
    fn finish(&self) -> Result<(), ExperimentalError> {
        Ok(())
    }
}

/// An experimental node, as its experimental sees it.
#[derive(Debug, Clone)]
pub struct ExperimentalNodeRef {
    pub unique_id: String,
    pub name: String,
    /// The experimental's own data for the node.
    pub data: ExperimentalData,
    /// The nodes it depends on.
    pub depends_on: Vec<ExperimentalDependency>,
}

impl ExperimentalNodeRef {
    /// The experimental's data for the node, if it is a `T`.
    pub fn data<T: std::any::Any>(&self) -> Option<&T> {
        self.data.get()
    }

    fn new(node: &DbtExperimentalNode, nodes: &Nodes) -> Self {
        let depends_on = node
            .__base_attr__
            .depends_on
            .nodes
            .iter()
            .map(|unique_id| {
                let dependency = nodes.get_node(unique_id);
                ExperimentalDependency {
                    unique_id: unique_id.clone(),
                    relation_name: dependency.and_then(|node| node.base().relation_name.clone()),
                    adapter_type: dependency.map(|node| node.base().adapter),
                }
            })
            .collect();
        Self {
            unique_id: node.__common_attr__.unique_id.clone(),
            name: node.__common_attr__.name.clone(),
            data: node.__experimental_attr__.data.clone(),
            depends_on,
        }
    }
}

/// A node an experimental node depends on.
#[derive(Debug, Clone)]
pub struct ExperimentalDependency {
    pub unique_id: String,
    /// The relation it writes, if any.
    pub relation_name: Option<String>,
    adapter_type: Option<AdapterType>,
}

/// How an experimental node's run went. An error is the `Err` of [`ExperimentalRun::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExperimentalOutcome {
    /// The node passed.
    Pass,
    /// The node failed, as a test fails: `failures` things were wrong.
    Fail { failures: usize, message: String },
}

/// What an experimental node's run can reach.
pub struct ExperimentalRunCtx {
    experimental_type: &'static str,
    adapter_store: Arc<AdapterStore>,
}

impl ExperimentalRunCtx {
    /// Runs `sql` on the target's adapter and returns its rows.
    pub async fn query(&self, sql: &str) -> Result<Arc<RecordBatch>, ExperimentalError> {
        self.query_on(self.adapter_store.default_adapter_type(), sql)
            .await
    }

    /// Runs `sql` on the adapter `dependency` runs on and returns its rows.
    pub async fn query_for(
        &self,
        dependency: &ExperimentalDependency,
        sql: &str,
    ) -> Result<Arc<RecordBatch>, ExperimentalError> {
        let adapter_type = dependency
            .adapter_type
            .unwrap_or_else(|| self.adapter_store.default_adapter_type());
        self.query_on(adapter_type, sql).await
    }

    /// Runs blocking work (file or network IO) off the async pool. Not for the warehouse: use
    /// [`Self::query`].
    pub async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, ExperimentalError> {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|e| ExperimentalError::new(format!("blocking work did not finish: {e}")))
    }

    /// Reports a warning and carries on.
    pub fn warn(&self, warning: ExperimentalError) {
        emit_warn_log_from_fs_error(
            *warning.into_fs_error(self.experimental_type, ErrorCode::ExperimentalWarning),
        );
    }

    async fn query_on(
        &self,
        adapter_type: AdapterType,
        sql: &str,
    ) -> Result<Arc<RecordBatch>, ExperimentalError> {
        let adapter = self
            .adapter_store
            .get(adapter_type)
            .map_err(|e| ExperimentalError::new(e.to_string()))?;
        let sql = sql.to_string();
        // The warehouse is reached on the runtime pool, like any other node's queries.
        run_blocking_task_operation(move || {
            adapter
                .execute_without_state(None, &sql, true, None)
                .map(|(_, table)| table.to_record_batch())
        })
        .await
        .map_err(|e| ExperimentalError::new(e.to_string()))?
        .map_err(|e| ExperimentalError::new(e.to_string()))
    }
}

/// The task the core runs for one selected experimental node.
pub struct ExperimentalTask {
    experimental_type: &'static str,
    node: Arc<DbtExperimentalNode>,
    node_ref: ExperimentalNodeRef,
    run: Arc<dyn ExperimentalRun>,
}

impl ExperimentalTask {
    async fn run(&self, ctx: &TaskRunnerCtx) -> FsResult<NodeStatus> {
        let start = SystemTime::now();
        let unique_id = self.node_ref.unique_id.clone();
        let run_ctx = ExperimentalRunCtx {
            experimental_type: self.experimental_type,
            adapter_store: Arc::clone(ctx.adapter_store()),
        };
        let (status, failures, message) = match self.run.run(&self.node_ref, &run_ctx).await {
            Ok(ExperimentalOutcome::Pass) => (NodeStatus::Succeeded, Some(0), None),
            Ok(ExperimentalOutcome::Fail { failures, message }) => {
                // A failed experimental node counts as an error, as a failed test does.
                increment_metric(
                    FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalErrors),
                    1,
                );
                (NodeStatus::Errored, Some(failures.max(1)), Some(message))
            }
            Err(error) => {
                let message = error.message().to_string();
                emit_error_log_from_fs_error(
                    *error.into_fs_error(self.experimental_type, ErrorCode::ExperimentalError),
                );
                (NodeStatus::Errored, None, Some(message))
            }
        };
        ctx.inner.run_stats.upsert_sync(
            unique_id.clone(),
            Stat::new(
                unique_id,
                start,
                failures,
                status.clone(),
                message,
                ctx.thread_id,
            )
            .as_assertion(),
        );
        Ok(status)
    }
}

impl Task for ExperimentalTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(self.run(ctx))
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Experimental
    }

    fn task_type(&self) -> &str {
        self.experimental_type
    }

    fn work_node_id(&self) -> &str {
        &self.node_ref.unique_id
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        vec![self.node.clone()]
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Run)
    }
}

/// The experimentals an invocation runs with, and every key ever reserved under `experimental:`.
#[derive(Clone, Default)]
pub struct Experimentals {
    experimentals: Vec<Arc<dyn ExperimentalNode>>,
    reserved: Vec<&'static str>,
}

impl Experimentals {
    /// `reserved` lists every key ever released under `experimental:` (experimental types and
    /// definition keys), registered or not. A reserved key is never released: projects may use it
    /// whether or not its experimental still exists.
    ///
    /// # Panics
    ///
    /// If an experimental's type or one of its definition keys isn't reserved or is claimed by
    /// another experimental, or its flag is on by default or is a behavior flag. If a reserved key
    /// isn't a lowercase identifier, names something dbt itself uses, or is reserved twice.
    pub fn new(experimentals: Vec<Arc<dyn ExperimentalNode>>, reserved: &[&'static str]) -> Self {
        let mut reserved_types = BTreeSet::new();
        for experimental_type in reserved {
            assert!(
                is_identifier(experimental_type),
                "experimental type `{experimental_type}` is not a lowercase identifier"
            );
            assert!(
                !is_dbt_name(experimental_type),
                "experimental type `{experimental_type}` is a name dbt uses"
            );
            assert!(
                reserved_types.insert(*experimental_type),
                "experimental type `{experimental_type}` is reserved twice"
            );
        }
        let mut claimed = BTreeSet::new();
        for experimental in &experimentals {
            let experimental_type = experimental.experimental_type();
            check_flag(experimental_type, experimental.flag());
            for key in std::iter::once(experimental.key())
                .chain(experimental.definition_keys().iter().copied())
            {
                assert!(
                    reserved_types.contains(key),
                    "experimental `{experimental_type}` reads `{key}`, which isn't reserved"
                );
                assert!(
                    claimed.insert(key),
                    "`{key}` is read by more than one experimental, or twice by `{experimental_type}`"
                );
            }
        }
        Self {
            experimentals,
            reserved: reserved.to_vec(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn ExperimentalNode>> {
        self.experimentals.iter()
    }

    /// Every feature flag this binary knows: dbt's own, and the flags that turn on its
    /// experimentals, which aren't in `dbt_flags::ALL`.
    pub fn known_flags(&self) -> Vec<&'static FlagDef> {
        dbt_flags::ALL
            .iter()
            .copied()
            .chain(
                self.experimentals
                    .iter()
                    .map(|experimental| experimental.flag()),
            )
            .collect()
    }

    /// The experimental that owns `command`, if one does.
    pub fn command_owner(&self, command: FsCommand) -> Option<&Arc<dyn ExperimentalNode>> {
        let FsCommand::Extension(name) = command else {
            return None;
        };
        self.experimentals
            .iter()
            .find(|experimental| experimental.command() == Some(name))
    }

    /// The same experimentals, for resolving a project.
    pub fn resolve_experimentals(&self) -> ResolveExperimentals {
        ResolveExperimentals {
            experimentals: self
                .experimentals
                .iter()
                .map(|experimental| Arc::clone(experimental) as Arc<dyn ResolveExperimental>)
                .collect(),
            reserved: self.reserved.clone(),
        }
    }
}

fn check_flag(experimental_type: &str, flag: &FlagDef) {
    assert!(
        matches!(flag.stage, Stage::Internal | Stage::Preview),
        "experimental `{experimental_type}`'s flag `{}` must be an internal or preview flag",
        flag.name
    );
    assert!(
        !flag.default,
        "experimental `{experimental_type}`'s flag `{}` must be off by default",
        flag.name
    );
}

fn is_identifier(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// Whether dbt itself uses `name` as a resource type, a unique id prefix, or a value
/// `resource_type:` gives its own meaning.
fn is_dbt_name(name: &str) -> bool {
    matches!(name, "group" | "doc" | "relation")
        || NodeType::from_str_name(&format!("NODE_TYPE_{}", name.to_ascii_uppercase())).is_some()
}

/// One invocation's experimental runs, by experimental type.
#[derive(Clone, Default)]
pub struct ExperimentalRuns {
    started: Arc<StartedRuns>,
}

/// The runs that have started. Dropping them finishes any not yet finished, so an invocation that
/// stops early, or an experimental that can't start, still finishes the runs that did.
#[derive(Default)]
struct StartedRuns {
    runs: BTreeMap<&'static str, Arc<dyn ExperimentalRun>>,
    finished: AtomicBool,
}

impl StartedRuns {
    fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        for (experimental_type, run) in &self.runs {
            if let Err(error) = run.finish() {
                emit_error_log_from_fs_error(
                    *error.into_fs_error(experimental_type, ErrorCode::ExperimentalError),
                );
            }
        }
    }
}

impl Drop for StartedRuns {
    fn drop(&mut self) {
        self.finish();
    }
}

impl ExperimentalRuns {
    /// Starts every experimental with selected nodes, giving each its own, in schedule order.
    ///
    /// Experimental nodes only run, so a command whose graph has no Run phase starts no
    /// experimental. An experimental's own command runs only that experimental's nodes.
    pub fn start(
        experimentals: &Experimentals,
        arg: &RunTasksArgs,
        nodes: &Nodes,
        schedule: &Schedule<String>,
        runs_nodes: bool,
    ) -> FsResult<Self> {
        if !runs_nodes {
            return Ok(Self::default());
        }
        let mut started = StartedRuns::default();
        let command_owner = experimentals
            .command_owner(arg.command)
            .map(|experimental| experimental.experimental_type());
        let invocation_id = arg.io.invocation_id.to_string();
        for experimental in experimentals.iter() {
            let experimental_type = experimental.experimental_type();
            if command_owner.is_some_and(|owner| owner != experimental_type) {
                continue;
            }
            let selected: Vec<ExperimentalNodeRef> = schedule
                .sorted_nodes
                .iter()
                .filter(|unique_id| schedule.selected_nodes.contains(*unique_id))
                .filter_map(|unique_id| nodes.experimentals.get(unique_id))
                .filter(|node| node.__experimental_attr__.experimental_type == experimental_type)
                .map(|node| ExperimentalNodeRef::new(node, nodes))
                .collect();
            if selected.is_empty() {
                continue;
            }
            let args = ExperimentalRunArgs {
                command: arg.command,
                invocation_id: &invocation_id,
                project_dir: &arg.io.in_dir,
                target_dir: &arg.io.out_dir,
                selected: &selected,
            };
            match experimental.start(&args) {
                Ok(Some(run)) => {
                    started.runs.insert(experimental_type, run);
                }
                Ok(None) => {}
                Err(error) => {
                    return Err(
                        error.into_fs_error(experimental_type, ErrorCode::ExperimentalError)
                    );
                }
            }
        }
        Ok(Self {
            started: Arc::new(started),
        })
    }

    /// Whether the experimental that adds nodes of `experimental_type` is running.
    pub fn is_running(&self, experimental_type: &str) -> bool {
        self.started.runs.contains_key(experimental_type)
    }

    /// The task for an experimental node, if its experimental is running.
    pub fn task(&self, node: &Arc<DbtExperimentalNode>, nodes: &Nodes) -> Option<Arc<dyn Task>> {
        let (experimental_type, run) = self
            .started
            .runs
            .get_key_value(node.__experimental_attr__.experimental_type.as_str())?;
        Some(Arc::new(ExperimentalTask {
            experimental_type,
            node: Arc::clone(node),
            node_ref: ExperimentalNodeRef::new(node, nodes),
            run: Arc::clone(run),
        }))
    }

    /// Finishes every run, once, reporting any that fails to. Runs not finished by the time the
    /// last clone is dropped are finished then.
    pub fn finish(&self) {
        self.started.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_flags::Remote;
    use std::sync::atomic::AtomicUsize;

    static TEST_FLAG: FlagDef = flag(false, Stage::Preview);
    static ON_BY_DEFAULT: FlagDef = flag(true, Stage::Preview);
    static BEHAVIOR: FlagDef = flag(false, Stage::Behavior);

    const fn flag(default: bool, stage: Stage) -> FlagDef {
        FlagDef {
            name: "test_experimentals",
            description: "Experimentals for testing experimentals.",
            stage,
            default,
            remote: Remote::Never,
            aliases: &[],
            legacy_env: &[],
            tracking: None,
            remove_by: None,
            docs_url: None,
        }
    }

    const WIDGET: &str = "widget";
    const GADGET: &str = "gadget";
    const GIZMO: &str = "gizmo";

    /// An experimental of a type. `widget` also owns the `widgets` command.
    struct TestExperimental(&'static str, &'static FlagDef);

    impl ResolveExperimental for TestExperimental {
        fn experimental_type(&self) -> &'static str {
            self.0
        }
        fn flag(&self) -> &'static FlagDef {
            self.1
        }
        fn resolve(
            &self,
            _package: &ExperimentalPackage<'_>,
            _diagnostics: &mut ExperimentalDiagnostics,
        ) -> Result<Vec<ExperimentalNodeDef>, ExperimentalError> {
            Ok(vec![])
        }
    }

    /// A `widget` experimental that also reads `keys` under `experimental:`.
    struct WithKeys(&'static [&'static str]);

    impl ResolveExperimental for WithKeys {
        fn experimental_type(&self) -> &'static str {
            WIDGET
        }
        fn definition_keys(&self) -> &'static [&'static str] {
            self.0
        }
        fn flag(&self) -> &'static FlagDef {
            &TEST_FLAG
        }
        fn resolve(
            &self,
            _package: &ExperimentalPackage<'_>,
            _diagnostics: &mut ExperimentalDiagnostics,
        ) -> Result<Vec<ExperimentalNodeDef>, ExperimentalError> {
            Ok(vec![])
        }
    }

    impl ExperimentalNode for WithKeys {
        fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            Ok(None)
        }
    }

    impl ExperimentalNode for TestExperimental {
        fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            Ok(Some(Arc::new(TestRun)))
        }
        fn command(&self) -> Option<&'static str> {
            (self.0 == WIDGET).then_some("widgets")
        }
    }

    struct TestRun;

    #[async_trait]
    impl ExperimentalRun for TestRun {
        async fn run(
            &self,
            _node: &ExperimentalNodeRef,
            _ctx: &ExperimentalRunCtx,
        ) -> Result<ExperimentalOutcome, ExperimentalError> {
            Ok(ExperimentalOutcome::Pass)
        }
    }

    /// An experimental whose runs count how often they finish, or one that can't start.
    struct CountingExperimental {
        experimental_type: &'static str,
        starts: bool,
        finishes: Arc<AtomicUsize>,
    }

    impl ResolveExperimental for CountingExperimental {
        fn experimental_type(&self) -> &'static str {
            self.experimental_type
        }
        fn flag(&self) -> &'static FlagDef {
            &TEST_FLAG
        }
        fn resolve(
            &self,
            _package: &ExperimentalPackage<'_>,
            _diagnostics: &mut ExperimentalDiagnostics,
        ) -> Result<Vec<ExperimentalNodeDef>, ExperimentalError> {
            Ok(vec![])
        }
    }

    impl ExperimentalNode for CountingExperimental {
        fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            if !self.starts {
                return Err(ExperimentalError::new("can't start"));
            }
            Ok(Some(Arc::new(CountingRun(Arc::clone(&self.finishes)))))
        }
    }

    struct CountingRun(Arc<AtomicUsize>);

    #[async_trait]
    impl ExperimentalRun for CountingRun {
        async fn run(
            &self,
            _node: &ExperimentalNodeRef,
            _ctx: &ExperimentalRunCtx,
        ) -> Result<ExperimentalOutcome, ExperimentalError> {
            Ok(ExperimentalOutcome::Pass)
        }
        fn finish(&self) -> Result<(), ExperimentalError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Starts `widget` and then `gadget`, both with a selected node, and counts how often the
    /// `widget` run finishes.
    fn start_counting(gadget_starts: bool) -> (FsResult<ExperimentalRuns>, Arc<AtomicUsize>) {
        let finishes = Arc::new(AtomicUsize::new(0));
        let experimentals = Experimentals::new(
            vec![
                Arc::new(CountingExperimental {
                    experimental_type: WIDGET,
                    starts: true,
                    finishes: Arc::clone(&finishes),
                }),
                Arc::new(CountingExperimental {
                    experimental_type: GADGET,
                    starts: gadget_starts,
                    finishes: Arc::new(AtomicUsize::new(0)),
                }),
            ],
            &[WIDGET, GADGET],
        );
        let (nodes, schedule) = nodes_and_schedule();
        let arg = RunTasksArgs {
            command: FsCommand::Build,
            ..Default::default()
        };
        let runs = ExperimentalRuns::start(&experimentals, &arg, &nodes, &schedule, true);
        (runs, finishes)
    }

    #[test]
    fn a_experimental_that_cant_start_finishes_the_runs_that_did() {
        let (runs, finishes) = start_counting(false);
        assert!(runs.is_err());
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runs_finish_once() {
        let (runs, finishes) = start_counting(true);
        let runs = runs.unwrap();
        let clone = runs.clone();
        runs.finish();
        clone.finish();
        drop(runs);
        drop(clone);
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn runs_not_finished_finish_when_dropped() {
        let (runs, finishes) = start_counting(true);
        let runs = runs.unwrap();
        assert_eq!(finishes.load(Ordering::SeqCst), 0);
        drop(runs);
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
    }

    fn experimentals(experimental_types: &[&'static str]) -> Experimentals {
        Experimentals::new(
            experimental_types
                .iter()
                .map(|experimental_type| {
                    Arc::new(TestExperimental(experimental_type, &TEST_FLAG))
                        as Arc<dyn ExperimentalNode>
                })
                .collect(),
            &[WIDGET, GADGET, GIZMO],
        )
    }

    /// A selected `widget` node and a selected `gadget` node.
    fn nodes_and_schedule() -> (Nodes, Schedule<String>) {
        let mut nodes = Nodes::default();
        let mut schedule = Schedule::default();
        for experimental_type in ["widget", "gadget"] {
            let mut node = DbtExperimentalNode::default();
            node.__common_attr__.unique_id = format!("{experimental_type}.p.n");
            node.__experimental_attr__.experimental_type = experimental_type.to_string();
            let unique_id = node.__common_attr__.unique_id.clone();
            nodes
                .experimentals
                .insert(unique_id.clone(), Arc::new(node));
            schedule.sorted_nodes.push(unique_id.clone());
            schedule.selected_nodes.insert(unique_id);
        }
        (nodes, schedule)
    }

    /// The experimental types `command` starts, with only `selected` experimental types' nodes
    /// selected.
    fn started(command: FsCommand, runs_nodes: bool, selected: &[&str]) -> Vec<&'static str> {
        let experimentals = experimentals(&[WIDGET, GADGET, GIZMO]);
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule
            .selected_nodes
            .retain(|id| selected.iter().any(|kind| id.starts_with(kind)));
        let arg = RunTasksArgs {
            command,
            ..Default::default()
        };
        let runs =
            ExperimentalRuns::start(&experimentals, &arg, &nodes, &schedule, runs_nodes).unwrap();
        ["widget", "gadget", "gizmo"]
            .into_iter()
            .filter(|experimental_type| runs.is_running(experimental_type))
            .collect()
    }

    #[test]
    fn only_experimentals_with_selected_nodes_start() {
        assert_eq!(
            started(FsCommand::Build, true, &["widget", "gadget"]),
            ["widget", "gadget"]
        );
        assert_eq!(started(FsCommand::Build, true, &["gadget"]), ["gadget"]);
    }

    #[test]
    fn graphs_that_run_no_nodes_start_no_experimental() {
        assert!(started(FsCommand::Compile, false, &["widget", "gadget"]).is_empty());
    }

    #[test]
    fn a_experimentals_own_command_starts_only_that_experimental() {
        assert_eq!(
            started(FsCommand::Extension("widgets"), true, &["widget", "gadget"]),
            ["widget"]
        );
    }

    #[test]
    fn running_experimentals_hand_out_tasks_for_their_nodes() {
        let experimentals = experimentals(&[WIDGET, GADGET]);
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule
            .selected_nodes
            .retain(|id| id.starts_with("widget"));
        let arg = RunTasksArgs {
            command: FsCommand::Build,
            ..Default::default()
        };
        let runs = ExperimentalRuns::start(&experimentals, &arg, &nodes, &schedule, true).unwrap();
        assert!(
            runs.task(&nodes.experimentals["widget.p.n"], &nodes)
                .is_some()
        );
        assert!(
            runs.task(&nodes.experimentals["gadget.p.n"], &nodes)
                .is_none()
        );
        runs.finish();
    }

    #[test]
    fn experimentals_are_found_by_command() {
        let experimentals = experimentals(&[WIDGET, GADGET]);
        assert_eq!(
            experimentals
                .command_owner(FsCommand::Extension("widgets"))
                .map(|experimental| experimental.experimental_type()),
            Some(WIDGET)
        );
        let resolve = experimentals.resolve_experimentals();
        assert_eq!(resolve.experimentals.len(), 2);
        assert_eq!(resolve.reserved, [WIDGET, GADGET, GIZMO]);
    }

    #[test]
    #[should_panic(expected = "experimental `widget` reads `widget`, which isn't reserved")]
    fn a_experimental_needs_a_reserved_type() {
        Experimentals::new(
            vec![Arc::new(TestExperimental(WIDGET, &TEST_FLAG))],
            &[GADGET],
        );
    }

    #[test]
    fn a_experimental_reads_its_reserved_definition_keys() {
        Experimentals::new(vec![Arc::new(WithKeys(&[GIZMO]))], &[WIDGET, GIZMO]);
    }

    #[test]
    #[should_panic(expected = "experimental `widget` reads `gizmo`, which isn't reserved")]
    fn a_experimentals_definition_keys_are_reserved() {
        Experimentals::new(vec![Arc::new(WithKeys(&[GIZMO]))], &[WIDGET]);
    }

    #[test]
    #[should_panic(expected = "`gadget` is read by more than one experimental")]
    fn a_key_is_read_by_one_experimental() {
        Experimentals::new(
            vec![
                Arc::new(WithKeys(&[GADGET])),
                Arc::new(TestExperimental(GADGET, &TEST_FLAG)),
            ],
            &[WIDGET, GADGET],
        );
    }

    #[test]
    #[should_panic(expected = "must be off by default")]
    fn a_experimentals_flag_is_off_by_default() {
        Experimentals::new(
            vec![Arc::new(TestExperimental(WIDGET, &ON_BY_DEFAULT))],
            &[WIDGET],
        );
    }

    /// An experimental's flag is a known feature flag, so `--feature` turns it on without an
    /// "Unknown feature flag" warning.
    #[test]
    fn experimental_flags_are_known() {
        let snapshot = dbt_flags::FlagSnapshot::builder()
            .cli(["test_experimentals"])
            .build();
        let without = Experimentals::new(vec![], &[WIDGET]);
        assert_eq!(
            snapshot.warnings(&without.known_flags()),
            ["Unknown feature flag 'test_experimentals' in --feature; it has no effect"]
        );
        let with = Experimentals::new(
            vec![Arc::new(TestExperimental(WIDGET, &TEST_FLAG))],
            &[WIDGET],
        );
        assert!(snapshot.warnings(&with.known_flags()).is_empty());
    }

    #[test]
    #[should_panic(expected = "must be an internal or preview flag")]
    fn a_experimentals_flag_is_not_a_behavior_flag() {
        Experimentals::new(
            vec![Arc::new(TestExperimental(WIDGET, &BEHAVIOR))],
            &[WIDGET],
        );
    }

    #[test]
    fn a_experimental_type_cannot_be_a_name_dbt_uses() {
        for name in [
            "unit_test",
            "model",
            "experimental",
            "group",
            "doc",
            "relation",
        ] {
            let result = std::panic::catch_unwind(|| Experimentals::new(vec![], &[name]));
            assert!(result.is_err(), "{name} should be rejected");
        }
    }

    #[test]
    #[should_panic(expected = "experimental type `widget` is reserved twice")]
    fn a_experimental_type_is_reserved_once() {
        Experimentals::new(vec![], &[WIDGET, GADGET, WIDGET]);
    }
}
