use crate::expr::Expr;
use crate::expr::ir::{ArithmeticOp, ComparisonOp, Function};
use std::borrow::Borrow;
use std::hash::Hash;

pub struct Evaluator {
    // Using dyn dispatch might be faster than match-based
    expr: Expr,
}

#[derive(Debug, PartialEq)]
pub enum Value {
    Int(i64),
    Bool(bool),
}

pub trait Bindings {
    fn get_variable(&self, name: &str) -> Option<i64>;
}

impl<K, V> Bindings for std::collections::HashMap<K, V>
where
    K: Borrow<str> + Hash + Eq,
    V: Into<i64> + Copy,
{
    fn get_variable(&self, name: &str) -> Option<i64> {
        self.get(name).map(|v| (*v).into())
    }
}

impl<K, V> Bindings for linked_hash_map::LinkedHashMap<K, V>
where
    K: Borrow<str> + Hash + Eq,
    V: Into<i64> + Copy,
{
    fn get_variable(&self, name: &str) -> Option<i64> {
        self.get(name).map(|v| (*v).into())
    }
}

impl Evaluator {
    pub fn new(expr: Expr) -> Self {
        Self { expr }
    }

    pub fn eval(&self, bindings: &impl Bindings) -> Result<Value, String> {
        eval_expr(&self.expr, bindings)
    }
}

fn eval_expr(expr: &Expr, bindings: &impl Bindings) -> Result<Value, String> {
    use Expr::*;
    use Value::*;
    match expr {
        Integer(val) => Ok(Int(*val)),
        Variable(var) => bindings.get_variable(var).map_or_else(
            || Err(format!("Variable not found: {var}")),
            |val| Ok(Int(val)),
        ),
        ArithmeticBinary(lhs, op, rhs) => {
            eval_arithmetic(eval_expr(lhs, bindings)?, op, eval_expr(rhs, bindings)?)
        }
        ComparisonBinary(lhs, op, rhs) => {
            eval_comparison(eval_expr(lhs, bindings)?, op, eval_expr(rhs, bindings)?)
        }
        Call(f, args) => {
            let args = args
                .iter()
                .try_fold(Vec::new(), |mut acc, arg| -> Result<_, String> {
                    acc.push(eval_expr(arg, bindings)?);
                    Ok(acc)
                })?;
            eval_function(f, args)
        }
    }
}

fn eval_arithmetic(lhs: Value, op: &ArithmeticOp, rhs: Value) -> Result<Value, String> {
    use ArithmeticOp::*;
    use Value::Int;
    if let (Int(lhs), Int(rhs)) = (&lhs, &rhs) {
        let result = match op {
            Add => lhs.checked_add(*rhs),
            Subtract => lhs.checked_sub(*rhs),
        };
        if let Some(result) = result {
            return Ok(Int(result));
        }
    }
    Err(format!("Cannot evaluate: {lhs:?} {op:?} {rhs:?}"))
}

fn eval_comparison(lhs: Value, op: &ComparisonOp, rhs: Value) -> Result<Value, String> {
    use ComparisonOp::*;
    use Value::*;
    if let (Int(lhs), Int(rhs)) = (&lhs, &rhs) {
        let result = match op {
            LessThan => lhs < rhs,
            LessThanOrEqual => lhs <= rhs,
            GreaterThan => lhs > rhs,
            GreaterThanOrEqual => lhs >= rhs,
            Equal => lhs == rhs,
            NotEqual => lhs != rhs,
        };
        return Ok(Bool(result));
    }
    Err(format!("Cannot evaluate: {lhs:?} {op:?} {rhs:?}"))
}

fn eval_function(f: &Function, args: Vec<Value>) -> Result<Value, String> {
    use Function::*;
    match f {
        Min => eval_min(args),
        Max => eval_max(args),
        If => eval_if(args),
    }
}

fn eval_min(args: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Int(
        args_to_numbers(args)?.into_iter().min().unwrap(),
    ))
}

fn eval_max(args: Vec<Value>) -> Result<Value, String> {
    Ok(Value::Int(
        args_to_numbers(args)?.into_iter().max().unwrap(),
    ))
}

fn eval_if(args: Vec<Value>) -> Result<Value, String> {
    let [Value::Bool(cond), Value::Int(then), Value::Int(else_)] = args[..] else {
        return Err(format!("Expected (bool, int, int) arguments, got {args:?}"));
    };
    Ok(Value::Int(if cond { then } else { else_ }))
}

