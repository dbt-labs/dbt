use chrono::{DateTime, LocalResult, NaiveDateTime, Offset, TimeZone};
use chrono_tz::{OffsetComponents, OffsetName, Tz, TZ_VARIANTS};
use minijinja::{arg_utils::ArgsIter, Error, ErrorKind, Value};

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
                let dt = dt_val
                    .downcast_object_ref::<crate::modules::py_datetime::datetime::PyDateTime>()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "localize() expects a datetime object",
                        )
                    })?;

                // Get the naive datetime
                use crate::modules::py_datetime::datetime::DateTimeState;
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
                let result = crate::modules::py_datetime::datetime::PyDateTime {
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
                let dt = dt_val
                    .downcast_object_ref::<crate::modules::py_datetime::datetime::PyDateTime>()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "normalize() expects a datetime object",
                        )
                    })?;

                // pytz.normalize() requires an aware datetime (it corrects the
                // timezone information after arithmetic); a naive datetime has
                // no tzinfo to normalize.
                use crate::modules::py_datetime::datetime::DateTimeState;
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
            "utcoffset" | "dst" | "tzname" => {
                let iter = ArgsIter::new(method, &["dt"], args);
                let dt_val: &Value = iter.next_arg()?;
                iter.finish()?;

                // Like pytz, a None datetime answers None, None and the zone name.
                if dt_val.is_none() {
                    return Ok(match method {
                        "tzname" => Value::from(self.tz.name()),
                        _ => Value::from(()),
                    });
                }

                let dt = dt_val
                    .downcast_object_ref::<crate::modules::py_datetime::datetime::PyDateTime>()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            format!("{method}() expects a datetime object"),
                        )
                    })?;

                // A naive datetime is read as local time, like localize(); an aware
                // one is converted so the answer describes the same instant.
                use crate::modules::py_datetime::datetime::DateTimeState;
                let local_dt = match &dt.state {
                    DateTimeState::Naive(naive) => self.from_local(naive).ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            format!("ambiguous or invalid local time for {method}()"),
                        )
                    })?,
                    DateTimeState::Aware(aware) => aware.with_timezone(&self.tz),
                    DateTimeState::FixedOffset(fixed) => fixed.with_timezone(&self.tz),
                };

                match method {
                    "utcoffset" => Ok(Value::from_object(PyTimeDelta::new(
                        self.utcoffset(&local_dt),
                    ))),
                    "dst" => Ok(Value::from_object(PyTimeDelta::new(self.dst(&local_dt)))),
                    "tzname" => Ok(Value::from(self.tzname(&local_dt))),
                    _ => unreachable!(),
                }
            }
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("Timezone object has no method named '{method}'"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            // pytz exposes the canonical zone name, e.g. `utc.zone` => 'UTC'
            "zone" => Some(Value::from(self.tz.name())),
            _ => None,
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
    use crate::modules::py_datetime::datetime::{DateTimeState, PyDateTime};
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike};
    use minijinja::Environment;

    #[test]
    fn test_localize_naive_datetime() {
        // Create a naive datetime
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 6, 15).unwrap(),
            NaiveTime::from_hms_opt(12, 30, 0).unwrap(),
        );

        // Localize to UTC
        let utc_tz = PytzTimezone::new(chrono_tz::UTC);

        // Manually call localize logic
        let aware_dt_result = utc_tz
            .tz
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "ambiguous"));

        assert!(aware_dt_result.is_ok());
        let aware_dt = aware_dt_result.unwrap();

        // The time should be the same (localize doesn't convert)
        assert_eq!(aware_dt.hour(), 12);
        assert_eq!(aware_dt.minute(), 30);

        // Create the result PyDateTime
        let result = PyDateTime {
            state: DateTimeState::Aware(aware_dt),
            tzinfo: Some(utc_tz),
        };

        // Should be aware with UTC timezone
        assert!(result.tzinfo.is_some());
        assert_eq!(result.tzinfo.as_ref().unwrap().tz, chrono_tz::UTC);
    }

    #[test]
    fn test_localize_different_timezone() {
        // Create a naive datetime
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 12, 1).unwrap(),
            NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
        );

        // Localize to US/Pacific
        let pacific_tz = PytzTimezone::new(chrono_tz::US::Pacific);
        let aware_dt_result = pacific_tz
            .tz
            .from_local_datetime(&naive)
            .single()
            .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "ambiguous"));

        assert!(aware_dt_result.is_ok());
        let aware_dt = aware_dt_result.unwrap();

        // The time should remain 15:00 (localize doesn't convert, just adds timezone)
        assert_eq!(aware_dt.hour(), 15);
        assert_eq!(aware_dt.minute(), 0);

        // Create the result PyDateTime
        let result = PyDateTime {
            state: DateTimeState::Aware(aware_dt),
            tzinfo: Some(pacific_tz),
        };

        // Should be aware with Pacific timezone
        assert!(result.tzinfo.is_some());
        assert_eq!(result.tzinfo.as_ref().unwrap().tz, chrono_tz::US::Pacific);
    }

    #[test]
    fn test_zone_attribute() {
        let utc = Value::from_object(PytzTimezone::new(chrono_tz::UTC));
        assert_eq!(utc.get_attr("zone").unwrap(), Value::from("UTC"));

        let eastern = timezone(&[Value::from("US/Eastern")]).unwrap();
        assert_eq!(eastern.get_attr("zone").unwrap(), Value::from("US/Eastern"));

        // Lookup is case-insensitive, but `zone` is the canonical name
        let new_york = timezone(&[Value::from("america/new_york")]).unwrap();
        assert_eq!(
            new_york.get_attr("zone").unwrap(),
            Value::from("America/New_York")
        );
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

    fn naive(y: i32, m: u32, d: u32, h: u32, min: u32) -> Value {
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(y, m, d).unwrap(),
            NaiveTime::from_hms_opt(h, min, 0).unwrap(),
        );
        Value::from_object(PyDateTime {
            state: DateTimeState::Naive(naive),
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
            render_tz_methods(tz, naive(2024, 7, 4, 12, 0)).unwrap(),
            "-1 day, 20:00:00|1:00:00|EDT"
        );
    }

    #[test]
    fn test_utcoffset_dst_tzname_in_winter() {
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        assert_eq!(
            render_tz_methods(tz, naive(2024, 1, 4, 12, 0)).unwrap(),
            "-1 day, 19:00:00|0:00:00|EST"
        );
    }

    #[test]
    fn test_utcoffset_dst_tzname_utc() {
        let tz = PytzTimezone::new(chrono_tz::UTC);
        assert_eq!(
            render_tz_methods(tz, naive(2024, 7, 4, 12, 0)).unwrap(),
            "0:00:00|0:00:00|UTC"
        );
    }

    #[test]
    fn test_utcoffset_rejects_ambiguous_local_time() {
        // 01:30 on 2024-11-03 happens twice in New York (fall back).
        let tz = PytzTimezone::new(chrono_tz::America::New_York);
        let err = render_tz_methods(tz, naive(2024, 11, 3, 1, 30)).unwrap_err();
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
