/// Provides functionality to wrap UnionExec nodes with a symbolic node that
/// combines symbolic state from all inputs.
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
use datafusion::physical_plan::union::UnionExec;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::sym_expr::Operator as SymbolicOperator;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

/// Symbolic node wrapper for DataFusion UnionExec.
/// Combines inputs by OR-ing row constraints and reusing column expressions.
#[derive(Debug)]
pub struct SymEvalUnionExec {
    child: Arc<dyn ExecutionPlan>,
    row_expressions: Arc<Mutex<SymbolicExpr>>,
    column_expressions: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalUnionExec {
    /// Creates a new SymEvalUnionExec node.
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expressions: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expressions: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalUnionExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalUnionExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalUnionExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalUnionExec {
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
        // Sanity: ensure child is UnionExec
        let _ = self
            .child
            .as_any()
            .downcast_ref::<UnionExec>()
            .expect("Expected UnionExec as child of SymEvalUnionExec");

        let mut combined_row: Option<SymbolicExpr> = None;
        let mut chosen_columns: Option<Vec<SymbolicExpr>> = None;
        let mut merged_table_columns: HashMap<String, Vec<SQLColumn>> = HashMap::new();

        for input in self.child.children() {
            let (rows, cols, tables) = get_row_and_column_expressions_from_node(input);

            // Choose column expressions from the first input (schemas must match for union)
            if chosen_columns.is_none() {
                chosen_columns = Some(cols);
            }

            // Merge table columns (prefer first occurrence on conflicts)
            for (k, v) in tables.into_iter() {
                merged_table_columns.entry(k).or_insert(v);
            }

            // OR all row constraints from all inputs into a single expression
            combined_row = Some(match combined_row {
                None => rows,
                Some(acc) => SymbolicExpr::BinaryExpr {
                    left: Box::new(acc),
                    op: SymbolicOperator::Or,
                    right: Box::new(rows),
                },
            });
        }
        let row_expr = combined_row
            .map(|e| SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: Some(vec![e]),
            })
            .unwrap_or(SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: None,
            });
        let col_vec = chosen_columns.unwrap_or_default();
        (row_expr, col_vec, merged_table_columns)
    }
}

impl ExecutionPlan for SymEvalUnionExec {
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
        panic!("SymEvalUnionExec does not support with_new_children");
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
        "SymEvalUnionExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
