use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use minijinja::value::{Enumerator, Object, ObjectRepr, Value, ValueKind};
use minijinja::{AutoEscape, Error, State};

use once_cell::sync::OnceCell;
use pyo3::prelude::*;
use pyo3::pybacked::PyBackedStr;
use pyo3::types::{PyDict, PyList, PySequence, PyTuple};

use crate::error_support::{to_minijinja_error, to_py_error};
use crate::state::{bind_state, StateRef};

static AUTO_ESCAPE_CACHE: Mutex<BTreeMap<String, AutoEscape>> = Mutex::new(BTreeMap::new());
static MARK_SAFE: OnceCell<Py<PyAny>> = OnceCell::new();

fn is_safe_attr(name: &str) -> bool {
    !name.starts_with('_')
}

fn is_dictish(val: &Bound<'_, PyAny>) -> bool {
    val.hasattr("__getitem__").unwrap_or(false) && val.hasattr("items").unwrap_or(false)
}

pub struct DynamicObject {
    pub inner: Py<PyAny>,
}

impl DynamicObject {
    pub fn new(inner: Py<PyAny>) -> DynamicObject {
        DynamicObject { inner }
    }
}

impl fmt::Debug for DynamicObject {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Python::with_gil(|py| write!(f, "{}", self.inner.bind(py)))
    }
}

impl Object for DynamicObject {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            if inner.downcast::<PySequence>().is_ok() {
                ObjectRepr::Seq
            } else if is_dictish(inner) {
                ObjectRepr::Map
            } else if inner.iter().is_ok() {
                ObjectRepr::Iterable
            } else {
                ObjectRepr::Plain
            }
        })
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        Python::with_gil(|py| write!(f, "{}", self.inner.bind(py)))
    }

    fn call(self: &Arc<Self>, state: &State, args: &[Value]) -> Result<Value, Error> {
        Python::with_gil(|py| -> Result<Value, Error> {
            bind_state(state, || {
                let inner = self.inner.bind(py);
                let (py_args, py_kwargs) =
                    to_python_args(py, inner, args).map_err(to_minijinja_error)?;
                Ok(to_minijinja_value(
                    &inner
                        .call(py_args, py_kwargs.as_ref())
                        .map_err(to_minijinja_error)?,
                ))
            })
        })
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State,
        name: &str,
        args: &[Value],
    ) -> Result<Value, Error> {
        if !is_safe_attr(name) {
            return Err(Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "insecure method call",
            ));
        }
        Python::with_gil(|py| -> Result<Value, Error> {
            bind_state(state, || {
                let inner = self.inner.bind(py);
                let (py_args, py_kwargs) =
                    to_python_args(py, inner, args).map_err(to_minijinja_error)?;
                Ok(to_minijinja_value(
                    &inner
                        .call_method(name, py_args, py_kwargs.as_ref())
                        .map_err(to_minijinja_error)?,
                ))
            })
        })
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            match inner.get_item(to_python_value_impl(py, key.clone()).ok()?) {
                Ok(value) => Some(to_minijinja_value(&value)),
                Err(_) => {
                    if let Some(attr) = key.as_str() {
                        if is_safe_attr(attr) {
                            if let Ok(rv) = inner.getattr(attr) {
                                return Some(to_minijinja_value(&rv));
                            }
                        }
                    }
                    None
                }
            }
        })
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Python::with_gil(|py| {
            let inner = self.inner.bind(py);
            if inner.downcast::<PySequence>().is_ok() {
                Enumerator::Seq(inner.len().unwrap_or(0))
            } else if let Ok(iter) = inner.iter() {
                Enumerator::Values(
                    iter.filter_map(|x| match x {
                        Ok(x) => Some(to_minijinja_value(&x)),
                        Err(_) => None,
                    })
                    .collect(),
                )
            } else {
                Enumerator::NonEnumerable
            }
        })
    }
}

pub fn to_minijinja_value(value: &Bound<'_, PyAny>) -> Value {
    if value.is_none() {
        Value::from(())
    } else if let Ok(val) = value.extract::<bool>() {
        Value::from(val)
    } else if let Ok(val) = value.extract::<i64>() {
        Value::from(val)
    } else if let Ok(val) = value.extract::<f64>() {
        Value::from(val)
    } else if let Ok(val) = value.extract::<PyBackedStr>() {
        if let Ok(to_html) = value.getattr("__html__") {
            if to_html.is_callable() {
                // TODO: if to_minijinja_value returns results we could
                // report the swallowed error of __html__.
                if let Ok(html) = to_html.call0() {
                    if let Ok(val) = html.extract::<PyBackedStr>() {
                        return Value::from_safe_string(val.to_string());
                    }
                }
            }
        }
        Value::from(val.to_string())
    } else {
        Value::from_object(DynamicObject::new(value.clone().unbind()))
    }
}

