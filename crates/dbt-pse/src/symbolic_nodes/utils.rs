/// Provides utility functions for working with symbolic expressions and types.
/// These functions handle common operations like type conversion, hole resolution,
/// and expression manipulation.
use crate::symbolic_nodes::{SymbolicExpr, SymbolicOperator, SymbolicScalarValue};

use arrow::array::{Array, BooleanArray, as_primitive_array};
use datafusion::physical_plan::expressions::Literal;
use datafusion_common::ScalarValue;
use datafusion_common::arrow::array::ArrayRef;
use datafusion_common::arrow::array::Date32Array;
use datafusion_common::arrow::array::Decimal128Array;
use datafusion_common::arrow::array::FixedSizeListArray;
use datafusion_common::arrow::array::LargeStringArray;
use datafusion_common::arrow::array::StringArray;
use datafusion_common::arrow::array::StringViewArray;
use datafusion_common::arrow::array::Time32MillisecondArray;
use datafusion_common::arrow::array::Time32SecondArray;
use datafusion_common::arrow::array::Time64MicrosecondArray;
use datafusion_common::arrow::array::Time64NanosecondArray;
use datafusion_common::arrow::array::TimestampMicrosecondArray;
use datafusion_common::arrow::array::TimestampMillisecondArray;
use datafusion_common::arrow::array::TimestampNanosecondArray;
use datafusion_common::arrow::array::TimestampSecondArray;
use datafusion_common::arrow::datatypes::DataType;
use datafusion_common::arrow::datatypes::Float64Type;
use datafusion_common::arrow::datatypes::Int32Type;
use datafusion_common::arrow::datatypes::Int64Type;
use datafusion_common::arrow::datatypes::TimeUnit;

use chrono::{DateTime, NaiveDate, NaiveTime};

/// Returns true if stats collection is enabled via SDF_SYM_COLLECT_STATS env var.
pub fn collect_stats_enabled() -> bool {
    match std::env::var("SDF_SYM_COLLECT_STATS") {
        Ok(v) => {
            let v_lower = v.to_ascii_lowercase();
            v == "1" || v_lower == "true" || v_lower == "yes"
        }
        Err(_) => false,
    }
}

/// Checks if a symbolic expression contains any hole expressions.
/// This is used to determine if an expression needs to be resolved with concrete values.
pub fn contains_hole(expr: &SymbolicExpr) -> bool {
    match expr {
        SymbolicExpr::Hole { .. } => true,
        SymbolicExpr::BinaryExpr { left, right, .. } => contains_hole(left) || contains_hole(right),
        SymbolicExpr::InListExpr { left, right } => {
            contains_hole(left) || right.iter().any(contains_hole)
        }
        SymbolicExpr::GenericRow { constraints, .. } => {
            constraints.as_ref().unwrap().iter().any(contains_hole)
        }
        SymbolicExpr::AggregateExpr {
            arg, generic_row, ..
        } => contains_hole(arg) || contains_hole(generic_row),
        SymbolicExpr::IfThenElse {
            condition,
            then_branch,
            else_branch,
        } => contains_hole(condition) || contains_hole(then_branch) || contains_hole(else_branch),
        SymbolicExpr::Not(inner) => contains_hole(inner),
        SymbolicExpr::IsNull(inner) => contains_hole(inner),
        SymbolicExpr::IsNotNull(inner) => contains_hole(inner),
        _ => false,
    }
}

