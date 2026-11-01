/// Provides functionality to wrap AggregateExec nodes with symbolic nodes for
/// building symbolic expressions that represent aggregation operations.
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use arrow::datatypes::SchemaRef;
use datafusion::error::Result;
use datafusion::execution::context::TaskContext;
use datafusion::physical_plan::DisplayAs;
use datafusion::physical_plan::DisplayFormatType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::PlanProperties;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::physical_plan::Statistics;
use datafusion::physical_plan::aggregates::AggregateExec;
use datafusion::physical_plan::metrics::MetricsSet;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::physical_expr_translater::match_and_handle_expr;
use crate::symbolic_nodes::physical_expr_translater::translate_aggregate_expr;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

/// Symbolic node wrapper for DataFusion AggregateExec.
/// Handles translation of aggregate expressions into symbolic form and maintains
/// the symbolic state during execution.
#[derive(Debug)]
pub struct SymEvalAggregationExec {
    child: Arc<dyn ExecutionPlan>,
    row_expressions: Arc<Mutex<SymbolicExpr>>,
    column_expressions: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalAggregationExec {
    /// Creates a new SymEvalAggregationExec node.
    ///
    /// # Arguments
    /// * `child` - The AggregateExec node to wrap
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expressions: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expressions: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalAggregationExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalAggregationExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalAggregationExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalAggregationExec {
    fn get_child(&self) -> &Arc<dyn ExecutionPlan> {
        &self.child
    }

    fn get_row_expression(&self) -> SymbolicExpr {
        self.row_expressions.lock().unwrap().clone()
    }

    fn get_column_expression(&self) -> Vec<SymbolicExpr> {
        self.column_expressions.lock().unwrap().clone()
    }

    fn get_table_columns(&self) -> HashMap<String, Vec<SQLColumn>> {
        self.table_columns.lock().unwrap().clone()
    }

    fn update_row_expression(&self, new_row_expressions: SymbolicExpr) {
        let mut guard = self.row_expressions.lock().unwrap();
        *guard = new_row_expressions;
    }

    fn update_column_expression(&self, new_column_expressions: Vec<SymbolicExpr>) {
        let mut guard = self.column_expressions.lock().unwrap();
        *guard = new_column_expressions;
    }

    fn update_table_columns(&self, table_columns: HashMap<String, Vec<SQLColumn>>) {
        let mut guard = self.table_columns.lock().unwrap();
        *guard = table_columns;
    }

    fn build_row_and_column_expressions(
        &self,
    ) -> (
        SymbolicExpr,
        Vec<SymbolicExpr>,
        HashMap<String, Vec<SQLColumn>>,
    ) {
        if let Some(aggregate_exec) = self.child.as_any().downcast_ref::<AggregateExec>() {
            let exprs = aggregate_exec.aggr_expr();
            let child = self.child.children()[0].clone();
            let group_by_expr = aggregate_exec.group_expr();
            let (row_expressions, column_expressions, table_columns) =
                get_row_and_column_expressions_from_node(&child);

            // The group by columns are always indexed first in the downstream projection
            // So we need to add the group by columns to the column_expressions first
            let mut new_column_expressions: Vec<SymbolicExpr> = vec![];
            for expr in group_by_expr.expr() {
                new_column_expressions
                    .push(match_and_handle_expr(expr.0.as_ref(), &column_expressions));
            }
            for expr in exprs {
                new_column_expressions.push(translate_aggregate_expr(
                    expr.as_ref(),
                    &row_expressions,
                    &column_expressions,
                ));
            }
            return (row_expressions, new_column_expressions, table_columns);
        }
        panic!("Expected AggregateExec");
    }
}

impl ExecutionPlan for SymEvalAggregationExec {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.child.schema()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.child]
    }

    fn with_new_children(
        self: Arc<Self>,
        _: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        panic!("SymEvalAggregationExec does not support with_new_children");
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        // Use the default implementation from SymbolicNode
        self.execute_symbolic(partition, context)
    }

    fn name(&self) -> &str {
        "SymEvalAggregationExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
