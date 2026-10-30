/// Module for symbolic execution and test generation of DataFusion physical plans.
/// Provides functionality to wrap physical execution plans with symbolic nodes
/// for building symbolic expressions.
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

mod physical_expr_translater;
pub mod sym_expr;
pub mod utils;
use crate::coverage::transform_plan_expressions;
use crate::named_memory_exec::NamedMemoryExec;
use datafusion::arrow::datatypes::DataType;
use datafusion::physical_plan::coalesce_partitions::CoalescePartitionsExec;
use datafusion::physical_plan::repartition::RepartitionExec;
use datafusion::physical_plan::sorts::sort::SortExec;
use datafusion_catalog::memory::DataSourceExec;
use datafusion_common::error::Result;
pub use physical_expr_translater::match_and_handle_expr;
pub use physical_expr_translater::translate_aggregate_expr;
pub use physical_expr_translater::translate_literal_expr;
pub use sym_expr::AggregateFunction as SymbolicAggregateFunction;
pub use sym_expr::Expr as SymbolicExpr;
pub use sym_expr::Operator as SymbolicOperator;
pub use sym_expr::ScalarValue as SymbolicScalarValue;
pub use sym_expr::Z3Type as SymbolicZ3Type;
pub use sym_expr::add_constraint_to_row;
pub use utils::array_to_literal;
pub use utils::arrow_type_to_sql_type;
pub use utils::chop_rightmost_conjunct;
pub use utils::conjunct_exprs;
pub use utils::contains_gen_var;
use utils::contains_hole;
pub use utils::num_days_to_date;
pub use utils::num_timestamp_to_datetime;
pub use utils::num_to_time;
pub use utils::resolve_holes;
pub use utils::simplify_holes;
pub use utils::simplify_holes_relaxed;
pub use utils::symbolic_scalar_from_struct_timestamp_fields;

mod sym_aggregation_exec;
mod sym_bounded_window_exec;
mod sym_coalesce_partitions_exec;
mod sym_crossjoin_exec;
mod sym_empty_exec;
mod sym_filter_exec;
mod sym_hashjoin_exec;
mod sym_memory_exec;
mod sym_nestedloopjoin_exec;
mod sym_nodes;
mod sym_pass_through_exec;
mod sym_projection_exec;
mod sym_root_exec;
mod sym_union_exec;

use sym_nodes::CallbackPhysicalExpr;

use datafusion::physical_expr::PhysicalExpr;
use datafusion::physical_expr::ScalarFunctionExpr;
use datafusion::physical_expr::expressions::BinaryExpr;
use datafusion::physical_expr::expressions::CastExpr;
use datafusion::physical_expr::expressions::InListExpr;
use datafusion::physical_expr::expressions::LikeExpr;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::physical_plan::aggregates::AggregateExec;
use datafusion::physical_plan::empty::EmptyExec;
use datafusion::physical_plan::filter::FilterExec;
use datafusion::physical_plan::joins::CrossJoinExec;
use datafusion::physical_plan::joins::HashJoinExec;
use datafusion::physical_plan::joins::NestedLoopJoinExec;
use datafusion::physical_plan::projection::ProjectionExec;
use datafusion::physical_plan::union::UnionExec;
use datafusion::physical_plan::windows::BoundedWindowAggExec;
use datafusion_expr::Operator as DFOperator;

use crate::symbolic_nodes::sym_aggregation_exec::SymEvalAggregationExec;
use crate::symbolic_nodes::sym_coalesce_partitions_exec::SymEvalCoalescePartitionsExec;
use crate::symbolic_nodes::sym_crossjoin_exec::SymCrossJoinExec;
use crate::symbolic_nodes::sym_filter_exec::SymEvalFilterExec;
use crate::symbolic_nodes::sym_hashjoin_exec::SymHashJoinExec;
use crate::symbolic_nodes::sym_memory_exec::SymEvalMemoryExec;
use crate::symbolic_nodes::sym_nestedloopjoin_exec::SymEvalNestedLoopJoinExec;

use crate::symbolic_nodes::sym_bounded_window_exec::SymEvalBoundedWindowAggExec;
use crate::symbolic_nodes::sym_empty_exec::SymEvalEmptyExec;
pub use crate::symbolic_nodes::sym_nodes::{
    SymbolicNode, reset_collect_constraints_ms, take_collect_constraints_ms,
};
use crate::symbolic_nodes::sym_pass_through_exec::SymEvalPassThroughExec;
use crate::symbolic_nodes::sym_projection_exec::SymEvalProjectionExec;
use crate::symbolic_nodes::sym_root_exec::SymRootExec;
use crate::symbolic_nodes::sym_union_exec::SymEvalUnionExec;

