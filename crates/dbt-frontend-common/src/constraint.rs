use crate::error::InternalResult;
use crate::expr::{Evaluator, Value, try_parse};
use crate::internal_err;
use arrow::datatypes::DataType;
use linked_hash_map::LinkedHashMap;
use std::rc::Rc;

#[derive(Debug, Clone)]
pub struct ConstraintMap {
    pub types: LinkedHashMap<String, DataType>,
    pub integers: LinkedHashMap<String, u32>,
}

impl Default for ConstraintMap {
    fn default() -> Self {
        Self::new()
    }
}

impl ConstraintMap {
    pub fn new() -> Self {
        ConstraintMap {
            types: LinkedHashMap::new(),
            integers: LinkedHashMap::new(),
        }
    }
}

pub fn populate_constraint_map(
    constraint_map: Rc<ConstraintMap>,
    additional_constraints: &LinkedHashMap<String, String>,
) -> InternalResult<Option<Rc<ConstraintMap>>> {
    if additional_constraints.is_empty() {
        return Ok(Some(constraint_map));
    }

    let mut bindings = LinkedHashMap::new();
    for entry in &constraint_map.integers {
        bindings.insert(entry.0.as_str(), *entry.1);
    }

    for entry in additional_constraints {
        let key = entry.0;
        let expression = entry.1;
        if bindings.contains_key(key.as_str()) {
            return internal_err!(
                "Value for key '{}' already exists in bindings: {:?} when processing constraints {:?} with additional constraints {:?}",
                key,
                bindings,
                constraint_map,
                additional_constraints,
            );
        }
        // This could be done once, when loading functions from manifest file
        let evaluator = Evaluator::new(try_parse(expression)?);

        match (key.as_str(), evaluator.eval(&bindings)?) {
            ("condition", Value::Bool(val)) => {
                if val {
                    continue;
                } else {
                    return Ok(None);
                }
            }
            (_, Value::Int(value)) => {
                let value: u32 = value.try_into().map_err(|_err| {
                    format!(
                        "Processing of [{key}] => [{expression}] produced result out of range: {value:?}"
                    )
                })?;
                bindings.insert(key, value);
            }
            (key, value) => {
                return internal_err!(
                    "Processing of [{}] => [{}] produced unexpected result: {:?}",
                    key,
                    expression,
                    value
                );
            }
        }
    }

    let mut new_map = (*constraint_map).clone();
    for entry in bindings {
        let key = entry.0;
        let value = entry.1;
        new_map.integers.insert(key.to_string(), value);
    }
    Ok(Some(Rc::new(new_map)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_populate_constraint_map_empty_input() -> InternalResult<()> {
        let mut bound_values = ConstraintMap::new();
        bound_values.integers.insert("p".to_string(), 24);
        bound_values.integers.insert("s".to_string(), 10);
        let bound_values = Rc::new(bound_values);

        let result = populate_constraint_map(bound_values.clone(), &LinkedHashMap::new())?.unwrap();

        assert!(Rc::ptr_eq(&result, &bound_values));
        Ok(())
    }

    #[test]
    fn test_populate_constraint_map() -> InternalResult<()> {
        let mut bound_values = ConstraintMap::new();
        bound_values.integers.insert("p".to_string(), 24);
        bound_values.integers.insert("s".to_string(), 10);
        let mut expected_integers = bound_values.integers.clone();

        let mut additional_constraints = LinkedHashMap::new();
        additional_constraints.insert("rp".to_string(), "p + 3".to_string());
        additional_constraints.insert("rs".to_string(), "s + 3".to_string());

        let result =
            populate_constraint_map(Rc::new(bound_values), &additional_constraints)?.unwrap();

        expected_integers.insert("rp".to_string(), 27);
        expected_integers.insert("rs".to_string(), 13);
        assert_eq!(result.integers, expected_integers);
        Ok(())
    }

    #[test]
    fn test_populate_constraint_map_successful_condition() -> InternalResult<()> {
        let mut bound_values = ConstraintMap::new();
        bound_values.integers.insert("p".to_string(), 24);
        bound_values.integers.insert("s".to_string(), 10);
        let mut expected_integers = bound_values.integers.clone();

        let mut additional_constraints = LinkedHashMap::new();
        additional_constraints.insert("rp".to_string(), "p + 3".to_string());
        additional_constraints.insert("condition".to_string(), "p + 3 < 42".to_string());

        let result =
            populate_constraint_map(Rc::new(bound_values), &additional_constraints)?.unwrap();

        expected_integers.insert("rp".to_string(), 27);
        assert_eq!(result.integers, expected_integers);
        Ok(())
    }

    #[test]
    fn test_populate_constraint_map_failing_condition() -> InternalResult<()> {
        let mut bound_values = ConstraintMap::new();
        bound_values.integers.insert("p".to_string(), 24);
        bound_values.integers.insert("s".to_string(), 10);

        let mut additional_constraints = LinkedHashMap::new();
        additional_constraints.insert("rp".to_string(), "p + 3".to_string());
        additional_constraints.insert("condition".to_string(), "p + 3 >= 42".to_string());

        assert!(populate_constraint_map(Rc::new(bound_values), &additional_constraints)?.is_none());
        Ok(())
    }

    #[test]
    fn test_populate_constraint_map_in_order() -> InternalResult<()> {
        let mut bound_values = ConstraintMap::new();
        bound_values.integers.insert("p".to_string(), 24);
        bound_values.integers.insert("s".to_string(), 10);
        let mut expected_integers = bound_values.integers.clone();

        let mut additional_constraints = LinkedHashMap::new();
        additional_constraints.insert("b".to_string(), "p + 3".to_string());
        additional_constraints.insert("a".to_string(), "b - s".to_string());

        let result =
            populate_constraint_map(Rc::new(bound_values), &additional_constraints)?.unwrap();

        expected_integers.insert("b".to_string(), 27);
        expected_integers.insert("a".to_string(), 17);
        assert_eq!(result.integers, expected_integers);
        Ok(())
    }
}
