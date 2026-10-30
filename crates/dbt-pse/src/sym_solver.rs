use crate::concolic_profile::ConcolicProfile;
/// Provides functionality for solving symbolic constraints.
use crate::symbolic_nodes::{
    SymbolicAggregateFunction, SymbolicExpr, SymbolicOperator, SymbolicScalarValue, SymbolicZ3Type,
    conjunct_exprs,
};
use arrow_schema::DataType;
use std::collections::HashMap;
use std::time::Instant;
use z3::{SatResult, Solver, ast::Ast, ast::Bool, ast::Dynamic, ast::Int, ast::Real, ast::String};

/// Represents a nullable Z3 expression.
#[derive(Clone, Debug)]
enum Z3Expr {
    Int(Int, Bool),
    Bool(Bool, Bool),
    String(String, Bool),
    Real(Real, Bool),
}

impl Z3Expr {
    pub fn new_int_var(var_name: &str) -> Self {
        let var = Int::new_const(var_name);
        let is_null = Bool::new_const(format!("{var_name}_is_null"));
        Self::Int(var, is_null)
    }

    pub fn new_bool_var(var_name: &str) -> Self {
        let var = Bool::new_const(var_name);
        let is_null = Bool::new_const(format!("{var_name}_is_null"));
        Self::Bool(var, is_null)
    }

    pub fn new_string_var(var_name: &str) -> Self {
        let var = String::new_const(var_name);
        let is_null = Bool::new_const(format!("{var_name}_is_null"));
        Self::String(var, is_null)
    }

    pub fn new_real_var(var_name: &str) -> Self {
        let var = Real::new_const(var_name);
        let is_null = Bool::new_const(format!("{var_name}_is_null"));
        Self::Real(var, is_null)
    }

    pub fn new_int_i64_literal(value: i64) -> Self {
        let var = Int::from_i64(value);
        let is_null = Bool::from_bool(false);
        Self::Int(var, is_null)
    }

    pub fn new_int_u64_literal(value: u64) -> Self {
        let var = Int::from_u64(value);
        let is_null = Bool::from_bool(false);
        Self::Int(var, is_null)
    }

    pub fn new_bool_literal(value: bool) -> Self {
        let var = Bool::from_bool(value);
        let is_null = Bool::from_bool(false);
        Self::Bool(var, is_null)
    }

    pub fn new_string_literal(value: &str) -> Self {
        let var = String::from(value);
        let is_null = Bool::from_bool(false);
        Self::String(var, is_null)
    }

    pub fn new_real_literal(value: i64, scale: i32) -> Self {
        let var = Real::from_rational(value, 10_i64.pow(scale as u32));
        let is_null = Bool::from_bool(false);
        Self::Real(var, is_null)
    }

    pub fn new_bool_expr(expr: Bool, is_null: Bool) -> Self {
        Self::Bool(expr, is_null)
    }

    pub fn new_int_expr(expr: Int, is_null: Bool) -> Self {
        Self::Int(expr, is_null)
    }

    pub fn new_string_expr(expr: String, is_null: Bool) -> Self {
        Self::String(expr, is_null)
    }

    pub fn new_real_expr(expr: Real, is_null: Bool) -> Self {
        Self::Real(expr, is_null)
    }

    pub fn new_null_int() -> Self {
        let var = Int::from_i64(0);
        let is_null = Bool::from_bool(true);
        Self::Int(var, is_null)
    }

    pub fn new_null_bool() -> Self {
        let var = Bool::from_bool(false);
        let is_null = Bool::from_bool(true);
        Self::Bool(var, is_null)
    }

    pub fn new_null_string() -> Self {
        let var = String::from("");
        let is_null = Bool::from_bool(true);
        Self::String(var, is_null)
    }

    pub fn new_null_real() -> Self {
        let var = Real::from_rational(0, 1);
        let is_null = Bool::from_bool(true);
        Self::Real(var, is_null)
    }
}

/// Converts a generic variable to a variable.
///
/// # Arguments
/// * `expr` - The symbolic expression to convert.
/// * `row` - The row number.
///
/// # Returns
/// The converted symbolic expression.
fn generic_var_to_var(expr: &SymbolicExpr, row: usize) -> SymbolicExpr {
    match expr {
        SymbolicExpr::GenericVar {
            table,
            column,
            arrow_type,
            z3_type,
            column_name,
        } => SymbolicExpr::variable(
            table.clone(),
            *column,
            row,
            arrow_type.clone(),
            *z3_type,
            column_name.clone(),
        ),
        SymbolicExpr::BinaryExpr { left, op, right } => SymbolicExpr::binary(
            generic_var_to_var(left, row),
            *op,
            generic_var_to_var(right, row),
        ),
        SymbolicExpr::InListExpr { left, right } => {
            SymbolicExpr::in_list(generic_var_to_var(left, row), right.clone())
        }
        _ => expr.clone(),
    }
}

/// Expands row constraints.
///
/// # Arguments
/// * `expr` - The symbolic expression to expand.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The expanded symbolic expression.
pub fn expand_row_constraints(expr: &SymbolicExpr, num_rows: usize) -> Vec<SymbolicExpr> {
    match expr {
        SymbolicExpr::GenericRow { constraints, .. } => {
            let mut expanded_exprs = Vec::new();
            for row in 1..=num_rows {
                let mut cond = conjunct_exprs(constraints.as_ref().unwrap()).unwrap();
                cond = generic_var_to_var(&cond, row);
                expanded_exprs.push(cond);
            }
            expanded_exprs
        }
        _ => panic!("Unsupported expression: {expr:?}"),
    }
}

pub fn flip_constraint(expr: &SymbolicExpr) -> SymbolicExpr {
    match expr {
        SymbolicExpr::Not(expr) => (**expr).to_owned(),
        _ => SymbolicExpr::Not(Box::new(expr.clone())),
    }
}

