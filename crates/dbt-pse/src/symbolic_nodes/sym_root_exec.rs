/// Provides functionality for the root symbolic execution node that holds the complete
/// symbolic expression for the entire plan. This node is responsible for resolving holes
/// in the symbolic expressions with concrete values during streaming.
use std::any::Any;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use arrow::datatypes::Schema;
use arrow::datatypes::SchemaRef;
use datafusion::error::Result;
use datafusion::execution::context::TaskContext;
use datafusion::physical_plan::DisplayAs;
use datafusion::physical_plan::DisplayFormatType;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::PlanProperties;
use datafusion::physical_plan::SendableRecordBatchStream;
use datafusion::physical_plan::Statistics;
use datafusion::physical_plan::empty::EmptyExec;
use datafusion::physical_plan::metrics::MetricsSet;
use datafusion_common::arrow::array::ArrayRef;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::array_to_literal;
use crate::symbolic_nodes::contains_hole;
use crate::symbolic_nodes::get_row_and_column_expressions_from_node;
use crate::symbolic_nodes::resolve_holes;
use crate::symbolic_nodes::sym_nodes::SymbolicNode;
use crate::symbolic_nodes::translate_literal_expr;

/// Symbolic root node that maintains the complete symbolic expression for the entire plan.
/// Unlike other symbolic nodes, this node can have its child replaced and can resolve holes
/// in the symbolic expressions with concrete values during streaming.
#[derive(Debug)]
pub struct SymRootExec {
    child: Arc<Mutex<Arc<dyn ExecutionPlan>>>,
    row_expression: Arc<Mutex<SymbolicExpr>>,
    column_expression: Arc<Mutex<Vec<SymbolicExpr>>>,
    table_columns: Arc<Mutex<HashMap<String, Vec<SQLColumn>>>>,
}

impl SymRootExec {
    /// Creates a new SymRootExec node.
    ///
    /// # Arguments
    /// * `child` - The execution plan node to wrap, wrapped in a mutex to allow replacement
    pub fn new() -> Self {
        Self {
            child: Arc::new(Mutex::new(Arc::new(EmptyExec::new(SchemaRef::new(
                Schema::empty(),
            ))))),
            row_expression: Arc::new(Mutex::new(SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: Some(vec![SymbolicExpr::Literal(SymbolicScalarValue::Boolean(
                    Some(true),
                ))]),
            })),
            column_expression: Arc::new(Mutex::new(vec![])),
            table_columns: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Replaces the child execution plan.
    /// This is needed since the root node is created before the complete plan is built.
    ///
    /// # Arguments
    /// * `child` - The new execution plan to use
    pub fn replace_child(&self, child: Arc<dyn ExecutionPlan>) {
        let mut guard = self.child.lock().unwrap();
        *guard = child;
    }

    /// Resolves holes in the symbolic expressions with concrete values.
    /// This is called during streaming when we have concrete values for scalar functions.
    ///
    /// # Arguments
    /// * `expr_id` - The ID of the hole to resolve
    /// * `function_value` - The concrete value to use for the hole
    pub fn resolve_holes(&self, expr_id: usize, function_value: ArrayRef) {
        if function_value.is_empty() {
            return;
        }
        // First translate the concrete value into a symbolic literal
        let value = translate_literal_expr(&array_to_literal(&function_value));
        // Only proceed if we have row expressions and the value is not null
        let row_expr = self.get_row_expression();
        if value != SymbolicExpr::Literal(SymbolicScalarValue::Null) {
            // Extract the constraints from the row expression
            let path_constraints = match row_expr {
                SymbolicExpr::GenericRow { constraints, .. } => {
                    constraints.expect("Constraints should be Some")
                }
                t => panic!(
                    "Expected GenericRow expression: {:?} for plan node: {:?}",
                    t, self
                ),
            };

            // Replace all holes with the concrete value and update the row expression
            let path_constraints = path_constraints
                .iter()
                .map(|c| {
                    if contains_hole(c) {
                        resolve_holes(expr_id, value.clone(), c.clone())
                    } else {
                        c.clone()
                    }
                })
                .collect();
            self.update_row_expression(SymbolicExpr::GenericRow {
                table: "".to_string(),
                constraints: Some(path_constraints),
            });
        }
    }
}

impl DisplayAs for SymRootExec {
    fn fmt_as(&self, t: DisplayFormatType, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match t {
            DisplayFormatType::Default | DisplayFormatType::TreeRender => {
                write!(f, "SymRootExec")
            }
            DisplayFormatType::Verbose => {
                write!(f, "SymRootExec: constraints={:?}", self.row_expression)
            }
        }
    }
}

impl SymbolicNode for SymRootExec {
    fn get_child(&self) -> &Arc<dyn ExecutionPlan> {
        let child = self.child.lock().unwrap().clone();
        Box::leak(Box::new(child))
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
        // Since this is the root node, we just get the expressions from our child
        let upstream_symbolic_node = self.get_child();
        get_row_and_column_expressions_from_node(upstream_symbolic_node)
    }
}

impl ExecutionPlan for SymRootExec {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.get_child().schema()
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        vec![&self.get_child()]
    }

    fn with_new_children(
        self: Arc<Self>,
        _: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        panic!("SymRootExec does not support with_new_children");
    }

    fn metrics(&self) -> Option<MetricsSet> {
        self.get_child().metrics()
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
        "SymRootExec"
    }

    fn partition_statistics(&self, partition: Option<usize>) -> Result<Statistics> {
        self.get_child().partition_statistics(partition)
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        self.get_child().properties()
    }
}
