//! Experimentals: their own kind of node, resolved by the core and run in the task graph.
//!
//! An experimental never implements [`Task`]. The core wraps each selected experimental node in a
//! [`ExperimentalTask`], which calls the experimental's [`ExperimentalRun::run`] on the async pool
//! and turns its outcome into the node's result. Experimentals see only their own nodes, their own
//! data and their nodes' dependencies, and reach the warehouse only through [`ExperimentalRunCtx`].

use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use arrow::array::RecordBatch;
use async_trait::async_trait;
use dbt_adapter::adapter::{AdapterStore, quote_component};
use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter_core::AdapterType;
use dbt_common::io_args::FsCommand;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, FsResult};
use dbt_flags::{FlagDef, Stage};
use dbt_jinja_utils::DocMacro;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::utils::render_sql;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::profiles::TargetContext;
use dbt_schemas::schemas::relations::base::{BaseRelation, ComponentName};
use dbt_schemas::schemas::{InjectedNode, InternalDbtNodeAttributes, Nodes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::{TP, Task, TasksForNode, run_blocking_task_operation};
use dbt_telemetry::NodeType;
use minijinja::State;
use minijinja::Value as MinijinjaValue;
use minijinja::arg_utils::ArgParser;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, OverlayMap};

pub use dbt_parser::experimental::{
    ExperimentalDiagnostics, ExperimentalError, InjectedNodeDef, KindEntries, KindEntry,
};
pub use dbt_parser::resolve::resolvable_node::{
    Extension, ExtensionSet, Extensions, IndexCtx, IndexedName, LinkCtx, NoExtension, NoExtensions,
    ResolvableNode, ResolvableNodes, ResolveCtx,
};

use crate::injected::{InjectedCommand, InjectedRun, InjectedTasks, StartCtx, TasksCtx};

/// Adds its own kind of node to a project ([`ResolvableNode`]) and runs the ones selected.
///
/// The kind owns top-level keys of properties files ([`ResolvableNode::properties_keys`]) and has
/// a feature flag that turns it on ([`ResolvableNode::flag`]), off by default.
///
/// An experimental node runs only when it is selected directly, and only in its Run phase: it has
/// nothing to render or analyze.
#[async_trait]
pub trait Experimental: ResolvableNode {
    /// The kind of node the experimental adds, e.g. `widget`: the prefix of its nodes' unique ids
    /// (`<experimental type>.<package>.<name>`), and a value `resource_type:` selects.
    fn experimental_type(&self) -> &'static str;

    /// Prepares to run this experimental's selected nodes. Called once per invocation, after nodes
    /// are selected and before the task graph is built, so an error stops the invocation before any
    /// node runs. `None` runs none of them.
    async fn start(
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
    /// The options of the extension command that runs only this experimental's nodes, by name.
    /// Empty for any other command.
    pub command_options: &'a BTreeMap<String, Vec<String>>,
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
    node: Arc<dyn InjectedNode>,
    /// The nodes it depends on.
    pub depends_on: Vec<ExperimentalDependency>,
}

impl ExperimentalNodeRef {
    /// The node, if it is a `T`: the node type of the experimental that added it.
    pub fn node<T: std::any::Any>(&self) -> Option<&T> {
        self.node.as_any().downcast_ref()
    }

    fn new(node: &Arc<dyn InjectedNode>, nodes: &Nodes) -> Self {
        let depends_on = node
            .base()
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
            unique_id: node.common().unique_id.clone(),
            name: node.common().name.clone(),
            node: Arc::clone(node),
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
    jinja: Option<NodeJinja>,
    /// The target's database and schema, as `target.database` and `target.schema` render.
    database: String,
    schema: String,
}

/// What [`ExperimentalRunCtx::render`] renders with: the node's compile context.
#[derive(Clone)]
struct NodeJinja {
    env: Arc<JinjaEnv>,
    context: OverlayMap,
    path: PathBuf,
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

    /// The target's adapter type.
    pub fn adapter_type(&self) -> AdapterType {
        self.adapter_store.default_adapter_type()
    }

