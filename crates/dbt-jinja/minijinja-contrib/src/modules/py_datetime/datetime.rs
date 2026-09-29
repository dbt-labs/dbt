use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use chrono::{
    format::{Fixed, Item, StrftimeItems},
    offset::Offset,
    DateTime, Datelike, Duration, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Timelike, Utc,
    Weekday,
};
use chrono_tz::Tz;
use minijinja::arg_utils::ArgsIter;
use minijinja::{arg_utils::ArgParser, value::Object, value::ObjectRepr, Error, ErrorKind, Value};

use crate::modules::py_datetime::bound_method::BoundMethod;
use crate::modules::py_datetime::date::PyDate;
use crate::modules::py_datetime::find_microsecond_directive;
use crate::modules::py_datetime::strptime;
use crate::modules::py_datetime::time::PyTime;
use crate::modules::py_datetime::timedelta::PyTimeDelta;
use crate::modules::py_datetime::tzinfo::PyFixedTimezone;
use crate::modules::pytz::PytzTimezone;

/// An enum storing either a naive datetime or an aware datetime with a known timezone.
#[derive(Clone, Debug)]
pub enum DateTimeState {
    Naive(NaiveDateTime),
    Aware(DateTime<Tz>),
    FixedOffset(DateTime<chrono::FixedOffset>), // Add this variant
}

/// The user-facing "datetime" constructor object (Python's `datetime.datetime`).
#[derive(Clone, Debug)]
pub struct PyDateTime {
    pub state: DateTimeState,
    /// If `Some(...)`, this is an aware datetime with the given tzinfo object (pytz or fixed offset).
    /// If `None`, this is naive.
    pub tzinfo: Option<PytzTimezone>,
}

#[derive(Debug)]
enum DateTimeCmp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

/// The module object that the user calls as `datetime(...)`, or `datetime.now()`, etc.
#[derive(Clone, Debug)]
pub struct PyDateTimeClass;

