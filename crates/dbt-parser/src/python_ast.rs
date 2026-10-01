//! Python AST parsing and literal evaluation for Python models
//!
//! This module provides utilities for parsing Python model files and extracting
//! dbt function calls (ref, source, config) using AST analysis.

use dbt_common::{ErrorCode, FsResult, err};
use ruff_python_ast::{self as ast, Expr, ModModule, Stmt};
use ruff_python_parser::{Mode, parse_unchecked};
use ruff_text_size::{Ranged, TextRange};
use std::path::Path;

/// Error type for Python literal evaluation failures
#[derive(Debug)]
pub struct PythonLiteralEvalError {
    /// Error message describing what went wrong
    pub message: String,
    /// Line number where the error occurred
    pub line: usize,
    /// Column number where the error occurred
    pub col: usize,
}

/// Precompute the byte offsets where each line starts (0-based line index)
pub(crate) fn compute_line_starts(source: &str) -> Vec<usize> {
    let mut starts = Vec::new();
    starts.push(0);
    for (idx, byte) in source.bytes().enumerate() {
        if byte == b'\n' {
            starts.push(idx + 1);
        }
    }
    starts
}

/// Convert a byte offset into 1-based (line, column) coordinates
pub(crate) fn offset_to_line_col(offset: usize, line_starts: &[usize]) -> (usize, usize) {
    let idx = match line_starts.binary_search(&offset) {
        Ok(idx) => idx,
        Err(idx) => idx.saturating_sub(1),
    };
    let line_start = *line_starts.get(idx).unwrap_or(&0);
    (idx + 1, offset - line_start + 1)
}

fn range_start_line_col(range: TextRange, line_starts: &[usize]) -> (usize, usize) {
    let offset = range.start().to_u32() as usize;
    offset_to_line_col(offset, line_starts)
}

impl std::fmt::Display for PythonLiteralEvalError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Error when trying to literal_eval an arg to dbt.ref(), dbt.source(), dbt.config() or dbt.config.get() at line {}, col {}: {}",
            self.line, self.col, self.message
        )
    }
}

/// Represents a literal Python value that can be extracted from AST
#[derive(Debug, Clone, PartialEq)]
pub enum LiteralValue {
    /// A string literal
    String(String),
    /// An integer literal
    Integer(i64),
    /// A float literal
    Float(f64),
    /// A boolean literal
    Bool(bool),
    /// None literal
    None,
    /// A list literal
    List(Vec<LiteralValue>),
    /// A dict literal
    Dict(Vec<(LiteralValue, LiteralValue)>),
    /// A tuple literal
    Tuple(Vec<LiteralValue>),
}

impl LiteralValue {
    /// Convert to a String if possible
    pub fn as_string(&self) -> Option<String> {
        match self {
            LiteralValue::String(s) => Some(s.clone()),
            _ => None,
        }
    }

