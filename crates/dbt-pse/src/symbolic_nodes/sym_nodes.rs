/// Provides functionality for symbolic node traits and callback expressions.
/// This module defines the common behavior for all symbolic nodes and provides
/// a way to inject concrete values into symbolic expressions during streaming.
use std::any::Any;
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::thread_local;
use std::time::Instant;

use crate::symbolic_nodes::SQLColumn;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::sym_root_exec::SymRootExec;
use datafusion::arrow::record_batch::RecordBatch;
use datafusion::error::Result;
use datafusion::execution::context::TaskContext;
use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_plan::ColumnarValue;
use datafusion::physical_plan::ExecutionPlan;

use datafusion::physical_plan::SendableRecordBatchStream;
use std::fmt::Display;
use std::hash::{Hash, Hasher};

use arrow::datatypes::{DataType, Schema};

// Accumulated time (ms) spent in build_row_and_column_expressions across all symbolic nodes during one execution. Used for ConcolicProfile::collect_constraints_ms.
static COLLECT_CONSTRAINTS_MS: AtomicU64 = AtomicU64::new(0);

/// Resets the collect-constraints accumulator (call before executing the symbolic plan).
pub fn reset_collect_constraints_ms() {
    COLLECT_CONSTRAINTS_MS.store(0, Ordering::Relaxed);
}

/// Takes the accumulated collect-constraints ms and resets the accumulator (call after execution).
pub fn take_collect_constraints_ms() -> u64 {
    COLLECT_CONSTRAINTS_MS.swap(0, Ordering::Relaxed)
}

/// Adds milliseconds to the collect-constraints accumulator (called from execute_symbolic).
fn add_collect_constraints_ms(ms: u64) {
    COLLECT_CONSTRAINTS_MS.fetch_add(ms, Ordering::Relaxed);
}

// Simple thread-local tracer for indentation-based debug logging of symbolic traversal
thread_local! {
    static SYM_TRACE_DEPTH: Cell<usize> = const { Cell::new(0) };
}

#[inline]
fn sym_trace_enabled() -> bool {
    match std::env::var("SDF_SYM_TRACE") {
        Ok(v) => {
            let v_lower = v.to_ascii_lowercase();
            v == "1" || v_lower == "true" || v_lower == "yes"
        }
        Err(_) => false,
    }
}

pub struct SymTraceGuard {
    enabled: bool,
    node_id: String,
    node_name: String,
}

impl SymTraceGuard {
    fn enter(node_name: &str, node_id: String) -> Self {
        let enabled = sym_trace_enabled();
        if enabled {
            SYM_TRACE_DEPTH.with(|d| {
                let depth = d.get();
                let indent = "  ".repeat(depth);
                eprintln!(
                    "{}[T:{:?}] [N:{}] > {}",
                    indent,
                    thread::current().id(),
                    node_id,
                    node_name
                );
                d.set(depth + 1);
            });
        }
        Self {
            enabled,
            node_id,
            node_name: node_name.to_string(),
        }
    }
}

impl Drop for SymTraceGuard {
    fn drop(&mut self) {
        if !self.enabled {
            return;
        }
        SYM_TRACE_DEPTH.with(|d| {
            let depth = d.get().saturating_sub(1);
            d.set(depth);
            let indent = "  ".repeat(depth);
            eprintln!(
                "{}[T:{:?}] [N:{}] < {}{}",
                indent,
                thread::current().id(),
                self.node_id,
                self.node_name,
                if thread::panicking() { " (panic)" } else { "" }
            );
        });
    }
}

/// Enter the symbolic trace with the given node name and id and return a guard
/// that will log the exit when dropped.
pub fn sym_trace_enter(node_name: &str, node_id: String) -> SymTraceGuard {
    SymTraceGuard::enter(node_name, node_id)
}

/// A physical expression that evaluates another physical expression and injects
/// its concrete value into a symbolic expression during streaming.
pub struct CallbackPhysicalExpr {
    /// Unique identifier for matching holes in symbolic expressions
    expr_id: usize,
    /// The physical expression to evaluate
    physical_expr: Arc<dyn PhysicalExpr>,
    /// Reference to the root node for injecting values
    root_plan_node: Option<Arc<dyn ExecutionPlan>>,
}

impl CallbackPhysicalExpr {
    /// Creates a new CallbackPhysicalExpr.
    ///
    /// # Arguments
    /// * `expr_id` - Unique identifier for matching holes
    /// * `physical_expr` - The expression to evaluate
    /// * `root_plan_node` - Optional reference to the root node
    pub fn new(
        expr_id: usize,
        physical_expr: Arc<dyn PhysicalExpr>,
        root_plan_node: Option<Arc<dyn ExecutionPlan>>,
    ) -> Self {
        Self {
            expr_id,
            physical_expr,
            root_plan_node,
        }
    }

    /// Gets the wrapped physical expression.
    pub fn get_physical_expr(&self) -> &Arc<dyn PhysicalExpr> {
        &self.physical_expr
    }

    /// Gets the unique identifier for this callback.
    pub fn get_expr_id(&self) -> usize {
        self.expr_id
    }

    /// Gets the reference to the root node, if any.
    pub fn get_root_plan_node(&self) -> Option<&Arc<dyn ExecutionPlan>> {
        self.root_plan_node.as_ref()
    }
}