impl PyDateTimeClass {
    // ------------------------------------------------------------------
    // datetime(...)  =>  naive or aware, depending on kwarg tzinfo
    // ------------------------------------------------------------------
    fn new_datetime(args: &[Value]) -> Result<PyDateTime, Error> {
        // We accept signature like:
        //   datetime(year, month, day, hour=0, minute=0, second=0, microsecond=0, tzinfo=None)
        let mut parser = ArgParser::new(args, None);

        let year: i32 = parser.get::<i32>("year")?;
        let month: u32 = parser.get::<u32>("month")?;
        let day: u32 = parser.get::<u32>("day")?;

        let hour: u32 = parser.get_optional::<u32>("hour").unwrap_or(0);
        let minute: u32 = parser.get_optional::<u32>("minute").unwrap_or(0);
        let second: u32 = parser.get_optional::<u32>("second").unwrap_or(0);
        let microsecond: u32 = parser.get_optional::<u32>("microsecond").unwrap_or(0);

        // Optionally parse a tzinfo from kwargs
        // In real Python, it's a kwarg, so we can do:
        let tz_val = parser.get_optional::<Value>("tzinfo");

        // Build the naive date/time
        let date = NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "Invalid date components"))?;
        let time = NaiveTime::from_hms_micro_opt(hour, minute, second, microsecond)
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "Invalid time components"))?;
        let naive_dt = NaiveDateTime::new(date, time);

        // If tzinfo is provided, we interpret it as an aware datetime
        if let Some(tz_val) = tz_val {
            if tz_val.is_none() {
                // tzinfo=None => naive
                Ok(PyDateTime {
                    state: DateTimeState::Naive(naive_dt),
                    tzinfo: None,
                })
            } else if let Some(tz) = tz_val.downcast_object_ref::<PytzTimezone>() {
                let aware_dt = tz
                    .tz
                    .from_local_datetime(&naive_dt)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                Ok(PyDateTime {
                    state: DateTimeState::Aware(aware_dt),
                    tzinfo: Some(tz.clone()),
                })
            } else if let Some(ftz) = tz_val.downcast_object_ref::<PyFixedTimezone>() {
                let aware_dt = naive_dt
                    .and_local_timezone(ftz.offset)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(aware_dt),
                    tzinfo: None,
                })
            } else {
                Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "tzinfo must be a timezone or None",
                ))
            }
        } else {
            // no tzinfo => naive
            Ok(PyDateTime {
                state: DateTimeState::Naive(naive_dt),
                tzinfo: None,
            })
        }
    }

    // ------------------------------------------------------------------
    // datetime.now(tz=None)
    //   If tz=None => naive local
    //   If tz=some => aware in that tz
    // ------------------------------------------------------------------
    fn now(args: &[Value]) -> Result<PyDateTime, Error> {
        let mut parser = ArgParser::new(args, None);
        let tz_val = parser.get_optional::<Value>("tz");

        let local_now = chrono::Local::now(); // DateTime<Local>

        #[expect(clippy::unnecessary_unwrap)] // TODO fix this
        if tz_val.is_none() || (tz_val.is_some() && tz_val.as_ref().unwrap().is_none()) {
            // Return naive local datetime (matches Python behavior)
            let py_dt = PyDateTime {
                state: DateTimeState::Naive(local_now.naive_local()),
                tzinfo: None,
            };
            Ok(py_dt)
        } else if let Some(tz) = tz_val
            .as_ref()
            .unwrap()
            .downcast_object_ref::<PytzTimezone>()
        {
            let dt_utc = local_now.with_timezone(&chrono::Utc);
            let new_aware = dt_utc.with_timezone(&tz.tz);
            let py_dt = PyDateTime {
                state: DateTimeState::Aware(new_aware),
                tzinfo: Some(tz.clone()),
            };
            Ok(py_dt)
        } else if let Some(ftz) = tz_val
            .as_ref()
            .unwrap()
            .downcast_object_ref::<PyFixedTimezone>()
        {
            let dt_utc = local_now.with_timezone(&chrono::Utc);
            let new_aware = dt_utc.with_timezone(&ftz.offset);
            Ok(PyDateTime {
                state: DateTimeState::FixedOffset(new_aware),
                tzinfo: None,
            })
        } else {
            Err(Error::new(
                ErrorKind::InvalidArgument,
                "tz must be a timezone or None",
            ))
        }
    }

    // ------------------------------------------------------------------
    // datetime.utcnow()
    //   naive UTC
    // ------------------------------------------------------------------
    fn utcnow(_args: &[Value]) -> Result<PyDateTime, Error> {
        let now_utc = Utc::now();
        let py_dt = PyDateTime {
            state: DateTimeState::Naive(now_utc.naive_utc()),
            tzinfo: None,
        };
        Ok(py_dt)
    }

    // ------------------------------------------------------------------
    // datetime.today()
    //   naive local, at midnight
    // ------------------------------------------------------------------
    fn today(_args: &[Value]) -> Result<PyDateTime, Error> {
        let local_now = chrono::Local::now(); // DateTime<Local>
        let naive_local = local_now.naive_local();
        let py_dt = PyDateTime {
            state: DateTimeState::Naive(naive_local.date().and_hms_opt(0, 0, 0).unwrap()),
            tzinfo: None,
        };
        Ok(py_dt)
    }

    // ------------------------------------------------------------------
    // datetime.fromtimestamp(timestamp, tz=None)
    //   If tz=None => naive local
    //   If tz => interpret as aware in that tz
    // ------------------------------------------------------------------
    fn from_timestamp(args: &[Value]) -> Result<PyDateTime, Error> {
        let mut parser = ArgParser::new(args, None);
        let timestamp = parser.next_positional::<f64>()?;
        let tz_val = parser.get_optional::<Value>("tz");

        let secs = timestamp.trunc() as i64;
        let nanos = (timestamp.fract() * 1e9).round() as u32;

        if let Some(tz_val) = tz_val {
            if tz_val.is_none() {
                // interpret as naive local
                let local_dt = chrono::Local
                    .timestamp_opt(secs, nanos)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time for that timestamp",
                        )
                    })?;
                let py_dt = PyDateTime {
                    state: DateTimeState::Naive(local_dt.naive_local()),
                    tzinfo: None,
                };
                Ok(py_dt)
            } else if let Some(tz) = tz_val.downcast_object_ref::<PytzTimezone>() {
                let dt_utc = chrono::Utc
                    .timestamp_opt(secs, nanos)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "invalid or out of range timestamp",
                        )
                    })?;
                let new_aware = dt_utc.with_timezone(&tz.tz);
                let py_dt = PyDateTime {
                    state: DateTimeState::Aware(new_aware),
                    tzinfo: Some(tz.clone()),
                };
                Ok(py_dt)
            } else if let Some(ftz) = tz_val.downcast_object_ref::<PyFixedTimezone>() {
                let dt_utc = chrono::Utc
                    .timestamp_opt(secs, nanos)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "invalid or out of range timestamp",
                        )
                    })?;
                let new_aware = dt_utc.with_timezone(&ftz.offset);
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(new_aware),
                    tzinfo: None,
                })
            } else {
                Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "tz must be a timezone or None",
                ))
            }
        } else {
            // no tz => naive local
            let local_dt = chrono::Local
                .timestamp_opt(secs, nanos)
                .single()
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidArgument,
                        "ambiguous or invalid local time for that timestamp",
                    )
                })?;
            let py_dt = PyDateTime {
                state: DateTimeState::Naive(local_dt.naive_local()),
                tzinfo: None,
            };
            Ok(py_dt)
        }
    }

    // ------------------------------------------------------------------
    // datetime.combine(date, time[, tzinfo])
    //   Returns a new datetime object
    // ------------------------------------------------------------------
    fn combine(args: &[Value]) -> Result<PyDateTime, Error> {
        let mut parser = ArgParser::new(args, None);
        // date param
        let date_val = parser.next_positional::<Value>()?;
        // time param
        let time_val = parser.next_positional::<Value>()?;
        // optional tzinfo
        let tz_val = parser.get_optional::<Value>("tzinfo");

        let py_date = date_val
            .downcast_object_ref::<PyDate>()
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "combine expects a date"))?;
        let py_time = time_val
            .downcast_object_ref::<PyTime>()
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "combine expects a time"))?;

        let naive_dt = NaiveDateTime::new(py_date.date, py_time.time);

        if let Some(tz_val) = tz_val {
            if tz_val.is_none() {
                // naive
                let py_dt = PyDateTime {
                    state: DateTimeState::Naive(naive_dt),
                    tzinfo: None,
                };
                Ok(py_dt)
            } else if let Some(tz) = tz_val.downcast_object_ref::<PytzTimezone>() {
                let aware_dt = tz
                    .tz
                    .from_local_datetime(&naive_dt)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                let py_dt = PyDateTime {
                    state: DateTimeState::Aware(aware_dt),
                    tzinfo: Some(tz.clone()),
                };
                Ok(py_dt)
            } else if let Some(ftz) = tz_val.downcast_object_ref::<PyFixedTimezone>() {
                let aware_dt = naive_dt
                    .and_local_timezone(ftz.offset)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(aware_dt),
                    tzinfo: None,
                })
            } else {
                Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "tzinfo must be a timezone or None",
                ))
            }
        } else {
            // naive
            let py_dt = PyDateTime {
                state: DateTimeState::Naive(naive_dt),
                tzinfo: None,
            };
            Ok(py_dt)
        }
    }

    // ------------------------------------------------------------------
    // datetime.strptime(date_string, format)
    // ------------------------------------------------------------------
    fn strptime(args: &[Value]) -> Result<PyDateTime, Error> {
        let iter = ArgsIter::new("strptime", &["date_string", "format"], args);
        let date_str = iter.next_arg::<&str>()?;
        let fmt_str = iter.next_arg::<&str>()?;
        iter.finish()?;

        if let Ok(naive) = strptime::strptime(date_str, fmt_str) {
            return Ok(PyDateTime {
                state: DateTimeState::Naive(naive),
                tzinfo: None,
            });
        }

        // Fall back on old strptime since new version is not fully implemented yet
        let naive = Self::parse_datetime_with_fallback(date_str, fmt_str).map_err(|e| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("strptime parsing error: {e}"),
            )
        })?;

        // This yields a naive datetime. If you want to let user supply tz=..., parse it.
        Ok(PyDateTime {
            state: DateTimeState::Naive(naive),
            tzinfo: None,
        })
    }

    // datetime.fromisocalendar(year, week, day)
    //   ISO year/week/day -> naive datetime at midnight (00:00:00).
    //   Mirrors CPython's exact error wording.
    fn fromisocalendar(args: &[Value]) -> Result<PyDateTime, Error> {
        let iter = ArgsIter::new("fromisocalendar", &["year", "week", "day"], args);
        let year: i64 = iter.next_arg()?;
        let week: i64 = iter.next_arg()?;
        let day: i64 = iter.next_arg()?;
        iter.finish()?;

        if !(1..=9999).contains(&year) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("Year is out of range: {year}"),
            ));
        }
        if !(1..=53).contains(&week) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("Invalid week: {week}"),
            ));
        }
        if !(1..=7).contains(&day) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("Invalid day: {day} (range is [1, 7])"),
            ));
        }

        let weekday = match day {
            1 => Weekday::Mon,
            2 => Weekday::Tue,
            3 => Weekday::Wed,
            4 => Weekday::Thu,
            5 => Weekday::Fri,
            6 => Weekday::Sat,
            7 => Weekday::Sun,
            _ => unreachable!(),
        };

        let date =
            NaiveDate::from_isoywd_opt(year as i32, week as u32, weekday).ok_or_else(|| {
                Error::new(ErrorKind::InvalidArgument, format!("Invalid week: {week}"))
            })?;

        Ok(PyDateTime {
            state: DateTimeState::Naive(date.and_hms_opt(0, 0, 0).unwrap()),
            tzinfo: None,
        })
    }

    fn fromisoformat(args: &[Value]) -> Result<PyDateTime, Error> {
        let mut parser = ArgParser::new(args, None);
        let date_str: String = parser.next_positional()?;

        // First try parsing with timezone offset
        let error = match DateTime::parse_from_str(&date_str, "%Y-%m-%dT%H:%M:%S%.f%:z")
            .or_else(|_| DateTime::parse_from_str(&date_str, "%Y-%m-%d %H:%M:%S%.f%:z"))
        {
            Ok(dt) => {
                return Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(dt), // Keep as DateTime<FixedOffset>
                    tzinfo: Some(PytzTimezone { tz: Tz::UTC }), // Use UTC for tzinfo
                });
            }
            Err(e) => e,
        };

        // If no timezone, try parsing as naive
        if let Ok(naive) = NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%dT%H:%M:%S%.f")
            .or_else(|_| NaiveDateTime::parse_from_str(&date_str, "%Y-%m-%d %H:%M:%S%.f"))
        {
            return Ok(PyDateTime {
                state: DateTimeState::Naive(naive),
                tzinfo: None,
            });
        }

        // If none of the above worked, try parsing as date
        if let Ok(date) = NaiveDate::parse_from_str(&date_str, "%Y-%m-%d") {
            return Ok(PyDateTime {
                state: DateTimeState::Naive(date.and_hms_opt(0, 0, 0).unwrap()),
                tzinfo: None,
            });
        }

        Err(Error::new(
            ErrorKind::InvalidArgument,
            format!("fromisoformat parsing error: {date_str}: {error}"),
        ))
    }

    /// Attempts to parse a datetime string, filling in missing date or time parts.
    fn parse_datetime_with_fallback(input: &str, fmt: &str) -> Result<NaiveDateTime, String> {
        // First, try parsing the full datetime directly
        if let Ok(dt) = NaiveDateTime::parse_from_str(input, fmt) {
            return Ok(dt);
        }

        // Try parsing just the date
        if let Ok(date) = NaiveDate::parse_from_str(input, fmt) {
            return Ok(date.and_hms_opt(0, 0, 0).unwrap());
        }

        // Try parsing just the time
        if let Ok(time) = NaiveTime::parse_from_str(input, fmt) {
            let default_date = NaiveDate::from_ymd_opt(1900, 1, 1).unwrap();
            return Ok(default_date.and_time(time));
        }

        // Otherwise, return the error
        Err("Could not parse input as datetime, date, or time".to_string())
    }
}

