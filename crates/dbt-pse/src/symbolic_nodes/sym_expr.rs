/// Provides functionality for symbolic expressions used in symbolic execution.
/// These expressions represent the symbolic form of physical expressions and are used
/// to build constraints for test generation.
use crate::symbolic_nodes::num_days_to_date;
use crate::symbolic_nodes::num_timestamp_to_datetime;
use crate::symbolic_nodes::num_to_time;

use arrow_schema::{DataType, IntervalUnit, TimeUnit};
use datafusion::physical_plan::PhysicalExpr;
use std::sync::Arc;

/// Represents literal values in symbolic expressions.
/// These map directly to DataFusion's scalar values but are simplified
/// to only include types that can be represented in Z3.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    /// Represents `DataType::Null` (castable to/from any other type)
    Null,
    /// true or false value
    Boolean(Option<bool>),
    /// 32bit float
    Float32(Option<f32>),
    /// 64bit float
    Float64(Option<f64>),
    /// 128bit decimal, using the i128 to represent the decimal, precision scale
    Decimal128(Option<i128>, u8, i8),
    /// signed 8bit int
    Int8(Option<i8>),
    /// signed 16bit int
    Int16(Option<i16>),
    /// signed 32bit int
    Int32(Option<i32>),
    /// signed 64bit int
    Int64(Option<i64>),
    /// unsigned 8bit int
    UInt8(Option<u8>),
    /// unsigned 16bit int
    UInt16(Option<u16>),
    /// unsigned 32bit int
    UInt32(Option<u32>),
    /// unsigned 64bit int
    UInt64(Option<u64>),
    /// utf-8 encoded string
    Utf8(Option<String>),
    /// date32 value
    Date32(Option<i64>),
    /// timestamp value with timezone
    Timestamp(Option<i64>, TimeUnit, String),
    /// time64 value with microsecond/nanosecond precision
    Time64(Option<i64>, TimeUnit),
    /// time32 value with second/millisecond precision
    Time32(Option<i32>, TimeUnit),
}

impl ScalarValue {
    /// Converts the scalar value to a string representation.
    /// This is used for displaying values in a human-readable format.
    pub fn to_string_value(&self) -> String {
        match self {
            ScalarValue::Utf8(Some(s)) => s.clone(),
            ScalarValue::Decimal128(Some(d), _, _) => d.to_string(),
            ScalarValue::Int64(Some(i)) => i.to_string(),
            ScalarValue::Int32(Some(i)) => i.to_string(),
            ScalarValue::Int16(Some(i)) => i.to_string(),
            ScalarValue::Int8(Some(i)) => i.to_string(),
            ScalarValue::UInt64(Some(i)) => i.to_string(),
            ScalarValue::UInt32(Some(i)) => i.to_string(),
            ScalarValue::UInt16(Some(i)) => i.to_string(),
            ScalarValue::UInt8(Some(i)) => i.to_string(),
            ScalarValue::Boolean(Some(b)) => b.to_string(),
            ScalarValue::Date32(Some(i)) => num_days_to_date(*i),
            ScalarValue::Timestamp(Some(i), unit, _timezone) => {
                num_timestamp_to_datetime(*i, *unit)
            }
            ScalarValue::Time64(Some(i), unit) => num_to_time(*i, *unit),
            ScalarValue::Time32(Some(i), unit) => num_to_time(*i as i64, *unit),
            ScalarValue::Float32(Some(f)) => f.to_string(),
            ScalarValue::Float64(Some(f)) => f.to_string(),
            ScalarValue::Null => "NULL".to_string(),
            _ => panic!("Unexpected scalar value: {self:?}"),
        }
    }
}

/// Represents the types available in Z3 for symbolic reasoning.
/// DataFusion types are mapped to these simpler types that Z3 can handle.
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Z3Type {
    /// A boolean variable
    Boolean,
    /// A string variable
    String,
    /// An int variable
    Int,
    /// A float variable
    Float,
}

