use std::collections::BTreeMap;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_adbc::Backend;
use dbt_auth::AdapterConfig;
use dbt_auth::Auth;
use dbt_auth::auth_for_backend;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::ReplayMode;
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::ResolvedCloudConfig;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::project::QueryComment;
use dbt_schemas::schemas::relations::base::BaseRelation;
use minijinja::Value;

use crate::Adapter;
use crate::AdapterEngine;
use crate::cache::RelationCache;
use crate::engine::AdbcEngine;
use crate::engine::query_comment::QueryCommentConfig;
use crate::relation::do_create_relation;
use crate::sql_types::TypeOpsFactory;
use crate::stmt_splitter::StmtSplitter;

use super::AdapterImpl;

pub fn backend_of(adapter_type: AdapterType) -> Backend {
    match adapter_type {
        AdapterType::Postgres => Backend::Postgres,
        AdapterType::Snowflake => Backend::Snowflake,
        AdapterType::Bigquery => Backend::BigQuery,
        AdapterType::Databricks => Backend::Databricks,
        AdapterType::Redshift => Backend::Redshift,
        AdapterType::Salesforce => Backend::Salesforce,
        AdapterType::Spark => Backend::Spark,
        AdapterType::DuckDB => Backend::DuckDB,
        AdapterType::LakeCompute => Backend::LakeCompute,
        AdapterType::Fabric => Backend::SQLServer,
        AdapterType::ClickHouse => Backend::ClickHouse,
        AdapterType::Exasol => Backend::Exasol,
        AdapterType::Starburst => todo!("Starburst"),
        AdapterType::Athena => Backend::Athena,
        AdapterType::Trino => todo!("Trino"),
        AdapterType::Dremio => todo!("Dremio"),
        AdapterType::Oracle => todo!("Oracle"),
        AdapterType::Datafusion => todo!("Datafusion"),
    }
}

/// A factory for adapters, relations and columns.
///
/// It can create [Adapter] instances wrapped in `Arc`.
/// Similarly, it can create boxed `dyn BaseRelation`
/// and `Column` objects.
pub trait AdapterFactory: Send + Sync {
    #[allow(clippy::too_many_arguments)]
    fn create_adapter(
        &self,
        adapter_type: AdapterType,
        config: dbt_yaml::Mapping,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        replay_mode: Option<ReplayMode>,
        flags: BTreeMap<String, Value>,
        schema_cache: Option<Arc<dyn SchemaStoreTrait>>,
        quoting: ResolvedQuoting,
        query_comment: Option<QueryComment>,
        token: CancellationToken,
        cloud_config: Option<&ResolvedCloudConfig>,
        threads: Option<usize>,
    ) -> FsResult<Arc<Adapter>>;

    fn stmt_splitter(&self) -> Arc<dyn StmtSplitter>;

    fn create_relation_from_node(
        &self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
    ) -> Result<Box<dyn BaseRelation>, minijinja::Error>;
}

pub struct DefaultAdapterFactory;

impl DefaultAdapterFactory {
    fn create_engine(
        &self,
        adapter_type: AdapterType,
        adapter_config: AdapterConfig,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        quoting: ResolvedQuoting,
        query_comment: Option<QueryComment>,
        behavior_flag_overrides: BTreeMap<String, bool>,
        cloud_config: Option<&ResolvedCloudConfig>,
        threads: Option<usize>,
    ) -> FsResult<Arc<dyn AdapterEngine>> {
        let backend = backend_of(adapter_type);
        let auth: Arc<dyn Auth> = auth_for_backend(backend).into();
        let stmt_splitter = self.stmt_splitter();
        let type_ops = type_ops_factory.create(adapter_type);
        let relation_cache = Arc::new(RelationCache::default());

        let query_comment =
            QueryCommentConfig::from_query_comment(query_comment, adapter_type, true, cloud_config);
        let dbt_cloud_project_id = cloud_config.and_then(|c| c.project_id.clone());

        let engine = Arc::new(AdbcEngine::new(
            adapter_type,
            auth,
            adapter_config,
            quoting,
            query_comment,
            type_ops,
            stmt_splitter,
            relation_cache,
            behavior_flag_overrides,
            threads,
            dbt_cloud_project_id,
        ));
        Ok(engine)
    }
}

impl AdapterFactory for DefaultAdapterFactory {
    fn create_adapter(
        &self,
        adapter_type: AdapterType,
        config: dbt_yaml::Mapping,
        type_ops_factory: Arc<dyn TypeOpsFactory>,
        _replay_mode: Option<ReplayMode>,
        flags: BTreeMap<String, Value>,
        schema_cache: Option<Arc<dyn SchemaStoreTrait>>,
        quoting: ResolvedQuoting,
        query_comment: Option<QueryComment>,
        token: CancellationToken,
        cloud_config: Option<&ResolvedCloudConfig>,
        threads: Option<usize>,
    ) -> FsResult<Arc<Adapter>> {
        let adapter_config = AdapterConfig::new(config);

        let behavior_flag_overrides = flags
            .iter()
            .map(|(key, value)| {
                let bool_val = if value.is_true() {
                    true
                } else if let Some(s) = value.as_str() {
                    s == "true" || s.parse::<bool>().unwrap_or(false)
                } else {
                    false
                };
                (key.clone(), bool_val)
            })
            .collect::<BTreeMap<_, _>>();

        let engine = self.create_engine(
            adapter_type,
            AdapterConfig::new(adapter_config.repr().clone()),
            type_ops_factory,
            quoting,
            query_comment,
            behavior_flag_overrides,
            cloud_config,
            threads,
        )?;

        let adapter_impl = Arc::new(AdapterImpl::new(engine, schema_cache));

        let adapter: Arc<Adapter> = Arc::new(Adapter::new(adapter_impl, None, token));
        Ok(adapter)
    }

    fn stmt_splitter(&self) -> Arc<dyn StmtSplitter> {
        Arc::new(crate::stmt_splitter::DefaultStmtSplitter {})
    }

    fn create_relation_from_node(
        &self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
    ) -> Result<Box<dyn BaseRelation>, minijinja::Error> {
        let database = node.database();
        let schema = node.schema();
        let identifier = node.base().alias.clone();
        let relation_type = RelationType::from(node.materialized());
        let custom_quoting = node.quoting();
        do_create_relation(
            adapter_type,
            database,
            schema,
            Some(identifier),
            Some(relation_type),
            custom_quoting,
        )
    }
}