/// The actual module object, so user can do:
///   {{ datetime(...) }}, {{ datetime.now() }}, {{ datetime.fromtimestamp(...) }}, etc.
impl Object for PyDateTimeClass {
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        Ok(Value::from_object(Self::new_datetime(args)?))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "now" => Ok(Value::from_object(Self::now(args)?)),
            "utcnow" => Ok(Value::from_object(Self::utcnow(args)?)),
            "today" => Ok(Value::from_object(Self::today(args)?)),
            "fromtimestamp" => Ok(Value::from_object(Self::from_timestamp(args)?)),
            "combine" => Ok(Value::from_object(Self::combine(args)?)),
            "strptime" => Ok(Value::from_object(Self::strptime(args)?)),
            "fromisoformat" => Ok(Value::from_object(Self::fromisoformat(args)?)),
            "fromisocalendar" => Ok(Value::from_object(Self::fromisocalendar(args)?)),
            "strftime" => {
                // Handle strftime(datetime, format) case
                let mut parser = ArgParser::new(args, None);
                let datetime_val = parser.next_positional::<Value>()?;
                let format_val = parser.next_positional::<Value>()?;

                // Check if the first argument is a PyDateTime
                if let Some(datetime) = datetime_val.downcast_object_ref::<PyDateTime>() {
                    return datetime.strftime(&[format_val]);
                }

                // Check if it's a PyDate
                if let Some(date) = datetime_val.downcast_object_ref::<super::date::PyDate>() {
                    return date.strftime(&[format_val]);
                }

                // Check if it's a PyTime
                if let Some(time) = datetime_val.downcast_object_ref::<super::time::PyTime>() {
                    return time.strftime(&[format_val]);
                }

                // If we get here, the argument is not a valid datetime-like object
                Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "strftime expects a datetime, date, or time object as first argument",
                ))
            }
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("datetime has no method named '{method}'"),
            )),
        }
    }

    // Expose class methods as attributes so they can be referenced as
    // callables (e.g. `modules.datetime.datetime.strptime`).
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        let name = key.as_str()?;
        match name {
            "now" | "utcnow" | "today" | "fromtimestamp" | "combine" | "strptime"
            | "fromisoformat" | "fromisocalendar" | "strftime" => Some(Value::from_object(
                BoundMethod::new(Value::from_object(PyDateTimeClass), name),
            )),
            _ => None,
        }
    }
}

//
// Implementation of PyDateTime object
//
fn is_timezone_item(item: &Item<'_>) -> bool {
    matches!(
        item,
        Item::Fixed(
            Fixed::TimezoneName
                | Fixed::TimezoneOffsetColon
                | Fixed::TimezoneOffsetDoubleColon
                | Fixed::TimezoneOffsetTripleColon
                | Fixed::TimezoneOffsetColonZ
                | Fixed::TimezoneOffset
                | Fixed::TimezoneOffsetZ
        )
    )
}

impl PyDateTime {
    // convenience "naive" constructor
    pub fn new_naive(dt: NaiveDateTime) -> Self {
        PyDateTime {
            state: DateTimeState::Naive(dt),
            tzinfo: None,
        }
    }

    // convenience "aware" constructor
    pub fn new_aware(dt: DateTime<Tz>, tzinfo: Option<PytzTimezone>) -> Self {
        PyDateTime {
            state: DateTimeState::Aware(dt),
            tzinfo: Some(tzinfo.unwrap_or(PytzTimezone { tz: Tz::UTC })),
        }
    }

    // convenience "fixed offset" constructor
    pub fn new_fixed_offset(dt: DateTime<chrono::FixedOffset>) -> Self {
        PyDateTime {
            state: DateTimeState::FixedOffset(dt),
            tzinfo: None,
        }
    }

