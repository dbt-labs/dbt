/// Symbolic node wrapper for DataFusion CrossJoinExec.
/// Propagates symbolic constraints from both sides by conjunction and concatenates column symbols.
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
use datafusion::physical_plan::joins::CrossJoinExec;
use datafusion::physical_plan::metrics::MetricsSet;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicOperator;
use crate::symbolic_nodes::conjunct_exprs;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

#[derive(Debug)]
pub struct SymCrossJoinExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymCrossJoinExec {
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

impl DisplayAs for SymCrossJoinExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymCrossJoinExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymCrossJoinExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymCrossJoinExec {
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
        if let Some(cross) = self.child.as_any().downcast_ref::<CrossJoinExec>() {
            let left = cross.left();
            let right = cross.right();

            let (left_row, left_cols, left_tables) = get_row_and_column_expressions_from_node(left);
            let (right_row, right_cols, right_tables) =
                get_row_and_column_expressions_from_node(right);

            // Merge table columns
            let mut table_columns = left_tables;
            for (t, cols) in right_tables {
                table_columns.insert(t, cols);
            }

            // Extract constraints from both sides and conjunct
            let left_predicates = match left_row {
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: Some(cs),
                } => cs,
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: None,
                } => vec![],
                _ => vec![],
            };
            let right_predicates = match right_row {
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: Some(cs),
                } => cs,
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: None,
                } => vec![],
                _ => vec![],
            };

            let left_conj = conjunct_exprs(&left_predicates);
            let right_conj = conjunct_exprs(&right_predicates);

            let row_constraints = match (left_conj, right_conj) {
                (None, None) => None,
                (Some(l), None) => Some(vec![l]),
                (None, Some(r)) => Some(vec![r]),
                (Some(l), Some(r)) => Some(vec![SymbolicExpr::BinaryExpr {
                    left: Box::new(l),
                    op: SymbolicOperator::And,
                    right: Box::new(r),
                }]),
            };

            let final_row = SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: row_constraints,
            };

            // Columns: left then right, cross join preserves all
            let mut final_cols = Vec::with_capacity(left_cols.len() + right_cols.len());
            final_cols.extend(left_cols);
            final_cols.extend(right_cols);

            return (final_row, final_cols, table_columns);
        }
        panic!("Expected CrossJoinExec");
    }
}

impl ExecutionPlan for SymCrossJoinExec {
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
        panic!("SymCrossJoinExec does not support with_new_children");
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
        "SymCrossJoinExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