impl fmt::Debug for CallbackPhysicalExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Print only the expr_id and a concise representation of the inner expr,
        // explicitly omit the root plan node to avoid gigantic dumps.
        write!(
            f,
            "CallbackPhysicalExpr {{ expr_id: {}, physical_expr: {}, root_plan_node: <omitted> }}",
            self.expr_id, self.physical_expr
        )
    }
}

impl Hash for CallbackPhysicalExpr {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.physical_expr.hash(state);
    }
}

impl PartialEq for CallbackPhysicalExpr {
    fn eq(&self, other: &Self) -> bool {
        self.physical_expr.eq(&other.physical_expr)
    }
}

impl Eq for CallbackPhysicalExpr {}

impl Display for CallbackPhysicalExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "CallbackPhysicalExpr")
    }
}

impl PhysicalExpr for CallbackPhysicalExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn data_type(&self, input_schema: &Schema) -> Result<DataType> {
        self.physical_expr.data_type(input_schema)
    }

    fn nullable(&self, input_schema: &Schema) -> Result<bool> {
        self.physical_expr.nullable(input_schema)
    }

    fn evaluate(&self, batch: &RecordBatch) -> Result<ColumnarValue> {
        // First evaluate the wrapped expression
        let result = self.physical_expr.evaluate(batch)?;

        // If we have a root node, inject the result into the symbolic expression
        if let Some(root_plan_node) = self.get_root_plan_node()
            && let Some(sym_root_exec) = root_plan_node.as_any().downcast_ref::<SymRootExec>()
        {
            let callback_result = match result.clone().into_array(batch.num_rows()) {
                Ok(array) => array,
                Err(e) => {
                    return Err(e);
                }
            };
            sym_root_exec.resolve_holes(self.expr_id, callback_result);
        }
        Ok(result)
    }

    fn children(&self) -> Vec<&Arc<dyn PhysicalExpr>> {
        self.physical_expr.children()
    }

    fn with_new_children(
        self: Arc<Self>,
        children: Vec<Arc<dyn PhysicalExpr>>,
    ) -> Result<Arc<dyn PhysicalExpr>> {
        let new_inner = Arc::clone(&self.physical_expr).with_new_children(children)?;
        Ok(Arc::new(CallbackPhysicalExpr::new(
            self.expr_id,
            new_inner,
            self.root_plan_node.clone(),
        )))
    }

    fn fmt_sql(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.physical_expr.fmt_sql(f)
    }
}

/// Common trait for all symbolic execution nodes.
/// Provides functionality for maintaining symbolic expressions and table metadata
/// during plan execution.
pub trait SymbolicNode: ExecutionPlan {
    /// Gets the child execution plan.
    fn get_child(&self) -> &Arc<dyn ExecutionPlan>;

    /// Gets the current row symbolic expression.
    fn get_row_expression(&self) -> SymbolicExpr;

    /// Gets the current column symbolic expressions.
    fn get_column_expression(&self) -> Vec<SymbolicExpr>;

    /// Gets the current table column metadata.
    fn get_table_columns(&self) -> HashMap<String, Vec<SQLColumn>>;

    /// Builds the row and column symbolic expressions for this node.
    /// This is called during execution to update the symbolic state.
    fn build_row_and_column_expressions(
        &self,
    ) -> (
        SymbolicExpr,
        Vec<SymbolicExpr>,
        HashMap<String, Vec<SQLColumn>>,
    );

    // /// Prints the concrete Rust type of the symbolic node for debugging.
    // // #[allow(unused)]
    // // #[allow(clippy::dbg_macro)]
    // fn print_symbolic_node_type(&self) {
    //     let node_type = std::any::type_name_of_val(self);
    //     dbg!(node_type);
    // }
    /// Default implementation for executing a symbolic node.
    /// This ensures that child nodes are executed first and their symbolic
    /// state is properly propagated.
    fn execute_symbolic(
        &self,
        partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let node_id = format!("{:p}", self);
        let _trace = SymTraceGuard::enter(self.name(), node_id);
        // First execute child to ensure its constraints are updated
        let stream = self.get_child().execute(partition, context)?;

        // Now build our symbolic expressions based on updated child state
        let build_start = Instant::now();
        let (new_row_expression, new_column_expressions, table_columns) =
            self.build_row_and_column_expressions();
        add_collect_constraints_ms(build_start.elapsed().as_millis() as u64);

        // Update our symbolic state
        self.update_row_expression(new_row_expression);
        self.update_column_expression(new_column_expressions);
        self.update_table_columns(table_columns);

        Ok(stream)
    }

    /// Updates the row symbolic expression.
    fn update_row_expression(&self, new_row_expression: SymbolicExpr);

    /// Updates the column symbolic expressions.
    fn update_column_expression(&self, new_column_expressions: Vec<SymbolicExpr>);

    /// Updates the table column metadata.
    fn update_table_columns(&self, table_columns: HashMap<String, Vec<SQLColumn>>);
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::physical_expr::expressions::Column;
    use datafusion::physical_expr_common::physical_expr::fmt_sql;

    // DataFusion 53's ProjectionExec formats every projected expression as
    // SQL when it executes, to label its expression metrics.
    #[test]
    fn fmt_sql_is_the_wrapped_expressions() {
        let column: Arc<dyn PhysicalExpr> = Arc::new(Column::new("a", 0));
        let callback = CallbackPhysicalExpr::new(0, Arc::clone(&column), None);
        assert_eq!(
            fmt_sql(&callback).to_string(),
            fmt_sql(column.as_ref()).to_string()
        );
    }
}