    /// Return naive or aware's .year
    pub fn year(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().year()))
    }

    pub fn month(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().month()))
    }

    pub fn day(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().day()))
    }

    pub fn hour(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().hour()))
    }

    pub fn minute(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().minute()))
    }

    pub fn second(&self) -> Option<Value> {
        Some(Value::from(self.chrono_dt().second()))
    }

    /// Return .tzinfo. If naive => None
    pub fn tzinfo(&self) -> Option<Value> {
        if let Some(tz) = &self.tzinfo {
            return Some(Value::from_object(tz.clone()));
        }
        if let DateTimeState::FixedOffset(fdt) = &self.state {
            return Some(Value::from_object(PyFixedTimezone {
                offset: *fdt.offset(),
                name: None,
            }));
        }
        Some(Value::from(()))
    }

    /// "chrono_dt" is a helper method that returns a naive DateTime if we're naive,
    /// or the local datetime if we're aware. This is mostly for read-only field access.
    pub fn chrono_dt(&self) -> chrono::NaiveDateTime {
        match &self.state {
            DateTimeState::Naive(ndt) => *ndt,
            DateTimeState::Aware(adt) => adt.naive_local(),
            DateTimeState::FixedOffset(fdt) => fdt.naive_local(),
        }
    }

    /// strftime(format)
    pub fn strftime(&self, args: &[Value]) -> Result<Value, Error> {
        let fmt = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                "strftime requires one string argument",
            )
        })?;

        // Python's `%f` is six zero-padded microsecond digits; chrono's `%f` is
        // nine nanosecond digits. Format the surrounding pieces with chrono and
        // write the microseconds ourselves.
        let mut formatted = String::new();
        let mut remaining = fmt;
        while let Some(pos) = find_microsecond_directive(remaining) {
            self.format_into(&remaining[..pos], &mut formatted)?;
            let microseconds = self.chrono_dt().nanosecond() / 1_000;
            formatted.push_str(&format!("{microseconds:06}"));
            remaining = &remaining[pos + "%f".len()..];
        }
        self.format_into(remaining, &mut formatted)?;

        Ok(Value::from(formatted))
    }

    /// Append `fmt` formatted against this datetime to `out`. `fmt` is a slice of
    /// a `strftime` format string with no `%f` conversion left in it.
    fn format_into(&self, fmt: &str, out: &mut String) -> Result<(), Error> {
        if fmt.is_empty() {
            return Ok(());
        }
        let result = match &self.state {
            DateTimeState::Naive(ndt) => {
                // Python renders timezone directives as empty strings for naive datetimes.
                let items = StrftimeItems::new(fmt)
                    .filter(|item| !is_timezone_item(item))
                    .collect::<Vec<_>>();
                ndt.format_with_items(items.iter()).write_to(out)
            }
            DateTimeState::Aware(adt) => adt.format(fmt).write_to(out),
            DateTimeState::FixedOffset(fdt) => fdt.format(fmt).write_to(out),
        };
        result.map_err(|_| Error::new(ErrorKind::InvalidArgument, "invalid strftime format"))
    }

    /// Format with a custom date/time separator. `sep == 'T'` produces the
    /// ISO 8601 form returned by Python's `dt.isoformat()`; `sep == ' '`
    /// matches Python's `str(dt)` form.
    fn isoformat_with_sep(&self, sep: char) -> String {
        match &self.state {
            DateTimeState::Naive(ndt) => {
                // naive => omit offset
                let fmt = format!("%Y-%m-%d{sep}%H:%M:%S.%6f");
                let formatted = ndt.format(&fmt).to_string();
                // Only include decimal point and microseconds if they are non-zero
                if formatted.ends_with(".000000") {
                    formatted[..formatted.len() - 7].to_string()
                } else {
                    formatted // Keep all microsecond digits
                }
            }
            DateTimeState::Aware(adt) => {
                // aware => include offset
                let fmt = format!("%Y-%m-%d{sep}%H:%M:%S.%6f%:z");
                let formatted = adt.format(&fmt).to_string();
                if formatted.contains(".000000") {
                    formatted.replace(".000000", "")
                } else {
                    formatted // Keep all microsecond digits
                }
            }
            DateTimeState::FixedOffset(dt) => {
                let fmt = format!("%Y-%m-%d{sep}%H:%M:%S.%6f%:z");
                let formatted = dt.format(&fmt).to_string();
                if formatted.contains(".000000") {
                    formatted.replace(".000000", "")
                } else {
                    formatted // Keep all microsecond digits
                }
            }
        }
    }

    /// isoformat() -> "YYYY-MM-DDTHH:MM:SS[.ffffff][+HH:MM]"
    pub fn isoformat(&self) -> String {
        self.isoformat_with_sep('T')
    }

    /// .timestamp() -> float
    /// If naive, interpret as local in Python, or raise error. We'll do local for demo:
    pub fn timestamp(&self) -> f64 {
        match &self.state {
            DateTimeState::Naive(ndt) => {
                // interpret as local
                let local_dt = chrono::Local
                    .from_local_datetime(ndt)
                    .single()
                    .expect("ambiguous local time");
                local_dt.timestamp() as f64 + (local_dt.timestamp_subsec_nanos() as f64 * 1e-9)
            }
            DateTimeState::Aware(adt) => {
                // convert to UTC, then get timestamp
                let utc_dt = adt.with_timezone(&chrono::Utc);
                utc_dt.timestamp() as f64 + (utc_dt.timestamp_subsec_nanos() as f64 * 1e-9)
            }
            DateTimeState::FixedOffset(fdt) => {
                // fixed offset => interpret as local
                let local_dt = fdt.with_timezone(&chrono::Local);
                local_dt.timestamp() as f64 + (local_dt.timestamp_subsec_nanos() as f64 * 1e-9)
            }
        }
    }

    /// __add__(timedelta) or __sub__(timedelta or datetime)
    fn add_op(&self, args: &[Value], is_add: bool) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let rhs: Value = parser.next_positional()?;

        // If it's a PyTimeDelta
        if let Some(delta) = rhs.downcast_object_ref::<PyTimeDelta>() {
            // datetime + timedelta => datetime
            let dur = if is_add {
                delta.duration
            } else {
                -delta.duration
            };

            match &self.state {
                DateTimeState::Naive(ndt) => {
                    let new_naive = *ndt + dur;
                    Ok(Value::from_object(PyDateTime {
                        state: DateTimeState::Naive(new_naive),
                        tzinfo: self.tzinfo.clone(),
                    }))
                }
                DateTimeState::Aware(adt) => {
                    let new_aware = *adt + dur;
                    // same tzinfo
                    Ok(Value::from_object(PyDateTime {
                        state: DateTimeState::Aware(new_aware),
                        tzinfo: self.tzinfo.clone(),
                    }))
                }
                DateTimeState::FixedOffset(fdt) => {
                    let new_fdt = *fdt + dur;
                    Ok(Value::from_object(PyDateTime {
                        state: DateTimeState::FixedOffset(new_fdt),
                        tzinfo: self.tzinfo.clone(),
                    }))
                }
            }
        }
        // If it's another PyDateTime => return a timedelta
        else if let Some(other_dt) = rhs.downcast_object_ref::<PyDateTime>() {
            // datetime - datetime => timedelta
            if !is_add {
                // we do self - other
                let self_chrono = match &self.state {
                    DateTimeState::Naive(ndt) => chrono::Local
                        .from_local_datetime(ndt)
                        .single()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::InvalidArgument,
                                "ambiguous local time for naive datetime",
                            )
                        })?
                        .with_timezone(&chrono::Utc), // interpret naive as local -> utc
                    DateTimeState::Aware(adt) => adt.with_timezone(&chrono::Utc),
                    DateTimeState::FixedOffset(fdt) => fdt.with_timezone(&chrono::Utc),
                };

                let other_chrono = match &other_dt.state {
                    DateTimeState::Naive(ndt) => chrono::Local
                        .from_local_datetime(ndt)
                        .single()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::InvalidArgument,
                                "ambiguous local time for naive datetime",
                            )
                        })?
                        .with_timezone(&chrono::Utc),
                    DateTimeState::Aware(adt) => adt.with_timezone(&chrono::Utc),
                    DateTimeState::FixedOffset(fdt) => fdt.with_timezone(&chrono::Utc),
                };

                let diff = self_chrono.signed_duration_since(other_chrono);
                let td = PyTimeDelta::new(diff);
                Ok(Value::from_object(td))
            } else {
                // datetime + datetime not allowed in Python
                Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "Cannot add two datetime objects",
                ))
            }
        } else {
            Err(Error::new(
                ErrorKind::InvalidArgument,
                "Expected a timedelta or datetime on the right-hand side",
            ))
        }
    }

    fn cmp_op(&self, args: &[Value], cmp: DateTimeCmp) -> Result<Value, Error> {
        let iter = ArgsIter::new(
            match cmp {
                DateTimeCmp::Eq => "__eq__",
                DateTimeCmp::Neq => "__ne__",
                DateTimeCmp::Lt => "__lt__",
                DateTimeCmp::Le => "__le__",
                DateTimeCmp::Gt => "__gt__",
                DateTimeCmp::Ge => "__ge__",
            },
            &["other"],
            args,
        );
        let rhs = iter.next_arg::<Value>()?;
        iter.finish()?;

        let other_dt = rhs.downcast_object_ref::<PyDateTime>();
        let result = other_dt.and_then(|other| match (&self.state, &other.state) {
            (DateTimeState::Naive(l), DateTimeState::Naive(r)) => Some(l.cmp(r)),
            (DateTimeState::Aware(l), DateTimeState::Aware(r)) => Some(l.cmp(r)),
            (DateTimeState::FixedOffset(l), DateTimeState::FixedOffset(r)) => Some(l.cmp(r)),
            _ => None,
        });

        if let Some(o) = result {
            match cmp {
                DateTimeCmp::Eq => Ok(Value::from(o == Ordering::Equal)),
                DateTimeCmp::Neq => Ok(Value::from(o != Ordering::Equal)),
                DateTimeCmp::Lt => Ok(Value::from(matches!(o, Ordering::Less))),
                DateTimeCmp::Le => Ok(Value::from(matches!(o, Ordering::Less | Ordering::Equal))),
                DateTimeCmp::Gt => Ok(Value::from(matches!(o, Ordering::Greater))),
                DateTimeCmp::Ge => Ok(Value::from(matches!(
                    o,
                    Ordering::Greater | Ordering::Equal
                ))),
            }
        } else {
            // If not an eq/neq comparison, they must be the same type and state
            match cmp {
                DateTimeCmp::Eq => Ok(Value::from(false)),
                DateTimeCmp::Neq => Ok(Value::from(true)),
                _ => Err(Error::new(
                    ErrorKind::InvalidArgument,
                    if other_dt.is_some() {
                        "Can't compare offset-naive and offset-aware datetimes"
                    } else {
                        "Can only compare datetime objects"
                    },
                )),
            }
        }
    }

    pub fn weekday(&self) -> u32 {
        // Python's weekday() returns 0 for Monday, ... 6 for Sunday
        self.chrono_dt().weekday().num_days_from_monday()
    }

    pub fn isoweekday(&self) -> u32 {
        // Python's isoweekday() returns 1 for Monday, ... 7 for Sunday
        self.chrono_dt().weekday().num_days_from_monday() + 1
    }

    /// dt.date() => returns a PyDate
    pub fn date(&self) -> PyDate {
        let d = self.chrono_dt().date();
        PyDate::new(d)
    }

    /// dt.time() => returns a PyTime (naive time)
    pub fn time(&self) -> PyTime {
        let t = self.chrono_dt().time();
        PyTime::new(t, self.tzinfo.clone())
    }

    /// dt.replace(year=?, month=?, day=?, hour=?, minute=?, second=?, microsecond=?, tzinfo=?)
    pub fn replace(&self, args: &[Value]) -> Result<PyDateTime, Error> {
        let mut parser = ArgParser::new(args, None);

        let mut year = self.chrono_dt().year();
        let mut month = self.chrono_dt().month();
        let mut day = self.chrono_dt().day();
        let mut hour = self.chrono_dt().hour();
        let mut minute = self.chrono_dt().minute();
        let mut second = self.chrono_dt().second();
        let mut microsecond = self.chrono_dt().nanosecond() / 1000;

        // in Python, tzinfo can also be replaced
        let new_tzinfo_val = parser.consume_optional_only_from_kwargs::<Value>("tzinfo");

        if let Some(y) = parser.consume_optional_only_from_kwargs::<i32>("year") {
            year = y;
        }
        if let Some(m) = parser.consume_optional_only_from_kwargs::<u32>("month") {
            month = m;
        }
        if let Some(d) = parser.consume_optional_only_from_kwargs::<u32>("day") {
            day = d;
        }
        if let Some(h) = parser.consume_optional_only_from_kwargs::<u32>("hour") {
            hour = h;
        }
        if let Some(mi) = parser.consume_optional_only_from_kwargs::<u32>("minute") {
            minute = mi;
        }
        if let Some(s) = parser.consume_optional_only_from_kwargs::<u32>("second") {
            second = s;
        }
        if let Some(us) = parser.consume_optional_only_from_kwargs::<u32>("microsecond") {
            microsecond = us;
        }

        let new_date = NaiveDate::from_ymd_opt(year, month, day)
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "Invalid date components"))?;
        let new_time = NaiveTime::from_hms_micro_opt(hour, minute, second, microsecond)
            .ok_or_else(|| Error::new(ErrorKind::InvalidArgument, "Invalid time components"))?;
        let new_naive = NaiveDateTime::new(new_date, new_time);

        // parse tzinfo
        if let Some(tz_val) = new_tzinfo_val {
            if tz_val.is_none() {
                return Ok(PyDateTime {
                    state: DateTimeState::Naive(new_naive),
                    tzinfo: None,
                });
            } else if let Some(tz) = tz_val.downcast_object_ref::<PytzTimezone>() {
                let aware = tz
                    .tz
                    .from_local_datetime(&new_naive)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                return Ok(PyDateTime {
                    state: DateTimeState::Aware(aware),
                    tzinfo: Some(tz.clone()),
                });
            } else if let Some(ftz) = tz_val.downcast_object_ref::<PyFixedTimezone>() {
                let aware = new_naive
                    .and_local_timezone(ftz.offset)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidArgument,
                            "ambiguous or invalid local time in that timezone",
                        )
                    })?;
                return Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(aware),
                    tzinfo: None,
                });
            }
        }

        // tzinfo kwarg not provided or unrecognized — keep the same state type
        if let Some(ref tz) = self.tzinfo {
            let aware = tz
                .tz
                .from_local_datetime(&new_naive)
                .single()
                .ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidArgument,
                        "ambiguous or invalid local time in that timezone",
                    )
                })?;
            Ok(PyDateTime {
                state: DateTimeState::Aware(aware),
                tzinfo: self.tzinfo.clone(),
            })
        } else {
            match &self.state {
                DateTimeState::FixedOffset(fdt) => {
                    let aware = new_naive
                        .and_local_timezone(*fdt.offset())
                        .single()
                        .ok_or_else(|| {
                            Error::new(
                                ErrorKind::InvalidArgument,
                                "ambiguous or invalid local time in that timezone",
                            )
                        })?;
                    Ok(PyDateTime {
                        state: DateTimeState::FixedOffset(aware),
                        tzinfo: None,
                    })
                }
                _ => Ok(PyDateTime {
                    state: DateTimeState::Naive(new_naive),
                    tzinfo: None,
                }),
            }
        }
    }

    /// dt.astimezone(tz)
    /// If naive => interpret as local time, then convert to tz (matches CPython behavior)
    /// If aware => do a real offset conversion from old tz to new tz
    pub fn astimezone(&self, tz: &PytzTimezone) -> Result<PyDateTime, Error> {
        match &self.state {
            DateTimeState::Naive(naive_dt) => {
                // Interpret naive datetime as local time (matches CPython behavior)
                // This aligns with dbt Core's behavior for modules.datetime.datetime.utcnow().astimezone(modules.pytz.utc)
                let local_dt = chrono::Local
                    .from_local_datetime(naive_dt)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "ambiguous or invalid local time for naive datetime",
                        )
                    })?;

                // Convert to UTC, then to requested timezone
                let dt_utc = local_dt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&tz.tz);

                let py_dt = PyDateTime {
                    state: DateTimeState::Aware(new_aware),
                    tzinfo: Some(tz.clone()),
                };
                Ok(py_dt)
            }
            DateTimeState::Aware(old_dt) => {
                // convert from old tz to new tz
                let dt_utc = old_dt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&tz.tz);

                let py_dt = PyDateTime {
                    state: DateTimeState::Aware(new_aware),
                    tzinfo: Some(tz.clone()),
                };
                Ok(py_dt)
            }
            DateTimeState::FixedOffset(fdt) => {
                let dt_utc = fdt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&tz.tz);
                Ok(PyDateTime {
                    state: DateTimeState::Aware(new_aware),
                    tzinfo: Some(tz.clone()),
                })
            }
        }
    }

    /// dt.astimezone(fixed_tz) — convert to a fixed-offset timezone
    pub fn astimezone_fixed(&self, ftz: &PyFixedTimezone) -> Result<PyDateTime, Error> {
        match &self.state {
            DateTimeState::Naive(naive_dt) => {
                let local_dt = chrono::Local
                    .from_local_datetime(naive_dt)
                    .single()
                    .ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            "ambiguous or invalid local time for naive datetime",
                        )
                    })?;
                let dt_utc = local_dt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&ftz.offset);
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(new_aware),
                    tzinfo: None,
                })
            }
            DateTimeState::Aware(old_dt) => {
                let dt_utc = old_dt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&ftz.offset);
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(new_aware),
                    tzinfo: None,
                })
            }
            DateTimeState::FixedOffset(fdt) => {
                let dt_utc = fdt.with_timezone(&chrono::Utc);
                let new_aware = dt_utc.with_timezone(&ftz.offset);
                Ok(PyDateTime {
                    state: DateTimeState::FixedOffset(new_aware),
                    tzinfo: None,
                })
            }
        }
    }
}

