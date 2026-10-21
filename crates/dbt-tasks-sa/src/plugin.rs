//! Plugins: their own kind of node, resolved by the core and run in the task graph.
//!
//! A plugin never implements [`Task`]. The core wraps each selected plugin node in a
//! [`PluginTask`], which calls the plugin's [`PluginRun::run`] on the async pool and turns its
//! outcome into the node's result. Plugins see only their own nodes, their own data and their
//! nodes' dependencies, and reach the warehouse only through [`PluginRunCtx`].

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
use dbt_schemas::schemas::{DbtPluginNode, InternalDbtNodeAttributes, Nodes, PluginData};
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::{TP, Task, run_blocking_task_operation};
use dbt_telemetry::NodeType;

pub use dbt_parser::plugin::{
    PluginDiagnostics, PluginEntry, PluginEntryData, PluginError, PluginFile, PluginNodeDef,
    ResolvePlugin, ResolvePlugins,
};

/// Adds its own kind of node to a project ([`ResolvePlugin`]) and runs the ones selected.
///
/// A plugin node runs only when it is selected directly, and only in its Run phase: it has
/// nothing to render or analyze.
pub trait NodePlugin: ResolvePlugin {
    /// Prepares to run this plugin's selected nodes. Called once per invocation, after nodes are
    /// selected and before the task graph is built, so an error stops the invocation before any
    /// node runs. `None` runs none of them.
    fn start(&self, args: &PluginRunArgs<'_>) -> Result<Option<Arc<dyn PluginRun>>, PluginError>;

    /// The extension command whose task graph runs only this plugin's nodes, if the plugin adds
    /// one.
    fn command(&self) -> Option<&'static str> {
        None
    }
}

/// What a plugin's run starts from.
pub struct PluginRunArgs<'a> {
    pub command: FsCommand,
    pub invocation_id: &'a str,
    pub project_dir: &'a Path,
    /// Where the invocation writes its artifacts.
    pub target_dir: &'a Path,
    /// This plugin's selected nodes, in schedule order.
    pub selected: &'a [PluginNodeRef],
}

/// One invocation's run of a plugin's selected nodes.
#[async_trait]
pub trait PluginRun: Send + Sync {
    /// Runs one of the selected nodes, on the async pool.
    async fn run(
        &self,
        node: &PluginNodeRef,
        ctx: &PluginRunCtx,
    ) -> Result<PluginOutcome, PluginError>;

    /// Called once the task graph has run, whether or not it succeeded.
    fn finish(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

/// A plugin node, as its plugin sees it.
#[derive(Debug, Clone)]
pub struct PluginNodeRef {
    pub unique_id: String,
    pub name: String,
    /// The plugin's own data for the node.
    pub data: PluginData,
    /// The nodes it depends on.
    pub depends_on: Vec<PluginDependency>,
}

impl PluginNodeRef {
    /// The plugin's data for the node, if it is a `T`.
    pub fn data<T: std::any::Any>(&self) -> Option<&T> {
        self.data.get()
    }

    fn new(node: &DbtPluginNode, nodes: &Nodes) -> Self {
        let depends_on = node
            .__base_attr__
            .depends_on
            .nodes
            .iter()
            .map(|unique_id| {
                let dependency = nodes.get_node(unique_id);
                PluginDependency {
                    unique_id: unique_id.clone(),
                    relation_name: dependency.and_then(|node| node.base().relation_name.clone()),
                    adapter_type: dependency.map(|node| node.base().adapter),
                }
            })
            .collect();
        Self {
            unique_id: node.__common_attr__.unique_id.clone(),
            name: node.__common_attr__.name.clone(),
            data: node.__plugin_attr__.data.clone(),
            depends_on,
        }
    }
}

/// A node a plugin node depends on.
#[derive(Debug, Clone)]
pub struct PluginDependency {
    pub unique_id: String,
    /// The relation it writes, if any.
    pub relation_name: Option<String>,
    adapter_type: Option<AdapterType>,
}

/// How a plugin node's run went. An error is the `Err` of [`PluginRun::run`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOutcome {
    /// The node passed.
    Pass,
    /// The node failed, as a test fails: `failures` things were wrong.
    Fail { failures: usize, message: String },
}

/// What a plugin node's run can reach.
pub struct PluginRunCtx {
    plugin_type: &'static str,
    adapter_store: Arc<AdapterStore>,
}

impl PluginRunCtx {
    /// Runs `sql` on the target's adapter and returns its rows.
    pub async fn query(&self, sql: &str) -> Result<Arc<RecordBatch>, PluginError> {
        self.query_on(self.adapter_store.default_adapter_type(), sql)
            .await
    }