/// Counts of plan node types (Projection, Filter, Aggregate, Join, Memory).
#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct NodeCounts {
    pub projection: u32,
    pub filter: u32,
    pub aggregate: u32,
    pub join: u32,
    pub memory: u32,
    pub holes: usize,
}

impl NodeCounts {
    /// Format as: `{"Projection": N, "Filter": N, "Aggregate": N, "Join": N, "Memory": N}`.
    pub fn to_json_string(&self) -> String {
        format!(
            r#"{{"Projection": {}, "Filter": {}, "Aggregate": {}, "Join": {}, "Memory": {}, "Holes": {}}}"#,
            self.projection, self.filter, self.aggregate, self.join, self.memory, self.holes
        )
    }
}

/// Recursively counts Projection, Filter, Aggregate, Join, and Memory nodes in a plan.
/// Works on both raw DataFusion plans and symbolic-wrapped plans.
pub fn count_plan_nodes(plan: &Arc<dyn ExecutionPlan>) -> NodeCounts {
    let mut counts = NodeCounts::default();
    count_plan_nodes_inner(plan, &mut counts);
    counts.holes = current_expr_counter();
    counts
}

fn count_plan_nodes_inner(plan: &Arc<dyn ExecutionPlan>, counts: &mut NodeCounts) {
    let any = plan.as_any();
    // Raw DataFusion types
    if any.is::<ProjectionExec>() {
        counts.projection += 1;
    } else if any.is::<FilterExec>() {
        counts.filter += 1;
    } else if any.is::<AggregateExec>() {
        counts.aggregate += 1;
    } else if any.is::<HashJoinExec>()
        || any.is::<NestedLoopJoinExec>()
        || any.is::<CrossJoinExec>()
    {
        counts.join += 1;
    } else if any.is::<NamedMemoryExec>() {
        counts.memory += 1;
    }

    for child in plan.children() {
        count_plan_nodes_inner(child, counts);
    }
}

// Add static counter for expr indices
// We need this to match the CallbackPhysicalExprs with their corresponding
// holes in the symbolic expression and resolve them during streaming.
static EXPR_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Returns the current value of the global expression counter.
pub fn current_expr_counter() -> usize {
    EXPR_COUNTER.load(Ordering::SeqCst)
}

/// Represents a column in a SQL table with type information.
/// The arrow_type helps us alias types into Z3 types. For example, we can
/// alias a timestamp to an integer in Z3 and back.
///
/// TODO: We might need to store the exact type specified in the input
/// SQL CTAS from SLT since Datafusion might cast them and update the table schema
/// before we have a chance to get the original type specified.
#[derive(Debug, Clone)]
pub struct SQLColumn {
    pub table_name: String,
    pub column_name: String,
    pub arrow_type: DataType,
}

/// Resets the global expression counter used for generating unique IDs.
pub fn reset_expr_counter() {
    EXPR_COUNTER.store(0, Ordering::SeqCst);
}

