//! A validation module for MiniJinja, intended to provide validation utilities.
//!
//! This module provides functions to validate values against a set of possible values or types,
//! similar to dbt's validation module.

use minijinja::value::{Object, ObjectRepr};
use minijinja::{Error, ErrorKind, State, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Create a namespace with validation functions.
pub fn create_validation_namespace() -> BTreeMap<String, Value> {
    let mut validation_module = BTreeMap::new();

    // Add the any validation function as a ValidationAny object
    validation_module.insert("any".to_string(), Value::from_object(ValidationAny));

    validation_module
}

/// A validation object that offers different validation strategies
#[derive(Debug)]
struct ValidationAny;

impl Object for ValidationAny {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn is_true(self: &Arc<Self>) -> bool {
        true
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        Some(Value::from_object(ValidatorObject {
            valid_value_args: key.clone(),
        }))
    }
}
/// A validator object that stores valid values and provides a get method
#[derive(Debug)]
struct ValidatorObject {
    valid_value_args: Value,
}

impl Object for ValidatorObject {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }
    fn call(
        self: &Arc<Self>,
        _state: &State,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        // TODO: we need to implement validating the the value against the type
        let mut valid_type_or_value_list = Vec::new();

        // this is when user write validation.any[str], where str is a type
        if self.valid_value_args.is_undefined() {
            return Ok(Value::from(true));
        }

        if let Some(s) = self.valid_value_args.as_str() {
            // If key is a string, use it as the only valid value
            valid_type_or_value_list.push(Value::from(s));
        } else if let Ok(iter) = self.valid_value_args.try_iter() {
            // If key is iterable, collect all values
            for valid_value in iter {
                // when value is undefined, it is a type, we do not do any validation
                // in this case.
                if valid_value.is_undefined() {
                    return Ok(Value::from(true));
                } else {
                    valid_type_or_value_list.push(valid_value);
                }
            }
        } else {
            return Ok(Value::from(true));
        }

        let value = args[0].clone();
        for valid_value in &valid_type_or_value_list {
            if &value == valid_value {
                return Ok(Value::from(true));
            }
        }

        // If we get here, validation failed
        let valid_values_str = valid_type_or_value_list
            .iter()
            .map(|v| format!("\"{v}\""))
            .collect::<Vec<_>>()
            .join(", ");

        Err(Error::new(
            ErrorKind::InvalidArgument,
            format!("Expected value \"{value}\" to be one of [{valid_values_str}]"),
        ))
    }
}
