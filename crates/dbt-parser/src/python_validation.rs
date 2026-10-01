//! Python model validation
//!
//! This module validates the structure of Python model files to ensure they follow
//! dbt's requirements for Python models.

use dbt_common::{ErrorCode, FsResult, err};
use ruff_python_ast::{
    self as ast, Stmt, StmtFunctionDef,
    visitor::{Visitor, walk_stmt},
};
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

/// Visitor for validating Python model structure
pub struct PythonValidationVisitor {
    /// Number of `model` function definitions encountered
    pub model_function_count: usize,
    /// Collected validation error messages
    pub errors: Vec<String>,
}

impl Default for PythonValidationVisitor {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonValidationVisitor {
    /// Create a new validation visitor
    pub fn new() -> Self {
        Self {
            model_function_count: 0,
            errors: Vec::new(),
        }
    }

    /// Validate a function definition
    fn validate_model_function(&mut self, func: &StmtFunctionDef) {
        // Check function name
        if func.name.as_str() != "model" {
            return;
        }

        self.model_function_count += 1;

        // Check the first argument name when present
        if let Some(first_arg) = func.parameters.args.first() {
            if first_arg.parameter.name.as_str() != "dbt" {
                self.errors
                    .push("'dbt' not provided for model as the first argument".to_string());
            }
        } else {
            // Mirror dbt-core behavior: when there are no args, we still record the two-arg error below.
        }

        // dbt-core only considers positional arguments (not pos-only); require exactly two
        if func.parameters.args.len() != 2 {
            self.errors.push(
                "model function should have two args, `dbt` and a session to current warehouse"
                    .to_string(),
            );
        }

        // Require the final statement to be a single return
        let last_stmt_is_valid_return =
            if let Some(Stmt::Return(ast::StmtReturn { value, .. })) = func.body.last() {
                !matches!(value.as_deref(), Some(ast::Expr::Tuple(_)))
            } else {
                false
            };

        if !last_stmt_is_valid_return {
            self.errors
                .push("model function should return only one dataframe object".to_string());
        }
    }
}

impl Visitor<'_> for PythonValidationVisitor {
    fn visit_stmt(&mut self, stmt: &Stmt) {
        if let Stmt::FunctionDef(func) = stmt {
            self.validate_model_function(func);
        }

        walk_stmt(self, stmt);
    }
}

/// Validate a Python model file structure
pub fn validate_python_model(file_path: &Path, stmts: &[Stmt]) -> FsResult<()> {
    // Skip validation for empty files (like __init__.py), matching dbt-core behavior
    // In dbt-core, validation only runs if tree.body is truthy (non-empty)
    if stmts.is_empty() {
        return Ok(());
    }

    let mut validator = PythonValidationVisitor::new();

    for stmt in stmts {
        validator.visit_stmt(stmt);
    }

    // Enforce exactly one model function, matching dbt-core semantics
    if validator.model_function_count != 1 {
        return err!(
            code => ErrorCode::Generic,
            loc => file_path.to_path_buf(),
            "dbt allows exactly one model defined per python file, found {}",
            validator.model_function_count
        );
    }

    // Report any validation errors
    if !validator.errors.is_empty() {
        return err!(
            code => ErrorCode::Generic,
            loc => file_path.to_path_buf(),
            "{}",
            validator.errors.join("\n")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::python_ast::parse_python;

    #[test]
    fn test_valid_model_function() {
        let source = r#"
def model(dbt, session):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let result = validate_python_model(&path, &stmts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_missing_model_function() {
        let source = r#"
def some_other_function(dbt, session):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("dbt allows exactly one model defined per python file, found 0")
        );
    }

    #[test]
    fn test_wrong_parameter_name() {
        let source = r#"
def model(tbd, session):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("'dbt' not provided for model as the first argument")
        );
    }

    #[test]
    fn test_missing_return() {
        let source = r#"
def model(dbt, session):
    df = session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("model function should return only one dataframe object")
        );
    }

    #[test]
    fn test_tuple_return_rejected() {
        let source = r#"
def model(dbt, session):
    df1 = session.table('data1')
    df2 = session.table('data2')
    return df1, df2
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("model function should return only one dataframe object")
        );
    }

    #[test]
    fn test_wrong_number_of_parameters() {
        let source = r#"
def model(dbt):
    return dbt.ref('test')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(err.to_string().contains(
            "model function should have two args, `dbt` and a session to current warehouse"
        ));
    }

    #[test]
    fn test_too_many_parameters() {
        let source = r#"
def model(dbt, session, extra):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(err.to_string().contains(
            "model function should have two args, `dbt` and a session to current warehouse"
        ));
    }

    #[test]
    fn test_valid_with_type_hints() {
        let source = r#"
def model(dbt, session: Any) -> Any:
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let result = validate_python_model(&path, &stmts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_multiple_model_functions() {
        let source = r#"
def model(dbt, session):
    return session.table('data')


def model(dbt, session):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("dbt allows exactly one model defined per python file, found 2")
        );
    }

    // Additional tests from dbt-core test suite

    #[test]
    fn test_empty_file() {
        let source = "    ";
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        // Empty files should be silently skipped, matching dbt-core behavior
        let result = validate_python_model(&path, &stmts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_truly_empty_file() {
        // Test for completely empty file (0 bytes), like __init__.py
        let source = "";
        let path = PathBuf::from("__init__.py");
        let stmts = parse_python(source, &path).unwrap();
        // Empty files should be silently skipped, matching dbt-core behavior
        let result = validate_python_model(&path, &stmts);
        assert!(result.is_ok());
    }

    #[test]
    fn test_no_model_function() {
        let source = r#"
def some_other_function(dbt, session):
    return session.table('data')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("dbt allows exactly one model defined per python file, found 0")
        );
    }

    #[test]
    fn test_incorrect_function_name() {
        let source = r#"
def model1(dbt, session):
    dbt.config(materialized='table')
    return dbt.ref('some_model')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("dbt allows exactly one model defined per python file, found 0")
        );
    }

    #[test]
    fn test_no_arguments() {
        let source = r#"
import pandas as pd

def model():
    return pd.dataframe([1, 2])
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(err.to_string().contains(
            "model function should have two args, `dbt` and a session to current warehouse"
        ));
    }

    #[test]
    fn test_single_argument() {
        let source = r#"
def model(dbt):
    dbt.config(materialized="table")
    return dbt.ref('some_model')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(err.to_string().contains(
            "model function should have two args, `dbt` and a session to current warehouse"
        ));
    }

    #[test]
    fn test_incorrect_first_argument_name() {
        let source = r#"
def model(tbd, session):
    tbd.config(materialized="table")
    return tbd.ref('some_model')
"#;
        let path = PathBuf::from("test.py");
        let stmts = parse_python(source, &path).unwrap();
        let err = validate_python_model(&path, &stmts).unwrap_err();
        assert!(
            err.to_string()
                .contains("'dbt' not provided for model as the first argument")
        );
    }
}
