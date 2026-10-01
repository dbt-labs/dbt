use std::cell::RefCell;

use minijinja::{Error, ErrorKind};
use once_cell::sync::OnceCell;
use pyo3::ffi::PyErr_WriteUnraisable;
use pyo3::prelude::*;
use pyo3::types::PyTuple;

static TEMPLATE_ERROR: OnceCell<Py<PyAny>> = OnceCell::new();

thread_local! {
    static STASHED_ERROR: RefCell<Option<PyErr>> = const { RefCell::new(None) };
}

/// Provides information about a template error from the runtime.
#[pyclass(subclass, module = "minijinja._lowlevel", name = "ErrorInfo")]
pub struct ErrorInfo {
    err: minijinja::Error,
}

#[pymethods]
impl ErrorInfo {
    #[getter]
    pub fn get_kind(&self) -> String {
        format!("{:?}", self.err.kind())
    }

    #[getter]
    pub fn get_name(&self) -> Option<String> {
        self.err.name().map(|x| x.into())
    }

    #[getter]
    pub fn get_line(&self) -> Option<usize> {
        self.err.line()
    }

    #[getter]
    pub fn get_range(&self) -> Option<(usize, usize)> {
        self.err.range().map(|x| (x.start, x.end))
    }

    #[getter]
    pub fn get_template_source(&self) -> Option<&str> {
        self.err.template_source()
    }

    #[getter]
    pub fn get_description(&self) -> String {
        format!("{}", self.err)
    }

    #[getter]
    pub fn get_detail(&self) -> Option<&str> {
        self.err.detail()
    }

    #[getter]
    pub fn get_full_description(&self) -> String {
        use std::fmt::Write;
        let mut rv = format!("{:#}", self.err);
        let mut err = &self.err as &dyn std::error::Error;
        while let Some(next_err) = err.source() {
            rv.push('\n');
            writeln!(&mut rv, "caused by: {next_err:#}").unwrap();
            err = next_err;
        }
        rv
    }
}

pub fn to_minijinja_error(err: PyErr) -> Error {
    let msg = err.to_string();
    STASHED_ERROR.with(|stash| {
        *stash.borrow_mut() = Some(err);
    });
    Error::new(ErrorKind::TemplateNotFound, msg)
}

pub fn to_py_error(original_err: Error) -> PyErr {
    STASHED_ERROR.with(|stash| {
        stash
            .borrow_mut()
            .take()
            .unwrap_or_else(|| make_error(original_err))
    })
}

pub fn report_unraisable(py: Python<'_>, err: PyErr) {
    err.restore(py);
    unsafe {
        PyErr_WriteUnraisable(std::ptr::null_mut());
    }
}

fn make_error(err: Error) -> PyErr {
    Python::with_gil(|py| {
        let template_error: &Py<PyAny> = TEMPLATE_ERROR.get_or_init(|| {
            let module = py.import_bound("minijinja._internal").unwrap();
            let err = module.getattr("make_error").unwrap();
            err.into()
        });
        let args = PyTuple::new_bound(py, [Bound::new(py, ErrorInfo { err }).unwrap()]);
        PyErr::from_value_bound(template_error.call1(py, args).unwrap().bind(py).clone())
    })
}
