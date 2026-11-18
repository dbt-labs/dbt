/// Symbolic wrapper for DataFusion's BoundedWindowAggExec.
/// Minimal support for test generation: pass through row constraints and table metadata,
/// and append generic symbolic columns for each window expression output (e.g., row_number()).
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
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion::physical_plan::windows::BoundedWindowAggExec;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicZ3Type;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

#[derive(Debug)]
pub struct SymEvalBoundedWindowAggExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalBoundedWindowAggExec {
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(
                crate::symbolic_nodes::SymbolicScalarValue::Null,
            ))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalBoundedWindowAggExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalBoundedWindowAggExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalBoundedWindowAggExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalBoundedWindowAggExec {
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
        if self
            .child
            .as_any()
            .downcast_ref::<BoundedWindowAggExec>()
            .is_some()
        {
            // Get child's symbolic state
            let input = self.child.children()[0].clone();
            let (row_expr, col_exprs, table_cols) =
                get_row_and_column_expressions_from_node(&input);

            // Start with child's columns, then append a generic variable for each window expression output
            let mut new_cols = col_exprs;
            let base_idx = new_cols.len();

            let schema = self.child.schema();
            let fields_len = schema.fields().len();
            let added_cols = fields_len.saturating_sub(base_idx);

            // Choose a table name for derived columns: prefer single-table name if available, else "derived"
            let derived_table_name = if table_cols.len() == 1 {
                table_cols.keys().next().cloned().unwrap()
            } else {
                "derived".to_string()
            };

            for i in 0..added_cols {
                let field = schema.field(base_idx + i);
                let z3_ty = SymbolicZ3Type::from_column_type(field.data_type().clone());
                new_cols.push(SymbolicExpr::generic_var(
                    derived_table_name.clone(),
                    base_idx + i,
                    field.data_type().clone(),
                    z3_ty,
                    field.name().to_string(),
                ));
            }

            return (row_expr, new_cols, table_cols);
        }
        panic!("Expected BoundedWindowAggExec");
    }
}

impl ExecutionPlan for SymEvalBoundedWindowAggExec {
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
        panic!("SymEvalBoundedWindowAggExec does not support with_new_children");
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        self.execute_symbolic(partition, context)
    }

    fn name(&self) -> &str {
        "SymEvalBoundedWindowAggExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