    /// Convert to an integer if possible
    pub fn as_int(&self) -> Option<i64> {
        match self {
            LiteralValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Convert to a bool if possible
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            LiteralValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    /// Convert to a list if possible
    pub fn as_list(&self) -> Option<&Vec<LiteralValue>> {
        match self {
            LiteralValue::List(l) => Some(l),
            _ => None,
        }
    }

    /// Convert to a dict if possible
    pub fn as_dict(&self) -> Option<&Vec<(LiteralValue, LiteralValue)>> {
        match self {
            LiteralValue::Dict(d) => Some(d),
            _ => None,
        }
    }
}

/// Safely evaluate a Python expression to a literal value
///
/// This mimics Python's `ast.literal_eval` - only supports literal structures
/// (strings, bytes, numbers, tuples, lists, dicts, sets, booleans, None)
pub fn literal_eval(
    expr: &Expr,
    line_starts: &[usize],
) -> Result<LiteralValue, PythonLiteralEvalError> {
    let range = expr.range();
    let (line, col) = range_start_line_col(range, line_starts);

    match expr {
        Expr::StringLiteral(ast::ExprStringLiteral { value, .. }) => {
            Ok(LiteralValue::String(value.to_string()))
        }
        Expr::BytesLiteral(ast::ExprBytesLiteral { value, .. }) => {
            // Convert bytes to string (lossy) - concatenate all byte strings
            let bytes: Vec<u8> = value
                .iter()
                .flat_map(|b| {
                    let slice: &[u8] = b.as_ref();
                    slice.iter().copied()
                })
                .collect();
            Ok(LiteralValue::String(
                String::from_utf8_lossy(&bytes).into_owned(),
            ))
        }
        Expr::NumberLiteral(ast::ExprNumberLiteral { value, .. }) => match value {
            ast::Number::Int(i) => {
                let int_val: i64 = i.as_i64().ok_or_else(|| PythonLiteralEvalError {
                    message: "Integer too large".to_string(),
                    line,
                    col,
                })?;
                Ok(LiteralValue::Integer(int_val))
            }
            ast::Number::Float(f) => Ok(LiteralValue::Float(*f)),
            ast::Number::Complex { .. } => Err(PythonLiteralEvalError {
                message: "Complex numbers are not supported".to_string(),
                line,
                col,
            }),
        },
        Expr::BooleanLiteral(ast::ExprBooleanLiteral { value, .. }) => {
            Ok(LiteralValue::Bool(*value))
        }
        Expr::NoneLiteral(_) => Ok(LiteralValue::None),
        Expr::List(ast::ExprList { elts, .. }) | Expr::Set(ast::ExprSet { elts, .. }) => {
            let values: Result<Vec<_>, _> = elts
                .iter()
                .map(|elt| literal_eval(elt, line_starts))
                .collect();
            Ok(LiteralValue::List(values?))
        }
        Expr::Tuple(ast::ExprTuple { elts, .. }) => {
            let values: Result<Vec<_>, _> = elts
                .iter()
                .map(|elt| literal_eval(elt, line_starts))
                .collect();
            Ok(LiteralValue::Tuple(values?))
        }
        Expr::Dict(ast::ExprDict { items, .. }) => {
            let mut dict_items = Vec::new();
            for item in items {
                if let Some(key_expr) = &item.key {
                    let key = literal_eval(key_expr, line_starts)?;
                    let value = literal_eval(&item.value, line_starts)?;
                    dict_items.push((key, value));
                } else {
                    return Err(PythonLiteralEvalError {
                        message: "Dictionary unpacking (**dict) is not supported".to_string(),
                        line,
                        col,
                    });
                }
            }
            Ok(LiteralValue::Dict(dict_items))
        }
        Expr::UnaryOp(ast::ExprUnaryOp { op, operand, .. }) => match op {
            ast::UnaryOp::UAdd => literal_eval(operand, line_starts),
            ast::UnaryOp::USub => {
                let val = literal_eval(operand, line_starts)?;
                match val {
                    LiteralValue::Integer(i) => Ok(LiteralValue::Integer(-i)),
                    LiteralValue::Float(f) => Ok(LiteralValue::Float(-f)),
                    _ => Err(PythonLiteralEvalError {
                        message: "Unary minus only works on numbers".to_string(),
                        line,
                        col,
                    }),
                }
            }
            _ => Err(PythonLiteralEvalError {
                message: "Only unary + and - are supported".to_string(),
                line,
                col,
            }),
        },
        Expr::FString(_) => Err(PythonLiteralEvalError {
            message: "F-strings are not supported in dbt function calls".to_string(),
            line,
            col,
        }),
        _ => Err(PythonLiteralEvalError {
            message: "Non-literal expression found. In dbt python model, `dbt.ref`, `dbt.source`, `dbt.config`, `dbt.config.get` function args only support Python literal structures (strings, numbers, booleans, None, lists, dicts, tuples)".to_string(),
            line,
            col,
        }),
    }
}

/// Parse Python source code into an AST
pub fn parse_python(source: &str, filename: &Path) -> FsResult<Vec<Stmt>> {
    let parsed = parse_unchecked(source, Mode::Module);

    if !parsed.errors().is_empty() {
        let first_error = &parsed.errors()[0];
        return err!(
            code => ErrorCode::Generic,
            loc => filename.to_path_buf(),
            "Failed to parse Python file: {}", first_error
        );
    }

    match parsed.into_syntax() {
        ast::Mod::Module(ModModule { body, .. }) => Ok(body),
        ast::Mod::Expression(_) => err!(
            code => ErrorCode::Generic,
            loc => filename.to_path_buf(),
            "Expected module, got expression"
        ),
    }
}

/// Extract the full dotted name from an attribute chain (e.g., "dbt.config.get")
pub fn get_full_attr_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(ast::ExprName { id, .. }) => Some(id.to_string()),
        Expr::Attribute(ast::ExprAttribute { value, attr, .. }) => {
            let base = get_full_attr_name(value)?;
            Some(format!("{}.{}", base, attr))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_expr(source: &str) -> (Expr, Vec<usize>) {
        let parsed = parse_unchecked(source, Mode::Expression);
        let expr = match parsed.into_syntax() {
            ast::Mod::Expression(ast::ModExpression { body, .. }) => *body,
            _ => panic!("Expected expression"),
        };
        let line_starts = compute_line_starts(source);
        (expr, line_starts)
    }

    #[test]
    fn test_literal_eval_string() {
        let (expr, line_starts) = parse_expr("'hello'");
        let result = literal_eval(&expr, &line_starts).unwrap();
        assert_eq!(result, LiteralValue::String("hello".to_string()));
    }

    #[test]
    fn test_literal_eval_integer() {
        let (expr, line_starts) = parse_expr("42");
        let result = literal_eval(&expr, &line_starts).unwrap();
        assert_eq!(result, LiteralValue::Integer(42));
    }

    #[test]
    fn test_literal_eval_bool() {
        let (expr, line_starts) = parse_expr("True");
        let result = literal_eval(&expr, &line_starts).unwrap();
        assert_eq!(result, LiteralValue::Bool(true));
    }

    #[test]
    fn test_literal_eval_none() {
        let (expr, line_starts) = parse_expr("None");
        let result = literal_eval(&expr, &line_starts).unwrap();
        assert_eq!(result, LiteralValue::None);
    }

    #[test]
    fn test_literal_eval_list() {
        let (expr, line_starts) = parse_expr("[1, 2, 'three']");
        let result = literal_eval(&expr, &line_starts).unwrap();
        if let LiteralValue::List(items) = result {
            assert_eq!(items.len(), 3);
            assert_eq!(items[0], LiteralValue::Integer(1));
            assert_eq!(items[1], LiteralValue::Integer(2));
            assert_eq!(items[2], LiteralValue::String("three".to_string()));
        } else {
            panic!("Expected list");
        }
    }

    #[test]
    fn test_literal_eval_dict() {
        let (expr, line_starts) = parse_expr("{'key': 'value', 'num': 42}");
        let result = literal_eval(&expr, &line_starts).unwrap();
        if let LiteralValue::Dict(items) = result {
            assert_eq!(items.len(), 2);
        } else {
            panic!("Expected dict");
        }
    }

    #[test]
    fn test_literal_eval_negative_number() {
        let (expr, line_starts) = parse_expr("-42");
        let result = literal_eval(&expr, &line_starts).unwrap();
        assert_eq!(result, LiteralValue::Integer(-42));
    }

    #[test]
    fn test_literal_eval_rejects_variable() {
        let (expr, line_starts) = parse_expr("some_var");
        let result = literal_eval(&expr, &line_starts);
        assert!(result.is_err());
    }

    #[test]
    fn test_get_full_attr_name() {
        let (expr, _) = parse_expr("dbt.config.get");
        let name = get_full_attr_name(&expr).unwrap();
        assert_eq!(name, "dbt.config.get");
    }

    #[test]
    fn test_get_full_attr_name_simple() {
        let (expr, _) = parse_expr("dbt");
        let name = get_full_attr_name(&expr).unwrap();
        assert_eq!(name, "dbt");
    }
}