impl Z3Type {
    /// Maps a DataFusion column type to its corresponding Z3 type.
    /// See https://datafusion.apache.org/user-guide/sql/data_types.html#numeric-types
    /// for the complete list of DataFusion types.
    pub fn from_column_type(column_type: DataType) -> Self {
        match column_type {
            DataType::Boolean => Z3Type::Boolean,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Binary => Z3Type::String,
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64 => Z3Type::Int,
            DataType::Float32 | DataType::Float64 => Z3Type::Float,
            DataType::Decimal128(_, scale) => match scale {
                0 => Z3Type::Int,
                _ => Z3Type::Float,
            },
            DataType::Date32
            | DataType::Time64(_)
            | DataType::Timestamp(_, _)
            | DataType::Time32(_) => Z3Type::Int,
            DataType::Interval(IntervalUnit::MonthDayNano) => Z3Type::Int,
            DataType::FixedSizeList(field, size) => match size {
                1 => Self::from_column_type(field.data_type().clone()),
                _ => panic!("Unexpected fixed size list size: {size}"),
            },
            DataType::Struct(fields) => Self::from_column_type(fields[0].data_type().clone()),
            _ => panic!("Unexpected data type: {column_type:?}"),
        }
    }
}

/// Represents operators that can be applied to symbolic expressions.
/// These map to DataFusion's physical operators but are simplified
/// to only include operations that can be represented in Z3.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum Operator {
    /// Expressions are equal
    Equal,
    /// Expressions are not equal
    NotEqual,
    /// Left side is smaller than right side
    Less,
    /// Left side is smaller or equal to right side
    LessEqual,
    /// Left side is greater than right side
    Greater,
    /// Left side is greater or equal to right side
    GreaterEqual,
    /// Addition
    Plus,
    /// Subtraction
    Minus,
    /// Multiplication operator, like `*`
    Multiply,
    /// Division operator, like `/`
    Divide,
    /// Remainder operator, like `%`
    Modulo,
    /// Logical AND, like `&&`
    And,
    /// Logical OR, like `||`
    Or,
    /// Logical NOT, like `!`
    Not,
    /// Logical XOR, like `^`
    Xor,
}

/// Represents aggregate functions that can be applied to symbolic expressions.
/// These map to DataFusion's aggregate functions.
#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Hash)]
pub enum AggregateFunction {
    /// A function to sum up a list of expressions
    Sum,
    /// A function to count the number of non-null values in a list of expressions
    Count,
    /// A function to calculate the average of a list of expressions
    Avg,
    /// A function to calculate the maximum of a list of expressions
    Max,
    /// A function to calculate the minimum of a list of expressions
    Min,
    /// A function to calculate the median of a list of expressions
    Median,
    /// A function to calculate the mean of a list of expressions (alias for avg)
    Mean,
}

impl AggregateFunction {
    /// Creates an aggregate function from its string name.
    ///
    /// # Arguments
    /// * `func` - The name of the aggregate function
    ///
    /// # Panics
    /// * If the function name is not supported
    pub fn from_string(func: &str) -> Self {
        match func {
            "sum" => AggregateFunction::Sum,
            "count" => AggregateFunction::Count,
            "avg" => AggregateFunction::Avg,
            "max" => AggregateFunction::Max,
            "min" => AggregateFunction::Min,
            "median" => AggregateFunction::Median,
            "mean" => AggregateFunction::Mean,
            _ => panic!("Unsupported aggregate function: {func}"),
        }
    }
}

/// Represents a symbolic expression for a columnar array.
/// These expressions form a tree structure that represents the symbolic form
/// of physical expressions and can be used to generate Z3 constraints.
#[derive(Debug, Clone, PartialEq)]
pub enum Expr {
    /// A literal symbolic expression
    Literal(ScalarValue),

    /// A symbolic variable representing a cell in a table
    Variable {
        /// The name of the table
        table: String,
        /// The position of the column
        column: usize,
        /// The position of the row
        row: usize,
        /// The original data type of the column
        arrow_type: DataType,
        /// The variable type in Z3
        z3_type: Z3Type,
        /// The name of the column
        column_name: String,
    },

    /// A hole symbolic expression that will be filled with a concrete value during streaming
    Hole {
        /// The expression id that introduced the hole
        expr_id: usize,
        /// The physical expression that needs to be resolved
        physical_expr: Arc<dyn PhysicalExpr>,
    },

    /// A symbolic row representing a row in a table
    GenericRow {
        /// The name of the table
        table: String,
        /// The row constraints
        constraints: Option<Vec<Expr>>,
    },

    /// A symbolic generic variable representing a column
    GenericVar {
        /// The name of the table
        table: String,
        /// The position of the column
        column: usize,
        /// The original data type of the column
        arrow_type: DataType,
        /// The variable type in Z3
        z3_type: Z3Type,
        /// The name of the column
        column_name: String,
    },

    /// An IN list expression (e.g., x IN (1, 2, 3))
    InListExpr {
        /// The value to check
        left: Box<Expr>,
        /// The list of values to check against
        right: Vec<Expr>,
    },

