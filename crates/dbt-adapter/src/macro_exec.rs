use crate::errors::{AdapterError, AdapterErrorKind};
use crate::response::ResultObject;

use arrow::array::RecordBatch;
use dbt_agate::AgateTable;
use minijinja::dispatch_object::macro_namespace_template_resolver;
use minijinja::{State, Value};

use std::error::Error;
use std::sync::Arc;

// All four panic if the named macro doesn't exist in the template state.

pub fn execute_macro_wrapper(
    state: &State,
    args: &[Value],
    macro_name: &str,
) -> Result<Arc<RecordBatch>, AdapterError> {
    execute_macro_wrapper_with_package(state, args, macro_name, "dbt")
}

pub fn execute_macro(
    state: &State,
    args: &[Value],
    macro_name: &str,
) -> Result<Value, AdapterError> {
    execute_macro_with_package(state, args, macro_name, "dbt")
}

pub fn execute_macro_wrapper_with_package(
    state: &State,
    args: &[Value],
    macro_name: &str,
    package: &str,
) -> Result<Arc<RecordBatch>, AdapterError> {
    let result: Value = execute_macro_with_package(state, args, macro_name, package)?;
    convert_macro_result_to_record_batch(&result)
}

pub fn convert_macro_result_to_record_batch(
    result: &Value,
) -> Result<Arc<RecordBatch>, AdapterError> {
    // Depending on the macro impl, result can be either ResultObject or AgateTable
    let table = if let Some(result) = result.downcast_object::<ResultObject>() {
        result.table.as_ref().expect("AgateTable exists").to_owned()
    } else if let Some(result) = result.downcast_object::<AgateTable>() {
        result.as_ref().to_owned()
    } else {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("Unexpected result type {result}"),
        ));
    };

    let record_batch = table.original_record_batch();
    Ok(record_batch)
}

pub fn execute_macro_with_package(
    state: &State,
    args: &[Value],
    macro_name: &str,
    package: &str,
) -> Result<Value, AdapterError> {
    let template_name = macro_namespace_template_resolver(state, macro_name, &mut Vec::new())
        .unwrap_or_else(|| format!("{package}.{macro_name}"));
    let template = state.env().get_template(&template_name)?;
    let base_ctx = state.get_base_context();
    let state = template.eval_to_state(base_ctx, &[])?;
    let func = state
        .lookup(macro_name, &[])
        .unwrap_or_else(|| panic!("{macro_name} exists"));
    func.call(&state, args, &[]).map_err(|err| {
        if let Some(source) = err.source() {
            if let Some(adapter_err) = source.downcast_ref::<AdapterError>() {
                return adapter_err.clone();
            }
        }
        AdapterError::new(AdapterErrorKind::UnexpectedResult, err.to_string())
    })
}
