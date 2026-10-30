/// Provides functionality to wrap NestedLoopJoinExec nodes with symbolic nodes for
/// building symbolic expressions that represent nested loop join operations.
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
use datafusion::physical_plan::common::collect;
use datafusion::physical_plan::joins::NestedLoopJoinExec;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion_common::JoinSide;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicOperator;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::conjunct_exprs;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::match_and_handle_expr;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;

/// Symbolic node wrapper for DataFusion NestedLoopJoinExec.
/// Handles translation of join conditions into symbolic form and maintains
/// the symbolic state during execution.
#[derive(Debug)]
pub struct SymEvalNestedLoopJoinExec {
    child: Arc<dyn ExecutionPlan>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymEvalNestedLoopJoinExec {
    /// Creates a new SymEvalNestedLoopJoinExec node.
    ///
    /// # Arguments
    /// * `child` - The NestedLoopJoinExec node to wrap
    pub fn new(child: Arc<dyn ExecutionPlan>) -> Self {
        Self {
            child,
            row_expression: Arc::new(Mutex::new(SymbolicExpr::Literal(SymbolicScalarValue::Null))),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl DisplayAs for SymEvalNestedLoopJoinExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymEvalNestedLoopJoinExec")
            }
            DisplayFormatType::Verbose => {
                write!(
                    f,
                    "SymEvalNestedLoopJoinExec: row_expressions={:?}",
                    self.build_row_and_column_expressions()
                )
            }
        }
    }
}

impl SymbolicNode for SymEvalNestedLoopJoinExec {
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
        if let Some(nested_loop_join_exec) =
            self.child.as_any().downcast_ref::<NestedLoopJoinExec>()
        {
            // First get the symbolic expressions from both sides of the join
            let left_upstream_symbolic_node = nested_loop_join_exec.left();
            let right_upstream_symbolic_node = nested_loop_join_exec.right();
            let (left_row_expression, left_column_expression, left_table_columns) =
                get_row_and_column_expressions_from_node(left_upstream_symbolic_node);
            let (right_row_expression, right_column_expression, right_table_columns) =
                get_row_and_column_expressions_from_node(right_upstream_symbolic_node);

            // Merge table columns from both sides
            let mut table_columns = left_table_columns;
            for (table_name, columns) in right_table_columns {
                table_columns.insert(table_name, columns);
            }

            // Build symbolic expression for join filter (vacuous true if no filter)
            let symbolic_expr = if let Some(filter) = nested_loop_join_exec.filter() {
                let join_expr = filter.expression();
                let mut join_filter_sym_vars: Vec<SymbolicExpr> = vec![];
                for column_index in filter.column_indices() {
                    match column_index.side {
                        JoinSide::Left => join_filter_sym_vars
                            .push(left_column_expression[column_index.index].clone()),
                        JoinSide::Right => join_filter_sym_vars
                            .push(right_column_expression[column_index.index].clone()),
                        _ => panic!("Expected Left or Right"),
                    }
                }
                match_and_handle_expr(join_expr.as_ref(), &join_filter_sym_vars)
            } else {
                SymbolicExpr::Literal(SymbolicScalarValue::Boolean(Some(true)))
            };

            // We need to extract the row expressions from the left and right symbolic nodes
            // so that we can conjunct them with the join filter to get the final row symbolic expression
            let left_row_expression = match left_row_expression {
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
            let right_row_expression = match right_row_expression {
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

            // Conjunct the left and right row expressions with the join filter to get the final row symbolic
            // expression
            let left_row_expression = conjunct_exprs(&left_row_expression);
            let right_row_expression = conjunct_exprs(&right_row_expression);
            let final_row_expression = SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: match (left_row_expression, right_row_expression) {
                    (None, None) => Some(vec![symbolic_expr]),
                    (Some(left_row_expression), None) => Some(vec![SymbolicExpr::BinaryExpr {
                        left: Box::new(left_row_expression),
                        op: SymbolicOperator::And,
                        right: Box::new(symbolic_expr),
                    }]),
                    (None, Some(right_constraints)) => Some(vec![SymbolicExpr::BinaryExpr {
                        left: Box::new(right_constraints),
                        op: SymbolicOperator::And,
                        right: Box::new(symbolic_expr),
                    }]),
                    (Some(left_constraints), Some(right_constraints)) => {
                        Some(vec![SymbolicExpr::BinaryExpr {
                            left: Box::new(left_constraints),
                            op: SymbolicOperator::And,
                            right: Box::new(SymbolicExpr::BinaryExpr {
                                left: Box::new(right_constraints),
                                op: SymbolicOperator::And,
                                right: Box::new(symbolic_expr),
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
                if let Some(projection) = nested_loop_join_exec.projection() {
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
        panic!("Expected NestedLoopJoinExec");
    }
}

impl ExecutionPlan for SymEvalNestedLoopJoinExec {
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
        panic!("SymEvalNestedLoopJoinExec does not support with_new_children");
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.child.metrics()
    }

    fn execute(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        // We need this workaround since otherwise the left side of the join is lazily evaluated
        if let Some(nested_loop) = self.child.as_any().downcast_ref::<NestedLoopJoinExec>() {
            // Execute and collect the left side immediately
            let left_stream = nested_loop
                .left()
                .execute(partition, Arc::clone(&context))?;
            let _left_batches = futures::executor::block_on(collect(left_stream))?;
        } else {
            panic!("Expected NestedLoopJoinExec");
        }

        // Use the default implementation from SymbolicNode
        self.execute_symbolic(partition, context)
    }

    fn name(&self) -> &str {
        "SymEvalNestedLoopJoinExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.child.partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.child.properties()
    }
}
