use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeZone};
use chrono_tz::{OffsetComponents, OffsetName, Tz, TZ_VARIANTS};
use minijinja::{Error, ErrorKind, Value};

use crate::modules::py_datetime::datetime::{DateTimeState, PyDateTime};
use crate::modules::py_datetime::timedelta::PyTimeDelta;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, OnceLock};

/// A Python-like "pytz" timezone object that wraps a `chrono_tz::Tz`.
#[derive(Debug, Clone)]
pub struct PytzTimezone {
    pub tz: Tz,
}

static TIMEZONES: OnceLock<Arc<HashMap<String, Tz>>> = OnceLock::new();

fn get_timezones() -> Arc<HashMap<String, Tz>> {
    TIMEZONES
        .get_or_init(|| {
            Arc::new(
                TZ_VARIANTS
                    .iter()
                    .map(|tz| (tz.name().to_lowercase(), *tz))
                    .collect(),
            )
        })
        .clone()
}

impl PytzTimezone {
    /// Constructor that simply stores a chrono_tz::Tz
    pub fn new(tz: Tz) -> Self {
        PytzTimezone { tz }
    }

    /// Convert a naive local datetime to a `DateTime<Tz>`, returning `None` if it's invalid/ambiguous.
    pub fn from_local(&self, naive: &NaiveDateTime) -> Option<DateTime<Tz>> {
        match self.tz.from_local_datetime(naive) {
            LocalResult::None => None,
            LocalResult::Single(dt) => Some(dt),
            LocalResult::Ambiguous(_earliest, _latest) => {
                // Decide how you want to handle an ambiguous time (DST overlap).
                // For demonstration, we'll just return None to indicate "ambiguous."
                None
            }
        }
    }

    /// Convert a naive UTC datetime to a `DateTime<Tz>`, returning `None` if invalid.
    pub fn from_utc(&self, naive_utc: &NaiveDateTime) -> DateTime<Tz> {
        self.tz.from_utc_datetime(naive_utc)
    }

    /// Return the offset from UTC in effect at the given DateTime in this tz.
    pub fn utcoffset(&self, local_dt: &DateTime<Tz>) -> chrono::Duration {
        chrono::Duration::seconds(local_dt.offset().fix().local_minus_utc() as i64)
    }

    /// Return the daylight saving part of the offset in effect at the given DateTime.
    pub fn dst(&self, local_dt: &DateTime<Tz>) -> chrono::Duration {
        local_dt.offset().dst_offset()
    }

    /// Return the abbreviation in effect at the given DateTime ("EST", "EDT", "UTC"),
    /// or the numeric offset for zones without one.
    pub fn tzname(&self, local_dt: &DateTime<Tz>) -> String {
        match local_dt.offset().abbreviation() {
            Some(name) => name.to_string(),
            None => local_dt.offset().to_string(),
        }
    }

    /// Resolve the datetime argument of utcoffset(), dst() and tzname() in this tz.
    /// `None` stays `None`; a naive datetime is read as local time, like localize();
    /// an aware datetime is converted so the answer describes the same instant.
    fn resolve_argument(
        &self,
        method: &str,
        args: &[Value],
    ) -> Result<Option<DateTime<Tz>>, Error> {
        let dt_val = args.first().ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                format!("{method}() requires a datetime argument"),
            )
        })?;
        if dt_val.is_none() {
            return Ok(None);
        }
        let dt = dt_val.downcast_object_ref::<PyDateTime>().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("{method}() expects a datetime object"),
            )
        })?;

        let resolved = match &dt.state {
            DateTimeState::Naive(naive) => self.from_local(naive).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("ambiguous or invalid local time for {method}()"),
                )
            })?,
            DateTimeState::Aware(aware) => aware.with_timezone(&self.tz),
            DateTimeState::FixedOffset(fixed) => fixed.with_timezone(&self.tz),
        };
        Ok(Some(resolved))
    }

    /// `tz.utcoffset(dt)`, `tz.dst(dt)` and `tz.tzname(dt)` from Jinja.
    fn tzinfo_method(&self, method: &str, args: &[Value]) -> Result<Value, Error> {
        let Some(dt) = self.resolve_argument(method, args)? else {
            return Ok(match method {
                "tzname" => Value::from(self.tz.name()),
                _ => Value::from(()),
            });
        };
        Ok(match method {
            "utcoffset" => Value::from_object(PyTimeDelta::new(self.utcoffset(&dt))),
            "dst" => Value::from_object(PyTimeDelta::new(self.dst(&dt))),
            _ => Value::from(self.tzname(&dt)),
        })
    }
}

