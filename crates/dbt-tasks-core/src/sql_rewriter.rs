//! Rewrites of the SQL a node renders to.

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_jinja_utils::edit_mapping::SqlEdit;
use dbt_schemas::schemas::InternalDbtNodeAttributes;

use crate::RunTasksArgs;
use crate::metricflow::is_metricflow_sql;

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

/// The error of a rewriter that cannot expand the SQL of `node`, a model written in MetricFlow
/// SQL ([`is_metricflow_sql`]).
pub fn cannot_expand_metricflow_sql(node: &dyn InternalDbtNodeAttributes) -> Box<FsError> {
    fs_err!(
        ErrorCode::UnsupportedFusionFeature,
        "'{}' is written in MetricFlow SQL (`metricflow: true`), which this build of dbt cannot \
         expand",
        node.common().name
    )
}

/// Leaves the SQL as it rendered. `--empty` is applied where relations render.
///
/// It cannot expand MetricFlow SQL: a model written in it is an error.
#[derive(Default)]
pub struct DefaultSqlRewriter;

impl SqlRewriter for DefaultSqlRewriter {
    fn applies_empty(&self, _adapter_type: AdapterType) -> bool {
        false
    }

    fn rewrite(
        &self,
        node: &dyn InternalDbtNodeAttributes,
        _arg: &RunTasksArgs,
        _sql: &str,
    ) -> FsResult<Vec<SqlEdit>> {
        if is_metricflow_sql(node) {
            return Err(cannot_expand_metricflow_sql(node));
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use dbt_schemas::schemas::{CommonAttributes, DbtModel};

    use super::*;

    fn model(metricflow: Option<bool>) -> DbtModel {
        let mut model = DbtModel {
            __common_attr__: CommonAttributes {
                name: "counted".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        model.deprecated_config.metricflow = metricflow;
        model
    }

    /// The default rewriter leaves SQL as it rendered, and cannot expand a model written in
    /// MetricFlow SQL.
    #[test]
    fn the_default_rewriter_cannot_expand_metricflow_sql() {
        let arg = RunTasksArgs::default();
        let edits = DefaultSqlRewriter
            .rewrite(&model(None), &arg, "select 1")
            .unwrap();
        assert!(edits.is_empty());
        let error = DefaultSqlRewriter
            .rewrite(&model(Some(true)), &arg, "SELECT AGG(x) FROM sales")
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedFusionFeature);
        assert!(
            error
                .context
                .contains("'counted' is written in MetricFlow SQL"),
            "{}",
            error.context
        );
    }
}