/// Extracts symbolic expressions and table metadata from a symbolic execution node.
///
/// # Arguments
/// * `plan` - The execution plan node to analyze
///
/// # Returns
/// A tuple containing:
/// * Row expressions representing row symbolic expressions
/// * Column expressions for column symbolic expressions
/// * Mapping of table names to their column definitions
///
/// # Panics
/// * If the plan node is not a symbolic execution node
pub fn get_row_and_column_expressions_from_node(
    plan: &Arc<dyn ExecutionPlan>,
) -> (
    SymbolicExpr,
    Vec<SymbolicExpr>,
    HashMap<String, Vec<SQLColumn>>,
) {
    // Try each possible symbolic node type
    let (constraints, sym_vars, table_columns) =
        if let Some(node) = plan.as_any().downcast_ref::<SymEvalProjectionExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalFilterExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalPassThroughExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan
            .as_any()
            .downcast_ref::<SymEvalCoalescePartitionsExec>()
        {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalNestedLoopJoinExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymHashJoinExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymCrossJoinExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalMemoryExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalAggregationExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalUnionExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalEmptyExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymEvalBoundedWindowAggExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else if let Some(node) = plan.as_any().downcast_ref::<SymRootExec>() {
            (
                node.get_row_expression(),
                node.get_column_expression(),
                node.get_table_columns(),
            )
        } else {
            panic!("Expected a symbolic node: {:?}", plan);
        };
    (constraints, sym_vars, table_columns)
}

/// Wraps physical expressions with callback handlers for scalar functions.
///
/// # Arguments
/// * `expr` - The physical expression to wrap
/// * `root_plan_node` - The root execution plan node
///
/// # Returns
/// A wrapped physical expression that handles scalar function evaluation
pub fn wrap_scalar_function_expr(
    expr: Arc<dyn PhysicalExpr>,
    root_plan_node: &Arc<dyn ExecutionPlan>,
) -> Arc<dyn PhysicalExpr> {
    if expr.as_any().downcast_ref::<ScalarFunctionExpr>().is_some()
        || expr.as_any().downcast_ref::<LikeExpr>().is_some()
    {
        // Wrap scalar function exprs in CallbackPhysicalExprs
        let expr_id = EXPR_COUNTER.fetch_add(1, Ordering::SeqCst);
        return Arc::new(CallbackPhysicalExpr::new(
            expr_id,
            expr,
            Some(root_plan_node.clone()),
        ));
    } else if let Some(binary_expr) = expr.as_any().downcast_ref::<BinaryExpr>() {
        return Arc::new(BinaryExpr::new(
            wrap_scalar_function_expr(binary_expr.left().clone(), root_plan_node),
            *binary_expr.op(),
            wrap_scalar_function_expr(binary_expr.right().clone(), root_plan_node),
        ));
    } else if let Some(cast_expr) = expr.as_any().downcast_ref::<CastExpr>() {
        return Arc::new(CastExpr::new(
            wrap_scalar_function_expr(cast_expr.expr().clone(), root_plan_node),
            cast_expr.cast_type().clone(),
            Some(cast_expr.cast_options().clone()),
        ));
    } else if let Some(in_list_expr) = expr.as_any().downcast_ref::<InListExpr>() {
        let left_expr = wrap_scalar_function_expr(in_list_expr.expr().clone(), root_plan_node);
        let right_exprs = in_list_expr
            .list()
            .iter()
            .map(|e| wrap_scalar_function_expr(e.clone(), root_plan_node))
            .collect::<Vec<_>>();

        // with_new_children keeps the static filter, and cannot fail for InListExpr.
        return Arc::clone(&expr)
            .with_new_children(std::iter::once(left_expr).chain(right_exprs).collect())
            .expect("InListExpr::with_new_children is infallible");
    }
    expr
}

/// Recursively traverse a physical expression tree and wrap all ScalarFunctionExpr
/// nodes with CallbackPhysicalExpr so we can capture their values during execution.
fn wrap_scalar_functions_deep(
    expr: Arc<dyn PhysicalExpr>,
    root_plan_node: &Arc<dyn ExecutionPlan>,
) -> Result<Arc<dyn PhysicalExpr>> {
    // 1) transform children first using generic expression API
    let new_children = expr
        .children()
        .into_iter()
        .map(|c| wrap_scalar_functions_deep(Arc::clone(c), root_plan_node))
        .collect::<Result<Vec<_>>>()?;
    let expr = expr.with_new_children(new_children)?;

    // 2) if this node is a scalar function or LIKE or StringConcat, wrap it with a callback
    if expr.as_any().downcast_ref::<ScalarFunctionExpr>().is_some()
        || expr.as_any().downcast_ref::<LikeExpr>().is_some()
        || (expr
            .as_any()
            .downcast_ref::<BinaryExpr>()
            .is_some_and(|b| *b.op() == DFOperator::StringConcat))
    {
        let expr_id = EXPR_COUNTER.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new(CallbackPhysicalExpr::new(
            expr_id,
            expr,
            Some(root_plan_node.clone()),
        )))
    } else {
        Ok(expr)
    }
}

/// Wraps ScalarFunctionExprs with CallbackPhysicalExprs. This is done since
/// the values of ScalarFunctionExprs are not available during plan execution and only
/// upon streaming. The CallbackPhysicalExprs are used to match the holes in the symbolic expression
/// and inject them with the concrete values of the ScalarFunctionExprs during streaming. This way
/// we don't have to provide the semantics for the ScalarFunctionExprs in Z3.
///
/// # Arguments
/// * `plan` - The execution plan node to wrap
/// * `root_plan_node` - The root execution plan node
///
/// # Returns
/// A wrapped execution plan with ScalarFunctionExprs wrapped with CallbackPhysicalExprs.
///
///
/// TODO: Check if ScalarFunctionExprs can appear in nodes other than FilterExec and ProjectionExec
/// nodes.
pub fn wrap_callback_physical_expr(
    plan: &Arc<dyn ExecutionPlan>,
    root_plan_node: &Arc<dyn ExecutionPlan>,
) -> Arc<dyn ExecutionPlan> {
    // Use the generic plan expression transformation; panic on unsupported nodes
    let mut transform_expr =
        |expr: Arc<dyn PhysicalExpr>| wrap_scalar_functions_deep(expr, root_plan_node);

    transform_plan_expressions(plan.clone(), &mut transform_expr)
        .expect("wrap_callback_physical_expr: unsupported plan node for expression transform")
}