    /// Runs `sql` on the adapter `dependency` runs on and returns its rows.
    pub async fn query_for(
        &self,
        dependency: &PluginDependency,
        sql: &str,
    ) -> Result<Arc<RecordBatch>, PluginError> {
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
    ) -> Result<T, PluginError> {
        tokio::task::spawn_blocking(work)
            .await
            .map_err(|e| PluginError::new(format!("blocking work did not finish: {e}")))
    }

    /// Reports a warning and carries on.
    pub fn warn(&self, warning: PluginError) {
        emit_warn_log_from_fs_error(
            *warning.into_fs_error(self.plugin_type, ErrorCode::PluginWarning),
        );
    }

    async fn query_on(
        &self,
        adapter_type: AdapterType,
        sql: &str,
    ) -> Result<Arc<RecordBatch>, PluginError> {
        let adapter = self
            .adapter_store
            .get(adapter_type)
            .map_err(|e| PluginError::new(e.to_string()))?;
        let sql = sql.to_string();
        // The warehouse is reached on the runtime pool, like any other node's queries.
        run_blocking_task_operation(move || {
            adapter
                .execute_without_state(None, &sql, true, None)
                .map(|(_, table)| table.to_record_batch())
        })
        .await
        .map_err(|e| PluginError::new(e.to_string()))?
        .map_err(|e| PluginError::new(e.to_string()))
    }
}

/// The task the core runs for one selected plugin node.
pub struct PluginTask {
    plugin_type: &'static str,
    node: Arc<DbtPluginNode>,
    node_ref: PluginNodeRef,
    run: Arc<dyn PluginRun>,
}

impl PluginTask {
    async fn run(&self, ctx: &TaskRunnerCtx) -> FsResult<NodeStatus> {
        let start = SystemTime::now();
        let unique_id = self.node_ref.unique_id.clone();
        let run_ctx = PluginRunCtx {
            plugin_type: self.plugin_type,
            adapter_store: Arc::clone(ctx.adapter_store()),
        };
        let (status, failures, message) = match self.run.run(&self.node_ref, &run_ctx).await {
            Ok(PluginOutcome::Pass) => (NodeStatus::Succeeded, Some(0), None),
            Ok(PluginOutcome::Fail { failures, message }) => {
                // A failed plugin node counts as an error, as a failed test does.
                increment_metric(
                    FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalErrors),
                    1,
                );
                (NodeStatus::Errored, Some(failures.max(1)), Some(message))
            }
            Err(error) => {
                let message = error.message().to_string();
                emit_error_log_from_fs_error(
                    *error.into_fs_error(self.plugin_type, ErrorCode::PluginError),
                );
                (NodeStatus::Errored, None, Some(message))
            }
        };
        ctx.inner.run_stats.insert(
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

impl Task for PluginTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(self.run(ctx))
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Plugin
    }

