use std::{collections::HashSet, sync::Arc};

use crate::context::TaskRunnerCtx;
use arrow_schema::SchemaRef;
use async_trait::async_trait;
use dbt_adapter::sql_types::TypeOps;
use dbt_common::FsResult;
use dbt_schemas::schemas::DbtUnitTest;
use dbt_schemas::schemas::relations::base::BaseRelation;

#[async_trait]
pub trait RenderTaskHooks: Send + Sync {
    async fn will_fetch_schema_for_unit_test_relation(
        &self,
        ctx: &TaskRunnerCtx,
        unit_test_unique_id: &str,
        fetched: &mut HashSet<String>,
        relation: &Arc<dyn BaseRelation>,
        type_ops: &Arc<dyn TypeOps>,
    ) -> FsResult<()>;

    /// Optionally infer SQL fixture columns for model Jinja introspection.
    fn try_infer_unit_test_fixture_schema(
        &self,
        _ctx: &TaskRunnerCtx,
        _unit_test: &DbtUnitTest,
        _sql: &str,
    ) -> FsResult<Option<SchemaRef>> {
        Ok(None)
    }

    /// Optionally infer the tested model's schema without an adapter query.
    fn try_infer_unit_test_schema(
        &self,
        _ctx: &TaskRunnerCtx,
        _unit_test: &DbtUnitTest,
        _sql: &str,
    ) -> FsResult<Option<SchemaRef>> {
        Ok(None)
    }

    /// Called before a unit test's expected-schema query runs through the
    /// adapter. Returns an error when the query can't run on the unit test's
    /// execution engine as written. The default accepts every query.
    fn check_unit_test_schema_query(
        &self,
        _ctx: &TaskRunnerCtx,
        _unit_test: &DbtUnitTest,
        _sql: &str,
    ) -> FsResult<()> {
        Ok(())
    }
}