// Extracts epoch value from any Timestamp array
fn any_timestamp_to_i64(array: &ArrayRef) -> i64 {
    match array.data_type() {
        DataType::Timestamp(TimeUnit::Microsecond, _) => array
            .as_any()
            .downcast_ref::<TimestampMicrosecondArray>()
            .unwrap()
            .value(0),
        DataType::Timestamp(TimeUnit::Millisecond, _) => array
            .as_any()
            .downcast_ref::<TimestampMillisecondArray>()
            .unwrap()
            .value(0),
        DataType::Timestamp(TimeUnit::Nanosecond, _) => array
            .as_any()
            .downcast_ref::<TimestampNanosecondArray>()
            .unwrap()
            .value(0),
        DataType::Timestamp(TimeUnit::Second, _) => array
            .as_any()
            .downcast_ref::<TimestampSecondArray>()
            .unwrap()
            .value(0),
        other => panic!("Unsupported timestamp array type: {other:?}"),
    }
}

// Finds a Timestamp field within a StructArray and returns its epoch value (i64)
fn struct_timestamp_field_to_i64(struct_arr: &arrow::array::StructArray) -> Option<i64> {
    // Prefer a field explicitly named "timestamp"
    for (i, name) in struct_arr.column_names().iter().enumerate() {
        if *name == "timestamp" {
            let col = struct_arr.column(i);
            if let DataType::Timestamp(_, _) = col.data_type() {
                return Some(any_timestamp_to_i64(col));
            }
        }
    }
    // Otherwise, use the first Timestamp-typed field
    for i in 0..struct_arr.num_columns() {
        let col = struct_arr.column(i);
        if let DataType::Timestamp(_, _) = col.data_type() {
            return Some(any_timestamp_to_i64(col));
        }
    }
    None
}

/// Given Arrow Struct fields that include a field named 'timestamp',
/// produce a SymbolicScalarValue representing that time-like value using the provided i64.
/// Returns None if no suitable 'timestamp' field exists or type is unsupported.
pub fn symbolic_scalar_from_struct_timestamp_fields(
    value: i64,
    struct_fields: &arrow::datatypes::Fields,
) -> Option<SymbolicScalarValue> {
    let ts_field = struct_fields.iter().find(|f| f.name() == "timestamp")?;
    match ts_field.data_type() {
        DataType::Timestamp(unit, tz) => Some(SymbolicScalarValue::Timestamp(
            Some(value),
            *unit,
            tz.clone().map(|s| s.to_string()).unwrap_or_default(),
        )),
        DataType::Date32 => Some(SymbolicScalarValue::Date32(Some(value))),
        DataType::Time32(unit) => Some(SymbolicScalarValue::Time32(Some(value as i32), *unit)),
        DataType::Time64(unit) => Some(SymbolicScalarValue::Time64(Some(value), *unit)),
        _ => None,
    }
}

/// Boolean context for hole replacement in predicates
#[derive(Copy, Clone)]
enum BoolCtx {
    AndCtx,
    OrCtx,
}

fn bool_true() -> SymbolicExpr {
    SymbolicExpr::Literal(SymbolicScalarValue::Boolean(Some(true)))
}
fn bool_false() -> SymbolicExpr {
    SymbolicExpr::Literal(SymbolicScalarValue::Boolean(Some(false)))
}

