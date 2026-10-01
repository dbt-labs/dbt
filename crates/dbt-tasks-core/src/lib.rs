#![allow(clippy::cognitive_complexity)]
#![allow(clippy::too_many_arguments)]
#![allow(clippy::type_complexity)]

pub mod context;
pub mod context_factory;
mod generate_run_results;
pub mod lake_compute_catalog_attach;
pub mod lake_compute_mdls;
pub mod lake_compute_propagation;
pub mod local_schema_builder;
pub mod metricflow;
pub mod pretty_table;
pub mod render_task_hooks;
pub mod run_cache;
pub mod run_cache_lifecycle;
pub mod run_task_hooks;
mod run_tasks_args;
pub mod show_task_hooks;
pub mod span_manager;
pub mod static_analysis_buckets;
pub mod task;
pub mod task_runner_hooks;
pub mod task_spans;
pub mod test_aggregation;
pub mod unit_test_schema;
pub mod utils;
pub mod visitor;

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::{fmt, io};

pub use generate_run_results::generate_run_results;
pub use run_tasks_args::RunTasksArgs;

use dbt_adapter::response::AdapterResponse;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::{EvalArgs, IoArgs};
use dbt_common::stats::NodeStatus;
use dbt_common::{CompiledSpans, FsResult, MacroSpan};
use dbt_dag::schedule::Schedule;
use dbt_frontend_common::span::ReclassifySpan;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::{CommonAttributes, ContextRunResult};
use dbt_schemas::state::ResolverState;
use dbt_schemas::stats::Stats;

use self::context::TaskRunnerCtx;

/// Preview/show results produced during task execution.
pub struct Preview {
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Value>,
}

/// Per-node data returned by pre-run hooks.
pub trait PreTaskRunData: Send + Sync {
    /// Returns a value for `node_id`, or `None` if not present.
    fn get(&self, node_id: &str) -> Option<String>;
}

/// Abstract storage for task results. Implementations write serialized output
/// on demand.
pub trait StoreableResults: Send + Sync + fmt::Debug {
    /// Path relative to the output directory where results should be written.
    fn out_dir_relpath(&self) -> PathBuf;

    fn write_results(
        &self,
        resolver_state: &ResolverState,
        writer: &mut dyn io::Write,
    ) -> FsResult<()>;
}

/// Results that can show themselves to the user during the did_compile phase.
pub trait ShowableResults: Send + Sync + fmt::Debug {
    fn show(
        &self,
        arg: &EvalArgs,
        resolved_state: &ResolverState,
        schedule: &Schedule<String>,
        token: &CancellationToken,
    ) -> FsResult<()>;

    fn as_any(&self) -> &dyn Any;
}

pub trait CompiledSqlCache: Send + Sync {
    fn get_compiled_sql_path(&self, io: &IoArgs, common: &CommonAttributes) -> PathBuf;

    fn try_get_compiled_sql(
        &self,
        io: &IoArgs,
        common: &CommonAttributes,
    ) -> Option<(String, Vec<MacroSpan>, Vec<ReclassifySpan>)>;

    fn set_compiled_sql(
        &self,
        io: &IoArgs,
        common: &CommonAttributes,
        rendered_sql_maybe_with_cte: &str,
        spans: &dyn CompiledSpans,
    ) -> FsResult<()>;

    fn clear(&self, unique_id: &str);
}

/// Trait for executing ad-hoc SQL queries via dynamic dispatch.
pub trait AdhocRunner: Send + Sync {
    fn run_adhoc<'a>(
        self: Arc<Self>,
        instruction: &'a dbt_scheduler::instructions::Instruction,
        rendered_sql: &'a str,
        unique_id: Option<&'a str>,
        connection: &'a mut Option<Box<dyn dbt_adbc::Connection>>,
    ) -> Pin<
        Box<
            dyn Future<Output = FsResult<(Vec<arrow::array::RecordBatch>, arrow_schema::SchemaRef)>>
                + Send
                + 'a,
        >,
    >;
}

pub struct TaskRunnerStats {
    pub compile: Stats,
    pub run: Stats,
}

impl TaskRunnerStats {
    pub fn collect_as_results(
        &self,
        adapter_responses: &HashMap<String, AdapterResponse>,
    ) -> Vec<ContextRunResult> {
        self.run
            .stats
            .iter()
            .map(|stat| generate_run_results(stat, &self.run, adapter_responses))
            .collect()
    }

    /// Build a set of (database, schema) tuples from successfully executed relational nodes only.
    pub fn collect_successful_relational_nodes(
        &self,
        resolved_state: &ResolverState,
    ) -> HashSet<(String, String)> {
        self.run
            .stats
            .iter()
            .filter(|stat| {
                // Include only successful nodes
                stat.status == NodeStatus::Succeeded &&
            // Include only relational nodes (models, seeds, snapshots)
            (stat.unique_id.starts_with("model.") ||
             stat.unique_id.starts_with("seed.") ||
             stat.unique_id.starts_with("snapshot."))
            })
            .filter_map(|stat| {
                // Get the database and schema for this node
                resolved_state
                    .nodes
                    .get_node(&stat.unique_id)
                    .map(|node| (node.base().database.clone(), node.base().schema.clone()))
            })
            .collect() // deduplicate by collecting into a hashset
    }
}

pub struct RunTaskResults {
    pub stats: TaskRunnerStats,
    pub adapter_responses: HashMap<String, AdapterResponse>,
    pub storeables: Vec<Box<dyn StoreableResults>>,
    pub showables: Vec<Box<dyn ShowableResults>>,
    pub jinja_env: Arc<JinjaEnv>,
    pub resolved_state: Arc<ResolverState>,
    pub task_runner_ctx: Option<TaskRunnerCtx>,
    pub preview: Option<Result<Preview, String>>,
}
