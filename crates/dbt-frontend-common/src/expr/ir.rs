#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Expr {
    Integer(i64),
    Variable(String),
    ArithmeticBinary(Box<Expr>, ArithmeticOp, Box<Expr>),
    ComparisonBinary(Box<Expr>, ComparisonOp, Box<Expr>),
    Call(Function, Vec<Expr>),
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ArithmeticOp {
    Add,
    Subtract,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum ComparisonOp {
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Equal,
    NotEqual,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub enum Function {
    Min,
    Max,
    If,
}

impl Expr {
    pub fn validate(&self) -> Result<(), String> {
        self.validate_and_type()?;
        Ok(())
    }

    fn validate_and_type(&self) -> Result<Type, String> {
        match self {
            Expr::Integer(_) => Ok(Type::Int),
            Expr::Variable(_) => Ok(Type::Int),
            Expr::ArithmeticBinary(left, _, right) => {
                Expr::validate_expecting(left, Type::Int)?;
                Expr::validate_expecting(right, Type::Int)?;
                Ok(Type::Int)
            }
            Expr::ComparisonBinary(left, _, right) => {
                Expr::validate_expecting(left, Type::Int)?;
                Expr::validate_expecting(right, Type::Int)?;
                Ok(Type::Bool)
            }
            Expr::Call(f, args) => f.validate_and_type(args),
        }
    }

    fn validate_expecting(expr: &Expr, expected: Type) -> Result<(), String> {
        let actual = expr.validate_and_type()?;
        if actual == expected {
            Ok(())
        } else {
            Err(format!(
                "Expected {expected:?}, got expression {expr:?} of type {actual:?}"
            ))
        }
    }
}

impl Function {
    fn validate_and_type(&self, args: &[Expr]) -> Result<Type, String> {
        match self {
            Function::Min | Function::Max => {
                if args.len() < 2 {
                    let name = match self {
                        Function::Min => "min",
                        Function::Max => "max",
                        _ => unreachable!(),
                    };
                    return Err(format!(
                        "Expected at least two arguments to function {}, got {}",
                        name,
                        args.len()
                    ));
                }
                for a in args {
                    Expr::validate_expecting(a, Type::Int)?;
                }
                Ok(Type::Int)
            }
            Function::If => {
                let [cond, then, else_] = args else {
                    return Err(format!(
                        "Expected exactly three arguments to function if, got {}",
                        args.len()
                    ));
                };
                Expr::validate_expecting(cond, Type::Bool)?;
                Expr::validate_expecting(then, Type::Int)?;
                Expr::validate_expecting(else_, Type::Int)?;
                Ok(Type::Int)
            }
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Type {
    Int,
    Bool,
}