/// Simplifies a boolean predicate by replacing hole-containing atomic predicates
/// with context-appropriate constants (true in And-context, false in Or-context).
/// When `relax` is false, AND/OR reset the context for their children, which can make a
/// satisfiable predicate UNSAT (e.g. an OR whose branches all contain holes). When `relax`
/// is true, AND/OR keep the context and only NOT flips it, so the predicate is only relaxed.
fn simplify_holes_inner(expr: &SymbolicExpr, ctx: BoolCtx, relax: bool) -> (SymbolicExpr, bool) {
    match expr {
        // Preserve literals
        SymbolicExpr::Literal(_) => (expr.clone(), false),

        // Push through NOT by flipping context
        SymbolicExpr::Not(inner) => {
            let flipped = match ctx {
                BoolCtx::AndCtx => BoolCtx::OrCtx,
                BoolCtx::OrCtx => BoolCtx::AndCtx,
            };
            let (simplified_inner, dropped) = simplify_holes_inner(inner, flipped, relax);
            (SymbolicExpr::Not(Box::new(simplified_inner)), dropped)
        }

        // Conjunction
        SymbolicExpr::BinaryExpr { left, op, right } if matches!(op, SymbolicOperator::And) => {
            let child_ctx = if relax { ctx } else { BoolCtx::AndCtx };
            let (l, l_dropped) = simplify_holes_inner(left, child_ctx, relax);
            let (r, r_dropped) = simplify_holes_inner(right, child_ctx, relax);
            (
                SymbolicExpr::BinaryExpr {
                    left: Box::new(l),
                    op: *op,
                    right: Box::new(r),
                },
                l_dropped || r_dropped,
            )
        }

        // Disjunction
        SymbolicExpr::BinaryExpr { left, op, right } if matches!(op, SymbolicOperator::Or) => {
            let child_ctx = if relax { ctx } else { BoolCtx::OrCtx };
            let (l, l_dropped) = simplify_holes_inner(left, child_ctx, relax);
            let (r, r_dropped) = simplify_holes_inner(right, child_ctx, relax);
            (
                SymbolicExpr::BinaryExpr {
                    left: Box::new(l),
                    op: *op,
                    right: Box::new(r),
                },
                l_dropped || r_dropped,
            )
        }

        // GenericRow: simplify each constraint under And-context (constraints are conjunctive)
        SymbolicExpr::GenericRow { table, constraints } => {
            let mut any_dropped = false;
            let new_constraints = constraints.as_ref().map(|cs| {
                let mut out: Vec<SymbolicExpr> = Vec::with_capacity(cs.len());
                for c in cs.iter() {
                    let (sc, dropped) = simplify_holes_inner(c, BoolCtx::AndCtx, relax);
                    if dropped {
                        any_dropped = true;
                    }
                    out.push(sc);
                }
                out
            });
            (
                SymbolicExpr::GenericRow {
                    table: table.clone(),
                    constraints: new_constraints,
                },
                any_dropped,
            )
        }

        // InList, IfThenElse (boolean), IsNull, IsNotNull and all other non-combiner expressions are treated as atomic.
        // If such an atom contains a hole, replace with context-appropriate constant; else keep unchanged.
        _ => {
            if contains_hole(expr) {
                let replacement = match ctx {
                    BoolCtx::AndCtx => bool_true(),
                    BoolCtx::OrCtx => bool_false(),
                };
                (replacement, true)
            } else {
                (expr.clone(), false)
            }
        }
    }
}

/// Public entrypoint to simplify a predicate by replacing hole-containing atoms.
/// Starts in conjunctive (And) context.
pub fn simplify_holes(expr: &SymbolicExpr) -> (SymbolicExpr, bool) {
    simplify_holes_inner(expr, BoolCtx::AndCtx, false)
}

/// Like `simplify_holes`, but only ever relaxes the predicate, so it never makes it UNSAT.
/// Used as a fallback when `simplify_holes` has no solution.
pub fn simplify_holes_relaxed(expr: &SymbolicExpr) -> (SymbolicExpr, bool) {
    simplify_holes_inner(expr, BoolCtx::AndCtx, true)
}