fn args_to_numbers(args: Vec<Value>) -> Result<Vec<i64>, String> {
    args.into_iter()
        .map(|v| match v {
            Value::Int(i) => Ok(i),
            Value::Bool(b) => Err(format!("Unexpected bool argument: {b}")),
        })
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use super::super::try_parse;
    use super::*;
    use std::cell::RefCell;

    impl Bindings for () {
        fn get_variable(&self, _name: &str) -> Option<i64> {
            None
        }
    }

    impl<F> Bindings for F
    where
        F: Fn(&str) -> Option<i64>,
    {
        fn get_variable(&self, name: &str) -> Option<i64> {
            self(name)
        }
    }

    #[test]
    fn test_eval_const() {
        assert_eq!(eval("0", &()), Value::Int(0));
        assert_eq!(eval("1", &()), Value::Int(1));
    }

    #[test]
    fn test_eval_variable() {
        let calls = RefCell::new(Vec::new());
        assert_eq!(
            try_eval("a", &|v: &str| {
                calls.borrow_mut().push(v.to_string());
                None
            })
            .unwrap_err(),
            "Variable not found: a"
        );
        assert_eq!(calls.into_inner(), vec!["a".to_string()]);

        assert_eq!(
            eval("a", &|v: &str| {
                match v {
                    "a" => Some(42),
                    _ => None,
                }
            }),
            Value::Int(42)
        );
    }

    #[test]
    fn test_eval_arithmetic() {
        let bindings = |v: &str| match v {
            "a" => Some(42),
            "b" => Some(45),
            _ => None,
        };
        assert_eq!(eval("a + 3", &bindings), Value::Int(45));
        assert_eq!(eval("a - 3", &bindings), Value::Int(39));
        assert_eq!(eval("a + a + 3 -a", &bindings), Value::Int(45));
        assert_eq!(eval("a - 2 - 5", &bindings), Value::Int(35));
        assert_eq!(eval("a + b", &bindings), Value::Int(87));
        assert_eq!(eval("a - b", &bindings), Value::Int(-3));
        assert_eq!(eval("b - a", &bindings), Value::Int(3));
        assert_eq!(
            eval("0 - 1 - 2 - 4 - 8 - 16 - 32 - 64", &bindings),
            Value::Int(-127)
        );
        assert_eq!(
            eval("(0 - 1) - ((2 - 4) - ((8 - 16) - 32) - 64)", &bindings),
            Value::Int(25)
        );
        assert_eq!(
            try_eval("a + A", &bindings).unwrap_err(),
            "Variable not found: A"
        );
        assert_eq!(
            try_eval("a + c", &bindings).unwrap_err(),
            "Variable not found: c"
        );
    }

    #[test]
    fn test_eval_comparison() {
        let bindings = |v: &str| match v {
            "a" => Some(3),
            "b" => Some(5),
            _ => None,
        };
        assert_eq!(eval("a == 3", &bindings), Value::Bool(true));
        assert_eq!(eval("a != 3", &bindings), Value::Bool(false));
        assert_eq!(eval("a <= 3", &bindings), Value::Bool(true));
        assert_eq!(eval("a >= 3", &bindings), Value::Bool(true));
        assert_eq!(eval("a < 3", &bindings), Value::Bool(false));
        assert_eq!(eval("a > 3", &bindings), Value::Bool(false));

        assert_eq!(eval("b == 3", &bindings), Value::Bool(false));
        assert_eq!(eval("b != 3", &bindings), Value::Bool(true));
        assert_eq!(eval("b <= 3", &bindings), Value::Bool(false));
        assert_eq!(eval("b >= 3", &bindings), Value::Bool(true));
        assert_eq!(eval("b < 3", &bindings), Value::Bool(false));
        assert_eq!(eval("b > 3", &bindings), Value::Bool(true));

        assert_eq!(eval("a == 1 + 1 + 1", &bindings), Value::Bool(true));
        assert_eq!(eval("a == b - 2", &bindings), Value::Bool(true));
        assert_eq!(eval("b - a < 3", &bindings), Value::Bool(true));
        assert_eq!(eval("b - a < 2", &bindings), Value::Bool(false));
    }

    #[test]
    fn test_eval_min() {
        let bindings = |v: &str| match v {
            "a" => Some(3),
            "b" => Some(5),
            _ => None,
        };

        assert_eq!(eval("min(1, 3, 7, 99)", &bindings), Value::Int(1));
        assert_eq!(eval("min(1, 3, 7, 0-99)", &bindings), Value::Int(-99));
        assert_eq!(eval("min(3, a + b, a - b, 4)", &bindings), Value::Int(-2));
        assert_eq!(eval("min(3, a + b, b - a, 4)", &bindings), Value::Int(2));
    }

    #[test]
    fn test_eval_max() {
        let bindings = |v: &str| match v {
            "a" => Some(3),
            "b" => Some(5),
            _ => None,
        };

        assert_eq!(eval("max(1, 3, 7, 99)", &bindings), Value::Int(99));
        assert_eq!(eval("max(1, 3, 7, 0-99)", &bindings), Value::Int(7));
        assert_eq!(eval("max(3, a + b, a - b, 4)", &bindings), Value::Int(8));
        assert_eq!(eval("max(3, a + b, b - a, 4)", &bindings), Value::Int(8));
    }

    #[test]
    fn test_eval_if() {
        let bindings = |v: &str| match v {
            "a" => Some(3),
            "b" => Some(5),
            _ => None,
        };

        assert_eq!(eval("if(a < b, a, b)", &bindings), Value::Int(3));
        assert_eq!(eval("if(a > b, a, b)", &bindings), Value::Int(5));
    }

    fn eval(s: &str, bindings: &impl Bindings) -> Value {
        try_eval(s, bindings).unwrap()
    }

    fn try_eval(s: &str, bindings: &impl Bindings) -> Result<Value, String> {
        Evaluator::new(try_parse(s).unwrap()).eval(bindings.borrow())
    }
}
