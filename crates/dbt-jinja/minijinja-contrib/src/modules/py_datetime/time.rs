use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use chrono::{Local, NaiveDate, NaiveTime, Timelike};
use minijinja::arg_utils::{ArgParser, ArgsIter};
use minijinja::{value::Object, Error, ErrorKind, Value};

use crate::modules::py_datetime::find_microsecond_directive;
use crate::modules::py_datetime::timedelta::PyTimeDelta;
use crate::modules::pytz::PytzTimezone;

#[derive(Debug)]
enum TimeCmp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
pub struct PyTimeClass;

impl PyTimeClass {
    /// The constructor equivalent to `time(...)` in Python
    fn create_time(args: &[Value]) -> Result<PyTime, Error> {
        // Usage:
        //   time(hour=0, minute=0, second=0, microsecond=0)
        // or positional usage time(12, 34, 56, 123456)
        let mut parser = ArgParser::new(args, None);

        // We read them in either positional or named, defaulting if not given
        let hour: i64 = parser.get("hour").unwrap_or(0);
        let minute: i64 = parser.get("minute").unwrap_or(0);
        let second: i64 = parser.get("second").unwrap_or(0);
        let microsecond: i64 = parser.get("microsecond").unwrap_or(0);

        // Validate ranges
        if !(0..=23).contains(&hour) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("hour must be in 0..=23, got {hour}"),
            ));
        }
        if !(0..=59).contains(&minute) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("minute must be in 0..=59, got {minute}"),
            ));
        }
        if !(0..=59).contains(&second) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("second must be in 0..=59, got {second}"),
            ));
        }
        if !(0..1_000_000).contains(&microsecond) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("microsecond must be in 0..1_000_000, got {microsecond}"),
            ));
        }

        // Construct the NaiveTime
        let naive = NaiveTime::from_hms_micro_opt(
            hour as u32,
            minute as u32,
            second as u32,
            microsecond as u32,
        )
        .ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "invalid hour/minute/second/microsecond combo",
            )
        })?;

        // If you plan to allow tzinfo=, you can parse it similarly from parser.
        // For now, we keep it naive:
        Ok(PyTime { time: naive })
    }

    /// Return current local time as a naive `PyTime`.
    fn now(_args: &[Value]) -> Result<PyTime, Error> {
        let local_now = Local::now().time();
        Ok(PyTime { time: local_now })
    }

    /// fromisoformat("HH:MM:SS[.mmmmmm]")
    /// This is just a stub that you can expand.  
    fn fromisoformat(args: &[Value]) -> Result<PyTime, Error> {
        let iso_str = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                "fromisoformat() requires a string argument",
            )
        })?;

        // Minimal quick parse
        // Example: "23:59:59.999999"
        let parsed = match NaiveTime::parse_from_str(iso_str, "%H:%M:%S%.f") {
            Ok(nt) => nt,
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::InvalidArgument,
                    format!("Invalid iso time format: {iso_str}: {e}"),
                ));
            }
        };

        Ok(PyTime { time: parsed })
    }
}

// Implement the `Object` trait so that `time(...)`, `time.now()`, `time.fromisoformat(...)` are available
impl Object for PyTimeClass {
    /// Called when you do `time(...)` in the template
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        Self::create_time(args).map(Value::from_object)
    }

    /// Called when you do `time.now()` or `time.fromisoformat(...)`
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "now" => Self::now(args).map(Value::from_object),
            "fromisoformat" => Self::fromisoformat(args).map(Value::from_object),
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("time object has no method named '{method}'"),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PyTime {
    pub time: NaiveTime,
}

impl PyTime {
    /// Helper constructor if you want a direct `PyTime::new(...)`.
    pub fn new(time: NaiveTime, _tzinfo: Option<PytzTimezone>) -> Self {
        PyTime { time }
    }

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
        let mut result = String::new();
        let mut remaining = fmt;
        while let Some(pos) = find_microsecond_directive(remaining) {
            result.push_str(&self.time.format(&remaining[..pos]).to_string());
            let microseconds = self.time.nanosecond() / 1_000;
            result.push_str(&format!("{microseconds:06}"));
            remaining = &remaining[pos + "%f".len()..];
        }
        result.push_str(&self.time.format(remaining).to_string());