/// Removes the rightmost conjunct from an AND tree (or the last constraint of a GenericRow)
/// and returns a new, smaller expression. Prints the removed node. Returns None if nothing to chop.
pub fn chop_rightmost_conjunct(expr: SymbolicExpr) -> Option<SymbolicExpr> {
    match expr {
        // Conjunctive list: GenericRow holds constraints combined by AND
        SymbolicExpr::GenericRow {
            table,
            constraints: Some(mut vec),
        } => {
            if vec.is_empty() {
                return None;
            }
            // Try to chop deeper into the right-most constraint first
            if let Some(last) = vec.pop() {
                if let Some(new_last) = chop_rightmost_conjunct(last.clone()) {
                    vec.push(new_last);
                    return Some(SymbolicExpr::GenericRow {
                        table,
                        constraints: Some(vec),
                    });
                } else {
                    eprintln!("Chopped conjunct: {:?}", last);
                    // If no constraints remain, replace with true
                    if vec.is_empty() {
                        return Some(bool_true());
                    } else {
                        return Some(SymbolicExpr::GenericRow {
                            table,
                            constraints: Some(vec),
                        });
                    }
                }
            }
            None
        }
        // Conjunctive binary tree: AND
        SymbolicExpr::BinaryExpr { left, op, right } if matches!(op, SymbolicOperator::And) => {
            // Try to chop on the right arm first
            if let Some(new_right) = chop_rightmost_conjunct(*right.clone()) {
                return Some(SymbolicExpr::BinaryExpr {
                    left,
                    op,
                    right: Box::new(new_right),
                });
            }
            // Otherwise, drop the right arm entirely (chop the rightmost leaf)
            eprintln!("Chopped conjunct: {:?}", right);
            Some(*left)
        }
        // For all other nodes, do not chop inside
        _ => None,
    }
}

/// Converts an Arrow array into a DataFusion literal expression.
/// This is used to convert concrete values from execution into a form
/// that can be used in symbolic expressions.
///
/// # Arguments
/// * `array` - The Arrow array to convert
///
/// # Returns
/// A DataFusion literal expression
///
/// # Panics
/// * If the array type is not supported
pub fn array_to_literal(array: &ArrayRef) -> Literal {
    let scalar_value = match array.data_type() {
        DataType::Boolean => ScalarValue::Boolean(Some(
            array
                .as_any()
                .downcast_ref::<BooleanArray>()
                .unwrap()
                .value(0),
        )),
        DataType::Utf8 => ScalarValue::Utf8(Some(
            array
                .as_any()
                .downcast_ref::<StringArray>()
                .unwrap()
                .value(0)
                .to_string(),
        )),
        DataType::Utf8View => ScalarValue::Utf8(Some(
            array
                .as_any()
                .downcast_ref::<StringViewArray>()
                .unwrap()
                .value(0)
                .to_string(),
        )),
        DataType::LargeUtf8 => ScalarValue::Utf8(Some(
            array
                .as_any()
                .downcast_ref::<LargeStringArray>()
                .unwrap()
                .value(0)
                .to_string(),
        )),
        DataType::Int32 => {
            ScalarValue::Int32(Some(as_primitive_array::<Int32Type>(array).value(0)))
        }
        DataType::Int64 => {
            ScalarValue::Int64(Some(as_primitive_array::<Int64Type>(array).value(0)))
        }
        DataType::Decimal128(precision, scale) => {
            let array = array.as_any().downcast_ref::<Decimal128Array>().unwrap();
            if array.is_null(0) {
                ScalarValue::Decimal128(None, *precision, *scale)
            } else {
                let value = array.value(0);
                ScalarValue::Decimal128(Some(value), *precision, *scale)
            }
        }
        DataType::Date32 => {
            let value = ScalarValue::Date32(Some(
                array
                    .as_any()
                    .downcast_ref::<Date32Array>()
                    .unwrap()
                    .value(0),
            ));
            value.cast_to(&DataType::Int64).unwrap()
        }
        DataType::Timestamp(_, _) => ScalarValue::Int64(Some(any_timestamp_to_i64(array))),
        DataType::Time64(unit) => {
            let value = match unit {
                TimeUnit::Microsecond => ScalarValue::Time64Microsecond(Some(
                    array
                        .as_any()
                        .downcast_ref::<Time64MicrosecondArray>()
                        .unwrap()
                        .value(0),
                )),
                TimeUnit::Millisecond => ScalarValue::Time64Nanosecond(Some(
                    array
                        .as_any()
                        .downcast_ref::<Time64NanosecondArray>()
                        .unwrap()
                        .value(0),
                )),
                _ => panic!("Unsupported time64 unit: {unit:?}"),
            };
            value.cast_to(&DataType::Int64).unwrap()
        }
        DataType::Time32(unit) => {
            let value = match unit {
                TimeUnit::Second => ScalarValue::Time32Second(Some(
                    array
                        .as_any()
                        .downcast_ref::<Time32SecondArray>()
                        .unwrap()
                        .value(0),
                )),
                TimeUnit::Millisecond => ScalarValue::Time32Millisecond(Some(
                    array
                        .as_any()
                        .downcast_ref::<Time32MillisecondArray>()
                        .unwrap()
                        .value(0),
                )),
                _ => panic!("Unsupported time32 unit: {unit:?}"),
            };
            value.cast_to(&DataType::Int64).unwrap()
        }
        DataType::Float64 => {
            let arr = as_primitive_array::<Float64Type>(array);
            if arr.is_null(0) {
                ScalarValue::Float64(None)
            } else {
                ScalarValue::Float64(Some(arr.value(0)))
            }
        }
        DataType::FixedSizeList(_, size) => match size {
            1 => {
                let array = array.as_any().downcast_ref::<FixedSizeListArray>().unwrap();
                // Get the first (and only) value from the fixed size list
                array_to_literal(&array.value(0)).value().clone()
            }
            _ => panic!("Unexpected fixed size list size: {size}"),
        },
        DataType::Struct(_fields) => {
            let struct_arr = array
                .as_any()
                .downcast_ref::<arrow::array::StructArray>()
                .unwrap();
            if let Some(v) = struct_timestamp_field_to_i64(struct_arr) {
                ScalarValue::Int64(Some(v))
            } else {
                panic!(
                    "Unsupported struct array type: no timestamp field found: {:?}",
                    array.data_type()
                );
            }
        }
        _ => panic!("Unsupported array type: {:?}", array.data_type()),
    };

    Literal::new(scalar_value)
}