    fn task_type(&self) -> &str {
        self.plugin_type
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

/// The plugins an invocation runs with, and every plugin type ever reserved.
#[derive(Clone, Default)]
pub struct Plugins {
    plugins: Vec<Arc<dyn NodePlugin>>,
    reserved: Vec<&'static str>,
}

impl Plugins {
    /// `reserved` lists every plugin type ever released, registered or not. A reserved plugin type
    /// is never released: projects may use it whether or not its plugin still exists.
    ///
    /// # Panics
    ///
    /// If a plugin's type isn't reserved, or its flag is on by default or is a behavior flag. If a
    /// reserved plugin type isn't a lowercase identifier, names something dbt itself uses, or is
    /// reserved twice.
    pub fn new(plugins: Vec<Arc<dyn NodePlugin>>, reserved: &[&'static str]) -> Self {
        let mut reserved_types = BTreeSet::new();
        for plugin_type in reserved {
            assert!(
                is_identifier(plugin_type),
                "plugin type `{plugin_type}` is not a lowercase identifier"
            );
            assert!(
                !is_dbt_name(plugin_type),
                "plugin type `{plugin_type}` is a name dbt uses"
            );
            assert!(
                reserved_types.insert(*plugin_type),
                "plugin type `{plugin_type}` is reserved twice"
            );
        }
        let mut registered = BTreeSet::new();
        for plugin in &plugins {
            let plugin_type = plugin.plugin_type();
            assert!(
                reserved_types.contains(plugin_type),
                "plugin type `{plugin_type}` isn't reserved"
            );
            check_flag(plugin_type, plugin.flag());
            assert!(
                registered.insert(plugin_type),
                "two plugins have the plugin type `{plugin_type}`"
            );
        }
        Self {
            plugins,
            reserved: reserved.to_vec(),
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Arc<dyn NodePlugin>> {
        self.plugins.iter()
    }

    /// The plugin that owns `command`, if one does.
    pub fn command_owner(&self, command: FsCommand) -> Option<&Arc<dyn NodePlugin>> {
        let FsCommand::Extension(name) = command else {
            return None;
        };
        self.plugins
            .iter()
            .find(|plugin| plugin.command() == Some(name))
    }

    /// The same plugins, for resolving a project.
    pub fn resolve_plugins(&self) -> ResolvePlugins {
        ResolvePlugins {
            plugins: self
                .plugins
                .iter()
                .map(|plugin| Arc::clone(plugin) as Arc<dyn ResolvePlugin>)
                .collect(),
            reserved: self.reserved.clone(),
        }
    }
}

fn check_flag(plugin_type: &str, flag: &FlagDef) {
    assert!(
        matches!(flag.stage, Stage::Internal | Stage::Preview),
        "plugin `{plugin_type}`'s flag `{}` must be an internal or preview flag",
        flag.name
    );
    assert!(
        !flag.default,
        "plugin `{plugin_type}`'s flag `{}` must be off by default",
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

/// One invocation's plugin runs, by plugin type.
#[derive(Clone, Default)]
pub struct PluginRuns {
    started: Arc<StartedRuns>,
}

/// The runs that have started. Dropping them finishes any not yet finished, so an invocation that
/// stops early, or a plugin that can't start, still finishes the runs that did.
#[derive(Default)]
struct StartedRuns {
    runs: BTreeMap<&'static str, Arc<dyn PluginRun>>,
    finished: AtomicBool,
}

impl StartedRuns {
    fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        for (plugin_type, run) in &self.runs {
            if let Err(error) = run.finish() {
                emit_error_log_from_fs_error(
                    *error.into_fs_error(plugin_type, ErrorCode::PluginError),
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

impl PluginRuns {
    /// Starts every plugin with selected nodes, giving each its own, in schedule order.
    ///
    /// Plugin nodes only run, so a command whose graph has no Run phase starts no plugin. A
    /// plugin's own command runs only that plugin's nodes.
    pub fn start(
        plugins: &Plugins,
        arg: &RunTasksArgs,
        nodes: &Nodes,
        schedule: &Schedule<String>,
        runs_nodes: bool,
    ) -> FsResult<Self> {
        if !runs_nodes {
            return Ok(Self::default());
        }
        let mut started = StartedRuns::default();
        let command_owner = plugins
            .command_owner(arg.command)
            .map(|plugin| plugin.plugin_type());
        let invocation_id = arg.io.invocation_id.to_string();
        for plugin in plugins.iter() {
            let plugin_type = plugin.plugin_type();
            if command_owner.is_some_and(|owner| owner != plugin_type) {
                continue;
            }
            let selected: Vec<PluginNodeRef> = schedule
                .sorted_nodes
                .iter()
                .filter(|unique_id| schedule.selected_nodes.contains(*unique_id))
                .filter_map(|unique_id| nodes.plugins.get(unique_id))
                .filter(|node| node.__plugin_attr__.plugin_type == plugin_type)
                .map(|node| PluginNodeRef::new(node, nodes))
                .collect();
            if selected.is_empty() {
                continue;
            }
            let args = PluginRunArgs {
                command: arg.command,
                invocation_id: &invocation_id,
                project_dir: &arg.io.in_dir,
                target_dir: &arg.io.out_dir,
                selected: &selected,
            };
            match plugin.start(&args) {
                Ok(Some(run)) => {
                    started.runs.insert(plugin_type, run);
                }
                Ok(None) => {}
                Err(error) => return Err(error.into_fs_error(plugin_type, ErrorCode::PluginError)),
            }
        }
        Ok(Self {
            started: Arc::new(started),
        })
    }

    /// Whether the plugin that adds nodes of `plugin_type` is running.
    pub fn is_running(&self, plugin_type: &str) -> bool {
        self.started.runs.contains_key(plugin_type)
    }

    /// The task for a plugin node, if its plugin is running.
    pub fn task(&self, node: &Arc<DbtPluginNode>, nodes: &Nodes) -> Option<Arc<dyn Task>> {
        let (plugin_type, run) = self
            .started
            .runs
            .get_key_value(node.__plugin_attr__.plugin_type.as_str())?;
        Some(Arc::new(PluginTask {
            plugin_type,
            node: Arc::clone(node),
            node_ref: PluginNodeRef::new(node, nodes),
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
            name: "test_plugins",
            description: "Plugins for testing plugins.",
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

    /// A plugin of a type. `widget` also owns the `widgets` command.
    struct TestPlugin(&'static str, &'static FlagDef);

    impl ResolvePlugin for TestPlugin {
        fn plugin_type(&self) -> &'static str {
            self.0
        }
        fn flag(&self) -> &'static FlagDef {
            self.1
        }
    }

    impl NodePlugin for TestPlugin {
        fn start(
            &self,
            _args: &PluginRunArgs<'_>,
        ) -> Result<Option<Arc<dyn PluginRun>>, PluginError> {
            Ok(Some(Arc::new(TestRun)))
        }
        fn command(&self) -> Option<&'static str> {
            (self.0 == WIDGET).then_some("widgets")
        }
    }

    struct TestRun;

    #[async_trait]
    impl PluginRun for TestRun {
        async fn run(
            &self,
            _node: &PluginNodeRef,
            _ctx: &PluginRunCtx,
        ) -> Result<PluginOutcome, PluginError> {
            Ok(PluginOutcome::Pass)
        }
    }

    /// A plugin whose runs count how often they finish, or one that can't start.
    struct CountingPlugin {
        plugin_type: &'static str,
        starts: bool,
        finishes: Arc<AtomicUsize>,
    }

    impl ResolvePlugin for CountingPlugin {
        fn plugin_type(&self) -> &'static str {
            self.plugin_type
        }
        fn flag(&self) -> &'static FlagDef {
            &TEST_FLAG
        }
    }

    impl NodePlugin for CountingPlugin {
        fn start(
            &self,
            _args: &PluginRunArgs<'_>,
        ) -> Result<Option<Arc<dyn PluginRun>>, PluginError> {
            if !self.starts {
                return Err(PluginError::new("can't start"));
            }
            Ok(Some(Arc::new(CountingRun(Arc::clone(&self.finishes)))))
        }
    }

    struct CountingRun(Arc<AtomicUsize>);

    #[async_trait]
    impl PluginRun for CountingRun {
        async fn run(
            &self,
            _node: &PluginNodeRef,
            _ctx: &PluginRunCtx,
        ) -> Result<PluginOutcome, PluginError> {
            Ok(PluginOutcome::Pass)
        }
        fn finish(&self) -> Result<(), PluginError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// Starts `widget` and then `gadget`, both with a selected node, and counts how often the
    /// `widget` run finishes.
    fn start_counting(gadget_starts: bool) -> (FsResult<PluginRuns>, Arc<AtomicUsize>) {
        let finishes = Arc::new(AtomicUsize::new(0));
        let plugins = Plugins::new(
            vec![
                Arc::new(CountingPlugin {
                    plugin_type: WIDGET,
                    starts: true,
                    finishes: Arc::clone(&finishes),
                }),
                Arc::new(CountingPlugin {
                    plugin_type: GADGET,
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
        let runs = PluginRuns::start(&plugins, &arg, &nodes, &schedule, true);
        (runs, finishes)
    }

    #[test]
    fn a_plugin_that_cant_start_finishes_the_runs_that_did() {
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

    fn plugins(plugin_types: &[&'static str]) -> Plugins {
        Plugins::new(
            plugin_types
                .iter()
                .map(|plugin_type| {
                    Arc::new(TestPlugin(plugin_type, &TEST_FLAG)) as Arc<dyn NodePlugin>
                })
                .collect(),
            &[WIDGET, GADGET, GIZMO],
        )
    }

    /// A selected `widget` node and a selected `gadget` node.
    fn nodes_and_schedule() -> (Nodes, Schedule<String>) {
        let mut nodes = Nodes::default();
        let mut schedule = Schedule::default();
        for plugin_type in ["widget", "gadget"] {
            let mut node = DbtPluginNode::default();
            node.__common_attr__.unique_id = format!("{plugin_type}.p.n");
            node.__plugin_attr__.plugin_type = plugin_type.to_string();
            let unique_id = node.__common_attr__.unique_id.clone();
            nodes.plugins.insert(unique_id.clone(), Arc::new(node));
            schedule.sorted_nodes.push(unique_id.clone());
            schedule.selected_nodes.insert(unique_id);
        }
        (nodes, schedule)
    }

    /// The plugin types `command` starts, with only `selected` plugin types' nodes selected.
    fn started(command: FsCommand, runs_nodes: bool, selected: &[&str]) -> Vec<&'static str> {
        let plugins = plugins(&[WIDGET, GADGET, GIZMO]);
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule
            .selected_nodes
            .retain(|id| selected.iter().any(|kind| id.starts_with(kind)));
        let arg = RunTasksArgs {
            command,
            ..Default::default()
        };
        let runs = PluginRuns::start(&plugins, &arg, &nodes, &schedule, runs_nodes).unwrap();
        ["widget", "gadget", "gizmo"]
            .into_iter()
            .filter(|plugin_type| runs.is_running(plugin_type))
            .collect()
    }

    #[test]
    fn only_plugins_with_selected_nodes_start() {
        assert_eq!(
            started(FsCommand::Build, true, &["widget", "gadget"]),
            ["widget", "gadget"]
        );
        assert_eq!(started(FsCommand::Build, true, &["gadget"]), ["gadget"]);
    }

    #[test]
    fn graphs_that_run_no_nodes_start_no_plugin() {
        assert!(started(FsCommand::Compile, false, &["widget", "gadget"]).is_empty());
    }

    #[test]
    fn a_plugins_own_command_starts_only_that_plugin() {
        assert_eq!(
            started(FsCommand::Extension("widgets"), true, &["widget", "gadget"]),
            ["widget"]
        );
    }

    #[test]
    fn running_plugins_hand_out_tasks_for_their_nodes() {
        let plugins = plugins(&[WIDGET, GADGET]);
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule
            .selected_nodes
            .retain(|id| id.starts_with("widget"));
        let arg = RunTasksArgs {
            command: FsCommand::Build,
            ..Default::default()
        };
        let runs = PluginRuns::start(&plugins, &arg, &nodes, &schedule, true).unwrap();
        assert!(runs.task(&nodes.plugins["widget.p.n"], &nodes).is_some());
        assert!(runs.task(&nodes.plugins["gadget.p.n"], &nodes).is_none());
        runs.finish();
    }

    #[test]
    fn plugins_are_found_by_command() {
        let plugins = plugins(&[WIDGET, GADGET]);
        assert_eq!(
            plugins
                .command_owner(FsCommand::Extension("widgets"))
                .map(|plugin| plugin.plugin_type()),
            Some(WIDGET)
        );
        let resolve = plugins.resolve_plugins();
        assert_eq!(resolve.plugins.len(), 2);
        assert_eq!(resolve.reserved, [WIDGET, GADGET, GIZMO]);
    }

    #[test]
    #[should_panic(expected = "plugin type `widget` isn't reserved")]
    fn a_plugin_needs_a_reserved_type() {
        Plugins::new(vec![Arc::new(TestPlugin(WIDGET, &TEST_FLAG))], &[GADGET]);
    }

    #[test]
    #[should_panic(expected = "must be off by default")]
    fn a_plugins_flag_is_off_by_default() {
        Plugins::new(
            vec![Arc::new(TestPlugin(WIDGET, &ON_BY_DEFAULT))],
            &[WIDGET],
        );
    }

    #[test]
    #[should_panic(expected = "must be an internal or preview flag")]
    fn a_plugins_flag_is_not_a_behavior_flag() {
        Plugins::new(vec![Arc::new(TestPlugin(WIDGET, &BEHAVIOR))], &[WIDGET]);
    }

    #[test]
    fn a_plugin_type_cannot_be_a_name_dbt_uses() {
        for name in ["unit_test", "model", "plugin", "group", "doc", "relation"] {
            let result = std::panic::catch_unwind(|| Plugins::new(vec![], &[name]));
            assert!(result.is_err(), "{name} should be rejected");
        }
    }

    #[test]
    #[should_panic(expected = "plugin type `widget` is reserved twice")]
    fn a_plugin_type_is_reserved_once() {
        Plugins::new(vec![], &[WIDGET, GADGET, WIDGET]);
    }
}