/// Build the "pytz" module namespace for your Jinja environment.
///   e.g. `pytz.timezone("America/New_York")` => PytzTimezone
pub fn create_pytz_namespace() -> BTreeMap<String, Value> {
    let mut pytz_module = BTreeMap::new();
    // Register `timezone(...)`
    pytz_module.insert("timezone".to_string(), Value::from_function(timezone));
    pytz_module.insert(
        "utc".to_string(),
        Value::from_object(PytzTimezone::new(chrono_tz::UTC)),
    );
    // You could also add "utc", "country", etc.

    pytz_module
}

/// The top-level function for `pytz.timezone("America/New_York")`.
fn timezone(args: &[Value]) -> Result<Value, Error> {
    // Must provide a string zone name.
    let tz_name = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
        Error::new(
            ErrorKind::MissingArgument,
            "timezone() requires a string name argument",
        )
    })?;

    // Chrono Tz names are case-sensitive, but Python is not. This means we can't
    // just call `Tz::from_str`; instead, we need to manually look up the timezone.
    let timezones = get_timezones();
    match timezones.get(&tz_name.to_lowercase()) {
        Some(&tz) => Ok(Value::from_object(PytzTimezone::new(tz))),
        None => Err(Error::new(
            ErrorKind::InvalidArgument,
            format!("Invalid timezone name: {tz_name}"),
        )),
    }
}

/// If you want your `PytzTimezone` to also be a minijinja Object for calls like
///   {{ pytz_obj.method(...) }}
/// implement `Object`.
impl minijinja::value::Object for PytzTimezone {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "localize" => {
                // Get the datetime argument
                let dt_val = args.first().ok_or_else(|| {
                    Error::new(
                        ErrorKind::MissingArgument,
                        "localize() requires a datetime argument",
                    )
                })?;