/// Resolves holes in a symbolic expression by replacing them with concrete values.
///
/// # Arguments
/// * `expr_id_to_resolve` - The ID of the hole to resolve
/// * `value` - The concrete value to use
/// * `expr` - The expression containing holes
///
/// # Returns
/// A new expression with the specified hole replaced
pub fn resolve_holes(
    expr_id_to_resolve: usize,
    value: SymbolicExpr,
    expr: SymbolicExpr,
) -> SymbolicExpr {
    match expr.clone() {
        SymbolicExpr::Hole { expr_id, .. } => {
            if expr_id == expr_id_to_resolve {
                value
            } else {
                expr
            }
        }
        SymbolicExpr::BinaryExpr { left, right, op } => SymbolicExpr::BinaryExpr {
            left: Box::new(resolve_holes(expr_id_to_resolve, value.clone(), *left)),
            right: Box::new(resolve_holes(expr_id_to_resolve, value, *right)),
            op,
        },
        SymbolicExpr::GenericRow { table, constraints } => SymbolicExpr::GenericRow {
            table,
            constraints: constraints.map(|c| {
                c.iter()
                    .map(|c| resolve_holes(expr_id_to_resolve, value.clone(), c.clone()))
                    .collect()
            }),
        },
        SymbolicExpr::InListExpr { left, right } => SymbolicExpr::InListExpr {
            left: Box::new(resolve_holes(expr_id_to_resolve, value.clone(), *left)),
            right: right
                .iter()
                .map(|c| resolve_holes(expr_id_to_resolve, value.clone(), c.clone()))
                .collect(),
        },
        SymbolicExpr::AggregateExpr {
            func,
            arg,
            is_distinct,
            generic_row,
        } => SymbolicExpr::AggregateExpr {
            func,
            arg,
            is_distinct,
            generic_row: Box::new(resolve_holes(expr_id_to_resolve, value, *generic_row)),
        },
        SymbolicExpr::IfThenElse {
            condition,
            then_branch,
            else_branch,
        } => SymbolicExpr::IfThenElse {
            condition: Box::new(resolve_holes(expr_id_to_resolve, value.clone(), *condition)),
            then_branch: Box::new(resolve_holes(
                expr_id_to_resolve,
                value.clone(),
                *then_branch,
            )),
            else_branch: Box::new(resolve_holes(expr_id_to_resolve, value, *else_branch)),
        },
        _ => expr,
    }
}

