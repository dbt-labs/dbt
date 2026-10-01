use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::RecordBatch;
use arrow_schema::Schema;
use dbt_adapter_core::AdapterType;
use dbt_adbc::*;
use dbt_auth::AdapterConfig;
use dbt_common::AdapterResult;
use dbt_common::behavior_flags::Behavior;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::schemas::common::ResolvedQuoting;
use minijinja::State;

use crate::cache::RelationCache;
use crate::engine::query_comment::QueryCommentConfig;
use crate::sql_types::TypeOps;
use crate::stmt_splitter::StmtSplitter;

use super::adapter_engine::*;
use super::make_behavior;
use super::noop_connection::NoopConnection;
use super::sidecar_client::SidecarClient;

/// Sidecar engine for subprocess-based execution.
///
/// Routes execution to a sidecar backend via SidecarClient trait.
/// Implementation details (subprocess management, message protocol) remain
/// in closed-source crates.
#[derive(Clone)]
pub struct SidecarEngine {
    adapter_type: AdapterType,
    execution_backend: Backend,
    client: Arc<dyn SidecarClient>,
    quoting: ResolvedQuoting,
    config: Arc<AdapterConfig>,
    type_ops: Arc<dyn TypeOps>,
    stmt_splitter: Arc<dyn StmtSplitter>,
    query_comment: Arc<QueryCommentConfig>,
    /// Unused for sidecar adapters - required for API compatibility
    relation_cache: Arc<RelationCache>,
    /// Resolved behavior object
    behavior: Arc<Behavior>,
}

impl SidecarEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        adapter_type: AdapterType,
        execution_backend: Backend,
        client: Arc<dyn SidecarClient>,
        quoting: ResolvedQuoting,
        config: AdapterConfig,
        type_ops: Arc<dyn TypeOps>,
        stmt_splitter: Arc<dyn StmtSplitter>,
        query_comment: QueryCommentConfig,
        relation_cache: Arc<RelationCache>,
    ) -> Self {
        let behavior = make_behavior(adapter_type, &BTreeMap::new());
        Self {
            adapter_type,
            execution_backend,
            client,
            quoting,
            config: Arc::new(config),
            type_ops,
            stmt_splitter,
            query_comment: Arc::new(query_comment),
            relation_cache,
            behavior,
        }
    }
}

impl AdapterEngine for SidecarEngine {
    fn adapter_type(&self) -> AdapterType {
        self.adapter_type
    }

    fn backend(&self) -> Backend {
        self.execution_backend
    }

    fn quoting(&self) -> ResolvedQuoting {
        self.quoting
    }

    fn splitter(&self) -> &dyn StmtSplitter {
        self.stmt_splitter.as_ref()
    }

    fn type_ops(&self) -> &Arc<dyn TypeOps> {
        &self.type_ops
    }

    fn query_comment(&self) -> &QueryCommentConfig {
        &self.query_comment
    }

    fn config(&self, key: &str) -> Option<Cow<'_, str>> {
        self.config.get_string(key)
    }

    fn get_config(&self) -> &AdapterConfig {
        &self.config
    }

    fn relation_cache(&self) -> &Arc<RelationCache> {
        &self.relation_cache
    }

    fn new_connection(
        &self,
        state: Option<&State>,
        node_id: Option<String>,
    ) -> AdapterResult<Box<dyn Connection>> {
        self.client.new_connection(state, node_id)
    }

    fn new_connection_with_config(
        &self,
        _config: &AdapterConfig,
    ) -> AdapterResult<Box<dyn Connection>> {
        // Sidecar mode doesn't use config-based connections
        Ok(Box::new(NoopConnection) as Box<dyn Connection>)
    }

    fn execute_with_options(
        &self,
        _state: Option<&State>,
        ctx: &QueryCtx,
        _conn: &'_ mut dyn Connection,
        sql: &str,
        _options: Options,
        fetch: bool,
        _token: CancellationToken,
    ) -> AdapterResult<RecordBatch> {
        // Route through sidecar client
        let batch_opt = self.client.execute(ctx, sql, fetch)?;
        match batch_opt {
            Some(batch) => Ok(batch),
            None => Ok(RecordBatch::new_empty(Arc::new(Schema::empty()))),
        }
    }

    fn is_sidecar(&self) -> bool {
        true
    }

    fn physical_backend(&self) -> Option<Backend> {
        Some(self.execution_backend)
    }

    fn sidecar_client(&self) -> Option<&dyn SidecarClient> {
        Some(self.client.as_ref())
    }

    fn behavior(&self) -> &Arc<Behavior> {
        &self.behavior
    }

    fn behavior_flag_overrides(&self) -> &BTreeMap<String, bool> {
        static EMPTY: BTreeMap<String, bool> = BTreeMap::new();
        &EMPTY
    }
}
