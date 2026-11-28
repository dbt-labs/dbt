//! Injected tasks: the tasks of the nodes of a kind that a feature adds.
//!
//! A feature that adds a kind of node gives the task graph the tasks of those nodes by
//! implementing [`InjectedTasks`]. The tasks are ordinary [`Task`]s. The graph builder asks
//! [`InjectedRuns`] for the tasks of an injected node, as it asks the
//! [`TasksForNodeFactory`](crate::task::TasksForNodeFactory) for the tasks of one of dbt's own.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use dbt_common::FsResult;
use dbt_common::io_args::FsCommand;
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_dag::schedule::Schedule;
use dbt_schemas::schemas::{InjectedNode, Nodes};
use dbt_tasks_core::RunTasksArgs;
use petgraph::Graph;

// What a feature writes a task with, beside what this module defines.
pub use dbt_tasks_core::context::TaskRunnerCtx;
pub use dbt_tasks_core::task::{TP, Task, TasksForNode};
pub use dbt_telemetry::NodeType;

use crate::graph::command_phases;

/// What a feature gives the task graph for the nodes of the kinds it adds.
#[async_trait]
pub trait InjectedTasks: Send + Sync {
    /// The kinds of node it makes tasks for (`injected_kind()`).
    fn kinds(&self) -> &[&'static str];

    /// The phases it has tasks in. It is not started for a command whose graph has none of
    /// them.
    fn phases(&self) -> &'static [TP] {
        &[TP::Run]
    }

    /// The command that selects only its nodes, if it adds one, with the phases of its graph.
    fn command(&self) -> Option<InjectedCommand> {
        None
    }

    /// Runs once per invocation, after nodes are selected and before the graph is built, when
    /// a node of its kinds is selected and the command's graph has a phase it has tasks in.
    /// An error stops the invocation before any task runs. `None` gives its nodes no tasks.
    async fn start(&self, cx: &StartCtx<'_>) -> FsResult<Option<Arc<dyn InjectedRun>>>;
}

/// One invocation's tasks for a feature's nodes, and what those tasks share.
pub trait InjectedRun: Send + Sync {
    /// The tasks of `node`: any of render, analyze, run and show.
    ///
    /// A task's [`Task::task_phase`] is the phase of its slot. A node has one task of a
    /// [`Task::task_type`], so a node's tasks in two phases have two types. dbt's own tasks
    /// name the type for the phase (`render`, `analyze`, `run`, `show`), and the graph reads
    /// `run` in a type as a task of the run phase.
    fn tasks_for_node(&self, node: &Arc<dyn InjectedNode>, cx: &TasksCtx<'_>) -> TasksForNode;

    /// Runs once when the invocation ends, also after an error or a cancel.
    fn finish(&self) -> FsResult<()> {
        Ok(())
    }
}

/// An extension command that a feature adds. Its task graph has tasks only for the nodes of
/// the feature's kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectedCommand {
    /// The name of the extension command.
    pub name: &'static str,
    /// The phases of its task graph.
    pub phases: &'static [TP],
}

impl InjectedCommand {
    /// The command `name`, whose graph has only the run phase.
    pub const fn runs(name: &'static str) -> Self {
        Self {
            name,
            phases: &[TP::Run],
        }
    }
}

/// What a feature's run starts from.
pub struct StartCtx<'a> {
    pub command: FsCommand,
    pub invocation_id: &'a str,
    pub project_dir: &'a Path,
    /// Where the invocation writes its artifacts.
    pub target_dir: &'a Path,
    /// The phases of the command's task graph.
    pub phases: &'a [TP],
    /// The command's own options, by name. Empty for a command that has none.
    pub command_options: &'a BTreeMap<String, Vec<String>>,
    /// The selected nodes of the feature's kinds, in schedule order.
    pub selected: &'a [Arc<dyn InjectedNode>],
    pub nodes: &'a Nodes,
}

impl StartCtx<'_> {
    /// The values of the command's own option `name`, if it was given.
    pub fn command_option(&self, name: &str) -> Option<&[String]> {
        self.command_options.get(name).map(Vec::as_slice)
    }

    /// The selected nodes that are a `T`: the node type of the feature that added them.
    pub fn selected<T: Any>(&self) -> impl Iterator<Item = &T> {
        self.selected
            .iter()
            .filter_map(|node| node.as_any().downcast_ref())
    }
}