/// Converts an Arrow data type to its corresponding SQL type string.
/// This is used when generating SQL statements from symbolic expressions.
///
/// # Arguments
/// * `arrow_type` - The Arrow data type to convert
///
/// # Returns
/// The SQL type as a string
///
/// # Panics
/// * If the data type is not supported
pub fn arrow_type_to_sql_type(arrow_type: DataType) -> String {
    match arrow_type {
        DataType::Boolean => "BOOLEAN".to_string(),
        DataType::Utf8 => "VARCHAR".to_string(),
        DataType::LargeUtf8 => "VARCHAR".to_string(),
        DataType::Binary => "VARIANT".to_string(),
        DataType::Int8 => "TINYINT".to_string(),
        DataType::Int16 => "SMALLINT".to_string(),
        DataType::Int32 => "INTEGER".to_string(),
        DataType::Int64 => "BIGINT".to_string(),
        DataType::UInt8 => "TINYINT".to_string(),
        DataType::UInt16 => "SMALLINT".to_string(),
        DataType::UInt32 => "INTEGER".to_string(),
        DataType::UInt64 => "BIGINT".to_string(),
        DataType::Float16 => "FLOAT".to_string(),
        DataType::Float32 => "FLOAT".to_string(),
        DataType::Float64 => "DOUBLE".to_string(),
        DataType::Decimal128(_, scale) => match scale {
            0 => "INTEGER".to_string(),
            _ => panic!("Unsupported decimal scale: {scale}"),
        },
        DataType::Date32 => "DATE".to_string(),
        DataType::Timestamp(_, tz) => match tz {
            Some(_) => "TIMESTAMP_TZ".to_string(),
            None => "TIMESTAMP_NTZ".to_string(),
        },
        DataType::Time64(_) | DataType::Time32(_) => "TIME".to_string(),
        DataType::FixedSizeList(field, size) => match size {
            1 => arrow_type_to_sql_type(field.data_type().clone()),
            _ => panic!("Unsupported fixed size list size: {size}"),
        },
        DataType::Struct(fields) => {
            // Prefer a field named 'timestamp' if present; otherwise use first field
            if let Some(ts_field) = fields.iter().find(|f| f.name() == "timestamp") {
                arrow_type_to_sql_type(ts_field.data_type().clone())
            } else {
                arrow_type_to_sql_type(fields[0].data_type().clone())
            }
        }
        _ => panic!("Unsupported data type: {arrow_type:?}"),
    }
}

/// Combines multiple expressions with AND operators.
///
/// # Arguments
/// * `exprs` - The expressions to combine
///
/// # Returns
/// A new expression that is the conjunction of all input expressions,
/// or None if the input is empty
pub fn conjunct_exprs(exprs: &[SymbolicExpr]) -> Option<SymbolicExpr> {
    match exprs.len() {
        0 => None,
        1 => Some(exprs[0].clone()),
        _ => {
            let mut conjuncted_expr = exprs[0].clone();
            for item in exprs.iter().skip(1) {
                conjuncted_expr =
                    SymbolicExpr::binary(conjuncted_expr, SymbolicOperator::And, item.clone());
            }
            Some(conjuncted_expr)
        }
    }
}

