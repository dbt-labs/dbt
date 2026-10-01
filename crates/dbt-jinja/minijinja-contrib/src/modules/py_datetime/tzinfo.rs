use chrono::FixedOffset;
use minijinja::arg_utils::ArgParser;
use minijinja::{value::Object, Error, ErrorKind, Value};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use super::timedelta::PyTimeDelta;

// ── PyTzInfoClass (modules.datetime.tzinfo) ─────────────────────────

#[derive(Clone, Debug)]
pub struct PyTzInfoClass;

impl Object for PyTzInfoClass {
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        _args: &[Value],
        _listeners: &[Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        Err(Error::new(
            ErrorKind::InvalidOperation,
            "tzinfo(...) is not callable; use timezone(...) instead.",
        ))
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<class 'datetime.tzinfo'>")
    }
}

// ── PyTimezoneClass (modules.datetime.timezone) ─────────────────────

/// Python's `datetime.timezone` class. Supports:
///   - `timezone.utc`  — the UTC timezone constant
///   - `timezone(offset)` / `timezone(offset, name)` — fixed-offset timezone constructor
#[derive(Clone, Debug)]
pub struct PyTimezoneClass;

impl Object for PyTimezoneClass {
    /// `timezone(offset, name=None)` constructor
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let offset_val: Value = parser.next_positional()?;
        let name: Option<String> = parser.get_optional("name");

        let delta = offset_val
            .downcast_object_ref::<PyTimeDelta>()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "timezone() argument must be a timedelta",
                )
            })?;

        let total_secs = delta.duration.num_seconds() as i32;
        let offset = FixedOffset::east_opt(total_secs).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!(
                    "offset must be a timedelta strictly between -timedelta(hours=24) \
                     and timedelta(hours=24), got {total_secs}s"
                ),
            )
        })?;

        Ok(Value::from_object(PyFixedTimezone { offset, name }))
    }

    /// Attribute access: `timezone.utc`
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "utc" => Some(Value::from_object(PyFixedTimezone {
                offset: FixedOffset::east_opt(0).unwrap(),
                name: Some("UTC".to_string()),
            })),
            _ => None,
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<class 'datetime.timezone'>")
    }
}

// ── PyFixedTimezone ─────────────────────────────────────────────────

/// A fixed-offset timezone object, equivalent to Python's `datetime.timezone(offset)`.
#[derive(Clone, Debug)]
pub struct PyFixedTimezone {
    pub offset: FixedOffset,
    pub name: Option<String>,
}

impl PyFixedTimezone {
    pub fn total_seconds(&self) -> i32 {
        self.offset.local_minus_utc()
    }
}

impl fmt::Display for PyFixedTimezone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref name) = self.name {
            write!(f, "{name}")
        } else {
            let secs = self.offset.local_minus_utc();
            if secs == 0 {
                write!(f, "UTC")
            } else {
                let sign = if secs < 0 { '-' } else { '+' };
                let abs_secs = secs.unsigned_abs();
                let hours = abs_secs / 3600;
                let minutes = (abs_secs % 3600) / 60;
                write!(f, "UTC{sign}{hours:02}:{minutes:02}")
            }
        }
    }
}

impl Object for PyFixedTimezone {
    fn is_true(self: &Arc<Self>) -> bool {
        true
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "localize" => {
                let dt_val = args.first().ok_or_else(|| {
                    Error::new(
                        ErrorKind::MissingArgument,
                        "localize() requires a datetime argument",
                    )
                })?;

                let dt = dt_val
                    .downcast_object_ref::<crate::modules::py_datetime::datetime::PyDateTime>()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "localize() expects a datetime object",
                        )
                    })?;

                use crate::modules::py_datetime::datetime::DateTimeState;
                let naive_dt = match &dt.state {
                    DateTimeState::Naive(ndt) => *ndt,
                    _ => {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "localize() requires a naive datetime (tzinfo must be None)",
                        ));
                    }
                };

                let aware_dt = naive_dt
                    .and_local_timezone(self.offset)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "ambiguous or invalid local time for localize()",
                        )
                    })?;

                Ok(Value::from_object(
                    crate::modules::py_datetime::datetime::PyDateTime {
                        state: DateTimeState::FixedOffset(aware_dt),
                        tzinfo: None,
                    },
                ))
            }
            "utcoffset" => {
                let secs = self.total_seconds();
                Ok(Value::from_object(PyTimeDelta::new(
                    chrono::Duration::seconds(secs as i64),
                )))
            }
            "tzname" => Ok(Value::from(format!("{self}"))),
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("timezone object has no method named '{method}'"),
            )),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.as_ref(), f)
    }
}
