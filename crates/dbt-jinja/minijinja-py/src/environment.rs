use std::borrow::Cow;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Mutex;

use minijinja::syntax::SyntaxConfig;
use minijinja::value::{Rest, Value};
use minijinja::{context, escape_formatter, AutoEscape, Error, State, UndefinedBehavior};
use pyo3::exceptions::PyRuntimeError;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedStr;
use pyo3::types::{PyDict, PyTuple};

use crate::error_support::{report_unraisable, to_minijinja_error, to_py_error};
use crate::state::bind_state;
use crate::typeconv::{
    get_custom_autoescape, to_minijinja_value, to_python_args, to_python_value, DynamicObject,
};

thread_local! {
    static CURRENT_ENV: AtomicPtr<c_void> = const { AtomicPtr::new(std::ptr::null_mut()) };
}

struct Syntax {
    block_start: String,
    block_end: String,
    variable_start: String,
    variable_end: String,
    comment_start: String,
    comment_end: String,
    line_statement_prefix: String,
    line_comment_prefix: String,
}

impl Default for Syntax {
    fn default() -> Self {
        Self {
            block_start: "{%".into(),
            block_end: "%}".into(),
            variable_start: "{{".into(),
            variable_end: "}}".into(),
            comment_start: "{#".into(),
            comment_end: "#}".into(),
            line_statement_prefix: "".into(),
            line_comment_prefix: "".into(),
        }
    }
}
impl Syntax {
    fn compile(&self) -> Result<SyntaxConfig, Error> {
        SyntaxConfig::builder()
            .block_delimiters(self.block_start.clone(), self.block_end.clone())
            .variable_delimiters(self.variable_start.clone(), self.variable_end.clone())
            .comment_delimiters(self.comment_start.clone(), self.comment_end.clone())
            .line_statement_prefix(self.line_statement_prefix.clone())
            .line_comment_prefix(self.line_comment_prefix.clone())
            .build()
    }
}

macro_rules! syntax_setter {
    ($slf:expr, $value:expr, $field:ident, $default:expr) => {{
        let value = $value;
        let mut inner = $slf.inner.lock().unwrap();
        if inner.syntax.is_none() {
            if value == $default {
                return Ok(());
            }
            inner.syntax = Some(Syntax::default());
        }
        if let Some(ref mut syntax) = inner.syntax {
            if syntax.$field != value {
                syntax.$field = value.into();
                let syntax_config = syntax.compile().map_err(to_py_error)?;
                inner.env.set_syntax(syntax_config);
            }
        }
        Ok(())
    }};
}

macro_rules! syntax_getter {
    ($slf:expr, $field:ident, $default:expr) => {{
        $slf.inner
            .lock()
            .unwrap()
            .syntax
            .as_ref()
            .map_or($default, |x| &x.$field)
            .into()
    }};
}

struct Inner {
    env: minijinja::Environment<'static>,
    loader: Option<Py<PyAny>>,
    auto_escape_callback: Option<Py<PyAny>>,
    finalizer_callback: Option<Py<PyAny>>,
    path_join_callback: Option<Py<PyAny>>,
    syntax: Option<Syntax>,
}

/// Represents a MiniJinja environment.
#[pyclass(subclass, module = "minijinja._lowlevel")]
pub struct Environment {
    inner: Mutex<Inner>,
    reload_before_render: AtomicBool,
}

#[pymethods]
impl Environment {
    #[new]
    fn py_new() -> PyResult<Self> {
        Ok(Environment {
            inner: Mutex::new(Inner {
                env: minijinja::Environment::new(),
                loader: None,
                auto_escape_callback: None,
                finalizer_callback: None,
                path_join_callback: None,
                syntax: None,
            }),
            reload_before_render: AtomicBool::new(false),
        })
    }