        Ok(Value::from(result))
    }

    fn replace(&self, args: &[Value]) -> Result<PyTime, Error> {
        let mut parser = ArgParser::new(args, None);

        let mut hour = self.time.hour();
        let mut minute = self.time.minute();
        let mut second = self.time.second();
        let mut microsecond = self.time.nanosecond() / 1_000;

        if let Some(h) = parser.consume_optional_only_from_kwargs::<u32>("hour") {
            hour = h;
        }
        if let Some(m) = parser.consume_optional_only_from_kwargs::<u32>("minute") {
            minute = m;
        }
        if let Some(s) = parser.consume_optional_only_from_kwargs::<u32>("second") {
            second = s;
        }
        if let Some(us) = parser.consume_optional_only_from_kwargs::<u32>("microsecond") {
            microsecond = us;
        }

        let naive = NaiveTime::from_hms_micro_opt(hour, minute, second, microsecond).ok_or_else(
            || {
                Error::new(
                    ErrorKind::InvalidArgument,
                    format!(
                        "Invalid time: hour={hour}, minute={minute}, second={second}, microsecond={microsecond}"
                    ),
                )
            },
        )?;

        Ok(PyTime { time: naive })
    }

    /// Handle time + timedelta operations
    fn add_op(&self, args: &[Value], is_add: bool) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let rhs: Value = parser.next_positional()?;

        // We only support adding/subtracting a timedelta to a time object
        if let Some(delta) = rhs.downcast_object_ref::<PyTimeDelta>() {
            // Apply the duration in the correct direction
            let duration = if is_add {
                delta.duration
            } else {
                -delta.duration
            };

            // Start with midnight on a dummy date and add our time
            let midnight = NaiveDate::from_ymd_opt(2000, 1, 1)
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap();
            let time_on_dummy_date = midnight
                + chrono::Duration::seconds(
                    self.time.hour() as i64 * 3600
                        + self.time.minute() as i64 * 60
                        + self.time.second() as i64,
                )
                + chrono::Duration::nanoseconds(self.time.nanosecond() as i64);

            // Apply the delta
            let new_datetime = time_on_dummy_date + duration;

            // Extract just the time portion
            let new_time = new_datetime.time();

            return Ok(Value::from_object(PyTime::new(new_time, None)));
        }
        // Special case for subtraction with another time
        else if !is_add {
            if let Some(other_time) = rhs.downcast_object_ref::<PyTime>() {
                // Compute difference between the times
                // We'll use a dummy date to create naive datetimes
                let dummy_date = NaiveDate::from_ymd_opt(2000, 1, 1).unwrap();

                let self_dt = dummy_date.and_time(self.time);
                let other_dt = dummy_date.and_time(other_time.time);

                // Determine if we need to add a day
                let (diff, negative) = if self_dt >= other_dt {
                    (self_dt - other_dt, false)
                } else {
                    // If other time is greater, we'll assume it's from the previous day
                    let next_day = dummy_date.succ_opt().unwrap();
                    let self_dt_next = next_day.and_time(self.time);
                    (self_dt_next - other_dt, false)
                };

                // Create a timedelta with the correct sign
                let duration = if negative { -diff } else { diff };
                return Ok(Value::from_object(PyTimeDelta::new(duration)));
            }
        }

        Err(Error::new(
            ErrorKind::InvalidOperation,
            if is_add {
                "Cannot add this type to a time object"
            } else {
                "Cannot subtract this type from a time object"
            },
        ))
    }

    fn cmp_op(&self, args: &[Value], cmp: TimeCmp) -> Result<Value, Error> {
        let iter = ArgsIter::new(
            match cmp {
                TimeCmp::Eq => "__eq__",
                TimeCmp::Neq => "__ne__",
                TimeCmp::Lt => "__lt__",
                TimeCmp::Le => "__le__",
                TimeCmp::Gt => "__gt__",
                TimeCmp::Ge => "__ge__",
            },
            &["other"],
            args,
        );
        let rhs = iter.next_arg::<Value>()?;
        iter.finish()?;
        if let Some(other_time) = rhs.downcast_object_ref::<PyTime>() {
            let o = self.time.cmp(&other_time.time);
            match cmp {
                TimeCmp::Eq => Ok(Value::from(o == Ordering::Equal)),
                TimeCmp::Neq => Ok(Value::from(o != Ordering::Equal)),
                TimeCmp::Lt => Ok(Value::from(matches!(o, Ordering::Less))),
                TimeCmp::Le => Ok(Value::from(matches!(o, Ordering::Less | Ordering::Equal))),
                TimeCmp::Gt => Ok(Value::from(matches!(o, Ordering::Greater))),
                TimeCmp::Ge => Ok(Value::from(matches!(
                    o,
                    Ordering::Greater | Ordering::Equal
                ))),
            }
        } else {
            match cmp {
                TimeCmp::Eq => Ok(Value::from(false)),
                TimeCmp::Neq => Ok(Value::from(true)),
                _ => Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "Can only compare time objects",
                )),
            }
        }
    }
}