//
// Implement the `Object` trait for PyDateTime so Jinja can call methods
//
impl Object for PyDateTime {
    // Without this, the default `ObjectRepr::Map` (with no enumerable fields,
    // since `enumerate` is not overridden either) makes `tojson`/`|string`
    // render this as the literal "{}" and `is mapping` report true. `Plain`
    // makes stringification/serialization go through `render()` below, and
    // matches the sibling `PyDate` object.
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn is_true(self: &Arc<Self>) -> bool {
        true
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            // "strftime(format)"
            "strftime" => self.strftime(args),

            "astimezone" => {
                let tz_val = args.first().ok_or_else(|| {
                    Error::new(
                        ErrorKind::MissingArgument,
                        "astimezone() requires an argument",
                    )
                })?;
                if let Some(tz) = tz_val.downcast_object_ref::<PytzTimezone>() {
                    let new_dt = self.astimezone(tz)?;
                    Ok(Value::from_object(new_dt))
                } else if let Some(ftz) = tz_val.downcast_object_ref::<PyFixedTimezone>() {
                    let new_dt = self.astimezone_fixed(ftz)?;
                    Ok(Value::from_object(new_dt))
                } else {
                    Err(Error::new(
                        ErrorKind::InvalidArgument,
                        "astimezone() expects a timezone object",
                    ))
                }
            }

            // "replace(...)"
            "replace" => {
                let replaced = self.replace(args)?;
                Ok(Value::from_object(replaced))
            }

            // "date()"
            "date" => Ok(Value::from_object(self.date())),

            // "time()"
            "time" => Ok(Value::from_object(self.time())),

            // "weekday()"
            "weekday" => Ok(Value::from(self.weekday())),
            "isoweekday" => Ok(Value::from(self.isoweekday())),

            // "isoformat()"
            "isoformat" => Ok(Value::from(self.isoformat())),

            // "utcoffset()"
            // Python semantics:
            // - naive datetime => None
            // - aware datetime => timedelta
            "utcoffset" => {
                if !args.is_empty() {
                    return Err(Error::new(
                        ErrorKind::InvalidArgument,
                        "utcoffset() takes no arguments",
                    ));
                }
                match &self.state {
                    DateTimeState::Naive(_) => Ok(Value::from(())),
                    DateTimeState::Aware(adt) => {
                        let secs = adt.offset().fix().local_minus_utc();
                        Ok(Value::from_object(PyTimeDelta::new(Duration::seconds(
                            secs as i64,
                        ))))
                    }
                    DateTimeState::FixedOffset(fdt) => {
                        let secs = fdt.offset().local_minus_utc();
                        Ok(Value::from_object(PyTimeDelta::new(Duration::seconds(
                            secs as i64,
                        ))))
                    }
                }
            }

            // "timestamp()"
            "timestamp" => Ok(Value::from(self.timestamp())),

            // Arithmetic
            "__add__" => self.add_op(args, true),
            "__sub__" => self.add_op(args, false),

            // Comparison
            "__eq__" => self.cmp_op(args, DateTimeCmp::Eq),
            "__ne__" => self.cmp_op(args, DateTimeCmp::Neq),
            "__gt__" => self.cmp_op(args, DateTimeCmp::Gt),
            "__ge__" => self.cmp_op(args, DateTimeCmp::Ge),
            "__lt__" => self.cmp_op(args, DateTimeCmp::Lt),
            "__le__" => self.cmp_op(args, DateTimeCmp::Le),

            "fromtimestamp" => Ok(Value::from_object(PyDateTimeClass::from_timestamp(args)?)),
            "now" => Ok(Value::from_object(PyDateTimeClass::now(args)?)),
            "utcnow" => Ok(Value::from_object(PyDateTimeClass::utcnow(args)?)),
            "today" => Ok(Value::from_object(PyDateTimeClass::today(args)?)),
            "strptime" => Ok(Value::from_object(PyDateTimeClass::strptime(args)?)),
            "combine" => Ok(Value::from_object(PyDateTimeClass::combine(args)?)),
            "fromisoformat" => Ok(Value::from_object(PyDateTimeClass::fromisoformat(args)?)),
            "fromisocalendar" => Ok(Value::from_object(PyDateTimeClass::fromisocalendar(args)?)),

            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("datetime has no method named '{method}'"),
            )),
        }
    }

    // Provide direct attribute access
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "year" => self.year(),
            "month" => self.month(),
            "day" => self.day(),
            "hour" => self.hour(),
            "minute" => self.minute(),
            "second" => self.second(),
            "tzinfo" => self.tzinfo(),
            _ => None,
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Match Python's `str(datetime)`: space between date and time
        // (note: `dt.isoformat()` keeps the 'T' separator).
        write!(f, "{}", self.isoformat_with_sep(' '))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
    use minijinja::args;
    use minijinja::Environment;
    use minijinja::Value;

    #[test]
    fn test_strptime_with_fallback() {
        let result = PyDateTimeClass::strptime(args!("2023-01-02 15:30:45", "%Y-%m-%d %H:%M:%S"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.chrono_dt(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2023, 1, 2).unwrap(),
                NaiveTime::from_hms_opt(15, 30, 45).unwrap()
            )
        );

        let result = PyDateTimeClass::strptime(args!("15:30:45", "%H:%M:%S"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.chrono_dt(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(1900, 1, 1).unwrap(),
                NaiveTime::from_hms_opt(15, 30, 45).unwrap()
            )
        );

        let result = PyDateTimeClass::strptime(args!("invalid", "%Y-%m-%d"));
        assert!(result.is_err());

        let result = PyDateTimeClass::strptime(args!("2023-01-02", "%Y-%m-%d"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(
            dt.chrono_dt(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2023, 1, 2).unwrap(),
                NaiveTime::from_hms_opt(0, 0, 0).unwrap()
            )
        );
    }

    #[test]
    fn test_parse_datetime_with_fallback() {
        // Test full datetime parsing
        let result = PyDateTimeClass::parse_datetime_with_fallback(
            "2023-01-02 15:30:45",
            "%Y-%m-%d %H:%M:%S",
        );
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2023, 1, 2).unwrap(),
                NaiveTime::from_hms_opt(15, 30, 45).unwrap()
            )
        );

        // Test date-only parsing
        let result = PyDateTimeClass::parse_datetime_with_fallback("2023-01-02", "%Y-%m-%d");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(2023, 1, 2).unwrap(),
                NaiveTime::from_hms_opt(0, 0, 0).unwrap()
            )
        );

        // Test time-only parsing
        let result = PyDateTimeClass::parse_datetime_with_fallback("15:30:45", "%H:%M:%S");
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            NaiveDateTime::new(
                NaiveDate::from_ymd_opt(1900, 1, 1).unwrap(),
                NaiveTime::from_hms_opt(15, 30, 45).unwrap()
            )
        );

        // Test invalid format
        let result = PyDateTimeClass::parse_datetime_with_fallback("invalid", "%Y-%m-%d");
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Could not parse input as datetime, date, or time".to_string()
        );
    }

    #[test]
    fn test_fromisoformat() {
        let result = PyDateTimeClass::fromisoformat(args!("2023-01-02T15:30:45"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.isoformat(), "2023-01-02T15:30:45");

        let result = PyDateTimeClass::fromisoformat(args!("2023-01-02T15:30:45.000001"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.isoformat(), "2023-01-02T15:30:45.000001");

        // Test with trailing zeros in microseconds
        let result = PyDateTimeClass::fromisoformat(args!("2023-01-02T15:30:45.100000"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.isoformat(), "2023-01-02T15:30:45.100000");

        // Test with space instead of T
        let result = PyDateTimeClass::fromisoformat(args!("2023-01-02 15:30:45.100000"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.isoformat(), "2023-01-02T15:30:45.100000");

        // Test with microseconds and timezone
        let result = PyDateTimeClass::fromisoformat(args!("2023-01-02 15:30:45.100000+01:00"));
        assert!(result.is_ok());
        let dt = result.unwrap();
        assert_eq!(dt.isoformat(), "2023-01-02T15:30:45.100000+01:00");
    }

    #[test]
    fn test_tzinfo_property() {
        // Test naive datetime - tzinfo should be None (not undefined)
        let naive_dt = PyDateTime::new_naive(NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
            NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
        ));
        let tzinfo_value = naive_dt.tzinfo();
        assert!(tzinfo_value.is_some()); // Should return Some(Value) not None
        assert!(tzinfo_value.unwrap().is_none()); // The Value should be Python's None

        // Test aware datetime - tzinfo should be a PytzTimezone object
        let tz = crate::modules::pytz::PytzTimezone::new(chrono_tz::UTC);
        let aware_dt = PyDateTime::new_aware(
            chrono_tz::UTC
                .from_local_datetime(&NaiveDateTime::new(
                    NaiveDate::from_ymd_opt(2023, 1, 1).unwrap(),
                    NaiveTime::from_hms_opt(12, 0, 0).unwrap(),
                ))
                .unwrap(),
            Some(tz),
        );
        let tzinfo_value = aware_dt.tzinfo();
        assert!(tzinfo_value.is_some());
        assert!(!tzinfo_value.unwrap().is_none()); // Should not be Python's None
    }

    #[test]
    fn test_astimezone_naive_datetime() {
        use crate::modules::pytz::PytzTimezone;

        // Create a naive UTC datetime
        let naive_dt = PyDateTime::new_naive(NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 6, 15).unwrap(),
            NaiveTime::from_hms_opt(12, 30, 0).unwrap(),
        ));

        // Convert to UTC - should interpret as local time then convert to UTC
        let utc_tz = PytzTimezone::new(chrono_tz::UTC);
        let result = naive_dt.astimezone(&utc_tz);
        assert!(result.is_ok());

        let aware_dt = result.unwrap();
        // Should be aware with UTC timezone
        assert!(aware_dt.tzinfo.is_some());
        assert_eq!(aware_dt.tzinfo.unwrap().tz, chrono_tz::UTC);

        // The datetime should have been interpreted as local time and converted to UTC
        match aware_dt.state {
            DateTimeState::Aware(_) => {} // Expected
            _ => panic!("Expected aware datetime"),
        }
    }

    #[test]
    fn test_astimezone_aware_datetime() {
        use crate::modules::pytz::PytzTimezone;

        // Create an aware datetime in US/Eastern
        let eastern_tz = PytzTimezone::new(chrono_tz::US::Eastern);
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 6, 15).unwrap(),
            NaiveTime::from_hms_opt(12, 30, 0).unwrap(),
        );
        let eastern_dt = chrono_tz::US::Eastern.from_local_datetime(&naive).unwrap();
        let aware_dt = PyDateTime::new_aware(eastern_dt, Some(eastern_tz));

        // Convert to UTC
        let utc_tz = PytzTimezone::new(chrono_tz::UTC);
        let result = aware_dt.astimezone(&utc_tz);
        assert!(result.is_ok());

        let utc_aware = result.unwrap();
        assert!(utc_aware.tzinfo.is_some());
        assert_eq!(utc_aware.tzinfo.unwrap().tz, chrono_tz::UTC);

        // The time should be converted from Eastern to UTC
        // 12:30 EDT (UTC-4 in summer) should be 16:30 UTC
        match utc_aware.state {
            DateTimeState::Aware(dt) => {
                assert_eq!(dt.hour(), 16); // 12:30 + 4 hours
            }
            _ => panic!("Expected aware datetime"),
        }
    }

    #[test]
    fn test_astimezone_different_timezones() {
        use crate::modules::pytz::PytzTimezone;

        // Create aware datetime in Tokyo
        let tokyo_tz = PytzTimezone::new(chrono_tz::Asia::Tokyo);
        let naive = NaiveDateTime::new(
            NaiveDate::from_ymd_opt(2023, 12, 1).unwrap(),
            NaiveTime::from_hms_opt(15, 0, 0).unwrap(),
        );
        let tokyo_dt = chrono_tz::Asia::Tokyo.from_local_datetime(&naive).unwrap();
        let aware_dt = PyDateTime::new_aware(tokyo_dt, Some(tokyo_tz));

        // Convert to US/Pacific
        let pacific_tz = PytzTimezone::new(chrono_tz::US::Pacific);
        let result = aware_dt.astimezone(&pacific_tz);
        assert!(result.is_ok());

        let pacific_aware = result.unwrap();
        assert!(pacific_aware.tzinfo.is_some());
        assert_eq!(pacific_aware.tzinfo.unwrap().tz, chrono_tz::US::Pacific);

        // Tokyo (UTC+9) 15:00 -> UTC 06:00 -> Pacific (UTC-8 in winter) 22:00 previous day
        match pacific_aware.state {
            DateTimeState::Aware(dt) => {
                assert_eq!(dt.day(), 30); // Previous day
                assert_eq!(dt.hour(), 22); // 15:00 - 17 hours
            }
            _ => panic!("Expected aware datetime"),
        }
    }

    #[test]
    fn test_utcoffset() {
        let env = Environment::new();

        // fixed offset => timedelta with the corresponding offset in seconds
        let dt = PyDateTimeClass::fromisoformat(args!("2026-01-29T08:20:52-05:00")).unwrap();
        let template = env
            .template_from_str("{{ dt.utcoffset().total_seconds() }}")
            .unwrap();
        let result = template
            .render(minijinja::context!(dt => Value::from_object(dt)), &[])
            .unwrap();
        assert_eq!(result, "-18000.0");

        // naive => None
        let dt = PyDateTimeClass::fromisoformat(args!("2026-01-29T08:20:52")).unwrap();
        let template = env.template_from_str("{{ dt.utcoffset() }}").unwrap();
        let result = template
            .render(minijinja::context!(dt => Value::from_object(dt)), &[])
            .unwrap();
        assert_eq!(result, "None");
    }

    // ── datetime.fromisocalendar ─────────────────────────────────────────
    //
    // Same error wordings as PyDateClass::fromisocalendar — CPython-exact.

    fn dt_fromisocalendar_err(args: &[Value]) -> String {
        PyDateTimeClass::fromisocalendar(args)
            .unwrap_err()
            .detail()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn test_datetime_fromisocalendar_happy_path_is_midnight() {
        let dt = PyDateTimeClass::fromisocalendar(args!(2024, 1, 1)).unwrap();
        // Confirm the date and time pieces line up with CPython's
        // datetime.fromisocalendar(2024, 1, 1) -> 2024-01-01 00:00:00.
        match dt.state {
            DateTimeState::Naive(naive) => {
                assert_eq!(naive.year(), 2024);
                assert_eq!(naive.month(), 1);
                assert_eq!(naive.day(), 1);
                assert_eq!(naive.hour(), 0);
                assert_eq!(naive.minute(), 0);
                assert_eq!(naive.second(), 0);
            }
            other => panic!("expected naive datetime, got {other:?}"),
        }
    }

    #[test]
    fn test_datetime_fromisocalendar_invalid_week() {
        assert_eq!(dt_fromisocalendar_err(args!(2024, 0, 1)), "Invalid week: 0");
        assert_eq!(
            dt_fromisocalendar_err(args!(2024, 54, 1)),
            "Invalid week: 54"
        );
        // 2024 has only 52 ISO weeks; CPython errors with "Invalid week: 53".
        assert_eq!(
            dt_fromisocalendar_err(args!(2024, 53, 1)),
            "Invalid week: 53"
        );
    }

    #[test]
    fn test_datetime_fromisocalendar_invalid_day() {
        assert_eq!(
            dt_fromisocalendar_err(args!(2024, 1, 0)),
            "Invalid day: 0 (range is [1, 7])"
        );
        assert_eq!(
            dt_fromisocalendar_err(args!(2024, 1, 8)),
            "Invalid day: 8 (range is [1, 7])"
        );
    }

    #[test]
    fn test_datetime_fromisocalendar_year_out_of_range() {
        assert_eq!(
            dt_fromisocalendar_err(args!(0, 1, 1)),
            "Year is out of range: 0"
        );
        assert_eq!(
            dt_fromisocalendar_err(args!(10000, 1, 1)),
            "Year is out of range: 10000"
        );
    }
    // ── render() / Python's str(datetime) ────────────────────────────────
    //
    // Bare `{{ dt }}` should produce Python's `str(datetime)` form — space
    // separator between date and time. `dt.isoformat()` keeps the 'T'
    // separator (Python distinguishes the two; we now do too).

    fn render_dt(dt: PyDateTime) -> String {
        let mut env = Environment::new();
        env.add_global("dt", Value::from_object(dt));
        env.template_from_str("{{ dt }}")
            .unwrap()
            .render(minijinja::context!(), &[])
            .unwrap()
    }

    #[test]
    fn test_str_naive_no_microseconds_uses_space() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-01T12:30:45")).unwrap();
        assert_eq!(render_dt(dt), "2024-01-01 12:30:45");
    }

    #[test]
    fn test_str_naive_with_microseconds_uses_space() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-01T12:30:45.123456")).unwrap();
        assert_eq!(render_dt(dt), "2024-01-01 12:30:45.123456");
    }

    #[test]
    fn test_strftime_naive_omits_timezone_directives() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-01T12:30:45.123456")).unwrap();
        let formatted = dt
            .strftime(&[Value::from("%Y-%m-%d %H:%M:%S %z %Z")])
            .unwrap();
        assert_eq!(formatted.as_str(), Some("2024-01-01 12:30:45  "));
    }

    #[test]
    fn test_str_naive_drops_zero_microseconds() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-01T00:00:00.000000")).unwrap();
        assert_eq!(render_dt(dt), "2024-01-01 00:00:00");
    }

    #[test]
    fn test_str_aware_includes_offset() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-15T08:30:00-05:00")).unwrap();
        assert_eq!(render_dt(dt), "2024-01-15 08:30:00-05:00");
    }

    #[test]
    fn test_str_aware_with_microseconds_and_offset() {
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-15T08:30:00.500000+00:00")).unwrap();
        assert_eq!(render_dt(dt), "2024-01-15 08:30:00.500000+00:00");
    }

    #[test]
    fn test_isoformat_still_uses_t_separator() {
        // Sibling-of-the-fix invariant: isoformat() must NOT change.
        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-01T12:30:45")).unwrap();
        assert_eq!(dt.isoformat(), "2024-01-01T12:30:45");

        let dt = PyDateTimeClass::fromisoformat(args!("2024-01-15T08:30:00.500000+00:00")).unwrap();
        assert_eq!(dt.isoformat(), "2024-01-15T08:30:00.500000+00:00");
    }
}
