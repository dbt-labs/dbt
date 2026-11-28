//! What a task reaches to run its node: the warehouse, a relation in the target's schema, and
//! templates rendered as the node's SQL is. And how a task records the result of a node that
//! asserts something.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use arrow::array::RecordBatch;
use dbt_adapter::adapter::{AdapterStore, quote_component};
use dbt_adapter::relation::{RelationObject, create_relation};
use dbt_adapter_core::AdapterType;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, FsResult};
use dbt_jinja_utils::DocMacro;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultRenderingEventListenerFactory;
use dbt_jinja_utils::phases::compile::DependencyValidationConfig;
use dbt_jinja_utils::utils::render_sql;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::profiles::TargetContext;
use dbt_schemas::schemas::relations::base::{BaseRelation, ComponentName};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::run_blocking_task_operation;
use minijinja::State;
use minijinja::Value as MinijinjaValue;
use minijinja::arg_utils::ArgParser;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Object, OverlayMap};

use crate::injected_kinds::ExperimentalError;

/// What the run of a node can reach.
pub struct NodeRunCtx {
    /// What the node's warnings are named for: its kind.
    kind: String,
    adapter_store: Arc<AdapterStore>,
    jinja: Option<NodeJinja>,
    /// The target's database and schema, as `target.database` and `target.schema` render.
    database: String,
    schema: String,
}

/// What [`NodeRunCtx::render`] renders with: the node's compile context.
#[derive(Clone)]
struct NodeJinja {
    env: Arc<JinjaEnv>,
    context: OverlayMap,
    path: PathBuf,
}

impl NodeRunCtx {
    /// What a task reaches to run `node`.
    ///
    /// Templates render with the node's compile context. When that context can't be built, the
    /// reason is reported as a warning and [`Self::render`] fails.
    pub fn for_node(ctx: &TaskRunnerCtx, node: Arc<dyn InternalDbtNodeAttributes>) -> Self {
        let jinja = match node_jinja(ctx, &node) {
            Ok(jinja) => Some(jinja),
            Err(error) => {
                emit_warn_log_from_fs_error(*error);
                None
            }
        };
        let (database, schema) = target_namespace(ctx);
        Self {
            kind: kind_of(node.as_ref()).to_string(),
            adapter_store: Arc::clone(ctx.adapter_store()),
            jinja,
            database,
            schema,
        }
    }

    /// Runs `sql` on the target's adapter and returns its rows.
    pub async fn query(&self, sql: &str) -> Result<Arc<RecordBatch>, ExperimentalError> {
        self.query_on(self.adapter_store.default_adapter_type(), sql)
            .await
    }

    /// Runs `sql` on the adapter that `node` runs on and returns its rows. For a query on a
    /// node that the task's node depends on.
    pub async fn query_for(
        &self,
        node: &dyn InternalDbtNodeAttributes,
        sql: &str,
    ) -> Result<Arc<RecordBatch>, ExperimentalError> {
        self.query_on(node.base().adapter, sql).await
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
            *warning.into_fs_error(&self.kind, ErrorCode::ExperimentalWarning),
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

/// A relation in the target's database and schema, from [`NodeRunCtx::target_relation`].
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
fn node_jinja(
    ctx: &TaskRunnerCtx,
    node: &Arc<dyn InternalDbtNodeAttributes>,
) -> FsResult<NodeJinja> {
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

/// What the errors and warnings of `node` are named for: its kind, for a node of a kind that a
/// feature adds, or else its resource type.
fn kind_of(node: &dyn InternalDbtNodeAttributes) -> &str {
    node.injected_kind()
        .unwrap_or_else(|| node.resource_type().as_static_ref())
}

/// How the run of a node that asserts something went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssertionOutcome {
    /// The node passed.
    Pass,
    /// The node failed, as a test fails: `failures` things were wrong.
    Fail { failures: usize, message: String },
}

/// Records how the run of `node` went in the run's stats, as the result of a test, and returns
/// the node's status. `started` is when the run started.
///
/// A failure is reported, and counts, as an error, as a failed test does. So is an `Err`: the
/// node could not run.
pub fn record_assertion_result(
    ctx: &TaskRunnerCtx,
    node: &dyn InternalDbtNodeAttributes,
    started: SystemTime,
    result: Result<AssertionOutcome, ExperimentalError>,
) -> NodeStatus {
    let kind = kind_of(node);
    let (status, failures, message) = match result {
        Ok(AssertionOutcome::Pass) => (NodeStatus::Succeeded, Some(0), None),
        Ok(AssertionOutcome::Fail { failures, message }) => {
            emit_error_log_from_fs_error(
                *ExperimentalError::new(format!("'{}' failed: {message}", node.common().name))
                    .into_fs_error(kind, ErrorCode::ExperimentalError),
            );
            (NodeStatus::Errored, Some(failures.max(1)), Some(message))
        }
        Err(error) => {
            let message = error.message().to_string();
            emit_error_log_from_fs_error(*error.into_fs_error(kind, ErrorCode::ExperimentalError));
            (NodeStatus::Errored, None, Some(message))
        }
    };
    let unique_id = node.common().unique_id.clone();
    ctx.inner.run_stats.upsert_sync(
        unique_id.clone(),
        Stat::new(
            unique_id,
            started,
            failures,
            status.clone(),
            message,
            ctx.thread_id,
        )
        .as_assertion(),
    );
    status
}
