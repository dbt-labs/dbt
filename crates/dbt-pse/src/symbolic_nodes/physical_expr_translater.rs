use datafusion::physical_expr::aggregate::AggregateFunctionExpr;
use datafusion::physical_plan::PhysicalExpr;
use datafusion::physical_plan::expressions::BinaryExpr;
use datafusion::physical_plan::expressions::CaseExpr;
use datafusion::physical_plan::expressions::CastExpr;
use datafusion::physical_plan::expressions::Column;
use datafusion::physical_plan::expressions::InListExpr;
use datafusion::physical_plan::expressions::IsNotNullExpr;
use datafusion::physical_plan::expressions::IsNullExpr;
use datafusion::physical_plan::expressions::Literal;
use datafusion::physical_plan::expressions::NotExpr;
use datafusion_common::ScalarValue;
/// Module for translating physical expressions into symbolic expressions.
/// This module provides functionality to convert DataFusion physical expressions
/// into their symbolic equivalents for symbolic execution and test generation.
use datafusion_common::arrow::datatypes::DataType;
use datafusion_expr::Operator;

use crate::symbolic_nodes::CallbackPhysicalExpr;
use crate::symbolic_nodes::SymbolicAggregateFunction;
use crate::symbolic_nodes::SymbolicExpr;
use crate::symbolic_nodes::SymbolicOperator;
use crate::symbolic_nodes::SymbolicScalarValue;
use crate::symbolic_nodes::array_to_literal;

use datafusion_common::arrow::array::FixedSizeListArray;

use arrow::array::Array;

