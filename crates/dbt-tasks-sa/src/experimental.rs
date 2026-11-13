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
use std::sync::atomic::{AtomicBool, Ordering};
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
use dbt_dag::schedule::Schedule;
use dbt_flags::{FlagDef, Stage};
use dbt_jinja_utils::DocMacro;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::utils::render_sql;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::profiles::TargetContext;
use dbt_schemas::schemas::relations::base::{BaseRelation, ComponentName};
use dbt_schemas::schemas::{
    DbtExperimentalNode, ExperimentalData, InternalDbtNodeAttributes, Nodes,
};
use dbt_tasks_core::RunTasksArgs;
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::{TP, Task, run_blocking_task_operation};
use dbt_telemetry::NodeType;
use minijinja::State;
use minijinja::Value as MinijinjaValue;
use minijinja::arg_utils::ArgParser;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, OverlayMap};

pub use dbt_parser::experimental::{
    ExperimentalDiagnostics, ExperimentalEntry, ExperimentalError, ExperimentalFile,
    ExperimentalNodeDef, ExperimentalPackage, ResolveExperimental, ResolveExperimentals,
};

/// Adds its own kind of node to a project ([`ResolveExperimental`]) and runs the ones selected.
///
/// An experimental node runs only when it is selected directly, and only in its Run phase: it has
/// nothing to render or analyze.
#[async_trait]
pub trait ExperimentalNode: ResolveExperimental {
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
    node: Arc<DbtExperimentalNode>,
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
fn node_jinja(ctx: &TaskRunnerCtx, node: &Arc<DbtExperimentalNode>) -> FsResult<NodeJinja> {
    let base = ctx.run_base_context(node.as_ref())?;
    let (mut context, _) = ctx.build_compile_node_context(
        Arc::clone(node),
        &base,
        DependencyValidationConfig::new_validated(),
    )?;
    let state = ctx.resolver_state();
    let package = node.__common_attr__.package_name.clone();
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
        .__base_attr__
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
        path: PathBuf::from(node.__common_attr__.original_file_path.to_string()),
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
    pub async fn start(
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
                command_options: &arg.command_options,
            };
            match experimental.start(&args).await {
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

    #[async_trait]
    impl ExperimentalNode for WithKeys {
        async fn start(
            &self,
            _args: &ExperimentalRunArgs<'_>,
        ) -> Result<Option<Arc<dyn ExperimentalRun>>, ExperimentalError> {
            Ok(None)
        }
    }

    #[async_trait]
    impl ExperimentalNode for TestExperimental {
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

    #[async_trait]
    impl ExperimentalNode for CountingExperimental {
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
        let runs = futures::executor::block_on(ExperimentalRuns::start(
            &experimentals,
            &arg,
            &nodes,
            &schedule,
            true,
        ));
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
        let runs = futures::executor::block_on(ExperimentalRuns::start(
            &experimentals,
            &arg,
            &nodes,
            &schedule,
            runs_nodes,
        ))
        .unwrap();
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
        let runs = futures::executor::block_on(ExperimentalRuns::start(
            &experimentals,
            &arg,
            &nodes,
            &schedule,
            true,
        ))
        .unwrap();
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
