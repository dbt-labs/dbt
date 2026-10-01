use pyo3::prelude::*;

mod environment;
mod error_support;
mod state;
mod typeconv;

#[pymodule]
fn _lowlevel(_py: Python, m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<environment::Environment>()?;
    m.add_class::<state::StateRef>()?;
    m.add_class::<error_support::ErrorInfo>()?;
    Ok(())
}
