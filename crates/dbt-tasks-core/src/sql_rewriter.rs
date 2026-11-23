//! Rewrites of the SQL a node renders to.

use dbt_adapter_core::AdapterType;
use dbt_common::FsResult;
use dbt_jinja_utils::edit_mapping::SqlEdit;
use dbt_schemas::schemas::InternalDbtNodeAttributes;

use crate::RunTasksArgs;

/// Rewrites the SQL a node renders to, before that SQL is compiled or run.
pub trait SqlRewriter: Send + Sync {
    /// Whether [`Self::rewrite`] applies `--empty` to the SQL of `adapter_type`
    /// nodes. Where it does, relations render after
    /// [`EMPTY_TABLE_MARKER`](dbt_schemas::filter::EMPTY_TABLE_MARKER) and
    /// `rewrite` empties the marked tables. Where it does not, each relation
    /// renders as a zero-row subquery.
    fn applies_empty(&self, adapter_type: AdapterType) -> bool;

    /// The edits to make to `sql`, which `node` rendered to in a run with `arg`.
    /// The edits must not overlap.
    fn rewrite(
        &self,
        node: &dyn InternalDbtNodeAttributes,
        arg: &RunTasksArgs,
        sql: &str,
    ) -> FsResult<Vec<SqlEdit>>;
}

/// Leaves the SQL as it rendered. `--empty` is applied where relations render.
#[derive(Default)]
pub struct DefaultSqlRewriter;

impl SqlRewriter for DefaultSqlRewriter {
    fn applies_empty(&self, _adapter_type: AdapterType) -> bool {
        false
    }

    fn rewrite(
        &self,
        _node: &dyn InternalDbtNodeAttributes,
        _arg: &RunTasksArgs,
        _sql: &str,
    ) -> FsResult<Vec<SqlEdit>> {
        Ok(Vec::new())
    }
}
