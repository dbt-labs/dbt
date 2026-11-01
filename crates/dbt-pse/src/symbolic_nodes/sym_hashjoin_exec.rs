/// Provides functionality to wrap HashJoinExec nodes with symbolic nodes for
/// building symbolic expressions that represent hash join operations.
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
use datafusion::physical_plan::joins::HashJoinExec;
use datafusion::physical_plan::metrics::MetricsSet;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicOperator;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::conjunct_exprs;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::match_and_handle_expr;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

/// Symbolic node wrapper for DataFusion HashJoinExec.
/// Handles translation of join conditions into symbolic form and maintains
/// the symbolic state during execution.
#[derive(Debug)]
pub struct SymHashJoinExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymHashJoinExec {
    /// Creates a new SymHashJoinExec node.
    ///
    /// # Arguments
    /// * `child` - The HashJoinExec node to wrap
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymHashJoinExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymHashJoinExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymHashJoinExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymHashJoinExec {
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
        if let Some(hash_join_exec) = self.child.as_any().downcast_ref::<HashJoinExec>() {
            // Get left & right upstream symbolic nodes
            let left_upstream_symbolic_node = hash_join_exec.left();
            let right_upstream_symbolic_node = hash_join_exec.right();
            let (left_row_expression, left_column_expression, left_table_columns) =
                get_row_and_column_expressions_from_node(left_upstream_symbolic_node);
            let (right_row_expression, right_column_expression, right_table_columns) =
                get_row_and_column_expressions_from_node(right_upstream_symbolic_node);

            // Merge table columns from both sides
            let mut table_columns = left_table_columns;
            for (table_name, columns) in right_table_columns {
                table_columns.insert(table_name, columns);
            }

            // Build symbolic expression for join condition
            let joining_column_pairs = hash_join_exec.on();
            let mut join_sym_expr = None;
            for (left_column_expr, right_column_expr) in joining_column_pairs {
                let left_sym_expr =
                    match_and_handle_expr(left_column_expr.as_ref(), &left_column_expression);
                let right_sym_expr =
                    match_and_handle_expr(right_column_expr.as_ref(), &right_column_expression);
                // HashJoins only support equality joins
                join_sym_expr = match join_sym_expr.clone() {
                    Some(join_sym_expr) => Some(SymbolicExpr::BinaryExpr {
                        left: Box::new(join_sym_expr),
                        op: SymbolicOperator::And,
                        right: Box::new(SymbolicExpr::BinaryExpr {
                            left: Box::new(left_sym_expr),
                            op: SymbolicOperator::Equal,
                            right: Box::new(right_sym_expr),
                        }),
                    }),
                    None => Some(SymbolicExpr::BinaryExpr {
                        left: Box::new(left_sym_expr),
                        op: SymbolicOperator::Equal,
                        right: Box::new(right_sym_expr),
                    }),
                };
            }
            let join_sym_expr = join_sym_expr.unwrap();

            // We need to extract the row expressions from the left and right symbolic nodes
            // so that we can conjunct them with the join condition to get the final row symbolic expression.
            let left_predicates = match left_row_expression {
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: Some(constraints),
                } => constraints,
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: None,
                } => vec![],
                _ => panic!("Expected GenericRow"),
            };
            let right_predicates = match right_row_expression {
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: Some(constraints),
                } => constraints,
                SymbolicExpr::GenericRow {
                    table: _,
                    constraints: None,
                } => vec![],
                t => panic!(
                    "Expected GenericRow: {:?} for plan node: {:?}",
                    t, hash_join_exec
                ),
            };

            // Combine constraints with join condition
            let left_row_expression = conjunct_exprs(&left_predicates);
            let right_row_expression = conjunct_exprs(&right_predicates);
            let final_row_expression = SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: match (left_row_expression, right_row_expression) {
                    (None, None) => Some(vec![join_sym_expr]),
                    (Some(left_row_expression), None) => Some(vec![SymbolicExpr::BinaryExpr {
                        left: Box::new(left_row_expression),
                        op: SymbolicOperator::And,
                        right: Box::new(join_sym_expr),
                    }]),
                    (None, Some(right_constraints)) => Some(vec![SymbolicExpr::BinaryExpr {
                        left: Box::new(right_constraints),
                        op: SymbolicOperator::And,
                        right: Box::new(join_sym_expr),
                    }]),
                    (Some(left_constraints), Some(right_constraints)) => {
                        Some(vec![SymbolicExpr::BinaryExpr {
                            left: Box::new(left_constraints),
                            op: SymbolicOperator::And,
                            right: Box::new(SymbolicExpr::BinaryExpr {
                                left: Box::new(right_constraints),
                                op: SymbolicOperator::And,
                                right: Box::new(join_sym_expr),
                            }),
                        }])
                    }
                },
            };

            // Now we need to compute the final column symbolic expression.
            // For this we first assume that the join is going to preserve all the columns from both sides
            // (the indexing is always ordered left to right)
            // Then we drop the columns that are not in the projection.
            let mut pre_join_column_expression = vec![];
            for column_expression in left_column_expression {
                pre_join_column_expression.push(column_expression.clone());
            }
            for column_expression in right_column_expression {
                pre_join_column_expression.push(column_expression.clone());
            }

            // If the join has a projection, we need to drop the columns that are not in the projection
            let final_column_expression: Vec<SymbolicExpr> =
                if let Some(projection) = hash_join_exec.projection.clone() {
                    let mut joined_column_expression: Vec<SymbolicExpr> = vec![];
                    for column_index in projection.iter() {
                        joined_column_expression
                            .push(pre_join_column_expression[*column_index].clone());
                    }
                    joined_column_expression
                } else {
                    pre_join_column_expression
                };

            return (final_row_expression, final_column_expression, table_columns);
        }
        panic!("Expected HashJoinExec");
    }
}

impl ExecutionPlan for SymHashJoinExec {
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
        panic!("SymHashJoinExec does not support with_new_children");
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
        "SymHashJoinExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