/// What the tasks of one node are made from.
pub struct TasksCtx<'a> {
    /// The phases of the task graph.
    pub phases: &'a [TP],
    pub nodes: &'a Nodes,
    /// Whether the node is selected. A node that is only upstream of a selected node is not.
    pub selected: bool,
}

impl TasksCtx<'_> {
    /// `task` in the run phase, if the node is selected and the graph has a run phase. No
    /// task in any other phase.
    pub fn run_task_if_selected<T: Task + 'static>(
        &self,
        task: impl FnOnce() -> T,
    ) -> TasksForNode {
        let runs = self.selected && self.phases.contains(&TP::Run);
        TasksForNode {
            renderable: None,
            analyzeable: None,
            runnable: runs.then(|| Arc::new(task()) as Arc<dyn Task>),
            showable: None,
        }
    }
}

/// How many injected nodes `graph` has a task for, by kind. A node with tasks in two phases
/// is one node.
pub fn injected_nodes_with_tasks(graph: &Graph<Arc<dyn Task>, ()>) -> BTreeMap<String, u64> {
    let nodes: BTreeSet<(String, String)> = graph
        .node_weights()
        .flat_map(|task| task.dbt_nodes())
        .filter_map(|node| {
            let kind = node.injected_kind()?.to_string();
            Some((kind, node.common().unique_id.clone()))
        })
        .collect();
    let mut by_kind = BTreeMap::new();
    for (kind, _) in nodes {
        *by_kind.entry(kind).or_default() += 1;
    }
    by_kind
}

/// One invocation's runs of the features that have selected nodes, by kind of node.
#[derive(Clone, Default)]
pub struct InjectedRuns {
    started: Arc<StartedRuns>,
}

/// The command of the invocation, when a feature added it.
struct OwnCommand {
    /// The kinds of the feature that added it. Only their nodes get tasks.
    kinds: Vec<&'static str>,
    phases: &'static [TP],
}

/// The runs that have started. Dropping them finishes any not yet finished, so an invocation
/// that stops early, or a feature that can't start, still finishes the runs that did.
#[derive(Default)]
struct StartedRuns {
    /// In the order they started.
    runs: Vec<Arc<dyn InjectedRun>>,
    by_kind: BTreeMap<&'static str, Arc<dyn InjectedRun>>,
    own_command: Option<OwnCommand>,
    finished: AtomicBool,
}

impl StartedRuns {
    fn finish(&self) {
        if self.finished.swap(true, Ordering::AcqRel) {
            return;
        }
        for run in &self.runs {
            if let Err(error) = run.finish() {
                emit_error_log_from_fs_error(*error);
            }
        }
    }
}

impl Drop for StartedRuns {
    fn drop(&mut self) {
        self.finish();
    }
}

impl InjectedRuns {
    /// Starts every feature in `injected` that has selected nodes and a phase in the
    /// command's graph, giving each its own nodes, in schedule order.
    ///
    /// A command that a feature adds starts only that feature.
    pub async fn start(
        injected: &[Arc<dyn InjectedTasks>],
        arg: &RunTasksArgs,
        nodes: &Nodes,
        schedule: &Schedule<String>,
    ) -> FsResult<Self> {
        let mut started = StartedRuns::default();
        let owner = injected.iter().position(|tasks| {
            tasks
                .command()
                .is_some_and(|command| arg.command == FsCommand::Extension(command.name))
        });
        let Some(phases) = command_phases(arg.command, injected) else {
            return Ok(Self::default());
        };
        if let Some(owner) = owner {
            started.own_command = Some(OwnCommand {
                kinds: injected[owner].kinds().to_vec(),
                phases,
            });
        }
        let invocation_id = arg.io.invocation_id.to_string();
        for (index, tasks) in injected.iter().enumerate() {
            if owner.is_some_and(|owner| owner != index) {
                continue;
            }
            if !tasks.phases().iter().any(|phase| phases.contains(phase)) {
                continue;
            }
            let kinds = tasks.kinds();
            let selected: Vec<Arc<dyn InjectedNode>> = schedule
                .sorted_nodes
                .iter()
                .filter(|unique_id| schedule.selected_nodes.contains(*unique_id))
                .filter_map(|unique_id| nodes.injected.get(unique_id))
                .filter(|node| {
                    node.injected_kind()
                        .is_some_and(|kind| kinds.contains(&kind))
                })
                .cloned()
                .collect();
            if selected.is_empty() {
                continue;
            }
            let cx = StartCtx {
                command: arg.command,
                invocation_id: &invocation_id,
                project_dir: &arg.io.in_dir,
                target_dir: &arg.io.out_dir,
                phases,
                command_options: &arg.command_options,
                selected: &selected,
                nodes,
            };
            // An error drops `started`, which finishes the runs that did start.
            if let Some(run) = tasks.start(&cx).await? {
                for kind in kinds {
                    started.by_kind.insert(*kind, Arc::clone(&run));
                }
                started.runs.push(run);
            }
        }
        Ok(Self {
            started: Arc::new(started),
        })
    }

