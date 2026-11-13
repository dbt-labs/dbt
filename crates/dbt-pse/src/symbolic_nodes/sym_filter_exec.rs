/// Provides functionality to wrap FilterExec nodes with symbolic nodes for
/// building symbolic expressions that represent filter operations.
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
use datafusion::physical_plan::filter::FilterExec;
use datafusion::physical_plan::metrics::MetricsSet;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::add_constraint_to_row;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::match_and_handle_expr;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

/// Symbolic node wrapper for DataFusion FilterExec.
/// Handles translation of filter predicates into symbolic form and maintains
/// the symbolic state during execution.
#[derive(Debug)]
pub struct SymEvalFilterExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalFilterExec {
    /// Creates a new SymEvalFilterExec node.
    ///
    /// # Arguments
    /// * `child` - The FilterExec node to wrap
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalFilterExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalFilterExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalFilterExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalFilterExec {
    fn get_child(&self) -> &Arc<dyn ExecutionPlan> {
        &self.child
    }

    fn get_row_expression(&self) -> SymbolicExpr {
        self.row_expression.lock().unwrap().clone()
    }

    fn get_column_expression(&self) -> Vec<SymbolicExpr> {
        self.column_expression.lock().unwrap().clone()
    }

    fn get_table_columns(&self) -> HashMap<String, Vec<SQLColumn>> {
        self.table_columns.lock().unwrap().clone()
    }

    fn update_row_expression(&self, new_constraints: SymbolicExpr) {
        let mut guard = self.row_expression.lock().unwrap();
        *guard = new_constraints;
    }

    fn update_column_expression(&self, new_sym_vars: Vec<SymbolicExpr>) {
        let mut guard = self.column_expression.lock().unwrap();
        *guard = new_sym_vars;
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
        if let Some(filter_exec) = self.child.as_any().downcast_ref::<FilterExec>() {
            let expr = filter_exec.predicate();
            let upstream_symbolic_node = self.child.children()[0].clone();
            let (row_expression, column_expression, table_columns) =
                get_row_and_column_expressions_from_node(&upstream_symbolic_node);
            let mut upstream_row_expression = row_expression;

            // Translate the filter expression into a symbolic expression
            let symbolic_expr = match_and_handle_expr(expr.as_ref(), &column_expression);
            add_constraint_to_row(&mut upstream_row_expression, symbolic_expr);

            // If the filter has a projection, we need to drop the columns that are not in the projection
            let new_column_expression = if filter_exec.projection().is_some() {
                let projection_columns = filter_exec.projection().as_ref().unwrap();
                let mut new_column_expression = vec![];
                for index in projection_columns.iter() {
                    new_column_expression.push(column_expression[*index].clone());
                }
                new_column_expression
            } else {
                column_expression
            };
            return (
                upstream_row_expression,
                new_column_expression,
                table_columns,
            );
        }
        panic!("Expected FilterExec");
    }
}

impl ExecutionPlan for SymEvalFilterExec {
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
        panic!("SymEvalFilterExec does not support with_new_children");
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
        "SymEvalFilterExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