/// Handles a literal.
///
/// # Arguments
/// * `ctx` - The context.
/// * `val` - The symbolic scalar value.
///
/// # Returns
/// The Z3 expression.
fn handle_literal(val: &SymbolicScalarValue) -> Z3Expr {
    match val {
        SymbolicScalarValue::Decimal128(i, _, scale) => {
            if let Some(i) = i {
                if *scale == 0 {
                    Z3Expr::new_int_i64_literal(*i as i64)
                } else {
                    Z3Expr::new_real_literal(*i as i64, *scale as i32)
                }
            } else if *scale == 0 {
                Z3Expr::new_null_int()
            } else {
                Z3Expr::new_null_real()
            }
        }
        SymbolicScalarValue::Utf8(s) => {
            if let Some(s) = s {
                Z3Expr::new_string_literal(s.as_str())
            } else {
                Z3Expr::new_null_string()
            }
        }
        SymbolicScalarValue::Int64(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_i64_literal(*i)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::Int32(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_i64_literal(*i as i64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::Int16(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_i64_literal(*i as i64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::Int8(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_i64_literal(*i as i64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::UInt64(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_u64_literal(*i)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::UInt32(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_u64_literal(*i as u64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::UInt16(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_u64_literal(*i as u64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::UInt8(i) => {
            if let Some(i) = i {
                Z3Expr::new_int_u64_literal(*i as u64)
            } else {
                Z3Expr::new_null_int()
            }
        }
        SymbolicScalarValue::Boolean(b) => {
            if let Some(b) = b {
                Z3Expr::new_bool_literal(*b)
            } else {
                Z3Expr::new_null_bool()
            }
        }
        _ => panic!("Unsupported literal: {val:?}"),
    }
}

/// Creates a range constraint for an integer variable.
///
/// # Arguments
/// * `ctx` - The context.
/// * `var_name` - The name of the variable.
/// * `low` - The low value.
/// * `high` - The high value.
fn int_var_range_constraint(var_name: &str, low: i64, high: i64) -> Z3Expr {
    let var = Int::new_const(var_name);
    let min = Int::from_i64(low);
    let max = Int::from_i64(high);

    // Create range constraint: low <= var <= high
    Z3Expr::Bool(
        Bool::and(&[
            &var.ge(&min), // greater-equal
            &var.le(&max), // less-equal
        ]),
        Bool::from_bool(false),
    )
}

/// Gets a range constraint for a variable.
///
/// # Arguments
/// * `ctx` - The context.
/// * `var_name` - The name of the variable.
/// * `arrow_type` - The arrow type.
///
fn get_range_constraint(var_name: &str, arrow_type: &DataType) -> Z3Expr {
    match arrow_type {
        DataType::Int8 => int_var_range_constraint(var_name, -128, 127),
        DataType::Int16 => int_var_range_constraint(var_name, -32768, 32767),
        DataType::Int32 => int_var_range_constraint(var_name, -2147483648, 2147483647),
        DataType::Int64 => {
            int_var_range_constraint(var_name, -9223372036854775808, 9223372036854775807)
        }
        DataType::Date32
        | DataType::Timestamp(_, _)
        | DataType::Time64(_)
        | DataType::Time32(_) => int_var_range_constraint(var_name, 0, 9223372036854775807),
        DataType::UInt8 => int_var_range_constraint(var_name, 0, 255),
        DataType::UInt16 => int_var_range_constraint(var_name, 0, 65535),
        DataType::UInt32 => int_var_range_constraint(var_name, 0, 4294967295),
        DataType::UInt64 => {
            let var = Int::new_const(var_name);
            let min = Int::from_u64(0);
            let max = Int::from_u64(18446744073709551615);

            Z3Expr::Bool(
                Bool::and(&[
                    &var.ge(&min), // greater-equal
                    &var.le(&max), // less-equal
                ]),
                Bool::from_bool(false),
            )
        }
        DataType::Decimal128(_, scale) => match scale {
            // TODO: Add support for dealing with i128 values. For now we are using i64.
            0 => int_var_range_constraint(var_name, -9223372036854775808, 9223372036854775807),
            _ => panic!("Unsupported scale for decimal: {scale:?}"),
        },
        DataType::FixedSizeList(field, size) => match size {
            1 => get_range_constraint(var_name, field.data_type()),
            _ => panic!("Unsupported size for fixed size list: {size:?}"),
        },
        DataType::Struct(fields) => {
            // Only accept a field explicitly named "timestamp" with a supported time-like type
            if let Some(f) = fields.iter().find(|f| {
                f.name() == "timestamp"
                    && matches!(
                        f.data_type(),
                        DataType::Timestamp(_, _)
                            | DataType::Date32
                            | DataType::Time64(_)
                            | DataType::Time32(_)
                    )
            }) {
                return get_range_constraint(var_name, f.data_type());
            }
            panic!(
                "Unsupported struct for int range: missing 'timestamp' field with time-like type: {:?}",
                arrow_type
            );
        }
        _ => panic!("Unsupported arrow type for int: {arrow_type:?}"),
    }
}

/// Handles a variable.
///
/// # Arguments
/// * `ctx` - The context.
/// * `table` - The table name.
/// * `column` - The column number.
/// * `row` - The row number.
/// * `column_name` - The column name.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `z3_type` - The Z3 type.
/// * `arrow_type` - The arrow type.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_variable(
    table: &str,
    column: usize,
    row: usize,
    column_name: &str,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    z3_type: &SymbolicZ3Type,
    arrow_type: &DataType,
) -> Z3Expr {
    let var_name = format!("{table}.{column}.{row}.{column_name}");
    variable_types.insert(var_name.clone(), arrow_type.clone());
    let z3_expr = match z3_type {
        SymbolicZ3Type::Int => {
            let range_constraint = get_range_constraint(&var_name, arrow_type);
            additional_constraints.push(range_constraint);
            Z3Expr::new_int_var(&var_name)
        }
        SymbolicZ3Type::String => Z3Expr::new_string_var(&var_name),
        SymbolicZ3Type::Boolean => Z3Expr::new_bool_var(&var_name),
        SymbolicZ3Type::Float => Z3Expr::new_real_var(&var_name),
    };
    variables.push(z3_expr.clone());
    z3_expr
}

/// Generic helper for handling in-list expressions
fn handle_in_list_generic<T: Clone>(
    l: &T,
    l_is_null: &Bool,
    values: &[Option<(T, Bool)>],
    eq_fn: impl Fn(&T, &T) -> Bool,
) -> Z3Expr {
    // Situations when IN LIST is NULL
    // 3 IN [1, 2, NULL] => NULL
    // 3 IN [1, 2] => FALSE
    // 1 IN [1, 2, NULL] => TRUE
    // NULL IN [1, 2, NULL] => NULL
    //
    // It is TRUE when match is found and FALSE when no match is found and no element is NULL
    // and when the LHS is non-NULL. Every other case is NULL.

    // Check if the list has any NULL values
    let list_has_null = Bool::or(
        &values
            .iter()
            .filter_map(|opt| opt.as_ref().map(|(_, v_is_null)| (*v_is_null).clone()))
            .collect::<Vec<_>>(),
    );
    // Check if any of the values in the list match the LHS and the LHS is not NULL and the value is not NULL
    let any_match: Vec<_> = values
        .iter()
        .filter_map(|opt| {
            opt.as_ref()
                .map(|(v, v_is_null)| Bool::and(&[eq_fn(l, v), v_is_null.not(), l_is_null.not()]))
        })
        .collect::<Vec<_>>();
    // Disjunction of all the matches
    let in_list_expr = Bool::or(&any_match);
    // If the list has any NULL values, the result is NULL if there is no match
    let in_list_is_null = Bool::and(&[list_has_null, in_list_expr.not()]);
    // If the LHS is NULL, the result is NULL
    let in_list_is_null = Bool::or(&[l_is_null.clone(), in_list_is_null]);

    Z3Expr::new_bool_expr(in_list_expr, in_list_is_null)
}

/// Handles an in list expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `left` - The left expression.
/// * `right` - The right expression.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `variable_range_constraints` - The variable range constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_in_list(
    left: &SymbolicExpr,
    right: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    variable_range_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let left_expr = expr_to_z3(
        left,
        variables,
        variable_types,
        variable_range_constraints,
        num_rows,
    );
    let right_exprs = right
        .iter()
        .map(|expr| {
            expr_to_z3(
                expr,
                variables,
                variable_types,
                variable_range_constraints,
                num_rows,
            )
        })
        .collect::<Vec<_>>();
    match left_expr {
        Z3Expr::String(l, l_is_null) => {
            let values = right_exprs
                .iter()
                .map(|v| match v {
                    Z3Expr::String(v, v_is_null) => Some((v.clone(), v_is_null.clone())),
                    _ => panic!("Expected string expression"),
                })
                .collect::<Vec<_>>();
            handle_in_list_generic(&l, &l_is_null, &values, |a, b| a.eq(b))
        }
        Z3Expr::Int(l, l_is_null) => {
            let values = right_exprs
                .iter()
                .map(|v| match v {
                    Z3Expr::Int(v, v_is_null) => Some((v.clone(), v_is_null.clone())),
                    _ => panic!("Expected int expression"),
                })
                .collect::<Vec<_>>();
            handle_in_list_generic(&l, &l_is_null, &values, |a, b| a.eq(b))
        }
        Z3Expr::Bool(l, l_is_null) => {
            let values = right_exprs
                .iter()
                .map(|v| match v {
                    Z3Expr::Bool(v, v_is_null) => Some((v.clone(), v_is_null.clone())),
                    _ => panic!("Expected bool expression"),
                })
                .collect::<Vec<_>>();
            handle_in_list_generic(&l, &l_is_null, &values, |a, b| a.eq(b))
        }
        Z3Expr::Real(l, l_is_null) => {
            let values = right_exprs
                .iter()
                .map(|v| match v {
                    Z3Expr::Real(v, v_is_null) => Some((v.clone(), v_is_null.clone())),
                    _ => panic!("Expected real expression"),
                })
                .collect::<Vec<_>>();
            handle_in_list_generic(&l, &l_is_null, &values, |a, b| a.eq(b))
        }
    }
}

/// Handles an if-then-else expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `condition` - The condition.
/// * `then_branch` - The then branch.
/// * `else_branch` - The else branch.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `variable_range_constraints` - The variable range constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_if_then_else(
    condition: &SymbolicExpr,
    then_branch: &SymbolicExpr,
    else_branch: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    variable_range_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let cond_expr = expr_to_z3(
        condition,
        variables,
        variable_types,
        variable_range_constraints,
        num_rows,
    );
    let then_expr = expr_to_z3(
        then_branch,
        variables,
        variable_types,
        variable_range_constraints,
        num_rows,
    );
    let else_expr = expr_to_z3(
        else_branch,
        variables,
        variable_types,
        variable_range_constraints,
        num_rows,
    );
    // If the condition is TRUE, then the result is the then branch
    // If the condition is FALSE or NULL, then the result is the else branch
    // The nullability of the result is the same as the nullability of the branch taken
    let cond_true = match cond_expr {
        Z3Expr::Bool(expr, is_null) => Bool::and(&[expr, is_null.not()]),
        _ => panic!("Expected boolean expression"),
    };
    match (then_expr.clone(), else_expr.clone()) {
        (Z3Expr::Int(then, then_is_null), Z3Expr::Int(else_, else_is_null)) => {
            Z3Expr::new_int_expr(
                Bool::ite(&cond_true, &then, &else_),
                Bool::ite(&cond_true, &then_is_null, &else_is_null),
            )
        }
        (Z3Expr::String(then, then_is_null), Z3Expr::String(else_, else_is_null)) => {
            Z3Expr::new_string_expr(
                Bool::ite(&cond_true, &then, &else_),
                Bool::ite(&cond_true, &then_is_null, &else_is_null),
            )
        }
        (Z3Expr::Bool(then, then_is_null), Z3Expr::Bool(else_, else_is_null)) => {
            Z3Expr::new_bool_expr(
                Bool::ite(&cond_true, &then, &else_),
                Bool::ite(&cond_true, &then_is_null, &else_is_null),
            )
        }
        (Z3Expr::Real(then, then_is_null), Z3Expr::Real(else_, else_is_null)) => {
            Z3Expr::new_real_expr(
                Bool::ite(&cond_true, &then, &else_),
                Bool::ite(&cond_true, &then_is_null, &else_is_null),
            )
        }
        _ => {
            panic!("if-then-else expression unsupported for type: {then_expr:?} and {else_expr:?}")
        }
    }
}

/// Handles a binary expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `left` - The left expression.
/// * `op` - The operator.
/// * `right` - The right expression.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_binary_expr(
    left: &SymbolicExpr,
    op: &SymbolicOperator,
    right: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let l = expr_to_z3(
        left,
        variables,
        variable_types,
        additional_constraints,
        num_rows,
    );
    let r = expr_to_z3(
        right,
        variables,
        variable_types,
        additional_constraints,
        num_rows,
    );

    match (l.clone(), r.clone()) {
        (Z3Expr::Int(l, l_is_null), Z3Expr::Int(r, r_is_null)) => match op {
            SymbolicOperator::Plus => {
                Z3Expr::new_int_expr(Int::add(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Minus => {
                Z3Expr::new_int_expr(Int::sub(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Multiply => {
                Z3Expr::new_int_expr(Int::mul(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Divide => {
                Z3Expr::new_int_expr(l.div(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Modulo => {
                Z3Expr::new_int_expr(l.rem(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Less => {
                Z3Expr::new_bool_expr(l.lt(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::LessEqual => {
                Z3Expr::new_bool_expr(l.le(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Greater => {
                Z3Expr::new_bool_expr(l.gt(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::GreaterEqual => {
                Z3Expr::new_bool_expr(l.ge(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Equal => {
                Z3Expr::new_bool_expr(l.eq(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::NotEqual => {
                Z3Expr::new_bool_expr(l.eq(&r).not(), Bool::or(&[l_is_null, r_is_null]))
            }
            _ => panic!("Unsupported operator: {op:?}"),
        },
        (Z3Expr::String(l, l_is_null), Z3Expr::String(r, r_is_null)) => match op {
            SymbolicOperator::Equal => {
                Z3Expr::new_bool_expr(l.eq(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::NotEqual => {
                Z3Expr::new_bool_expr(l.eq(&r).not(), Bool::or(&[l_is_null, r_is_null]))
            }
            _ => panic!("Unsupported operator: {op:?}"),
        },
        (Z3Expr::Bool(l, l_is_null), Z3Expr::Bool(r, r_is_null)) => match op {
            SymbolicOperator::And => {
                Z3Expr::new_bool_expr(Bool::and(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Or => {
                Z3Expr::new_bool_expr(Bool::or(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Xor => {
                Z3Expr::new_bool_expr(l.xor(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Equal => {
                Z3Expr::new_bool_expr(l.eq(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::NotEqual => {
                Z3Expr::new_bool_expr(l.eq(&r).not(), Bool::or(&[l_is_null, r_is_null]))
            }
            _ => panic!("Unsupported operator: {op:?}"),
        },
        (Z3Expr::Real(l, l_is_null), Z3Expr::Real(r, r_is_null)) => match op {
            SymbolicOperator::Plus => {
                Z3Expr::new_real_expr(Real::add(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Minus => {
                Z3Expr::new_real_expr(Real::sub(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Multiply => {
                Z3Expr::new_real_expr(Real::mul(&[&l, &r]), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Divide => {
                Z3Expr::new_real_expr(l.div(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Less => {
                Z3Expr::new_bool_expr(l.lt(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::LessEqual => {
                Z3Expr::new_bool_expr(l.le(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Greater => {
                Z3Expr::new_bool_expr(l.gt(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::GreaterEqual => {
                Z3Expr::new_bool_expr(l.ge(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::Equal => {
                Z3Expr::new_bool_expr(l.eq(&r), Bool::or(&[l_is_null, r_is_null]))
            }
            SymbolicOperator::NotEqual => {
                Z3Expr::new_bool_expr(l.eq(&r).not(), Bool::or(&[l_is_null, r_is_null]))
            }
            _ => panic!("Unsupported operator: {op:?}"),
        },
        _ => panic!("Type mismatch in binary expression: {l:?} {op:?} {r:?}"),
    }
}

/// Handles the is_null expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expr` - The expression.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_is_null_expr(
    expr: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let expr = expr_to_z3(
        expr,
        variables,
        variable_types,
        additional_constraints,
        num_rows,
    );
    let is_null = match expr {
        Z3Expr::Int(_, is_null) => is_null,
        Z3Expr::Bool(_, is_null) => is_null,
        Z3Expr::String(_, is_null) => is_null,
        Z3Expr::Real(_, is_null) => is_null,
    };
    // IS NULL expression can never be NULL.
    Z3Expr::Bool(is_null, Bool::from_bool(false))
}

/// Handles the is_not_null expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expr` - The expression.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_is_not_null_expr(
    expr: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let expr = handle_is_null_expr(
        expr,
        variables,
        variable_types,
        additional_constraints,
        num_rows,
    );
    // IS NOT NULL expression can never be NULL.
    match expr {
        Z3Expr::Bool(is_null, _) => Z3Expr::Bool(is_null.not(), Bool::from_bool(false)),
        _ => panic!("Expected a boolean expression"),
    }
}

/// Handles the distinct constraint for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_args` - The expanded arguments.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_is_distinct_for_aggregate_expr(
    expanded_args: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) {
    // Convert the symbolic variables appearing in the aggregate expression arguments to Z3 expressions.
    let z3_exprs: Vec<Z3Expr> = expanded_args
        .iter()
        .map(|e| {
            expr_to_z3(
                e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .collect();

    // Check the type of the first element to determine the type of the distinct expression.
    let distinct_expr = match z3_exprs[0] {
        // Check first element to determine type
        Z3Expr::Int(_, _) => {
            let int_exprs: Vec<Int> = z3_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Int(i, _) => i.clone(),
                    _ => panic!("Expected all integer expressions"),
                })
                .collect();
            Z3Expr::Bool(Ast::distinct(&int_exprs), Bool::from_bool(false))
        }
        Z3Expr::Bool(_, _) => {
            let bool_exprs: Vec<Bool> = z3_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Bool(b, _) => b.clone(),
                    _ => panic!("Expected all boolean expressions"),
                })
                .collect();
            Z3Expr::Bool(Ast::distinct(&bool_exprs), Bool::from_bool(false))
        }
        Z3Expr::String(_, _) => {
            let string_exprs: Vec<String> = z3_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::String(s, _) => s.clone(),
                    _ => panic!("Expected all string expressions"),
                })
                .collect();
            Z3Expr::Bool(Ast::distinct(&string_exprs), Bool::from_bool(false))
        }
        Z3Expr::Real(_, _) => {
            let real_exprs: Vec<Real> = z3_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Real(f, _) => f.clone(),
                    _ => panic!("Expected all float expressions"),
                })
                .collect();
            Z3Expr::Bool(Ast::distinct(&real_exprs), Bool::from_bool(false))
        }
    };
    additional_constraints.push(distinct_expr);
}

/// Handles the count expression for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_rows` - The expanded rows.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_count_expr(
    expanded_args: &[SymbolicExpr],
    expanded_rows: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    // Only non-NULL values contribute to the count so we run this step to add range constraints to the aggregation arguments.
    let expanded_args: Vec<Bool> = expanded_args
        .iter()
        .map(|e| {
            expr_to_z3(
                e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .map(|e| match e {
            Z3Expr::Int(_, is_null) => is_null,
            Z3Expr::Bool(_, is_null) => is_null,
            Z3Expr::String(_, is_null) => is_null,
            Z3Expr::Real(_, is_null) => is_null,
        })
        .collect::<Vec<Bool>>();
    // If row constraint is true, it contributes 1 to the count.
    let if_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(1), 0, 0));
    // If row constraint is false, it contributes 0 to the count.
    let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(0), 0, 0));

    // For each row constraint, we create an if-then-else expression that returns 1 if the row constraint is true, and 0 if it is false.
    // Then we convert the if-then-else expression to a Z3 expression and collect the results. The count_exprs has to be of Int type.
    let count_expr = expanded_rows
        .iter()
        .map(|e| SymbolicExpr::if_then_else(e.clone(), if_branch.clone(), else_branch.clone()))
        .map(|e| {
            expr_to_z3(
                &e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .map(|e| match e {
            Z3Expr::Int(i, _) => i,
            _ => panic!("Count expression must be of Int type"),
        })
        .collect::<Vec<Int>>();

    // If the arg variable is NULL, the count is 0.
    // If the arg variable is not NULL, the count is determined by the row-constraint.
    let count_expr = count_expr
        .iter()
        .enumerate()
        .map(|(i, e)| Bool::ite(&expanded_args[i], &Int::from_i64(0), e))
        .collect::<Vec<Int>>();

    // Sum up the count_exprs to get the total count.
    // The nullability of the result is false because the count is always a non-negative integer.
    Z3Expr::Int(Int::add(&count_expr), Bool::from_bool(false))
}

/// Handles the sum expression for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_args` - The expanded arguments.
/// * `expanded_rows` - The expanded rows.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_sum_expr(
    expanded_args: &[SymbolicExpr],
    expanded_rows: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    // For each row constraint, we create an if-then-else expression that returns the symbolic variable if the row constraint
    // is true, and 0 if it is false.
    let if_branches: Vec<SymbolicExpr> = expanded_args.to_vec();
    let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(None, 0, 0));
    let sum_exprs = expanded_rows
        .iter()
        .enumerate()
        .map(|(i, e)| {
            SymbolicExpr::if_then_else(
                SymbolicExpr::binary(
                    e.clone(),
                    SymbolicOperator::And,
                    SymbolicExpr::IsNotNull(Box::new(expanded_args[i].clone())),
                ),
                if_branches[i].clone(),
                else_branch.clone(),
            )
        })
        .map(|e| {
            expr_to_z3(
                &e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .collect::<Vec<Z3Expr>>();

    match sum_exprs[0].clone() {
        Z3Expr::Int(_, _) => {
            let int_exprs: Vec<(Int, Bool)> = sum_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Int(i, i_is_null) => (i.clone(), i_is_null.clone()),
                    _ => panic!("Expected all integer expressions"),
                })
                .collect::<Vec<(Int, Bool)>>();
            // The nullability of the result is the same as the nullability of the conjuncted nullability of the arguments.
            // Only non-null values contribute to the sum.
            let sum = int_exprs.iter().skip(1).fold(
                int_exprs[0].clone(),
                |(acc, acc_is_null), (x, x_is_null)| {
                    // If both values are null, result is null
                    // If one value is null, use the non-null value
                    // If neither is null, add them
                    let new_is_null = Bool::and(&[acc_is_null.clone(), x_is_null.clone()]);
                    let new_val = Bool::ite(
                        &acc_is_null,
                        x,
                        &Bool::ite(x_is_null, &acc, &Int::add(&[&acc, x])),
                    );
                    (new_val, new_is_null)
                },
            );
            Z3Expr::Int(sum.0, sum.1)
        }
        // TODO: Add support for float.
        _ => panic!("Sum expression must be of Int type"),
    }
}

/// Handles the max expression for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_args` - The expanded arguments.
/// * `expanded_rows` - The expanded rows.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_max_expr(
    expanded_args: &[SymbolicExpr],
    expanded_rows: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    // For each row constraint, we create an if-then-else expression that returns the symbolic variable if the row constraint
    // is true, and NULL if it is false.
    let if_branches: Vec<SymbolicExpr> = expanded_args.to_vec();
    // Else is NULL.
    let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(None, 0, 0));
    let max_exprs = expanded_rows
        .iter()
        .enumerate()
        .map(|(i, e)| {
            SymbolicExpr::if_then_else(
                SymbolicExpr::binary(
                    e.clone(),
                    SymbolicOperator::And,
                    SymbolicExpr::IsNotNull(Box::new(expanded_args[i].clone())),
                ),
                if_branches[i].clone(),
                else_branch.clone(),
            )
        })
        .map(|e| {
            expr_to_z3(
                &e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .collect::<Vec<Z3Expr>>();

    match max_exprs[0].clone() {
        Z3Expr::Int(_, _) => {
            let int_exprs: Vec<(Int, Bool)> = max_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Int(i, i_is_null) => (i.clone(), i_is_null.clone()),
                    _ => panic!("Expected all integer expressions"),
                })
                .collect::<Vec<(Int, Bool)>>();
            // The nullability of the result is the same as the nullability of the conjuncted nullability of the arguments.
            // Only non-null values contribute to the max.
            let max = int_exprs.iter().skip(1).fold(
                int_exprs[0].clone(),
                |(acc, acc_is_null), (x, x_is_null)| {
                    // Only consider x if it's not null
                    let cond = Bool::and(&[x_is_null.not(), acc.gt(x)]);
                    // If both values are null, result is null
                    // If one value is null, use the non-null value
                    // If neither is null, use the greater value
                    let new_is_null = Bool::and(&[acc_is_null.clone(), x_is_null.clone()]);
                    let new_val = Bool::ite(
                        &acc_is_null,
                        x,
                        &Bool::ite(x_is_null, &acc, &Bool::ite(&cond, &acc, x)),
                    );
                    (new_val, new_is_null)
                },
            );
            Z3Expr::Int(max.0, max.1)
        }
        _ => panic!("Max expression must be of Int type"),
    }
}

/// Handles the min expression for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_args` - The expanded arguments.
/// * `expanded_rows` - The expanded rows.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_min_expr(
    expanded_args: &[SymbolicExpr],
    expanded_rows: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    // For each row constraint, we create an if-then-else expression that returns the symbolic variable if the row constraint
    // is true, and NULL if it is false.
    let if_branches: Vec<SymbolicExpr> = expanded_args.to_vec();
    // Else is NULL.
    let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(None, 0, 0));
    let min_exprs = expanded_rows
        .iter()
        .enumerate()
        .map(|(i, e)| {
            SymbolicExpr::if_then_else(
                SymbolicExpr::binary(
                    e.clone(),
                    SymbolicOperator::And,
                    SymbolicExpr::IsNotNull(Box::new(expanded_args[i].clone())),
                ),
                if_branches[i].clone(),
                else_branch.clone(),
            )
        })
        .map(|e| {
            expr_to_z3(
                &e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .collect::<Vec<Z3Expr>>();

    match min_exprs[0].clone() {
        Z3Expr::Int(_, _) => {
            let int_exprs: Vec<(Int, Bool)> = min_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Int(i, i_is_null) => (i.clone(), i_is_null.clone()),
                    _ => panic!("Expected all integer expressions"),
                })
                .collect::<Vec<(Int, Bool)>>();
            // The nullability of the result is the same as the nullability of the conjuncted nullability of the arguments.
            // Only non-null values contribute to the min.
            let min = int_exprs.iter().skip(1).fold(
                int_exprs[0].clone(),
                |(acc, acc_is_null), (x, x_is_null)| {
                    // Only consider x if it's not null
                    let cond = Bool::and(&[x_is_null.not(), acc.lt(x)]);
                    // If both values are null, result is null
                    // If one value is null, use the non-null value
                    // If neither is null, use the smaller value
                    let new_is_null = Bool::and(&[acc_is_null.clone(), x_is_null.clone()]);
                    let new_val = Bool::ite(
                        &acc_is_null,
                        x,
                        &Bool::ite(x_is_null, &acc, &Bool::ite(&cond, &acc, x)),
                    );
                    (new_val, new_is_null)
                },
            );
            Z3Expr::Int(min.0, min.1)
        }
        _ => panic!("Min expression must be of Int type"),
    }
}

/// Handles the median expression for an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expanded_args` - The expanded arguments.
/// * `expanded_rows` - The expanded rows.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn handle_median_expr(
    expanded_args: &[SymbolicExpr],
    expanded_rows: &[SymbolicExpr],
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let if_branches: Vec<SymbolicExpr> = expanded_args.to_vec();
    let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(None, 0, 0));
    let median_exprs = expanded_rows
        .iter()
        .enumerate()
        .map(|(i, e)| {
            SymbolicExpr::if_then_else(
                SymbolicExpr::binary(
                    e.clone(),
                    SymbolicOperator::And,
                    SymbolicExpr::IsNotNull(Box::new(expanded_args[i].clone())),
                ),
                if_branches[i].clone(),
                else_branch.clone(),
            )
        })
        .map(|e| {
            expr_to_z3(
                &e,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        })
        .collect::<Vec<Z3Expr>>();

    match median_exprs[0].clone() {
        Z3Expr::Int(_, _) => {
            let int_exprs: Vec<(Int, Bool)> = median_exprs
                .iter()
                .map(|expr| match expr {
                    Z3Expr::Int(i, is_null) => (i.clone(), is_null.clone()),
                    _ => panic!("Expected all integer expressions"),
                })
                .collect();

            // For each non-null value, calculate its rank (number of non-null values less than or equal to it)
            let ranks: Vec<(Int, Bool)> = int_exprs
                .iter()
                .map(|(x, _)| {
                    int_exprs.iter().fold(
                        (Int::from_i64(0), Bool::from_bool(false)),
                        |(acc, acc_is_null), (y, y_is_null)| {
                            let is_less_or_equal = y.le(x);
                            let new_val = acc
                                + Bool::ite(
                                    &Bool::and(&[&is_less_or_equal, &y_is_null.not()]),
                                    &Int::from_i64(1),
                                    &Int::from_i64(0),
                                );
                            (new_val, Bool::and(&[acc_is_null, y_is_null.clone()]))
                        },
                    )
                })
                .collect();

            // The median is the value whose rank is (n+1)/2 for odd n, or either n/2 or (n/2)+1 for even n
            let non_null_count = int_exprs
                .iter()
                .fold(Int::from_i64(0), |acc, (_, is_null)| {
                    acc + Bool::ite(&is_null.not(), &Int::from_i64(1), &Int::from_i64(0))
                });
            let target_rank = (non_null_count + Int::from_i64(1)) / Int::from_i64(2);

            // Find the value with the target rank
            let (median, is_null) = int_exprs.iter().zip(ranks.iter()).fold(
                (int_exprs[0].0.clone(), Bool::from_bool(true)),
                |(acc, acc_is_null), ((val, val_is_null), (rank, _))| {
                    let is_median = rank.eq(&target_rank);
                    let new_val = Bool::ite(&is_median, val, &acc);
                    let new_is_null = Bool::and(&[acc_is_null, val_is_null.clone()]);
                    (new_val, new_is_null)
                },
            );

            Z3Expr::Int(median, is_null)
        }
        _ => panic!("Median expression must be of Int type"),
    }
}

/// Handles an aggregate expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `func` - The function.
/// * `arg` - The argument.
/// * `is_distinct` - Whether the aggregate is distinct.
/// * `generic_row` - The generic row.
///
/// # Returns
/// The Z3 expression.
#[allow(clippy::too_many_arguments)]
fn handle_aggregate_expr(
    func: &SymbolicAggregateFunction,
    arg: &SymbolicExpr,
    is_distinct: &bool,
    generic_row: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    let expanded_args: Vec<SymbolicExpr> = (1..=num_rows)
        .map(|row| generic_var_to_var(arg, row))
        .collect();
    if *is_distinct {
        handle_is_distinct_for_aggregate_expr(
            &expanded_args,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        );
    }

    let expanded_rows = expand_row_constraints(generic_row, num_rows);
    match func {
        SymbolicAggregateFunction::Count => handle_count_expr(
            &expanded_args,
            &expanded_rows,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicAggregateFunction::Sum => handle_sum_expr(
            &expanded_args,
            &expanded_rows,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        // Always return a real expression for avg and mean.
        SymbolicAggregateFunction::Avg | SymbolicAggregateFunction::Mean => {
            let sum_expr = handle_sum_expr(
                &expanded_args,
                &expanded_rows,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            );
            let count_expr = handle_count_expr(
                &expanded_args,
                &expanded_rows,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            );
            match (sum_expr, count_expr) {
                // If either is NULL then AVG is NULL
                (Z3Expr::Int(i, sum_is_null), Z3Expr::Int(c, count_is_null)) => Z3Expr::Real(
                    Real::from_int(&i.div(&c)),
                    Bool::or(&[sum_is_null, count_is_null]),
                ),
                // TODO: Add support for float.
                _ => panic!("Sum expression must be of Int type"),
            }
        }
        SymbolicAggregateFunction::Min => handle_min_expr(
            &expanded_args,
            &expanded_rows,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicAggregateFunction::Max => handle_max_expr(
            &expanded_args,
            &expanded_rows,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicAggregateFunction::Median => handle_median_expr(
            &expanded_args,
            &expanded_rows,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
    }
}

/// Converts a symbolic expression to a Z3 expression.
///
/// # Arguments
/// * `ctx` - The context.
/// * `expr` - The symbolic expression.
/// * `variables` - The variables.
/// * `variable_types` - The variable types.
/// * `additional_constraints` - The additional constraints.
/// * `num_rows` - The number of rows.
///
/// # Returns
/// The Z3 expression.
fn expr_to_z3(
    expr: &SymbolicExpr,
    variables: &mut Vec<Z3Expr>,
    variable_types: &mut HashMap<std::string::String, DataType>,
    additional_constraints: &mut Vec<Z3Expr>,
    num_rows: usize,
) -> Z3Expr {
    match expr {
        SymbolicExpr::Literal(val) => handle_literal(val),
        SymbolicExpr::Variable {
            table,
            column,
            row,
            arrow_type,
            z3_type,
            column_name,
        } => handle_variable(
            table,
            *column,
            *row,
            column_name,
            variables,
            variable_types,
            additional_constraints,
            z3_type,
            arrow_type,
        ),
        SymbolicExpr::InListExpr { left, right } => handle_in_list(
            left,
            right,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),

        SymbolicExpr::IfThenElse {
            condition,
            then_branch,
            else_branch,
        } => handle_if_then_else(
            condition,
            then_branch,
            else_branch,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),

        SymbolicExpr::AggregateExpr {
            func,
            arg,
            is_distinct,
            generic_row,
        } => handle_aggregate_expr(
            func,
            arg,
            is_distinct,
            generic_row,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicExpr::GenericRow { constraints, .. } => {
            let constraints = conjunct_exprs(constraints.as_ref().unwrap()).unwrap();
            expr_to_z3(
                &constraints,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            )
        }
        SymbolicExpr::GenericVar { .. } => expr_to_z3(
            &generic_var_to_var(expr, num_rows),
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicExpr::BinaryExpr { left, op, right } => handle_binary_expr(
            left,
            op,
            right,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicExpr::Not(expr) => {
            let expr = expr_to_z3(
                expr,
                variables,
                variable_types,
                additional_constraints,
                num_rows,
            );
            match expr {
                Z3Expr::Bool(expr, expr_is_null) => Z3Expr::Bool(expr.not(), expr_is_null),
                _ => panic!("Expected boolean expression but got {expr:?}"),
            }
        }
        SymbolicExpr::IsNotNull(expr) => handle_is_not_null_expr(
            expr,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        SymbolicExpr::IsNull(expr) => handle_is_null_expr(
            expr,
            variables,
            variable_types,
            additional_constraints,
            num_rows,
        ),
        _ => panic!("Unsupported expression: {expr:?}"),
    }
}

/// Conjuncts Z3 expressions.
///
/// # Arguments
/// * `ctx` - The context.
/// * `exprs` - The expressions to conjunct.
///
/// # Returns
/// The conjuncted Z3 expression.
fn conjunct_z3_exprs(exprs: &[Z3Expr]) -> Z3Expr {
    let mut conjuncted = exprs[0].clone();
    for expr in exprs.iter().skip(1) {
        let (expr, expr_is_null) = match expr {
            Z3Expr::Bool(b, b_is_null) => (b, b_is_null),
            _ => panic!("Expected Bool expression"),
        };
        let (conjuncted_bool, conjuncted_bool_is_null) = match conjuncted {
            Z3Expr::Bool(b, b_is_null) => (b, b_is_null),
            _ => panic!("Expected Bool expression"),
        };
        conjuncted = Z3Expr::Bool(
            Bool::and(&[&conjuncted_bool, expr]),
            Bool::or(&[conjuncted_bool_is_null, expr_is_null.clone()]),
        );
    }
    conjuncted
}

/// Returns the total number of AST nodes in the given Z3 expression (this node + all descendants).
fn count_z3_ast_nodes(ast: &Dynamic) -> usize {
    1 + ast.children().iter().map(count_z3_ast_nodes).sum::<usize>()
}

/// Solves a symbolic constraint.
///
/// # Arguments
/// * `expr` - The symbolic expression.
/// * `num_rows` - The number of rows.
///
pub fn solve_constraint(
    expr: &SymbolicExpr,
    num_rows: usize,
    timing: &mut Option<&mut ConcolicProfile>,
) -> Option<HashMap<std::string::String, SymbolicScalarValue>> {
    let solver = Solver::new();
    let mut additional_constraints = Vec::new();
    let mut variables = Vec::new();
    let mut variable_types: HashMap<std::string::String, DataType> = HashMap::new();
    let mut var_map = HashMap::new();
    let t0 = Instant::now();

    let z3_expr = expr_to_z3(
        expr,
        &mut variables,
        &mut variable_types,
        &mut additional_constraints,
        num_rows,
    );
    additional_constraints.push(z3_expr);
    let z3_expr = conjunct_z3_exprs(&additional_constraints);
    if let Some(t) = timing.as_mut() {
        t.lowering_fol_ms.push(t0.elapsed().as_millis() as u64);
        t.solver_iterations += 1;
        t.symbolic_variables.push(2 * variables.len());
    }
    match z3_expr {
        Z3Expr::Bool(b, b_is_null) => {
            if let Some(t) = timing.as_mut() {
                let b_nodes = count_z3_ast_nodes(&Dynamic::from(b.clone()));
                let b_is_null_nodes = count_z3_ast_nodes(&Dynamic::from(b_is_null.clone()));
                t.fol_ast_nodes.push(b_nodes + b_is_null_nodes);
            }
            // Constraint must be TRUE (not NULL): in SQL, WHERE NULL excludes the row.
            let t0 = Instant::now();
            solver.assert(Bool::and(&[b, b_is_null.not()]));
            // Check if there's a solution
            let soln = solver.check();
            if let Some(t) = timing.as_mut() {
                t.z3_solver_ms.push(t0.elapsed().as_millis() as u64);
            }
            match soln {
                SatResult::Sat => {
                    if let Some(t) = timing.as_mut() {
                        t.sat_solutions += 1;
                    }
                    let model = solver.get_model().unwrap();
                    for var in variables {
                        let (value, name) = match var {
                            Z3Expr::Int(i, i_is_null) => {
                                let value = model.eval(&i, true).unwrap().as_i64().unwrap();
                                let is_null =
                                    model.eval(&i_is_null, true).unwrap().as_bool().unwrap();
                                let var_name = i.decl().name().to_string();
                                let var_type = variable_types.get(&var_name).unwrap();
                                if is_null {
                                    (SymbolicScalarValue::Null, var_name)
                                } else {
                                    match var_type {
                                        DataType::Int64
                                        | DataType::UInt64
                                        | DataType::Int32
                                        | DataType::UInt32
                                        | DataType::Int16
                                        | DataType::UInt16
                                        | DataType::Int8
                                        | DataType::UInt8 => (
                                            SymbolicScalarValue::Decimal128(
                                                Some(value as i128),
                                                38,
                                                0,
                                            ),
                                            var_name,
                                        ),
                                        DataType::Decimal128(precision, scale) => match scale {
                                            0 => (
                                                SymbolicScalarValue::Decimal128(
                                                    Some(value as i128),
                                                    *precision,
                                                    *scale,
                                                ),
                                                var_name,
                                            ),
                                            _ => panic!("Unsupported scale for decimal: {scale:?}"),
                                        },
                                        DataType::Date32 => {
                                            (SymbolicScalarValue::Date32(Some(value)), var_name)
                                        }
                                        DataType::Time64(unit) => (
                                            SymbolicScalarValue::Time64(Some(value), *unit),
                                            var_name,
                                        ),
                                        DataType::Time32(unit) => (
                                            SymbolicScalarValue::Time32(Some(value as i32), *unit),
                                            var_name,
                                        ),
                                        DataType::Timestamp(unit, _) => (
                                            SymbolicScalarValue::Timestamp(
                                                Some(value),
                                                *unit,
                                                "".to_string(), // TODO: add timezone support. eg: "UTC"
                                            ),
                                            var_name,
                                        ),
                                        // This handles the case where the variable is a Timestamp, Time32 or Time64
                                        // when optimizations are turned on.
                                        DataType::FixedSizeList(field, size) => match size {
                                            1 => {
                                                let var_name = i.decl().name().to_string();
                                                let var_type = field.data_type();
                                                match var_type {
                                                    DataType::Timestamp(unit, None) => (
                                                        SymbolicScalarValue::Timestamp(
                                                            Some(value),
                                                            *unit,
                                                            field.name().to_string(),
                                                        ),
                                                        var_name,
                                                    ),
                                                    DataType::Struct(struct_fields)
                                                        if struct_fields
                                                            .iter()
                                                            .any(|f| f.name() == "timestamp") =>
                                                    {
                                                        // Helper to extract a time-like scalar from a Struct with a 'timestamp' field
                                                        let extract_from_struct = |value: i64, struct_fields: &arrow::datatypes::Fields| -> SymbolicScalarValue {
                                                            let ts_field = struct_fields
                                                                .iter()
                                                                .find(|f| f.name() == "timestamp")
                                                                .unwrap();
                                                            match ts_field.data_type() {
                                                                DataType::Timestamp(unit, tz) => SymbolicScalarValue::Timestamp(
                                                                    Some(value),
                                                                    *unit,
                                                                    tz.clone().map(|s| s.to_string()).unwrap_or_default(),
                                                                ),
                                                                DataType::Date32 => SymbolicScalarValue::Date32(Some(value)),
                                                                DataType::Time32(unit) => SymbolicScalarValue::Time32(Some(value as i32), *unit),
                                                                DataType::Time64(unit) => SymbolicScalarValue::Time64(Some(value), *unit),
                                                                other => panic!(
                                                                    "Unsupported inner timestamp field type in Struct: {other:?}"
                                                                ),
                                                            }
                                                        };
                                                        (
                                                            extract_from_struct(
                                                                value,
                                                                struct_fields,
                                                            ),
                                                            var_name,
                                                        )
                                                    }
                                                    DataType::Time32(unit) => (
                                                        SymbolicScalarValue::Time32(
                                                            Some(value as i32),
                                                            *unit,
                                                        ),
                                                        var_name,
                                                    ),
                                                    DataType::Time64(unit) => (
                                                        SymbolicScalarValue::Time64(
                                                            Some(value),
                                                            *unit,
                                                        ),
                                                        var_name,
                                                    ),
                                                    _ => panic!(
                                                        "Unsupported variable type: {var_type:?}"
                                                    ),
                                                }
                                            }
                                            _ => {
                                                panic!(
                                                    "Unsupported size for fixed size list: {size:?}"
                                                )
                                            }
                                        },
                                        _ => panic!("Unsupported variable type: {var_type:?}"),
                                    }
                                }
                            }
                            Z3Expr::Bool(b, b_is_null) => {
                                let is_null =
                                    model.eval(&b_is_null, true).unwrap().as_bool().unwrap();
                                let value = model.eval(&b, true).unwrap().as_bool().unwrap();
                                if is_null {
                                    (SymbolicScalarValue::Null, b.decl().name().to_string())
                                } else {
                                    (
                                        SymbolicScalarValue::Boolean(Some(value)),
                                        b.decl().name().to_string(),
                                    )
                                }
                            }
                            Z3Expr::String(s, s_is_null) => {
                                let is_null =
                                    model.eval(&s_is_null, true).unwrap().as_bool().unwrap();
                                let value = model.eval(&s, true).unwrap().as_string().unwrap();
                                if is_null {
                                    (SymbolicScalarValue::Null, s.decl().name().to_string())
                                } else {
                                    (
                                        SymbolicScalarValue::Utf8(Some(value)),
                                        s.decl().name().to_string(),
                                    )
                                }
                            }
                            Z3Expr::Real(r, r_is_null) => {
                                let is_null =
                                    model.eval(&r_is_null, true).unwrap().as_bool().unwrap();
                                let value = model.eval(&r, true).unwrap().as_rational().unwrap();
                                if is_null {
                                    (SymbolicScalarValue::Null, r.decl().name().to_string())
                                } else {
                                    let (num, den) = value;
                                    let float_val = (num as f64) / (den as f64);
                                    (
                                        SymbolicScalarValue::Float64(Some(float_val)),
                                        r.decl().name().to_string(),
                                    )
                                }
                            }
                        };
                        var_map.insert(name, value);
                    }
                    Some(var_map)
                }
                SatResult::Unsat => {
                    eprintln!("No solution found.");
                    if let Some(t) = timing.as_mut() {
                        t.unsat_solutions += 1;
                    }
                    None
                }
                SatResult::Unknown => {
                    eprintln!("Solver could not determine if a solution exists.");
                    if let Some(t) = timing.as_mut() {
                        t.unknown_solutions += 1;
                    }
                    None
                }
            }
        }
        _ => panic!("Unsupported expression type"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::symbolic_nodes::SymbolicScalarValue;

    #[test]
    fn test_simple_relational_constraint() {
        // T_0_0 + 2 > 5
        let lhs = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "T".to_string(),
                0,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Plus,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let rhs = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(5), 0, 0));
        let expr = SymbolicExpr::binary(lhs, SymbolicOperator::Greater, rhs);
        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result = result.get("T.0.0.").unwrap();
        let lhs_val = match result {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result:?}"),
        };
        assert!((lhs_val.unwrap() + 2) > 5);
    }

    #[test]
    fn test_complex_relational_constraint() {
        // T_a_0 + 2 > 4 & T_a_0 != 3
        let lhs1 = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "T".to_string(),
                0,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Plus,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let rhs1 = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(4), 0, 0));
        let expr1 = SymbolicExpr::binary(lhs1, SymbolicOperator::Greater, rhs1);

        let lhs2 = SymbolicExpr::variable(
            "T".to_string(),
            0,
            0,
            DataType::Int64,
            SymbolicZ3Type::Int,
            "".to_string(),
        );
        let rhs2 = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(3), 0, 0));
        let expr2 = SymbolicExpr::binary(lhs2, SymbolicOperator::NotEqual, rhs2);

        let expr = SymbolicExpr::binary(expr1, SymbolicOperator::And, expr2);

        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result = result.get("T.0.0.").unwrap();
        let lhs_val = match result {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result:?}"),
        };
        assert!((lhs_val.unwrap() + 2) > 4);
        assert!(lhs_val.unwrap() != 3);
    }

    #[test]
    fn test_complex_relational_constraint_with_not() {
        // T_a_0 + 2 > 4 & !(T_a_0 != 3)
        let lhs1 = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "T".to_string(),
                0,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Plus,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let rhs1 = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(4), 0, 0));
        let expr1 = SymbolicExpr::binary(lhs1, SymbolicOperator::Greater, rhs1);

        let lhs2 = SymbolicExpr::variable(
            "T".to_string(),
            0,
            0,
            DataType::Int64,
            SymbolicZ3Type::Int,
            "".to_string(),
        );
        let rhs2 = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(3), 0, 0));
        let expr2 = SymbolicExpr::binary(lhs2, SymbolicOperator::NotEqual, rhs2);

        let expr = SymbolicExpr::binary(
            expr1,
            SymbolicOperator::And,
            SymbolicExpr::Not(Box::new(expr2)),
        );

        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result = result.get("T.0.0.").unwrap();
        let lhs_val = match result {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result:?}"),
        };
        assert!((lhs_val.unwrap() + 2) > 4);
        assert!(lhs_val.unwrap() == 3);
    }

    #[test]
    fn test_simple_relational_mv() {
        // T_0_0 + 2 > T_1_1
        let lhs = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "T".to_string(),
                0,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Plus,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let rhs = SymbolicExpr::variable(
            "T".to_string(),
            1,
            1,
            DataType::Int64,
            SymbolicZ3Type::Int,
            "".to_string(),
        );
        let expr = SymbolicExpr::binary(lhs, SymbolicOperator::Greater, rhs);
        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result1 = result.get("T.0.0.").unwrap();
        let lhs_val = match result1 {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result1:?}"),
        };
        let result2 = result.get("T.1.1.").unwrap();
        let rhs_val = match result2 {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result2:?}"),
        };
        assert!((lhs_val.unwrap() + 2) > rhs_val.unwrap());
    }

    #[test]
    fn test_if_then_else_then_branch() {
        // I1_0 = If (I1_2_0 > 2, 1, 0)
        let cond = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "I1".to_string(),
                2,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Greater,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let then_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(1), 0, 0));
        let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(0), 0, 0));
        let ite_expr = SymbolicExpr::if_then_else(cond, then_branch.clone(), else_branch);
        let expr = SymbolicExpr::binary(ite_expr, SymbolicOperator::Equal, then_branch);
        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result = result.get("I1.2.0.").unwrap();
        let lhs_val = match result {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result:?}"),
        };
        assert!(lhs_val.unwrap() != 2);
    }

    #[test]
    fn test_if_then_else_else_branch() {
        // I1_0 = If (I1_2_0 > 2, 1, 0)
        let cond = SymbolicExpr::binary(
            SymbolicExpr::variable(
                "I1".to_string(),
                2,
                0,
                DataType::Int64,
                SymbolicZ3Type::Int,
                "".to_string(),
            ),
            SymbolicOperator::Greater,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let then_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(1), 0, 0));
        let else_branch = SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(0), 0, 0));
        let ite_expr = SymbolicExpr::if_then_else(cond, then_branch, else_branch.clone());
        let expr = SymbolicExpr::binary(ite_expr, SymbolicOperator::Equal, else_branch);
        let result = solve_constraint(&expr, 1, &mut None);
        let result = result.unwrap();
        let result = result.get("I1.2.0.").unwrap();
        let lhs_val = match result {
            SymbolicScalarValue::Decimal128(Some(i), _, _) => Some(*i),
            _ => panic!("Unsupported literal: {result:?}"),
        };
        assert!(lhs_val.unwrap() < 2);
    }

    #[test]
    fn test_count_constraint() {
        // COUNT(ite(I1_1_r > 2, 1, 0), distinct = I1_1_r) > 2
        let column = SymbolicExpr::generic_var(
            "I1".to_string(),
            1,
            DataType::Int64,
            SymbolicZ3Type::Int,
            "".to_string(),
        );
        let generic_row = SymbolicExpr::generic_row(
            "I1".to_string(),
            Some(vec![SymbolicExpr::binary(
                column.clone(),
                SymbolicOperator::Greater,
                SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
            )]),
        );
        let count_expr =
            SymbolicExpr::aggregate(SymbolicAggregateFunction::Count, column, true, generic_row);
        let expr = SymbolicExpr::binary(
            count_expr,
            SymbolicOperator::Greater,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let mut result = HashMap::new();
        for i in 1..10 {
            result = solve_constraint(&expr, i, &mut None).unwrap_or_default();
            if !result.is_empty() {
                break;
            }
        }
        assert!(!result.is_empty());
    }

    #[test]
    fn test_sum_constraint() {
        // SUM(I1_1_r, distinct = I1_1_r) > 2
        let column = SymbolicExpr::generic_var(
            "I1".to_string(),
            1,
            DataType::Int64,
            SymbolicZ3Type::Int,
            "".to_string(),
        );
        let generic_row = SymbolicExpr::generic_row(
            "I1".to_string(),
            Some(vec![SymbolicExpr::binary(
                column.clone(),
                SymbolicOperator::Greater,
                SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
            )]),
        );
        let sum_expr =
            SymbolicExpr::aggregate(SymbolicAggregateFunction::Sum, column, true, generic_row);
        let expr = SymbolicExpr::binary(
            sum_expr,
            SymbolicOperator::Greater,
            SymbolicExpr::literal(SymbolicScalarValue::Decimal128(Some(2), 0, 0)),
        );
        let mut result = HashMap::new();
        for i in 1..10 {
            result = solve_constraint(&expr, i, &mut None).unwrap_or_default();
            if !result.is_empty() {
                break;
            }
        }
        assert!(!result.is_empty());
    }
}