                // Get the PyDateTime object
                let dt = dt_val.downcast_object_ref::<PyDateTime>().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidArgument,
                        "localize() expects a datetime object",
                    )
                })?;

                // Get the naive datetime
                let naive_dt = match &dt.state {
                    DateTimeState::Naive(ndt) => *ndt,
                    DateTimeState::Aware(_) => {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "localize() requires a naive datetime (tzinfo must be None)",
                        ));
                    }
                    DateTimeState::FixedOffset(_) => {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            "localize() requires a naive datetime (tzinfo must be None)",
                        ));
                    }
                };

                // Create an aware datetime by interpreting the naive datetime in this timezone
                let aware_dt =
                    self.tz
                        .from_local_datetime(&naive_dt)
                        .single()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::InvalidOperation,
                                "ambiguous or invalid local time for localize()",
                            )
                        })?;

                // Return a new PyDateTime object
                let result = PyDateTime {
                    state: DateTimeState::Aware(aware_dt),
                    tzinfo: Some(self.as_ref().clone()),
                };

                Ok(Value::from_object(result))
            }
            "normalize" => {
                // Get the datetime argument
                let dt_val = args.first().ok_or_else(|| {
                    Error::new(
                        ErrorKind::MissingArgument,
                        "normalize() requires a datetime argument",
                    )
                })?;

                // Get the PyDateTime object
                let dt = dt_val.downcast_object_ref::<PyDateTime>().ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidArgument,
                        "normalize() expects a datetime object",
                    )
                })?;

                // pytz.normalize() requires an aware datetime (it corrects the
                // timezone information after arithmetic); a naive datetime has
                // no tzinfo to normalize.
                if let DateTimeState::Naive(_) = &dt.state {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        "normalize() requires an aware datetime (tzinfo must be set)",
                    ));
                }

                // Represent the same instant in this timezone with the correct
                // offset. chrono always tracks offsets correctly, so converting
                // the instant into `self.tz` yields the normalized value.
                let result = dt.astimezone(self.as_ref())?;

                Ok(Value::from_object(result))
            }
            "utcoffset" | "dst" | "tzname" => self.tzinfo_method(method, args),
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("Timezone object has no method named '{method}'"),
            )),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // If you do {{ tz_obj }} in Jinja, it prints out the name
        write!(f, "{}", self.tz)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDateTime, TimeZone, Timelike};
    use minijinja::Environment;

    fn naive(iso: &str) -> NaiveDateTime {
        iso.parse().unwrap()
    }

    /// The localize() logic: interpret a naive datetime as local time in `tz`.
    fn localize(tz: Tz, iso: &str) -> PyDateTime {
        let aware_dt = tz.from_local_datetime(&naive(iso)).single().unwrap();
        PyDateTime {
            state: DateTimeState::Aware(aware_dt),
            tzinfo: Some(PytzTimezone::new(tz)),
        }
    }

    #[test]
    fn test_localize_naive_datetime() {
        // localize doesn't convert: the wall time stays 12:30, now in UTC
        let result = localize(chrono_tz::UTC, "2023-06-15T12:30:00");
        assert_eq!(result.chrono_dt().hour(), 12);
        assert_eq!(result.chrono_dt().minute(), 30);
        assert_eq!(result.tzinfo.as_ref().unwrap().tz, chrono_tz::UTC);
    }

    #[test]
    fn test_localize_different_timezone() {
        // localize doesn't convert: the wall time stays 15:00, now in US/Pacific
        let result = localize(chrono_tz::US::Pacific, "2023-12-01T15:00:00");
        assert_eq!(result.chrono_dt().hour(), 15);
        assert_eq!(result.tzinfo.as_ref().unwrap().tz, chrono_tz::US::Pacific);
    }

    #[test]
    fn test_normalize_converts_instant_to_timezone() {
        // normalize() represents the same instant in this timezone with the
        // correct offset; it delegates to PyDateTime::astimezone(self).
        let utc_dt = chrono_tz::UTC
            .with_ymd_and_hms(2023, 6, 15, 16, 30, 0)
            .single()
            .unwrap();
        let aware = PyDateTime {
            state: DateTimeState::Aware(utc_dt),
            tzinfo: Some(PytzTimezone::new(chrono_tz::UTC)),
        };

        let eastern = PytzTimezone::new(chrono_tz::US::Eastern);
        let result = aware.astimezone(&eastern).unwrap();

        match &result.state {
            DateTimeState::Aware(dt) => {
                // 16:30 UTC -> 12:30 EDT (UTC-4 in June)
                assert_eq!(dt.hour(), 12);
                assert_eq!(dt.minute(), 30);
                // The underlying instant is preserved.
                assert_eq!(dt.with_timezone(&chrono_tz::UTC), utc_dt);
            }
            other => panic!("expected aware datetime, got {other:?}"),
        }
        assert_eq!(result.tzinfo.as_ref().unwrap().tz, chrono_tz::US::Eastern);
    }

    fn naive_value(iso: &str) -> Value {
        Value::from_object(PyDateTime {
            state: DateTimeState::Naive(naive(iso)),
            tzinfo: None,
        })
    }

    fn render_tz_methods(tz: PytzTimezone, dt: Value) -> Result<String, Error> {
        let env = Environment::new();
        let template = env
            .template_from_str("{{ tz.utcoffset(dt) }}|{{ tz.dst(dt) }}|{{ tz.tzname(dt) }}")
            .unwrap();
        template.render(
            minijinja::context!(tz => Value::from_object(tz), dt => dt),
            &[],
        )
    }

    #[test]
    fn test_utcoffset_dst_tzname_in_summer() {
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        assert_eq!(
            render_tz_methods(tz, naive_value("2024-07-04T12:00:00")).unwrap(),
            "-1 day, 20:00:00|1:00:00|EDT"
        );
    }

    #[test]
    fn test_utcoffset_dst_tzname_in_winter() {
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        assert_eq!(
            render_tz_methods(tz, naive_value("2024-01-04T12:00:00")).unwrap(),
            "-1 day, 19:00:00|0:00:00|EST"
        );
    }

    #[test]
    fn test_utcoffset_dst_tzname_utc() {
        let tz = PytzTimezone::new(chrono_tz::UTC);
        assert_eq!(
            render_tz_methods(tz, naive_value("2024-07-04T12:00:00")).unwrap(),
            "0:00:00|0:00:00|UTC"
        );
    }

    #[test]
    fn test_utcoffset_rejects_ambiguous_local_time() {
        // 01:30 on 2024-11-03 happens twice in New York (fall back).
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        let err = render_tz_methods(tz, naive_value("2024-11-03T01:30:00")).unwrap_err();
        assert!(
            err.to_string()
                .contains("ambiguous or invalid local time for utcoffset()"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_aware_datetime_is_converted_to_the_timezone() {
        // 12:00 EDT is 16:00 UTC; pytz.utc answers for the instant, not the wall time.
        let eastern = chrono_tz::America::New_York
            .with_ymd_and_hms(2024, 7, 4, 12, 0, 0)
            .single()
            .unwrap();
        let aware = Value::from_object(PyDateTime {
            state: DateTimeState::Aware(eastern),
            tzinfo: Some(PytzTimezone::new(chrono_tz::America::New_York)),
        });
        assert_eq!(
            render_tz_methods(PytzTimezone::new(chrono_tz::UTC), aware.clone()).unwrap(),
            "0:00:00|0:00:00|UTC"
        );
        assert_eq!(
            render_tz_methods(PytzTimezone::new(chrono_tz::America::New_York), aware).unwrap(),
            "-1 day, 20:00:00|1:00:00|EDT"
        );
    }

    #[test]
    fn test_none_argument_matches_pytz() {
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        assert_eq!(
            render_tz_methods(tz, Value::from(())).unwrap(),
            "None|None|America/New_York"
        );
    }
}