    /// An aggregate expression (e.g., SUM(x))
    AggregateExpr {
        /// The aggregate function to apply
        func: AggregateFunction,
        /// The argument to aggregate
        arg: Box<Expr>,
        /// Whether to apply DISTINCT
        is_distinct: bool,
        /// The generic row to aggregate over
        generic_row: Box<Expr>,
    },

    /// A binary expression (e.g., x + y)
    BinaryExpr {
        /// The left side of the binary expression
        left: Box<Expr>,
        /// The operator to apply
        op: Operator,
        /// The right side of the binary expression
        right: Box<Expr>,
    },

    /// An if-then-else expression
    IfThenElse {
        /// The condition to check
        condition: Box<Expr>,
        /// The expression to evaluate if condition is true
        then_branch: Box<Expr>,
        /// The expression to evaluate if condition is false
        else_branch: Box<Expr>,
    },

    /// A logical NOT expression
    Not(Box<Expr>),

    /// An IS NULL check expression
    IsNull(Box<Expr>),

    /// An IS NOT NULL check expression
    IsNotNull(Box<Expr>),
}

impl Expr {
    /// Creates a new literal expression.
    pub fn literal(val: ScalarValue) -> Self {
        Expr::Literal(val)
    }

    /// Creates a new variable expression.
    pub fn variable(
        table: String,
        column: usize,
        row: usize,
        arrow_type: DataType,
        z3_type: Z3Type,
        column_name: String,
    ) -> Self {
        Expr::Variable {
            table,
            column,
            row,
            arrow_type,
            z3_type,
            column_name,
        }
    }

    /// Creates a new generic row expression.
    pub fn generic_row(table: String, constraints: Option<Vec<Expr>>) -> Self {
        Expr::GenericRow { table, constraints }
    }

    /// Creates a new generic variable expression.
    pub fn generic_var(
        table: String,
        column: usize,
        arrow_type: DataType,
        z3_type: Z3Type,
        column_name: String,
    ) -> Self {
        Expr::GenericVar {
            table,
            column,
            arrow_type,
            z3_type,
            column_name,
        }
    }

    /// Creates a new hole expression.
    pub fn hole(expr_id: usize, physical_expr: Arc<dyn PhysicalExpr>) -> Self {
        Expr::Hole {
            expr_id,
            physical_expr,
        }
    }

    /// Creates a new binary expression.
    pub fn binary(left: Expr, op: Operator, right: Expr) -> Self {
        Expr::BinaryExpr {
            left: Box::new(left),
            op,
            right: Box::new(right),
        }
    }

    /// Creates a new IN list expression.
    pub fn in_list(left: Expr, right: Vec<Expr>) -> Self {
        Expr::InListExpr {
            left: Box::new(left),
            right,
        }
    }

    /// Creates a new aggregate expression.
    pub fn aggregate(
        func: AggregateFunction,
        arg: Expr,
        is_distinct: bool,
        generic_row: Expr,
    ) -> Self {
        Expr::AggregateExpr {
            func,
            arg: Box::new(arg),
            is_distinct,
            generic_row: Box::new(generic_row),
        }
    }

    /// Creates a new if-then-else expression.
    pub fn if_then_else(condition: Expr, then_branch: Expr, else_branch: Expr) -> Self {
        Expr::IfThenElse {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
        }
    }

    /// Creates a new NOT expression.
    pub fn not_expr(expr: Expr) -> Self {
        Expr::Not(Box::new(expr))
    }

    /// Creates a new IS NULL check expression.
    pub fn is_null(expr: Expr) -> Self {
        Expr::IsNull(Box::new(expr))
    }

    /// Creates a new IS NOT NULL check expression.
    pub fn is_not_null(expr: Expr) -> Self {
        Expr::IsNotNull(Box::new(expr))
    }
}

/// Adds a constraint to a row expression.
///
/// # Arguments
/// * `row` - The row expression to add the constraint to
/// * `constraint` - The constraint to add
///
/// # Panics
/// * If the row expression is not a GenericRow
pub fn add_constraint_to_row(row: &mut Expr, constraint: Expr) {
    match row {
        Expr::GenericRow {
            constraints: Some(constraints),
            ..
        } => {
            constraints.push(constraint);
        }
        Expr::GenericRow { table, .. } => {
            *row = Expr::GenericRow {
                table: table.clone(),
                constraints: Some(vec![constraint]),
            };
        }
        _ => panic!("Expr is not a symbolic row"),
    }
}