    /// The tasks of an injected node: none if the feature of its kind did not start.
    pub fn tasks_for_node(&self, node: &Arc<dyn InjectedNode>, cx: &TasksCtx<'_>) -> TasksForNode {
        let run = node
            .injected_kind()
            .and_then(|kind| self.started.by_kind.get(kind));
        match run {
            Some(run) => run.tasks_for_node(node, cx),
            None => TasksForNode {
                renderable: None,
                analyzeable: None,
                runnable: None,
                showable: None,
            },
        }
    }

    /// The phases of the graph and the kinds whose nodes it has tasks for, when a feature
    /// added the invocation's command.
    pub fn own_command(&self) -> Option<(&'static [TP], &[&'static str])> {
        self.started
            .own_command
            .as_ref()
            .map(|command| (command.phases, command.kinds.as_slice()))
    }

    /// Finishes every run, once, reporting any that fails to. Runs not finished by the time
    /// the last clone is dropped are finished then.
    pub fn finish(&self) {
        self.started.finish();
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use dbt_common::stats::NodeStatus;
    use dbt_common::{ErrorCode, fs_err};
    use dbt_schemas::jinja_node::JinjaNodeFields;
    use dbt_schemas::schemas::{
        CommonAttributes, InjectedNodeType, InternalDbtNodeAttributes, NodeBaseAttributes,
    };

    const WIDGET: &str = "widget";
    const GADGET: &str = "gadget";
    const GIZMO: &str = "gizmo";

    /// A feature that adds one kind of node. It counts how often its run finishes.
    struct TestTasks {
        kinds: [&'static str; 1],
        phases: &'static [TP],
        command: Option<InjectedCommand>,
        starts: bool,
        finishes: Arc<AtomicUsize>,
    }

    impl TestTasks {
        fn new(kind: &'static str) -> Self {
            Self {
                kinds: [kind],
                phases: &[TP::Run],
                command: None,
                starts: true,
                finishes: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    #[async_trait]
    impl InjectedTasks for TestTasks {
        fn kinds(&self) -> &[&'static str] {
            &self.kinds
        }
        fn phases(&self) -> &'static [TP] {
            self.phases
        }
        fn command(&self) -> Option<InjectedCommand> {
            self.command
        }
        async fn start(&self, _cx: &StartCtx<'_>) -> FsResult<Option<Arc<dyn InjectedRun>>> {
            if !self.starts {
                return Err(fs_err!(ErrorCode::ExperimentalError, "can't start"));
            }
            Ok(Some(Arc::new(TestRun(Arc::clone(&self.finishes)))))
        }
    }

    struct TestRun(Arc<AtomicUsize>);

    impl InjectedRun for TestRun {
        fn tasks_for_node(&self, node: &Arc<dyn InjectedNode>, cx: &TasksCtx<'_>) -> TasksForNode {
            cx.run_task_if_selected(|| TestTask(Arc::clone(node)))
        }
        fn finish(&self) -> FsResult<()> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    struct TestTask(Arc<dyn InjectedNode>);

    impl Task for TestTask {
        fn run_task<'a>(
            &'a self,
            _ctx: &'a mut TaskRunnerCtx,
        ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
            Box::pin(async { Ok(NodeStatus::Succeeded) })
        }
        fn resource_type(&self) -> NodeType {
            NodeType::Experimental
        }
        fn task_type(&self) -> &str {
            "test"
        }
        fn work_node_id(&self) -> &str {
            &self.0.common().unique_id
        }
        fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
            vec![self.0.clone()]
        }
        fn task_phase(&self) -> Option<TP> {
            Some(TP::Run)
        }
    }

    /// The node type of the tests' features: dbt's attributes, and the node's kind.
    #[derive(Debug, Clone, Default, serde::Serialize, JinjaNodeFields)]
    struct TestNode {
        __common_attr__: CommonAttributes,
        __base_attr__: NodeBaseAttributes,
        #[serde(skip)]
        kind: &'static str,
    }

    impl InjectedNodeType for TestNode {
        fn kind(&self) -> &str {
            self.kind
        }
        fn common_attributes(&self) -> &CommonAttributes {
            &self.__common_attr__
        }
        fn common_attributes_mut(&mut self) -> &mut CommonAttributes {
            &mut self.__common_attr__
        }
        fn base_attributes(&self) -> &NodeBaseAttributes {
            &self.__base_attr__
        }
        fn base_attributes_mut(&mut self) -> &mut NodeBaseAttributes {
            &mut self.__base_attr__
        }
    }

    /// A selected `widget` node and a selected `gadget` node.
    fn nodes_and_schedule() -> (Nodes, Schedule<String>) {
        let mut nodes = Nodes::default();
        let mut schedule = Schedule::default();
        for kind in [WIDGET, GADGET] {
            let mut node = TestNode {
                kind,
                ..Default::default()
            };
            node.__common_attr__.unique_id = format!("{kind}.p.n");
            let unique_id = node.__common_attr__.unique_id.clone();
            nodes.injected.insert(unique_id.clone(), Arc::new(node));
            schedule.sorted_nodes.push(unique_id.clone());
            schedule.selected_nodes.insert(unique_id);
        }
        (nodes, schedule)
    }

    fn start(
        injected: &[Arc<dyn InjectedTasks>],
        command: FsCommand,
        nodes: &Nodes,
        schedule: &Schedule<String>,
    ) -> FsResult<InjectedRuns> {
        let arg = RunTasksArgs {
            command,
            ..Default::default()
        };
        futures::executor::block_on(InjectedRuns::start(injected, &arg, nodes, schedule))
    }

    /// `widget`, which owns the `widgets` command, then `gadget` and `gizmo`.
    fn features() -> Vec<Arc<dyn InjectedTasks>> {
        vec![
            Arc::new(TestTasks {
                command: Some(InjectedCommand::runs("widgets")),
                ..TestTasks::new(WIDGET)
            }),
            Arc::new(TestTasks::new(GADGET)),
            Arc::new(TestTasks::new(GIZMO)),
        ]
    }

    /// The kinds whose features `command` starts, with only the nodes of the kinds `selected`
    /// selected.
    fn started(command: FsCommand, selected: &[&str]) -> Vec<&'static str> {
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule
            .selected_nodes
            .retain(|id| selected.iter().any(|kind| id.starts_with(kind)));
        let runs = start(&features(), command, &nodes, &schedule).unwrap();
        [WIDGET, GADGET]
            .into_iter()
            .filter(|kind| gives_a_run_task(&runs, &nodes, kind))
            .collect()
    }

    /// Whether `runs` gives the node of `kind` a task in the run phase when it is selected:
    /// whether the feature of its kind started.
    fn gives_a_run_task(runs: &InjectedRuns, nodes: &Nodes, kind: &str) -> bool {
        let cx = TasksCtx {
            phases: &[TP::Run],
            nodes,
            selected: true,
        };
        runs.tasks_for_node(&nodes.injected[&format!("{kind}.p.n")], &cx)
            .runnable
            .is_some()
    }

    #[test]
    fn only_features_with_selected_nodes_start() {
        assert_eq!(
            started(FsCommand::Build, &[WIDGET, GADGET]),
            [WIDGET, GADGET]
        );
        assert_eq!(started(FsCommand::Build, &[GADGET]), [GADGET]);
    }

    #[test]
    fn a_graph_with_none_of_a_features_phases_does_not_start_it() {
        assert!(started(FsCommand::Compile, &[WIDGET, GADGET]).is_empty());
        // A command that builds no graph starts nothing.
        assert!(started(FsCommand::Parse, &[WIDGET, GADGET]).is_empty());
    }

    #[test]
    fn a_feature_starts_for_a_graph_with_one_of_its_phases() {
        let (nodes, schedule) = nodes_and_schedule();
        let injected: Vec<Arc<dyn InjectedTasks>> = vec![Arc::new(TestTasks {
            phases: &[TP::Render, TP::Run],
            ..TestTasks::new(WIDGET)
        })];
        let runs = start(&injected, FsCommand::Compile, &nodes, &schedule).unwrap();
        assert!(gives_a_run_task(&runs, &nodes, WIDGET));
    }

    #[test]
    fn a_features_own_command_starts_only_that_feature() {
        let command = FsCommand::Extension("widgets");
        assert_eq!(started(command, &[WIDGET, GADGET]), [WIDGET]);

        let (nodes, schedule) = nodes_and_schedule();
        let runs = start(&features(), command, &nodes, &schedule).unwrap();
        assert_eq!(runs.own_command(), Some((&[TP::Run][..], &[WIDGET][..])));
        let runs = start(&features(), FsCommand::Build, &nodes, &schedule).unwrap();
        assert_eq!(runs.own_command(), None);
    }

    #[test]
    fn a_started_feature_gives_its_selected_nodes_their_tasks() {
        let (nodes, mut schedule) = nodes_and_schedule();
        schedule.selected_nodes.retain(|id| id.starts_with(WIDGET));
        let runs = start(&features(), FsCommand::Build, &nodes, &schedule).unwrap();
        let tasks = |unique_id: &str, selected: bool| {
            let cx = TasksCtx {
                phases: &[TP::Render, TP::Analyze, TP::Run],
                nodes: &nodes,
                selected,
            };
            runs.tasks_for_node(&nodes.injected[unique_id], &cx)
        };
        let widget = tasks("widget.p.n", true);
        assert!(widget.runnable.is_some());
        assert!(widget.renderable.is_none() && widget.analyzeable.is_none());
        assert!(tasks("widget.p.n", false).runnable.is_none());
        // Its feature did not start: none of its nodes is selected.
        assert!(tasks("gadget.p.n", true).runnable.is_none());
    }

    /// The graph's injected nodes are counted by node and by kind, whatever the number of
    /// their tasks.
    #[test]
    fn injected_nodes_are_counted_when_the_graph_has_a_task_for_them() {
        let (nodes, _) = nodes_and_schedule();
        let mut graph: Graph<Arc<dyn Task>, ()> = Graph::new();
        assert!(injected_nodes_with_tasks(&graph).is_empty());
        // Two tasks of the widget, and none of the gadget.
        for _ in 0..2 {
            graph.add_node(Arc::new(TestTask(Arc::clone(
                &nodes.injected["widget.p.n"],
            ))));
        }
        assert_eq!(
            injected_nodes_with_tasks(&graph),
            BTreeMap::from([(WIDGET.to_string(), 1)])
        );
        graph.add_node(Arc::new(TestTask(Arc::clone(
            &nodes.injected["gadget.p.n"],
        ))));
        assert_eq!(
            injected_nodes_with_tasks(&graph),
            BTreeMap::from([(GADGET.to_string(), 1), (WIDGET.to_string(), 1)])
        );
    }

    /// Starts `widget` and then `gadget`, both with a selected node, and counts how often the
    /// `widget` run finishes.
    fn start_counting(gadget_starts: bool) -> (FsResult<InjectedRuns>, Arc<AtomicUsize>) {
        let widget = TestTasks::new(WIDGET);
        let finishes = Arc::clone(&widget.finishes);
        let injected: Vec<Arc<dyn InjectedTasks>> = vec![
            Arc::new(widget),
            Arc::new(TestTasks {
                starts: gadget_starts,
                ..TestTasks::new(GADGET)
            }),
        ];
        let (nodes, schedule) = nodes_and_schedule();
        let runs = start(&injected, FsCommand::Build, &nodes, &schedule);
        (runs, finishes)
    }

    #[test]
    fn a_feature_that_cant_start_finishes_the_runs_that_did() {
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
}