/// Converts a numeric timestamp to a datetime string.
/// This ensures consistent formatting of timestamps across different time units.
///
/// # Arguments
/// * `i` - The timestamp value
/// * `unit` - The time unit of the timestamp
///
/// # Returns
/// A formatted datetime string with nanosecond precision
pub fn num_timestamp_to_datetime(i: i64, unit: TimeUnit) -> String {
    let datetime = match unit {
        TimeUnit::Second => DateTime::from_timestamp(i, 0),
        TimeUnit::Millisecond => DateTime::from_timestamp(i / 1000, (i % 1000) as u32 * 1_000_000),
        TimeUnit::Microsecond => DateTime::from_timestamp_micros(i),
        TimeUnit::Nanosecond => {
            DateTime::from_timestamp(i / 1_000_000_000, (i % 1_000_000_000) as u32)
        }
    }
    .unwrap();
    // We always return to nearest nanosecond precision to ensure Z3 solution matches the
    // precision of the original data
    format!("'{}'", datetime.format("%Y-%m-%d %H:%M:%S%.9f"))
}

/// Converts a numeric time value to a time string.
/// This ensures consistent formatting of time values across different time units.
///
/// # Arguments
/// * `i` - The time value in the specified unit
/// * `unit` - The time unit of the value
///
/// # Returns
/// A formatted time string with nanosecond precision
pub fn num_to_time(i: i64, unit: TimeUnit) -> String {
    let nanos = match unit {
        TimeUnit::Second => i * 1_000_000_000,
        TimeUnit::Millisecond => i * 1_000_000,
        TimeUnit::Microsecond => i * 1_000,
        TimeUnit::Nanosecond => i,
    };

    let secs = nanos / 1_000_000_000;
    let nano_remainder = (nanos % 1_000_000_000) as u32;

    let time = NaiveTime::from_num_seconds_from_midnight_opt(secs as u32, nano_remainder).unwrap();
    format!("'{}'", time.format("%H:%M:%S%.9f"))
}

/// Converts a number of days since epoch to a date string.
///
/// # Arguments
/// * `days_since_epoch` - Number of days since 1970-01-01
///
/// # Returns
/// A formatted date string
pub fn num_days_to_date(days_since_epoch: i64) -> String {
    let date = NaiveDate::from_ymd_opt(1970, 1, 1)
        .unwrap()
        .checked_add_days(chrono::Days::new(days_since_epoch as u64))
        .unwrap();
    format!("'{}'", date.format("%Y-%m-%d"))
}

/// Checks if a symbolic expression contains any generic variables.
/// This is used to determine if a query can be solved with symbolic execution.
///
/// # Arguments
/// * `expr` - The expression to check
///
/// # Returns
/// True if the expression contains any generic variables, false otherwise
pub fn contains_gen_var(expr: &SymbolicExpr) -> bool {
    match expr {
        SymbolicExpr::GenericVar { .. } => true,
        SymbolicExpr::GenericRow {
            table: _,
            constraints,
        } => constraints.as_ref().unwrap().iter().any(contains_gen_var),
        SymbolicExpr::BinaryExpr { left, right, .. } => {
            contains_gen_var(left) || contains_gen_var(right)
        }
        SymbolicExpr::InListExpr { left, right } => {
            contains_gen_var(left) || (right.iter().any(contains_gen_var))
        }
        SymbolicExpr::AggregateExpr {
            arg, generic_row, ..
        } => contains_gen_var(arg) || contains_gen_var(generic_row),
        SymbolicExpr::IfThenElse {
            condition,
            then_branch,
            else_branch,
        } => {
            contains_gen_var(condition)
                || contains_gen_var(then_branch)
                || contains_gen_var(else_branch)
        }
        SymbolicExpr::Not(expr) => contains_gen_var(expr),
        SymbolicExpr::IsNull(expr) => contains_gen_var(expr),
        SymbolicExpr::IsNotNull(expr) => contains_gen_var(expr),
        SymbolicExpr::Literal(_) => false,
        SymbolicExpr::Variable { .. } => false,
        SymbolicExpr::Hole { .. } => false,
    }
}