    /// The relation `identifier` in the target's database and schema, quoted for the target's
    /// adapter. For an experimental that writes rows of its own during a run; it must drop them.
    pub fn target_relation(&self, identifier: &str) -> Result<TargetRelation, ExperimentalError> {
        if self.schema.is_empty() {
            return Err(ExperimentalError::new(
                "the target has no schema to write the experimental's rows in",
            ));
        }
        let adapter = self
            .adapter_store
            .get(self.adapter_store.default_adapter_type())
            .map_err(|e| ExperimentalError::new(e.to_string()))?;
        let (adapter_type, quoting) = (adapter.adapter_type(), adapter.engine().quoting());
        let quote =
            |name: &str, component| quote_component(adapter_type, &quoting, name, component);
        let rendered = format!(
            "{}.{}",
            quote(&self.schema, ComponentName::Schema),
            quote(identifier, ComponentName::Identifier)
        );
        let rendered = if self.database.is_empty() {
            rendered
        } else {
            format!(
                "{}.{rendered}",
                quote(&self.database, ComponentName::Database)
            )
        };
        let relation = create_relation(
            adapter_type,
            self.database.clone(),
            self.schema.clone(),
            Some(identifier.to_string()),
            Some(RelationType::Table),
            quoting,
        )
        .map_err(|e| ExperimentalError::new(e.to_string()))?;
        Ok(TargetRelation {
            rendered,
            relation: Arc::from(relation),
        })
    }

    /// Renders `template` as dbt renders a model's SQL: project and package macros, `var()`,
    /// `env_var()`, `target`, and `ref()`/`source()` to the relations of the node's dependencies.
    /// `doc()` renders a doc block, and `metric()` the name of a metric the node depends on.
    /// `this`, when given, is that relation, as a model's `this` is: a relation object that
    /// renders as its name, so a generic test's macro can take it as its `model`.
    pub async fn render(
        &self,
        template: &str,
        this: Option<&TargetRelation>,
    ) -> Result<String, ExperimentalError> {
        self.render_with(template, this, None).await
    }

    /// Renders a data test's SQL as [`Self::render`] does, with `this` the relation it tests, and
    /// keeps what its `config()` calls set, as dbt keeps a test macro's `config(fail_calc=...)`.
    pub async fn render_data_test(
        &self,
        template: &str,
        this: &TargetRelation,
    ) -> Result<RenderedDataTest, ExperimentalError> {
        let config = Arc::new(RecordingConfig::default());
        let sql = self
            .render_with(template, Some(this), Some(Arc::clone(&config)))
            .await?;
        let config = std::mem::take(&mut *config.set.lock().unwrap());
        Ok(RenderedDataTest { sql, config })
    }

    async fn render_with(
        &self,
        template: &str,
        this: Option<&TargetRelation>,
        config: Option<Arc<RecordingConfig>>,
    ) -> Result<String, ExperimentalError> {
        let jinja = self
            .jinja
            .clone()
            .ok_or_else(|| ExperimentalError::new("templates can't be rendered here"))?;
        let mut context = jinja.context;
        if let Some(this) = this {
            context.insert(
                "this".to_string(),
                RelationObject::new(Arc::clone(&this.relation)).into_value(),
            );
        }
        if let Some(config) = config {
            *config.node.lock().unwrap() = context.get("config");
            context.insert(
                "config".to_string(),
                MinijinjaValue::from_dyn_object(config),
            );
        }
        let template = template.to_string();
        // Macros may query the warehouse, so rendering happens on the blocking pool, where
        // connections are made.
        dbt_runtime::spawn_blocking(move || {
            render_sql(
                &template,
                &jinja.env,
                &context,
                &DefaultRenderingEventListenerFactory::default(),
                &jinja.path,
            )
        })
        .await
        .map_err(|e| ExperimentalError::new(format!("rendering did not finish: {e}")))?
        .map_err(|e| ExperimentalError::new(e.pretty()))
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

/// A data test's SQL, rendered, and what its `config()` calls set.
#[derive(Debug, Clone)]
pub struct RenderedDataTest {
    pub sql: String,
    /// Each key a `config()` call set, with the last value it set, such as `fail_calc`.
    pub config: BTreeMap<String, MinijinjaValue>,
}

/// `config` for a data test's SQL: each call's keywords are kept, and `config.get` and attributes
/// read them before the node's own config.
#[derive(Debug, Default)]
struct RecordingConfig {
    /// The node's `config`, which everything else goes to.
    node: Mutex<Option<MinijinjaValue>>,
    set: Mutex<BTreeMap<String, MinijinjaValue>>,
}

impl RecordingConfig {
    fn node(&self) -> Option<MinijinjaValue> {
        self.node.lock().unwrap().clone()
    }
}

impl Object for RecordingConfig {
    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, minijinja::Error> {
        let mut args = ArgParser::new(args, None);
        let mut set = args.drain_kwargs();
        // `config({...})`, as dbt also takes it.
        if let Some(mapping) = args.next_positional::<MinijinjaValue>().ok()
            && let Some(pairs) = mapping
                .as_object()
                .and_then(|object| object.try_iter_pairs())
        {
            for (key, value) in pairs {
                if let Some(key) = key.as_str() {
                    set.insert(key.to_string(), value);
                }
            }
        }
        self.set.lock().unwrap().extend(set);
        Ok(MinijinjaValue::from(""))
    }

    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        let set = key
            .as_str()
            .and_then(|key| self.set.lock().unwrap().get(key).cloned());
        set.or_else(|| {
            self.node()?
                .get_item(key)
                .ok()
                .filter(|value| !value.is_undefined())
        })
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, minijinja::Error> {
        if name == "get"
            && let Some(key) = args.first().and_then(MinijinjaValue::as_str)
            && let Some(value) = self.set.lock().unwrap().get(key)
        {
            return Ok(value.clone());
        }
        match self.node() {
            Some(node) => node.call_method(state, name, args, listeners),
            None => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("config has no method {name}"),
            )),
        }
    }
}

/// A relation in the target's database and schema, from [`ExperimentalRunCtx::target_relation`].
#[derive(Clone)]
pub struct TargetRelation {
    /// Quoted for the target's adapter.
    rendered: String,
    relation: Arc<dyn BaseRelation>,
}

impl TargetRelation {
    /// The relation's name, quoted for the target's adapter, for SQL.
    pub fn as_str(&self) -> &str {
        &self.rendered
    }
}

impl std::fmt::Display for TargetRelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rendered)
    }
}