/// Translates a DataFusion Literal expression into a symbolic expression.
///
/// This function handles various data types including:
/// - Boolean
/// - Integer types (i8, i16, i32, i64, u8, u16, u32, u64)
/// - Floating point types (f32, f64)
/// - Decimal128
/// - UTF-8 strings
/// - Timestamps and Dates (converted to i64)
/// - Fixed size lists (specifically for timestamp arrays)
///
/// # Arguments
/// * `expr` - The literal expression to translate
///
/// # Panics
/// * If the data type is unsupported
/// * If the scalar value doesn't match the expected type
pub fn translate_literal_expr(expr: &Literal) -> SymbolicExpr {
    let value = expr.value();
    match value.data_type() {
        DataType::Null => SymbolicExpr::Literal(SymbolicScalarValue::Null),
        DataType::Boolean => {
            let value = match value {
                ScalarValue::Boolean(Some(b)) => Some(*b),
                ScalarValue::Boolean(None) => None,
                _ => panic!("Expected ScalarValue::Boolean, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Boolean(value))
        }
        DataType::Int8 => {
            let value = match value {
                ScalarValue::Int8(Some(i)) => Some(*i),
                ScalarValue::Int8(None) => None,
                _ => panic!("Expected ScalarValue::Int8, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Int8(value))
        }
        DataType::Int16 => {
            let value = match value {
                ScalarValue::Int16(Some(i)) => Some(*i),
                ScalarValue::Int16(None) => None,
                _ => panic!("Expected ScalarValue::Int16, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Int16(value))
        }
        DataType::Int32 => {
            let value = match value {
                ScalarValue::Int32(Some(i)) => Some(*i),
                ScalarValue::Int32(None) => None,
                _ => panic!("Expected ScalarValue::Int32, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Int32(value))
        }
        DataType::Int64 => {
            let value = match value {
                ScalarValue::Int64(Some(i)) => Some(*i),
                ScalarValue::Int64(None) => None,
                _ => panic!("Expected ScalarValue::Int64, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Int64(value))
        }
        DataType::UInt8 => {
            let value = match value {
                ScalarValue::UInt8(Some(i)) => Some(*i),
                ScalarValue::UInt8(None) => None,
                _ => panic!("Expected ScalarValue::UInt8, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::UInt8(value))
        }
        DataType::UInt16 => {
            let value = match value {
                ScalarValue::UInt16(Some(i)) => Some(*i),
                ScalarValue::UInt16(None) => None,
                _ => panic!("Expected ScalarValue::UInt16, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::UInt16(value))
        }
        DataType::UInt32 => {
            let value = match value {
                ScalarValue::UInt32(Some(i)) => Some(*i),
                ScalarValue::UInt32(None) => None,
                _ => panic!("Expected ScalarValue::UInt32, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::UInt32(value))
        }
        DataType::UInt64 => {
            let value = match value {
                ScalarValue::UInt64(Some(i)) => Some(*i),
                ScalarValue::UInt64(None) => None,
                _ => panic!("Expected ScalarValue::UInt64, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::UInt64(value))
        }
        DataType::Float32 => {
            let value = match value {
                ScalarValue::Float32(Some(f)) => Some(*f),
                ScalarValue::Float32(None) => None,
                _ => panic!("Expected ScalarValue::Float32, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Float32(value))
        }
        DataType::Float64 => {
            let value = match value {
                ScalarValue::Float64(Some(f)) => Some(*f),
                ScalarValue::Float64(None) => None,
                _ => panic!("Expected ScalarValue::Float64, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Float64(value))
        }
        DataType::Decimal128(_, _) => {
            let value = match value {
                ScalarValue::Decimal128(Some(d), precision, scale) => {
                    Some((*d, *precision, *scale))
                }
                ScalarValue::Decimal128(None, _, _) => None,
                _ => panic!("Expected ScalarValue::Decimal128, got {value:?}"),
            };
            match value {
                Some((d, precision, scale)) => SymbolicExpr::Literal(
                    SymbolicScalarValue::Decimal128(Some(d), precision, scale),
                ),
                None => SymbolicExpr::Literal(SymbolicScalarValue::Null),
            }
        }
        DataType::Utf8 | DataType::Utf8View | DataType::LargeUtf8 => {
            let value = match value {
                ScalarValue::Utf8(value)
                | ScalarValue::Utf8View(value)
                | ScalarValue::LargeUtf8(value) => value.clone(),
                _ => panic!("Expected ScalarValue::Utf8 or similar, got {value:?}"),
            };
            SymbolicExpr::Literal(SymbolicScalarValue::Utf8(value))
        }
        DataType::Timestamp(_, _)
        | DataType::Time64(_)
        | DataType::Date64
        | DataType::Date32
        | DataType::Time32(_) => {
            translate_literal_expr(&Literal::new(value.cast_to(&DataType::Int64).unwrap()))
        }
        // Timestamps are stored in FixedSizeList(FieldRef, 1)
        DataType::FixedSizeList(_, size) => match size {
            1 => {
                let array = expr.value().to_array().unwrap();
                let array = array.as_any().downcast_ref::<FixedSizeListArray>().unwrap();
                translate_literal_expr(&array_to_literal(&array.value(0).clone()))
            }
            _ => panic!("Unexpected fixed size list size: {size}"),
        },
        _ => panic!("Unsupported data type: {value:?}"),
    }
}

/// Translates a DataFusion binary expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The binary expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing the variables in scope
///
/// # Returns
/// A symbolic expression representing the binary operation
fn translate_binary_expr(
    expr: &BinaryExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let left = match_and_handle_expr(expr.left().as_ref(), column_symbolic_expressions);
    let right = match_and_handle_expr(expr.right().as_ref(), column_symbolic_expressions);
    let op = expr.op();
    SymbolicExpr::BinaryExpr {
        left: Box::new(left),
        op: translate_binary_expr_op(op),
        right: Box::new(right),
    }
}

/// Converts a DataFusion case-when expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The case-when expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing the variables in scope
///
/// # Returns
/// A symbolic expression representing the case-when expression
fn translate_case_when_expr(expr: &CaseExpr, sym_vars: &Vec<SymbolicExpr>) -> SymbolicExpr {
    let when_then_expr = expr.when_then_expr();
    let else_expr = expr.else_expr();
    let mut composite_if_then_expr = None;
    let case_condition = expr.expr();
    for (condition, then_expr) in when_then_expr.iter().rev() {
        // Simple-case-expression (case-condition is Some) vs search-case-expression (case-condition is None)
        let condition = match case_condition {
            Some(expr) => SymbolicExpr::BinaryExpr {
                left: Box::new(match_and_handle_expr(expr.as_ref(), sym_vars)),
                op: SymbolicOperator::Equal,
                right: Box::new(match_and_handle_expr(condition.as_ref(), sym_vars)),
            },
            None => match_and_handle_expr(condition.as_ref(), sym_vars),
        };
        composite_if_then_expr = match composite_if_then_expr {
            None => Some(SymbolicExpr::IfThenElse {
                condition: Box::new(condition),
                then_branch: Box::new(match_and_handle_expr(then_expr.as_ref(), sym_vars)),
                else_branch: Box::new(match else_expr {
                    Some(expr) => match_and_handle_expr(expr.as_ref(), sym_vars),
                    None => SymbolicExpr::Literal(SymbolicScalarValue::Null),
                }),
            }),
            Some(previous_expr) => Some(SymbolicExpr::IfThenElse {
                condition: Box::new(condition),
                then_branch: Box::new(match_and_handle_expr(then_expr.as_ref(), sym_vars)),
                else_branch: Box::new(previous_expr),
            }),
        }
    }
    composite_if_then_expr
        .unwrap_or_else(|| panic!("Case when expression must have at least one when-then pair"))
}

/// Converts a DataFusion binary operator into its symbolic equivalent.
///
/// Supports arithmetic, comparison, and logical operators.
///
/// # Arguments
/// * `op` - The DataFusion operator to translate
///
/// # Panics
/// * If the operator is not supported
fn translate_binary_expr_op(op: &Operator) -> SymbolicOperator {
    match op {
        Operator::Plus => SymbolicOperator::Plus,
        Operator::Minus => SymbolicOperator::Minus,
        Operator::Multiply => SymbolicOperator::Multiply,
        Operator::Divide => SymbolicOperator::Divide,
        Operator::Eq => SymbolicOperator::Equal,
        Operator::NotEq => SymbolicOperator::NotEqual,
        Operator::Lt => SymbolicOperator::Less,
        Operator::LtEq => SymbolicOperator::LessEqual,
        Operator::Gt => SymbolicOperator::Greater,
        Operator::GtEq => SymbolicOperator::GreaterEqual,
        Operator::Modulo => SymbolicOperator::Modulo,
        Operator::And => SymbolicOperator::And,
        Operator::Or => SymbolicOperator::Or,
        Operator::IsDistinctFrom => SymbolicOperator::NotEqual,
        Operator::IsNotDistinctFrom => SymbolicOperator::Equal,
        _ => panic!("Unsupported operator: {op:?}"),
    }
}

/// Translates a column reference into a symbolic expression.
///
/// # Arguments
/// * `expr` - The column expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions containing the available variables
///
/// # Returns
/// The symbolic expression for the referenced column
///
/// # Panics
/// * If the column index is out of bounds
fn translate_column_expr(
    expr: &Column,
    column_symbolic_expressions: &[SymbolicExpr],
) -> SymbolicExpr {
    let column_index = expr.index();
    if column_index >= column_symbolic_expressions.len() {
        panic!(
            "Column index out of bounds: {column_index} for column: {expr:?}. Exprs: {column_symbolic_expressions:?}"
        );
    }
    column_symbolic_expressions[column_index].clone()
}

/// Translates a type cast expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The cast expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
fn translate_cast_expr(
    expr: &CastExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    match_and_handle_expr(expr.expr().as_ref(), column_symbolic_expressions)
}

/// Translates an IN list expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The IN list expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
/// A symbolic expression representing the IN list operation
///
/// Note: We always assume the left-hand-side to be the column we are checking list membership of and the
/// right-hand-side to be the list of values we are checking against. This is consistent with how the
/// symbolic expressions are translated into Z3 constraints.
fn translate_in_list_expr(
    expr: &InListExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let left = match_and_handle_expr(expr.expr().as_ref(), column_symbolic_expressions);
    let left_clone = left.clone();
    let right = expr
        .list()
        .iter()
        .map(|expr| match_and_handle_expr(expr.as_ref(), column_symbolic_expressions))
        .collect();
    let in_expr = SymbolicExpr::InListExpr {
        left: Box::new(left),
        right,
    };
    if expr.negated() {
        // NOT IN should also imply the LHS is NOT NULL for SQL WHERE filtering
        SymbolicExpr::BinaryExpr {
            left: Box::new(SymbolicExpr::Not(Box::new(in_expr))),
            op: SymbolicOperator::And,
            right: Box::new(SymbolicExpr::IsNotNull(Box::new(left_clone))),
        }
    } else {
        in_expr
    }
}

/// Translates a NOT expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The NOT expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
fn translate_not_expr(
    expr: &NotExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let expr = match_and_handle_expr(expr.arg().as_ref(), column_symbolic_expressions);
    SymbolicExpr::Not(Box::new(expr))
}

/// Translates an aggregate function expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The aggregate function expression to translate
/// * `constraints` - Vector of constraints that apply to the aggregation
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
/// A symbolic expression representing the aggregate operation
///
/// # TODO
/// * Handle multiple arguments
/// * Handle cases where constraints are empty
///
/// Note: This function is not called by `match_and_handle_expr` since aggregations are not
/// PhysicalExprs. It is called explicitly while building row and column symbolic expressions for
/// SymAggregateExec nodes.
pub fn translate_aggregate_expr(
    expr: &AggregateFunctionExpr,
    constraints: &SymbolicExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let args = expr.expressions();

    // TODO: Handle more than one argument
    // TODO: Handle cases where the constraints are empty - check if the column IS NOT NULL
    let column = match_and_handle_expr(args[0].as_ref(), column_symbolic_expressions);
    let func = expr.fun();
    let is_distinct = expr.is_distinct();
    let generic_row = constraints.clone();

    SymbolicExpr::aggregate(
        SymbolicAggregateFunction::from_string(func.name()),
        column,
        is_distinct,
        generic_row,
    )
}

/// Translates a callback physical expression into a symbolic hole expression.
///
/// # Arguments
/// * `expr` - The callback expression to translate
///
/// # Returns
/// A symbolic hole expression with the callback's ID and physical expression
///
/// The ID is used to match and replace the appropriate hole in the symbolic expression
/// with the concrete value of evaluating the physical expression during streaming.
fn translate_callback_expr(expr: &CallbackPhysicalExpr) -> SymbolicExpr {
    SymbolicExpr::hole(expr.get_expr_id(), expr.get_physical_expr().clone())
}

/// Translates an IS NULL expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The IS NULL expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
/// A symbolic expression representing the IS NULL operation
fn translate_is_null_expr(
    expr: &IsNullExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let expr = match_and_handle_expr(expr.arg().as_ref(), column_symbolic_expressions);
    SymbolicExpr::IsNull(Box::new(expr))
}

/// Translates an IS NOT NULL expression into a symbolic expression.
///
/// # Arguments
/// * `expr` - The IS NOT NULL expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
/// A symbolic expression representing the IS NOT NULL operation
fn translate_is_not_null_expr(
    expr: &IsNotNullExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    let expr = match_and_handle_expr(expr.arg().as_ref(), column_symbolic_expressions);
    SymbolicExpr::IsNotNull(Box::new(expr))
}

/// Main entry point for translating any physical expression into its symbolic equivalent.
///
/// This function dispatches to the appropriate translator based on the expression type.
///
/// # Arguments
/// * `expr` - The physical expression to translate
/// * `column_symbolic_expressions` - Vector of symbolic expressions representing variables in scope
///
/// # Returns
/// The translated symbolic expression
///
/// # Panics
/// * If the expression type is not supported
///
/// Aggregation expressions are not handled here since they DO NOT implement the
/// PhysicalExpr trait. They are handled separately in `translate_aggregate_expr`.
pub fn match_and_handle_expr(
    expr: &dyn PhysicalExpr,
    column_symbolic_expressions: &Vec<SymbolicExpr>,
) -> SymbolicExpr {
    if let Some(binary_expr) = expr.as_any().downcast_ref::<BinaryExpr>() {
        translate_binary_expr(binary_expr, column_symbolic_expressions)
    } else if let Some(literal_expr) = expr.as_any().downcast_ref::<Literal>() {
        translate_literal_expr(literal_expr)
    } else if let Some(column_expr) = expr.as_any().downcast_ref::<Column>() {
        translate_column_expr(column_expr, column_symbolic_expressions)
    } else if let Some(cast_expr) = expr.as_any().downcast_ref::<CastExpr>() {
        translate_cast_expr(cast_expr, column_symbolic_expressions)
    } else if let Some(in_list_expr) = expr.as_any().downcast_ref::<InListExpr>() {
        translate_in_list_expr(in_list_expr, column_symbolic_expressions)
    } else if let Some(case_when_expr) = expr.as_any().downcast_ref::<CaseExpr>() {
        translate_case_when_expr(case_when_expr, column_symbolic_expressions)
    } else if let Some(callback_expr) = expr.as_any().downcast_ref::<CallbackPhysicalExpr>() {
        translate_callback_expr(callback_expr)
    } else if let Some(not_expr) = expr.as_any().downcast_ref::<NotExpr>() {
        translate_not_expr(not_expr, column_symbolic_expressions)
    } else if let Some(is_null_expr) = expr.as_any().downcast_ref::<IsNullExpr>() {
        translate_is_null_expr(is_null_expr, column_symbolic_expressions)
    } else if let Some(is_not_null_expr) = expr.as_any().downcast_ref::<IsNotNullExpr>() {
        translate_is_not_null_expr(is_not_null_expr, column_symbolic_expressions)
    } else {
        panic!("Unsupported physical expression: {expr:?}");
    }
}