pub fn to_python_value(value: Value) -> PyResult<Py<PyAny>> {
    Python::with_gil(|py| to_python_value_impl(py, value))
}

fn mark_string_safe(py: Python<'_>, value: &str) -> PyResult<Py<PyAny>> {
    let mark_safe: &Py<PyAny> = MARK_SAFE.get_or_try_init::<_, PyErr>(|| {
        let module = py.import_bound("minijinja._internal")?;
        Ok(module.getattr("mark_safe")?.into())
    })?;
    mark_safe.call1(py, PyTuple::new_bound(py, [value]))
}

fn to_python_value_impl(py: Python<'_>, value: Value) -> PyResult<Py<PyAny>> {
    // if we are holding a true dynamic object, we want to allow bidirectional
    // conversion.  That means that when passing the object back to Python we
    // extract the retained raw Python reference.
    if let Some(pyobj) = value.downcast_object_ref::<DynamicObject>() {
        return Ok(pyobj.inner.clone_ref(py));
    }

    if let Some(obj) = value.as_object() {
        match obj.repr() {
            ObjectRepr::Plain => return Ok(obj.to_string().into_py(py)),
            ObjectRepr::Map => {
                let rv = PyDict::new_bound(py);
                if let Some(pair_iter) = obj.try_iter_pairs() {
                    for (key, value) in pair_iter {
                        rv.set_item(
                            to_python_value_impl(py, key)?,
                            to_python_value_impl(py, value)?,
                        )?;
                    }
                }
                return Ok(rv.into());
            }
            ObjectRepr::Seq | ObjectRepr::Iterable => {
                let rv = PyList::empty_bound(py);
                if let Some(iter) = obj.try_iter() {
                    for value in iter {
                        rv.append(to_python_value_impl(py, value)?)?;
                    }
                }
                return Ok(rv.into());
            }
            _ => {}
        }
    }

    match value.kind() {
        ValueKind::Undefined | ValueKind::None => Ok(().into_py(py)),
        ValueKind::Bool => Ok(value.is_true().into_py(py)),
        ValueKind::Number => {
            if let Ok(rv) = TryInto::<i64>::try_into(value.clone()) {
                Ok(rv.into_py(py))
            } else if let Ok(rv) = TryInto::<u64>::try_into(value.clone()) {
                Ok(rv.into_py(py))
            } else if let Ok(rv) = TryInto::<f64>::try_into(value) {
                Ok(rv.into_py(py))
            } else {
                unreachable!()
            }
        }
        ValueKind::String => {
            if value.is_safe() {
                Ok(mark_string_safe(py, value.as_str().unwrap())?)
            } else {
                Ok(value.as_str().unwrap().into_py(py))
            }
        }
        ValueKind::Bytes => Ok(value.as_bytes().unwrap().into_py(py)),
        kind => Err(to_py_error(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!("object {} cannot roundtrip", kind),
        ))),
    }
}

pub fn to_python_args<'py>(
    py: Python<'py>,
    callback: &Bound<'_, PyAny>,
    args: &[Value],
) -> PyResult<(Bound<'py, PyTuple>, Option<Bound<'py, PyDict>>)> {
    let mut py_args = Vec::new();
    let mut py_kwargs = None;

    if callback
        .getattr("__minijinja_pass_state__")
        .is_ok_and(|x| x.is_truthy().unwrap_or(false))
    {
        py_args.push(Bound::new(py, StateRef)?.to_object(py));
    }

    for arg in args {
        if arg.is_kwargs() {
            let kwargs = py_kwargs.get_or_insert_with(|| PyDict::new_bound(py));
            if let Ok(iter) = arg.try_iter() {
                for k in iter {
                    if let Ok(v) = arg.get_item(&k) {
                        kwargs
                            .set_item(to_python_value_impl(py, k)?, to_python_value_impl(py, v)?)?;
                    }
                }
            }
        } else {
            py_args.push(to_python_value_impl(py, arg.clone())?);
        }
    }
    let py_args = PyTuple::new_bound(py, py_args);
    Ok((py_args, py_kwargs))
}

pub fn get_custom_autoescape(value: &str) -> AutoEscape {
    let mut cache = AUTO_ESCAPE_CACHE.lock().unwrap();
    if let Some(rv) = cache.get(value).copied() {
        return rv;
    }
    let val = AutoEscape::Custom(Box::leak(value.to_string().into_boxed_str()));
    cache.insert(value.to_string(), val);
    val
}