impl std::fmt::Debug for TargetRelation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("TargetRelation")
            .field(&self.rendered)
            .finish()
    }
}

/// The task the core runs for one selected experimental node.
pub struct ExperimentalTask {
    experimental_type: &'static str,
    node: Arc<dyn InjectedNode>,
    node_ref: ExperimentalNodeRef,
    run: Arc<dyn ExperimentalRun>,
}

impl ExperimentalTask {
    async fn run(&self, ctx: &TaskRunnerCtx) -> FsResult<NodeStatus> {
        let start = SystemTime::now();
        let unique_id = self.node_ref.unique_id.clone();
        let jinja = match node_jinja(ctx, &self.node) {
            Ok(jinja) => Some(jinja),
            Err(error) => {
                emit_warn_log_from_fs_error(*error);
                None
            }
        };
        let (database, schema) = target_namespace(ctx);
        let run_ctx = ExperimentalRunCtx {
            experimental_type: self.experimental_type,
            adapter_store: Arc::clone(ctx.adapter_store()),
            jinja,
            database,
            schema,
        };
        let (status, failures, message) = match self.run.run(&self.node_ref, &run_ctx).await {
            Ok(ExperimentalOutcome::Pass) => (NodeStatus::Succeeded, Some(0), None),
            Ok(ExperimentalOutcome::Fail { failures, message }) => {
                // A failed experimental node is reported, and counts, as an error, as a failed
                // test does.
                emit_error_log_from_fs_error(
                    *ExperimentalError::new(format!("'{}' failed: {message}", self.node_ref.name))
                        .into_fs_error(self.experimental_type, ErrorCode::ExperimentalError),
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

/// The target's database and schema, as `target.database` and `target.schema` render: with the
/// adapter's defaults (DuckDB's `main`) where the profile leaves them out.
fn target_namespace(ctx: &TaskRunnerCtx) -> (String, String) {
    let profile = ctx.dbt_profile();
    let target = TargetContext::try_from(profile.default_db_config().clone())
        .ok()
        .and_then(|target| dbt_yaml::to_value(&target).ok());
    let get = |key: &str, fallback: &str| {
        target
            .as_ref()
            .and_then(|target| target.get(key))
            .and_then(|value| value.as_str().map(str::to_string))
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| fallback.to_string())
    };
    (
        get("database", &profile.database),
        get("schema", &profile.schema),
    )
}

/// The node's compile context, as a model's SQL is compiled with, plus `doc()` and `metric()`,
/// which no compile context has.
fn node_jinja(ctx: &TaskRunnerCtx, node: &Arc<dyn InjectedNode>) -> FsResult<NodeJinja> {
    let base = ctx.run_base_context(node.as_ref())?;
    let (mut context, _) = ctx.build_compile_node_context(
        Arc::clone(node),
        &base,
        DependencyValidationConfig::new_validated(),
    )?;
    let state = ctx.resolver_state();
    let package = node.common().package_name.clone();
    let root = state.root_project_name.clone();

    // Docs from the root project first, then the node's package, as core searches them.
    let docs = state
        .macros
        .docs_macros
        .values()
        .map(|doc| {
            (
                (doc.package_name.clone(), doc.name.clone()),
                doc.block_contents.clone(),
            )
        })
        .collect();
    let mut search_order = vec![root.clone()];
    if package != root {
        search_order.push(package);
    }
    context.insert(
        "doc".to_string(),
        MinijinjaValue::from_object(DocMacro::new_strict_with_search_order(search_order, docs)),
    );

    // At run time core's `metric()` is a reference that renders as the metric's name. Only
    // metrics the node depends on resolve.
    let metrics: BTreeSet<String> = node
        .base()
        .depends_on
        .nodes
        .iter()
        .filter_map(|id| state.nodes.metrics.get(id))
        .map(|metric| metric.__common_attr__.name.clone())
        .collect();
    context.insert(
        "metric".to_string(),
        MinijinjaValue::from_function(move |name: String| {
            if metrics.contains(&name) {
                Ok(MinijinjaValue::from_safe_string(name))
            } else {
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("metric('{name}') isn't a metric this node depends on"),
                ))
            }
        }),
    );
    Ok(NodeJinja {
        env: Arc::clone(&ctx.env),
        context,
        path: PathBuf::from(node.common().original_file_path.to_string()),
    })
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

/// The experimentals an invocation runs with.
#[derive(Clone, Default)]
pub struct Experimentals {
    experimentals: Vec<Arc<dyn Experimental>>,
    /// The tasks of the experimentals' nodes, one source for each experimental.
    tasks: Vec<Arc<dyn InjectedTasks>>,
    /// The kinds of node that have nothing to run: [`Self::with_kind`].
    kinds: Vec<Arc<dyn ResolvableNode>>,
    /// The kinds of the nodes that the experimentals and the kinds add.
    types: BTreeSet<&'static str>,
    /// The keys that the experimentals and the kinds read.
    keys: BTreeSet<&'static str>,
    /// dbt's own kinds of node, which the experimentals' kinds are added to.
    dbt: ResolvableNodes,
}

impl Experimentals {
    /// # Panics
    ///
    /// If an experimental's type or one of its keys isn't a lowercase identifier (or two, for a
    /// key of a section: `section.key`) or names something dbt itself uses, if two experimentals have the same type or key, or if an
    /// experimental has no flag, or one that is on by default or is a behavior flag.
    pub fn new(experimentals: Vec<Arc<dyn Experimental>>) -> Self {
        let mut registry = Self::default();
        for experimental in &experimentals {
            let experimental_type = experimental.experimental_type();
            registry.register(
                experimental_type,
                &[experimental_type],
                experimental.as_ref(),
            );
            registry.tasks.push(Arc::new(ExperimentalTasks {
                experimental: Arc::clone(experimental),
                kinds: [experimental_type],
            }));
        }
        registry.experimentals = experimentals;
        registry
    }

    /// Adds `kind`, a kind of node that has nothing to run: its nodes are definitions, which
    /// other nodes read. `node_kinds` are the kinds of its nodes: the prefixes of their unique
    /// ids, and values `resource_type:` selects.
    ///
    /// It resolves after the experimentals, and is held to the same rules.
    ///
    /// # Panics
    ///
    /// As [`Self::new`] does, for the kind's keys, its flag and the kinds of its nodes.
    pub fn with_kind(mut self, kind: Arc<dyn ResolvableNode>, node_kinds: &[&'static str]) -> Self {
        let name = node_kinds.first().expect("a kind adds nodes of some kind");
        self.register(name, node_kinds, kind.as_ref());
        self.kinds.push(kind);
        self
    }

    /// Checks `kind`, which adds nodes of the kinds `node_kinds`, against dbt's own names and
    /// what is registered so far. `name` names it in an error.
    fn register(&mut self, name: &str, node_kinds: &[&'static str], kind: &dyn ResolvableNode) {
        let dbt_keys: BTreeSet<&str> = ResolvableNodes::dbt().properties_keys().collect();
        for node_kind in node_kinds {
            check_name("experimental type", node_kind);
            assert!(
                self.types.insert(node_kind),
                "experimental type `{node_kind}` is registered twice"
            );
        }
        check_flag(name, kind.flag());
        for key in kind.properties_keys() {
            // A key of a section, `section.key`, is two names. The section is the top-level
            // key.
            let (top_level, section_key) = match key.split_once('.') {
                Some((section, section_key)) => (section, Some(section_key)),
                None => (*key, None),
            };
            check_name("key", top_level);
            if let Some(section_key) = section_key {
                check_name("key", section_key);
            }
            assert!(
                !dbt_keys.contains(top_level) && !matches!(top_level, "version" | "anchors"),
                "experimental `{name}` reads `{key}`, a key dbt reads"
            );
            assert!(
                self.keys.insert(*key),
                "`{key}` is read by more than one experimental, or twice by `{name}`"
            );
        }
    }

    /// The same experimentals, for a build whose own kinds of node read the extensions `E`
    /// beside the properties of their entries.
    pub fn with_extensions<E: Extensions>(mut self) -> Self {
        self.dbt = ResolvableNodes::dbt_with::<E>();
        self
    }

    /// What gives the task graph the tasks of the experimentals' nodes.
    pub fn injected_tasks(&self) -> &[Arc<dyn InjectedTasks>] {
        &self.tasks
    }

    /// Every feature flag this binary knows: dbt's own, and the flags that turn on its
    /// experimentals and its kinds, which need not be in `dbt_flags::ALL`.
    pub fn known_flags(&self) -> Vec<&'static FlagDef> {
        let mut known: Vec<&'static FlagDef> = dbt_flags::ALL.to_vec();
        let added = self
            .experimentals
            .iter()
            .filter_map(|experimental| experimental.flag())
            .chain(self.kinds.iter().filter_map(|kind| kind.flag()));
        for flag in added {
            if !known.iter().any(|known| known.name == flag.name) {
                known.push(flag);
            }
        }
        known
    }

    /// The name and description of every beta feature this binary knows, for `--help`.
    pub fn beta_features(&self) -> Vec<(&'static str, &'static str)> {
        self.known_flags()
            .into_iter()
            .filter(|flag| flag.stage == Stage::Beta)
            .map(|flag| (flag.name, flag.description))
            .collect()
    }

    /// The kinds of node a project resolves with: dbt's own, then the experimentals', then
    /// the kinds that have nothing to run.
    pub fn resolvable_nodes(&self) -> ResolvableNodes {
        self.experimentals
            .iter()
            .map(|experimental| Arc::clone(experimental) as Arc<dyn ResolvableNode>)
            .chain(self.kinds.iter().cloned())
            .fold(self.dbt.clone(), |nodes, kind| nodes.with(kind))
    }
}

/// Checks that `name`, an experimental type or a key, is a lowercase identifier that dbt doesn't
/// use.
fn check_name(what: &str, name: &str) {
    assert!(
        is_identifier(name),
        "{what} `{name}` is not a lowercase identifier"
    );
    assert!(!is_dbt_name(name), "{what} `{name}` is a name dbt uses");
}

fn check_flag(experimental_type: &str, flag: Option<&FlagDef>) {
    let Some(flag) = flag else {
        panic!("experimental `{experimental_type}` must have a feature flag");
    };
    assert!(
        matches!(flag.stage, Stage::Experimental | Stage::Beta),
        "experimental `{experimental_type}`'s flag `{}` must be an experimental or beta flag",
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

/// An experimental as the source of its nodes' tasks: each selected node gets one task, in the
/// run phase, which calls [`ExperimentalRun::run`].
struct ExperimentalTasks {
    experimental: Arc<dyn Experimental>,
    /// The experimental's type.
    kinds: [&'static str; 1],
}

#[async_trait]
impl InjectedTasks for ExperimentalTasks {
    fn kinds(&self) -> &[&'static str] {
        &self.kinds
    }

    fn command(&self) -> Option<InjectedCommand> {
        self.experimental.command().map(InjectedCommand::runs)
    }

    async fn start(&self, cx: &StartCtx<'_>) -> FsResult<Option<Arc<dyn InjectedRun>>> {
        let [experimental_type] = self.kinds;
        let selected: Vec<ExperimentalNodeRef> = cx
            .selected
            .iter()
            .map(|node| ExperimentalNodeRef::new(node, cx.nodes))
            .collect();
        let args = ExperimentalRunArgs {
            command: cx.command,
            invocation_id: cx.invocation_id,
            project_dir: cx.project_dir,
            target_dir: cx.target_dir,
            selected: &selected,
            command_options: cx.command_options,
        };
        let run = self.experimental.start(&args).await.map_err(|error| {
            error.into_fs_error(experimental_type, ErrorCode::ExperimentalError)
        })?;
        Ok(run.map(|run| {
            Arc::new(StartedExperimental {
                experimental_type,
                run,
            }) as Arc<dyn InjectedRun>
        }))
    }
}

/// One invocation's run of an experimental, as the source of its nodes' tasks.
struct StartedExperimental {
    experimental_type: &'static str,
    run: Arc<dyn ExperimentalRun>,
}

impl InjectedRun for StartedExperimental {
    fn tasks_for_node(&self, node: &Arc<dyn InjectedNode>, cx: &TasksCtx<'_>) -> TasksForNode {
        cx.run_task_if_selected(|| ExperimentalTask {
            experimental_type: self.experimental_type,
            node: Arc::clone(node),
            node_ref: ExperimentalNodeRef::new(node, cx.nodes),
            run: Arc::clone(&self.run),
        })
    }

    fn finish(&self) -> FsResult<()> {
        self.run.finish().map_err(|error| {
            error.into_fs_error(self.experimental_type, ErrorCode::ExperimentalError)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::injected::InjectedRuns;
    use dbt_dag::schedule::Schedule;
    use dbt_flags::Remote;
    use dbt_schemas::jinja_node::JinjaNodeFields;
    use dbt_schemas::schemas::{CommonAttributes, InjectedNodeType, NodeBaseAttributes};
    use dbt_tasks_core::RunTasksArgs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TEST_FLAG: FlagDef = flag(false, Stage::Beta);
    static ON_BY_DEFAULT: FlagDef = flag(true, Stage::Beta);
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

    /// The one key of an experimental of the type `experimental_type`: the type itself.
    fn own_key(experimental_type: &str) -> &'static [&'static str] {
        match experimental_type {
            WIDGET => &[WIDGET],
            GADGET => &[GADGET],
            GIZMO => &[GIZMO],
            _ => &[],
        }
    }

    /// An experimental of a type, with a flag or without. `widget` also owns the `widgets`
    /// command.
    struct TestExperimental(&'static str, Option<&'static FlagDef>);

    #[async_trait]
    impl ResolvableNode for TestExperimental {
        fn flag(&self) -> Option<&'static FlagDef> {
            self.1
        }
        fn properties_keys(&self) -> &'static [&'static str] {
            own_key(self.0)
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    /// A `widget` experimental that reads `keys`.
    struct WithKeys(&'static [&'static str]);

    #[async_trait]
    impl ResolvableNode for WithKeys {
        fn flag(&self) -> Option<&'static FlagDef> {
            Some(&TEST_FLAG)
        }
        fn properties_keys(&self) -> &'static [&'static str] {
            self.0
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl Experimental for WithKeys {
        fn experimental_type(&self) -> &'static str {
            WIDGET
        }
        async fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl Experimental for TestExperimental {
        fn experimental_type(&self) -> &'static str {
            self.0
        }
        async fn start(
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
        starts: bool,
        finishes: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ResolvableNode for CountingExperimental {
        fn flag(&self) -> Option<&'static FlagDef> {
            Some(&TEST_FLAG)
        }
        fn properties_keys(&self) -> &'static [&'static str] {
            own_key(WIDGET)
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl Experimental for CountingExperimental {
        fn experimental_type(&self) -> &'static str {
            WIDGET
        }
        async fn start(
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

    /// Starts `experimentals` for `command`, with a `widget` node and a `gadget` node selected.
    fn start(experimentals: &Experimentals, command: FsCommand) -> FsResult<InjectedRuns> {
        let (nodes, schedule) = nodes_and_schedule();
        let arg = RunTasksArgs {
            command,
            ..Default::default()
        };
        futures::executor::block_on(InjectedRuns::start(
            experimentals.injected_tasks(),
            &arg,
            &nodes,
            &schedule,
        ))
    }

    #[test]
    fn a_started_experimental_finishes_once() {
        let finishes = Arc::new(AtomicUsize::new(0));
        let experimentals = Experimentals::new(vec![Arc::new(CountingExperimental {
            starts: true,
            finishes: Arc::clone(&finishes),
        })]);
        let runs = start(&experimentals, FsCommand::Build).unwrap();
        assert!(runs.has_started(WIDGET));
        assert_eq!(finishes.load(Ordering::SeqCst), 0);
        runs.finish();
        drop(runs);
        assert_eq!(finishes.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_experimental_that_cant_start_stops_the_invocation() {
        let experimentals = Experimentals::new(vec![Arc::new(CountingExperimental {
            starts: false,
            finishes: Arc::new(AtomicUsize::new(0)),
        })]);
        let error = start(&experimentals, FsCommand::Build).err().unwrap();
        assert_eq!(error.code, ErrorCode::ExperimentalError);
        assert_eq!(error.to_string(), "widget: can't start");
    }

    fn experimentals(experimental_types: &[&'static str]) -> Experimentals {
        Experimentals::new(
            experimental_types
                .iter()
                .map(|experimental_type| {
                    Arc::new(TestExperimental(experimental_type, Some(&TEST_FLAG)))
                        as Arc<dyn Experimental>
                })
                .collect(),
        )
    }

    /// The node type of the tests' experimentals: dbt's attributes, and the node's kind.
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
        for experimental_type in [WIDGET, GADGET] {
            let mut node = TestNode {
                kind: experimental_type,
                ..Default::default()
            };
            node.__common_attr__.unique_id = format!("{experimental_type}.p.n");
            let unique_id = node.__common_attr__.unique_id.clone();
            nodes.injected.insert(unique_id.clone(), Arc::new(node));
            schedule.sorted_nodes.push(unique_id.clone());
            schedule.selected_nodes.insert(unique_id);
        }
        (nodes, schedule)
    }

    /// An experimental node has one task, in the run phase, when it is selected.
    #[test]
    fn a_started_experimental_gives_its_selected_nodes_a_run_task() {
        let experimentals = experimentals(&[WIDGET, GADGET]);
        let runs = start(&experimentals, FsCommand::Build).unwrap();
        let (nodes, _) = nodes_and_schedule();
        let tasks = |selected: bool| {
            let cx = TasksCtx {
                phases: &[TP::Render, TP::Analyze, TP::Run],
                nodes: &nodes,
                selected,
            };
            runs.tasks_for_node(&nodes.injected["widget.p.n"], &cx)
        };
        let selected = tasks(true);
        let task = selected.runnable.expect("a selected node runs");
        assert_eq!(task.task_type(), WIDGET);
        assert_eq!(task.task_phase(), Some(TP::Run));
        assert!(selected.renderable.is_none() && selected.analyzeable.is_none());
        assert!(tasks(false).runnable.is_none());
        runs.finish();
    }

    /// An experimental's own command has only the run phase, and starts only that experimental.
    #[test]
    fn a_experimentals_own_command_runs_only_its_nodes() {
        let experimentals = experimentals(&[WIDGET, GADGET]);
        let runs = start(&experimentals, FsCommand::Extension("widgets")).unwrap();
        assert_eq!(runs.own_command(), Some((&[TP::Run][..], &[WIDGET][..])));
        assert!(runs.has_started(WIDGET));
        assert!(!runs.has_started(GADGET));
    }

    /// A project resolves with dbt's kinds of node, then the experimentals'.
    #[test]
    fn experimentals_resolve_after_dbts_kinds() {
        let dbt_keys: Vec<&str> = ResolvableNodes::dbt().properties_keys().collect();
        let keys: Vec<&str> = experimentals(&[WIDGET, GADGET])
            .resolvable_nodes()
            .properties_keys()
            .collect();
        assert_eq!(keys[..dbt_keys.len()], dbt_keys);
        assert_eq!(keys[dbt_keys.len()..], [WIDGET, GADGET]);
    }

    /// A kind that has nothing to run resolves after the experimentals, and its flag is known.
    #[test]
    fn a_kind_with_nothing_to_run_resolves_last() {
        let dbt_keys = ResolvableNodes::dbt().properties_keys().count();
        let with_kind = experimentals(&[WIDGET]).with_kind(
            Arc::new(WithKeys(&[GIZMO, "workshop.parts"])),
            &[GADGET, GIZMO],
        );
        let keys: Vec<&str> = with_kind.resolvable_nodes().properties_keys().collect();
        assert_eq!(keys[dbt_keys..], [WIDGET, GIZMO, "workshop.parts"]);
        assert!(
            with_kind
                .known_flags()
                .iter()
                .any(|flag| flag.name == TEST_FLAG.name)
        );
        // It is not an experimental: nothing starts it.
        assert_eq!(with_kind.injected_tasks().len(), 1);
    }

    #[test]
    #[should_panic(expected = "experimental type `widget` is registered twice")]
    fn a_kind_adds_nodes_of_kinds_that_nothing_else_adds() {
        experimentals(&[WIDGET]).with_kind(Arc::new(WithKeys(&[GIZMO])), &[GADGET, WIDGET]);
    }

    #[test]
    #[should_panic(expected = "`widget` is read by more than one experimental")]
    fn a_kind_reads_keys_that_nothing_else_reads() {
        experimentals(&[WIDGET]).with_kind(Arc::new(WithKeys(&[WIDGET])), &[GADGET]);
    }

    #[test]
    fn a_experimental_reads_its_keys() {
        Experimentals::new(vec![Arc::new(WithKeys(&[WIDGET, GIZMO]))]);
    }

    #[test]
    #[should_panic(expected = "`gadget` is read by more than one experimental")]
    fn a_key_is_read_by_one_experimental() {
        Experimentals::new(vec![
            Arc::new(WithKeys(&[GADGET])),
            Arc::new(TestExperimental(GADGET, Some(&TEST_FLAG))),
        ]);
    }

    #[test]
    #[should_panic(expected = "experimental type `widget` is registered twice")]
    fn a_experimental_type_is_registered_once() {
        Experimentals::new(vec![
            Arc::new(WithKeys(&[GIZMO])),
            Arc::new(TestExperimental(WIDGET, Some(&TEST_FLAG))),
        ]);
    }

    #[test]
    #[should_panic(expected = "experimental `widget` reads `models`, a key dbt reads")]
    fn a_key_cannot_be_a_key_dbt_reads() {
        Experimentals::new(vec![Arc::new(WithKeys(&["models"]))]);
    }

    #[test]
    fn a_key_can_be_a_key_of_a_section() {
        let keys: Vec<&str> = Experimentals::new(vec![Arc::new(WithKeys(&["workshop.parts"]))])
            .resolvable_nodes()
            .properties_keys()
            .collect();
        assert_eq!(keys.last(), Some(&"workshop.parts"));
    }

    #[test]
    #[should_panic(expected = "experimental `widget` reads `models.parts`, a key dbt reads")]
    fn a_section_cannot_be_a_key_dbt_reads() {
        Experimentals::new(vec![Arc::new(WithKeys(&["models.parts"]))]);
    }

    #[test]
    #[should_panic(expected = "key `Parts` is not a lowercase identifier")]
    fn a_key_of_a_section_is_a_lowercase_identifier() {
        Experimentals::new(vec![Arc::new(WithKeys(&["workshop.Parts"]))]);
    }

    #[test]
    #[should_panic(expected = "key `parts.bolts` is not a lowercase identifier")]
    fn a_section_has_no_sections() {
        Experimentals::new(vec![Arc::new(WithKeys(&["workshop.parts.bolts"]))]);
    }

    #[test]
    #[should_panic(expected = "experimental `widget` must have a feature flag")]
    fn a_experimental_has_a_flag() {
        Experimentals::new(vec![Arc::new(TestExperimental(WIDGET, None))]);
    }

    #[test]
    #[should_panic(expected = "must be off by default")]
    fn a_experimentals_flag_is_off_by_default() {
        Experimentals::new(vec![Arc::new(TestExperimental(
            WIDGET,
            Some(&ON_BY_DEFAULT),
        ))]);
    }

    /// An experimental's flag is a known feature flag, so `--beta-features` turns it on without
    /// an "Unknown beta feature" warning.
    #[test]
    fn experimental_flags_are_known() {
        let snapshot = dbt_flags::FlagSnapshot::builder()
            .cli(["test_experimentals"])
            .build();
        let without = Experimentals::new(vec![]);
        assert_eq!(
            snapshot.warnings(&without.known_flags()),
            ["Unknown beta feature 'test_experimentals' in --beta-features; it has no effect"]
        );
        let with = experimentals(&[WIDGET]);
        assert!(snapshot.warnings(&with.known_flags()).is_empty());
    }

    #[test]
    #[should_panic(expected = "must be an experimental or beta flag")]
    fn a_experimentals_flag_is_not_a_behavior_flag() {
        Experimentals::new(vec![Arc::new(TestExperimental(WIDGET, Some(&BEHAVIOR)))]);
    }

    /// An experimental whose type and one key are `name`.
    struct Named(&'static str);

    #[async_trait]
    impl ResolvableNode for Named {
        fn flag(&self) -> Option<&'static FlagDef> {
            Some(&TEST_FLAG)
        }
        async fn resolve(&self, _ctx: &mut ResolveCtx<'_>) -> FsResult<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl Experimental for Named {
        fn experimental_type(&self) -> &'static str {
            self.0
        }
        async fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            Ok(None)
        }
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
            "Widget",
        ] {
            let result =
                std::panic::catch_unwind(|| Experimentals::new(vec![Arc::new(Named(name))]));
            assert!(result.is_err(), "{name} should be rejected");
        }
    }
}