impl Object for PyTime {
    fn is_true(self: &Arc<Self>) -> bool {
        true
    }
    /// If someone tries:  `some_time.attribute` in a template,
    /// we can provide direct read access to hour, minute, second, microsecond, etc.
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "hour" => Some(Value::from(self.time.hour())),
            "minute" => Some(Value::from(self.time.minute())),
            "second" => Some(Value::from(self.time.second())),
            "microsecond" => Some(Value::from(self.time.nanosecond() / 1_000)), // or / 1000
            _ => None,
        }
    }

    /// If someone calls `some_time.method(...)`,
    /// you can handle it here. Example: "isoformat", "replace", etc.
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "isoformat" => {
                if self.time.nanosecond() == 0 {
                    Ok(Value::from(self.time.format("%H:%M:%S").to_string()))
                } else {
                    Ok(Value::from(self.time.format("%H:%M:%S%.6f").to_string()))
                }
            }
            "replace" => self.replace(args).map(Value::from_object),
            "strftime" => self.strftime(args),

            // Add arithmetic operations
            "__add__" => self.add_op(args, true),
            "__sub__" => self.add_op(args, false),

            // Comparison
            "__eq__" => self.cmp_op(args, TimeCmp::Eq),
            "__ne__" => self.cmp_op(args, TimeCmp::Neq),
            "__lt__" => self.cmp_op(args, TimeCmp::Lt),
            "__le__" => self.cmp_op(args, TimeCmp::Le),
            "__gt__" => self.cmp_op(args, TimeCmp::Gt),
            "__ge__" => self.cmp_op(args, TimeCmp::Ge),

            "now" => PyTimeClass::now(args).map(Value::from_object),
            "fromisoformat" => PyTimeClass::fromisoformat(args).map(Value::from_object),

            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("time object has no method '{method}'"),
            )),
        }
    }

    /// If someone directly renders `{{ my_time }}` in a template,
    /// we produce an HH:MM:SS.uuuuuu string. Python's `str(time_obj)` works similarly.
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // If microseconds is zero, omit it like Python does
        if self.time.nanosecond() == 0 {
            write!(f, "{}", self.time.format("%H:%M:%S"))
        } else {
            write!(f, "{}", self.time.format("%H:%M:%S%.6f"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::py_datetime::timedelta::PyTimeDelta;
    use minijinja::context;
    use minijinja::Environment;

    #[test]
    fn test_time_strftime() {
        // Test the direct strftime method implementation
        let time = PyTime::new(
            NaiveTime::from_hms_micro_opt(14, 30, 45, 123456).unwrap(),
            None,
        );
        let time_arc = Arc::new(time);

        // Test with different format strings
        let result = time_arc.strftime(&[Value::from("%H:%M:%S")]).unwrap();
        assert_eq!(result.to_string(), "14:30:45");

        let result = time_arc.strftime(&[Value::from("%I:%M %p")]).unwrap();
        assert_eq!(result.to_string(), "02:30 PM");

        let result = time_arc.strftime(&[Value::from("%H:%M:%S.%f")]).unwrap();
        assert_eq!(result.to_string(), "14:30:45.123456");

        // Test error case - missing format argument
        let error = time_arc.strftime(&[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("strftime requires one string argument"));
    }

    #[test]
    fn test_time_strftime_in_template() {
        // Test strftime through the template engine
        let mut env = Environment::new();

        // Register the PyTimeModule to make time accessible in templates
        env.add_global("time", Value::from_object(PyTimeClass));

        // Create a template that uses time and strftime
        let template = env
            .template_from_str("{{ time(14, 30, 45, 123456).strftime('%H:%M:%S') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "14:30:45");

        // Test with a different format
        let template = env
            .template_from_str("{{ time(14, 30, 45, 123456).strftime('%I:%M %p') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "02:30 PM");

        // Test with microseconds
        let template = env
            .template_from_str("{{ time(14, 30, 45, 123456).strftime('%H:%M:%S.%f') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "14:30:45.123456");
    }

    #[test]
    fn test_time_addition() {
        // Create a time and a timedelta
        let time = PyTime::new(
            NaiveTime::from_hms_micro_opt(14, 30, 45, 123456).unwrap(),
            None,
        );
        let time_arc = Arc::new(time);

        // Adding 1 hour
        let delta_1hour = PyTimeDelta::new(chrono::Duration::hours(1));
        let args = [Value::from_object(delta_1hour)];

        let result = time_arc.add_op(&args, true).unwrap();
        let new_time = result.downcast_object_ref::<PyTime>().unwrap();
        assert_eq!(new_time.time.hour(), 15);
        assert_eq!(new_time.time.minute(), 30);
        assert_eq!(new_time.time.second(), 45);

        // Adding time that crosses midnight
        let time_evening = PyTime::new(NaiveTime::from_hms_opt(23, 30, 0).unwrap(), None);
        let time_evening_arc = Arc::new(time_evening);
        let delta_1hour = PyTimeDelta::new(chrono::Duration::hours(1));
        let args = [Value::from_object(delta_1hour)];

        let result = time_evening_arc.add_op(&args, true).unwrap();
        let new_time = result.downcast_object_ref::<PyTime>().unwrap();
        assert_eq!(new_time.time.hour(), 0);
        assert_eq!(new_time.time.minute(), 30);
    }

    #[test]
    fn test_time_subtraction() {
        // Create a time and a timedelta
        let time = PyTime::new(
            NaiveTime::from_hms_micro_opt(14, 30, 45, 123456).unwrap(),
            None,
        );
        let time_arc = Arc::new(time);

        // Subtracting 1 hour
        let delta_1hour = PyTimeDelta::new(chrono::Duration::hours(1));
        let args = [Value::from_object(delta_1hour)];

        let result = time_arc.add_op(&args, false).unwrap();
        let new_time = result.downcast_object_ref::<PyTime>().unwrap();
        assert_eq!(new_time.time.hour(), 13);
        assert_eq!(new_time.time.minute(), 30);
        assert_eq!(new_time.time.second(), 45);

        // Subtracting time that crosses midnight
        let time_morning = PyTime::new(NaiveTime::from_hms_opt(0, 30, 0).unwrap(), None);
        let time_morning_arc = Arc::new(time_morning);
        let delta_1hour = PyTimeDelta::new(chrono::Duration::hours(1));
        let args = [Value::from_object(delta_1hour)];

        let result = time_morning_arc.add_op(&args, false).unwrap();
        let new_time = result.downcast_object_ref::<PyTime>().unwrap();
        assert_eq!(new_time.time.hour(), 23);
        assert_eq!(new_time.time.minute(), 30);
    }

    #[test]
    fn test_time_subtraction_from_time() {
        // Create two times
        let time1 = PyTime::new(NaiveTime::from_hms_opt(14, 30, 0).unwrap(), None);
        let time1_arc = Arc::new(time1);

        let time2 = PyTime::new(NaiveTime::from_hms_opt(13, 15, 0).unwrap(), None);

        // Subtract time2 from time1
        let args = [Value::from_object(time2)];

        let result = time1_arc.add_op(&args, false).unwrap();
        let delta = result.downcast_object_ref::<PyTimeDelta>().unwrap();

        // Should be 1 hour and 15 minutes
        assert_eq!(delta.duration.num_minutes(), 75);
    }

    #[test]
    fn test_time_arithmetic_in_template() {
        // Test time arithmetic through the template engine
        let mut env = Environment::new();

        // Register the modules
        env.add_global("time", Value::from_object(PyTimeClass));
        // We also need to register the timedelta module
        env.add_global(
            "timedelta",
            Value::from_object(crate::modules::py_datetime::timedelta::PyTimeDeltaClass),
        );

        // Test adding hours
        let template = env
            .template_from_str("{{ (time(14, 30, 0) + timedelta(hours=1)).strftime('%H:%M:%S') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "15:30:00");

        // Test subtracting minutes
        let template = env
            .template_from_str(
                "{{ (time(14, 30, 0) - timedelta(minutes=45)).strftime('%H:%M:%S') }}",
            )
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "13:45:00");

        // Test time subtraction
        let template = env
            .template_from_str("{{ (time(14, 30, 0) - time(13, 15, 0)).seconds }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        // 1 hour 15 minutes = 75 minutes = 4500 seconds
        assert_eq!(result, "4500");
    }
}
