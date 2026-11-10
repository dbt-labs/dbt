//! Artifacts cross to Python as msgpack; the schemas live in `dbt/artifacts/schemas/`.

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyBytes;
use serde::Serialize;

/// Serialize to msgpack for the Python dataclasses.
///
/// Via `dbt_yaml::Value` as the JSON writer does: that flattens `__other__` and
/// `#[serde(flatten)]` and narrows >64-bit ints, so these bytes and the on-disk
/// JSON decode to the same object. `to_vec_named` because `to_vec` emits
/// positional arrays.
pub(crate) fn to_msgpack<T: Serialize>(py: Python<'_>, value: &T) -> PyResult<Py<PyBytes>> {
    let value =
        dbt_yaml::to_value(value).map_err(|e| PyValueError::new_err(format!("serialize: {e}")))?;
    let bytes = rmp_serde::to_vec_named(&value)
        .map_err(|e| PyValueError::new_err(format!("msgpack: {e}")))?;
    Ok(PyBytes::new(py, &bytes).unbind())
}