/// Wraps a physical execution plan with symbolic execution nodes.
///
/// # Arguments
/// * `plan` - The physical execution plan to wrap
///
/// # Returns
/// A physical plan with physical nodes wrapped with symbolic nodes and ScalarFunctionExprs
/// wrapped with CallbackPhysicalExprs. The root plan node (SymRootExec) will hold the row and column
/// symbolic expressions for the entire plan after the plan is executed.
pub fn wrap_symbolic_plan(plan: &Arc<dyn ExecutionPlan>) -> Arc<dyn ExecutionPlan> {
    // Reset the expr counter to ensure that each expr has a unique id
    reset_expr_counter();

    // Create a root plan node to hold the symbolic expression
    let root_plan_node: Arc<dyn ExecutionPlan> = Arc::new(SymRootExec::new());

    // Wrap the physical plan nodes with symbolic nodes
    let plan = wrap_symbolic_plan_inner(plan, &root_plan_node);

    // Replace the child of the root plan node with the wrapped plan. This work around to ensure that
    // the root plan node can be passed to the callback physical exprs.
    if let Some(root_plan_node) = root_plan_node.as_any().downcast_ref::<SymRootExec>() {
        root_plan_node.replace_child(plan);
    } else {
        panic!("Root plan node is not a SymRootExec");
    }
    root_plan_node
}

/// Internal helper to recursively wrap plan nodes with symbolic execution.
///
/// # Arguments
/// * `plan` - The execution plan node to wrap
/// * `root_plan_node` - The root execution plan node
///
/// # Returns
/// A wrapped execution plan node with symbolic execution capabilities
fn wrap_symbolic_plan_inner(
    plan: &Arc<dyn ExecutionPlan>,
    root_plan_node: &Arc<dyn ExecutionPlan>,
) -> Arc<dyn ExecutionPlan> {
    // First, transform expressions on the pure physical subtree so we don't
    // traverse into symbolic nodes with the transformer.
    let transformed_physical = wrap_callback_physical_expr(plan, root_plan_node);

    // Now wrap children of the transformed plan with symbolic nodes
    let children = transformed_physical
        .children()
        .into_iter()
        .map(|child| wrap_symbolic_plan_inner(child, root_plan_node))
        .collect::<Vec<_>>();
    let plan = transformed_physical
        .clone()
        .with_new_children(children)
        .unwrap_or_else(|_| transformed_physical.clone());

    // Wrap physical plan nodes with symbolic nodes
    match plan.as_any() {
        // terminal data source nodes map to themselves
        t if t.is::<DataSourceExec>() => plan,

        // wrapped nodes
        t if t.is::<FilterExec>() => Arc::new(SymEvalFilterExec::new(plan)),
        t if t.is::<ProjectionExec>() => Arc::new(SymEvalProjectionExec::new(plan)),
        t if t.is::<UnionExec>() => Arc::new(SymEvalUnionExec::new(plan)),
        t if t.is::<RepartitionExec>() => Arc::new(SymEvalPassThroughExec::new(plan)),
        t if t.is::<SortExec>() => Arc::new(SymEvalPassThroughExec::new(plan)),
        t if t.is::<CoalescePartitionsExec>() => Arc::new(SymEvalCoalescePartitionsExec::new(plan)),
        t if t.is::<NestedLoopJoinExec>() => Arc::new(SymEvalNestedLoopJoinExec::new(plan)),
        t if t.is::<HashJoinExec>() => Arc::new(SymHashJoinExec::new(plan)),
        t if t.is::<CrossJoinExec>() => Arc::new(SymCrossJoinExec::new(plan)),
        t if t.is::<AggregateExec>() => Arc::new(SymEvalAggregationExec::new(plan)),
        t if t.is::<EmptyExec>() => Arc::new(SymEvalEmptyExec::new(plan)),
        t if t.is::<BoundedWindowAggExec>() => Arc::new(SymEvalBoundedWindowAggExec::new(plan)),
        t if t.is::<NamedMemoryExec>() => Arc::new(SymEvalMemoryExec::new(plan)),
        _ => panic!("Unsupported plan node: {:?}", plan),
    }
}