    /// Enables or disables debug mode.
    #[setter]
    pub fn set_debug(&self, value: bool) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.env.set_debug(value);
        Ok(())
    }

    /// Enables or disables debug mode.
    #[getter]
    pub fn get_debug(&self) -> PyResult<bool> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.env.debug())
    }

    /// Sets the undefined behavior.
    #[setter]
    pub fn set_undefined_behavior(&self, value: &str) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.env.set_undefined_behavior(match value {
            "strict" => UndefinedBehavior::Strict,
            "lenient" => UndefinedBehavior::Lenient,
            "chainable" => UndefinedBehavior::Chainable,
            _ => {
                return Err(PyRuntimeError::new_err(
                    "invalid value for undefined behavior",
                ))
            }
        });
        Ok(())
    }

    /// Gets the undefined behavior.
    #[getter]
    pub fn get_undefined_behavior(&self) -> PyResult<&'static str> {
        let inner = self.inner.lock().unwrap();
        Ok(match inner.env.undefined_behavior() {
            UndefinedBehavior::Lenient => "lenient",
            UndefinedBehavior::Chainable => "chainable",
            UndefinedBehavior::Strict => "strict",
            _ => {
                return Err(PyRuntimeError::new_err(
                    "invalid value for undefined behavior",
                ))
            }
        })
    }

    /// Sets fuel
    #[setter]
    pub fn set_fuel(&self, value: Option<u64>) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner.env.set_fuel(value);
        Ok(())
    }

    /// Enables or disables debug mode.
    #[getter]
    pub fn get_fuel(&self) -> PyResult<Option<u64>> {
        let inner = self.inner.lock().unwrap();
        Ok(inner.env.fuel())
    }

    /// Registers a filter function.
    #[pyo3(text_signature = "(self, name, callback)")]
    pub fn add_filter(&self, name: &str, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(PyRuntimeError::new_err("expected callback"));
        }
        let callback: Py<PyAny> = callback.clone().unbind();
        self.inner.lock().unwrap().env.add_filter(
            name.to_string(),
            move |state: &State, args: Rest<Value>| -> Result<Value, Error> {
                Python::with_gil(|py| {
                    bind_state(state, || {
                        let (py_args, py_kwargs) = to_python_args(py, callback.bind(py), &args)
                            .map_err(to_minijinja_error)?;
                        let rv = callback
                            .call_bound(py, py_args, py_kwargs.as_ref())
                            .map_err(to_minijinja_error)?;
                        Ok(to_minijinja_value(rv.bind(py)))
                    })
                })
            },
        );
        Ok(())
    }

    /// Removes a filter function.
    #[pyo3(text_signature = "(self, name)")]
    pub fn remove_filter(&self, name: &str) -> PyResult<()> {
        self.inner.lock().unwrap().env.remove_filter(name);
        Ok(())
    }

    /// Registers a test function.
    #[pyo3(text_signature = "(self, name, callback)")]
    pub fn add_test(&self, name: &str, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(PyRuntimeError::new_err("expected callback"));
        }
        let callback: Py<PyAny> = callback.clone().unbind();
        self.inner.lock().unwrap().env.add_test(
            name.to_string(),
            move |state: &State, args: Rest<Value>| -> Result<bool, Error> {
                Python::with_gil(|py| {
                    bind_state(state, || {
                        let (py_args, py_kwargs) = to_python_args(py, callback.bind(py), &args)
                            .map_err(to_minijinja_error)?;
                        let rv = callback
                            .call_bound(py, py_args, py_kwargs.as_ref())
                            .map_err(to_minijinja_error)?;
                        Ok(to_minijinja_value(rv.bind(py)).is_true())
                    })
                })
            },
        );
        Ok(())
    }

    /// Removes a test function.
    #[pyo3(text_signature = "(self, name)")]
    pub fn remove_test(&self, name: &str) -> PyResult<()> {
        self.inner.lock().unwrap().env.remove_test(name);
        Ok(())
    }

    fn add_function(&self, name: &str, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        let callback: Py<PyAny> = callback.clone().unbind();
        self.inner.lock().unwrap().env.add_function(
            name.to_string(),
            move |state: &State, args: Rest<Value>| -> Result<Value, Error> {
                Python::with_gil(|py| {
                    bind_state(state, || {
                        let (py_args, py_kwargs) = to_python_args(py, callback.bind(py), &args)
                            .map_err(to_minijinja_error)?;
                        let rv = callback
                            .call_bound(py, py_args, py_kwargs.as_ref())
                            .map_err(to_minijinja_error)?;
                        Ok(to_minijinja_value(rv.bind(py)))
                    })
                })
            },
        );
        Ok(())
    }

    /// Registers a global
    #[pyo3(text_signature = "(self, name, value)")]
    pub fn add_global(&self, name: &str, value: &Bound<'_, PyAny>) -> PyResult<()> {
        if value.is_callable() {
            self.add_function(name, value)
        } else {
            self.inner
                .lock()
                .unwrap()
                .env
                .add_global(name.to_string(), to_minijinja_value(value));
            Ok(())
        }
    }

    /// Removes a global
    #[pyo3(text_signature = "(self, name)")]
    pub fn remove_global(&self, name: &str) -> PyResult<()> {
        self.inner.lock().unwrap().env.remove_global(name);
        Ok(())
    }

    /// Sets an auto escape callback.
    ///
    /// Note that because this interface in MiniJinja is infallible, the callback is
    /// not able to raise an error.
    #[setter]
    pub fn set_auto_escape_callback(&self, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(PyRuntimeError::new_err("expected callback"));
        }
        let callback: Py<PyAny> = callback.clone().unbind();
        let mut inner = self.inner.lock().unwrap();
        inner.auto_escape_callback = Python::with_gil(|py| Some(callback.clone_ref(py)));
        inner
            .env
            .set_auto_escape_callback(move |name: &str| -> AutoEscape {
                Python::with_gil(|py| {
                    let py_args = PyTuple::new_bound(py, [name]);
                    let rv = match callback.call_bound(py, py_args, None) {
                        Ok(value) => value,
                        Err(err) => {
                            report_unraisable(py, err);
                            return AutoEscape::None;
                        }
                    };
                    let rv = rv.bind(py);
                    if rv.is_none() {
                        return AutoEscape::None;
                    }
                    if let Ok(value) = rv.extract::<PyBackedStr>() {
                        match &value as &str {
                            "html" => AutoEscape::Html,
                            "json" => AutoEscape::Json,
                            other => get_custom_autoescape(other),
                        }
                    } else if let Ok(value) = rv.extract::<bool>() {
                        match value {
                            true => AutoEscape::Html,
                            false => AutoEscape::None,
                        }
                    } else {
                        AutoEscape::None
                    }
                })
            });
        Ok(())
    }

    #[getter]
    pub fn get_auto_escape_callback(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .auto_escape_callback
            .as_ref()
            .map(|x| x.clone_ref(py)))
    }

    /// Sets a finalizer.
    ///
    /// A finalizer is called before a value is rendered to customize it.
    #[setter]
    pub fn set_finalizer(&self, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(PyRuntimeError::new_err("expected callback"));
        }
        let callback: Py<PyAny> = callback.clone().unbind();
        let mut inner = self.inner.lock().unwrap();
        Python::with_gil(|py| {
            inner.finalizer_callback = Some(callback.clone_ref(py));
        });
        inner.env.set_formatter(move |output, state, value| {
            Python::with_gil(|py| -> Result<(), Error> {
                let maybe_new_value = bind_state(state, || -> Result<_, Error> {
                    let args = std::slice::from_ref(value);
                    let (py_args, py_kwargs) =
                        to_python_args(py, callback.bind(py), args).map_err(to_minijinja_error)?;
                    let rv = callback
                        .call_bound(py, py_args, py_kwargs.as_ref())
                        .map_err(to_minijinja_error)?;
                    if rv.is(&py.NotImplemented()) {
                        Ok(None)
                    } else {
                        Ok(Some(to_minijinja_value(rv.bind(py))))
                    }
                })?;
                let value = match maybe_new_value {
                    Some(ref new_value) => new_value,
                    None => value,
                };
                escape_formatter(output, state, value)
            })
        });
        Ok(())
    }

    #[getter]
    pub fn get_finalizer(&self, py: Python<'_>) -> PyResult<Option<Py<PyAny>>> {
        Ok(self
            .inner
            .lock()
            .unwrap()
            .finalizer_callback
            .as_ref()
            .map(|x| x.clone_ref(py)))
    }

    /// Sets a loader function for the environment.
    ///
    /// The loader function is invoked with the name of the template to load.  If the
    /// template exists the source code of the template should be returned a string,
    /// otherwise `None` can be used to indicate that the template does not exist.
    #[setter]
    pub fn set_loader(&self, callback: Option<&Bound<'_, PyAny>>) -> PyResult<()> {
        let callback = match callback {
            None => None,
            Some(callback) => {
                if !callback.is_callable() {
                    return Err(PyRuntimeError::new_err("expected callback"));
                }
                Some(callback.clone().unbind())
            }
        };
        let mut inner = self.inner.lock().unwrap();
        Python::with_gil(|py| {
            inner.loader = callback.as_ref().map(|x| x.clone_ref(py));
        });

        if let Some(callback) = callback {
            inner.env.set_loader(move |name| {
                Python::with_gil(|py| {
                    let callback = callback.bind(py);
                    let rv = callback
                        .call1(PyTuple::new_bound(py, [name]))
                        .map_err(to_minijinja_error)?;
                    if rv.is_none() {
                        Ok(None)
                    } else {
                        Ok(Some(rv.to_string()))
                    }
                })
            })
        }

        Ok(())
    }

    /// Returns the current loader.
    #[getter]
    pub fn get_loader(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.inner
            .lock()
            .unwrap()
            .loader
            .as_ref()
            .map(|x| x.clone_ref(py))
    }

    /// Sets a new path join callback.
    #[setter]
    pub fn set_path_join_callback(&self, callback: &Bound<'_, PyAny>) -> PyResult<()> {
        if !callback.is_callable() {
            return Err(PyRuntimeError::new_err("expected callback"));
        }
        let callback: Py<PyAny> = callback.clone().unbind();
        let mut inner = self.inner.lock().unwrap();
        Python::with_gil(|py| {
            inner.path_join_callback = Some(callback.clone_ref(py));
        });
        inner.env.set_path_join_callback(move |name, parent| {
            Python::with_gil(|py| {
                let callback = callback.bind(py);
                match callback.call1(PyTuple::new_bound(py, [name, parent])) {
                    Ok(rv) => Cow::Owned(rv.to_string()),
                    Err(err) => {
                        report_unraisable(py, err);
                        Cow::Borrowed(name)
                    }
                }
            })
        });
        Ok(())
    }

    /// Returns the current path join callback.
    #[getter]
    pub fn get_path_join_callback(&self, py: Python<'_>) -> Option<Py<PyAny>> {
        self.inner
            .lock()
            .unwrap()
            .path_join_callback
            .as_ref()
            .map(|x| x.clone_ref(py))
    }

    /// Triggers a reload of the templates.
    pub fn reload(&self, py: Python<'_>) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        let loader = inner.loader.as_ref().map(|x| x.clone_ref(py));
        if loader.is_some() {
            inner.env.clear_templates();
        }
        Ok(())
    }

    /// Can be used to instruct the environment to automatically reload templates
    /// before each render.
    #[setter]
    pub fn set_reload_before_render(&self, yes: bool) {
        self.reload_before_render.store(yes, Ordering::Relaxed);
    }

    #[getter]
    pub fn get_reload_before_render(&self) -> bool {
        self.reload_before_render.load(Ordering::Relaxed)
    }

    #[setter]
    pub fn set_variable_start_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, variable_start, "{{")
    }

    #[getter]
    pub fn get_variable_start_string(&self) -> String {
        syntax_getter!(self, variable_start, "{{")
    }

    #[setter]
    pub fn set_block_start_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, block_start, "{%")
    }

    #[getter]
    pub fn get_block_start_string(&self) -> String {
        syntax_getter!(self, block_start, "{%")
    }

    #[setter]
    pub fn set_comment_start_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, comment_start, "{#")
    }

    #[getter]
    pub fn get_comment_start_string(&self) -> String {
        syntax_getter!(self, comment_start, "{#")
    }

    #[setter]
    pub fn set_variable_end_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, variable_end, "}}")
    }

    #[getter]
    pub fn get_variable_end_string(&self) -> String {
        syntax_getter!(self, variable_end, "}}")
    }

    #[setter]
    pub fn set_block_end_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, block_end, "%}")
    }

    #[getter]
    pub fn get_block_end_string(&self) -> String {
        syntax_getter!(self, block_end, "%}")
    }

    #[setter]
    pub fn set_comment_end_string(&self, value: String) -> PyResult<()> {
        syntax_setter!(self, value, comment_end, "#}")
    }

    #[getter]
    pub fn get_comment_end_string(&self) -> String {
        syntax_getter!(self, comment_end, "#}")
    }

    #[setter]
    pub fn set_line_statement_prefix(&self, value: Option<String>) -> PyResult<()> {
        syntax_setter!(self, value.unwrap_or_default(), line_statement_prefix, "")
    }

    #[getter]
    pub fn get_line_statement_prefix(&self) -> Option<String> {
        let rv: String = syntax_getter!(self, line_statement_prefix, "");
        (!rv.is_empty()).then_some(rv)
    }

    #[setter]
    pub fn set_line_comment_prefix(&self, value: Option<String>) -> PyResult<()> {
        syntax_setter!(self, value.unwrap_or_default(), line_comment_prefix, "")
    }

    #[getter]
    pub fn get_line_comment_prefix(&self) -> Option<String> {
        let rv: String = syntax_getter!(self, line_comment_prefix, "");
        (!rv.is_empty()).then_some(rv)
    }

    /// Configures the trailing newline trimming feature.
    #[setter]
    pub fn set_keep_trailing_newline(&self, yes: bool) -> PyResult<()> {
        self.inner
            .lock()
            .unwrap()
            .env
            .set_keep_trailing_newline(yes);
        Ok(())
    }

    /// Returns the current value of the trailing newline trimming flag.
    #[getter]
    pub fn get_keep_trailing_newline(&self) -> PyResult<bool> {
        Ok(self.inner.lock().unwrap().env.keep_trailing_newline())
    }

    /// Configures the trim blocks feature.
    #[setter]
    pub fn set_trim_blocks(&self, yes: bool) -> PyResult<()> {
        self.inner.lock().unwrap().env.set_trim_blocks(yes);
        Ok(())
    }

    /// Returns the current value of the trim blocks flag.
    #[getter]
    pub fn get_trim_blocks(&self) -> PyResult<bool> {
        Ok(self.inner.lock().unwrap().env.trim_blocks())
    }

    /// Configures the lstrip blocks feature.
    #[setter]
    pub fn set_lstrip_blocks(&self, yes: bool) -> PyResult<()> {
        self.inner.lock().unwrap().env.set_lstrip_blocks(yes);
        Ok(())
    }

    /// Returns the current value of the lstrip blocks flag.
    #[getter]
    pub fn get_lstrip_blocks(&self) -> PyResult<bool> {
        Ok(self.inner.lock().unwrap().env.lstrip_blocks())
    }

    /// Manually adds a template to the environment.
    pub fn add_template(&self, name: String, source: String) -> PyResult<()> {
        let mut inner = self.inner.lock().unwrap();
        inner
            .env
            .add_template_owned(name, source)
            .map_err(to_py_error)
    }

    /// Removes a loaded template.
    pub fn remove_template(&self, name: &str) {
        self.inner.lock().unwrap().env.remove_template(name);
    }

    /// Clears all loaded templates.
    pub fn clear_templates(&self) {
        self.inner.lock().unwrap().env.clear_templates();
    }

    /// Renders a template looked up from the loader.
    ///
    /// The first argument is the name of the template, all other arguments must be passed
    /// as keyword arguments and are pass as render context of the template.
    #[pyo3(signature = (template_name, /, **ctx))]
    pub fn render_template(
        slf: PyRef<'_, Self>,
        py: Python<'_>,
        template_name: &str,
        ctx: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<String> {
        if slf.reload_before_render.load(Ordering::Relaxed) {
            slf.reload(py)?;
        }
        bind_environment(slf.as_ptr(), || {
            let inner = slf.inner.lock().unwrap();
            let tmpl = inner.env.get_template(template_name).map_err(to_py_error)?;
            let ctx = ctx
                .map(|ctx| Value::from_object(DynamicObject::new(ctx.as_any().clone().unbind())))
                .unwrap_or_else(|| context!());
            tmpl.render(ctx).map_err(to_py_error)
        })
    }

    /// Renders a template from a string
    ///
    /// The first argument is the source of the template, all other arguments must be passed
    /// as keyword arguments and are pass as render context of the template.
    #[pyo3(signature = (source, name=None, /, **ctx))]
    pub fn render_str(
        slf: PyRef<'_, Self>,
        source: &str,
        name: Option<&str>,
        ctx: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<String> {
        bind_environment(slf.as_ptr(), || {
            let ctx = ctx
                .map(|ctx| Value::from_object(DynamicObject::new(ctx.as_any().clone().unbind())))
                .unwrap_or_else(|| context!());
            slf.inner
                .lock()
                .unwrap()
                .env
                .render_named_str(name.unwrap_or("<string>"), source, ctx)
                .map_err(to_py_error)
        })
    }

    /// Evaluates an expression with a given context.
    #[pyo3(signature = (expression, /, **ctx))]
    pub fn eval_expr(
        slf: PyRef<'_, Self>,
        expression: &str,
        ctx: Option<&Bound<'_, PyDict>>,
    ) -> PyResult<Py<PyAny>> {
        bind_environment(slf.as_ptr(), || {
            let inner = slf.inner.lock().unwrap();
            let expr = inner
                .env
                .compile_expression(expression)
                .map_err(to_py_error)?;
            let ctx = ctx
                .map(|ctx| Value::from_object(DynamicObject::new(ctx.as_any().clone().unbind())))
                .unwrap_or_else(|| context!());
            to_python_value(expr.eval(ctx).map_err(to_py_error)?)
        })
    }
}

pub fn with_environment<R, F: FnOnce(Py<Environment>) -> PyResult<R>>(f: F) -> PyResult<R> {
    Python::with_gil(|py| {
        CURRENT_ENV.with(|handle| {
            let ptr = handle.load(Ordering::Relaxed) as *mut _;
            match unsafe { Py::<Environment>::from_borrowed_ptr_or_opt(py, ptr) } {
                Some(env) => f(env),
                None => Err(PyRuntimeError::new_err(
                    "environment cannot be used outside of template render",
                )),
            }
        })
    })
}

/// Invokes a function with the state stashed away.
pub fn bind_environment<R, F: FnOnce() -> R>(envptr: *mut pyo3::ffi::PyObject, f: F) -> R {
    let old_handle = CURRENT_ENV
        .with(|handle| handle.swap(envptr as *const _ as *mut c_void, Ordering::Relaxed));
    let rv = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    CURRENT_ENV.with(|handle| handle.store(old_handle, Ordering::Relaxed));
    match rv {
        Ok(rv) => rv,
        Err(payload) => std::panic::resume_unwind(payload),
    }
}
