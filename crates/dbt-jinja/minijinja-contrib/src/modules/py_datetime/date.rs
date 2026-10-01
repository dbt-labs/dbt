use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use chrono::{Datelike, Local, NaiveDate, NaiveDateTime, TimeZone, Weekday};
use minijinja::arg_utils::{ArgParser, ArgsIter};
use minijinja::{
    value::{DynObject, Object, ObjectRepr},
    Error, ErrorKind, Value,
};

use super::find_microsecond_directive;
use super::timedelta::PyTimeDelta;

#[derive(Debug)]
enum DateCmp {
    Eq,
    Neq,
    Lt,
    Le,
    Gt,
    Ge,
}

#[derive(Clone, Debug)]
pub(crate) struct PyDateClass;

impl PyDateClass {
    // ------------------
    // date(...)  constructor
    // ------------------
    fn create_date(args: &[Value]) -> Result<PyDate, Error> {
        // We want: date(year, month, day)
        let year = args
            .first()
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::new(ErrorKind::MissingArgument, "Year argument required"))?;

        let month = args
            .get(1)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::new(ErrorKind::MissingArgument, "Month argument required"))?;

        let day = args
            .get(2)
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::new(ErrorKind::MissingArgument, "Day argument required"))?;

        match NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32) {
            Some(date) => Ok(PyDate::new(date)),
            None => Err(Error::new(
                ErrorKind::InvalidArgument,
                "Invalid combination of year/month/day",
            )),
        }
    }

    // ------------------
    // date.today()
    // ------------------
    fn date_today(_args: &[Value]) -> Result<PyDate, Error> {
        // Return a new PyDate for today's local date
        let today = Local::now().naive_local().date();
        Ok(PyDate::new(today))
    }

    // ------------------
    // date.fromordinal(n)
    //   Construct from proleptic Gregorian ordinal
    // ------------------
    fn date_from_ordinal(args: &[Value]) -> Result<PyDate, Error> {
        let ordinal = args.first().and_then(|v| v.as_i64()).ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                "fromordinal() requires an integer ordinal",
            )
        })?;

        let naive = NaiveDate::from_num_days_from_ce_opt(ordinal as i32).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("Invalid ordinal: {ordinal}"),
            )
        })?;

        Ok(PyDate::new(naive))
    }

    // ------------------
    // date.fromtimestamp(ts)
    //   If following Python exactly, fromtimestamp() typically uses local time zone
    //   to interpret the seconds since epoch, returning a naive local date.
    // ------------------
    fn from_timestamp(args: &[Value]) -> Result<PyDate, Error> {
        let timestamp = args
            .first()
            .and_then(|v| f64::try_from(v.clone()).ok())
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::MissingArgument,
                    "fromtimestamp() requires a numeric timestamp",
                )
            })?;

        let secs = timestamp.trunc() as i64;
        let nanos_frac = (timestamp.fract() * 1e9).round() as u32;

        // If we want local time:
        let datetime = Local
            .timestamp_opt(secs, nanos_frac)
            .single()
            .ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidArgument,
                    "Timestamp is invalid or ambiguous (DST transition?)",
                )
            })?;

        let naive_date = datetime.naive_local().date();
        Ok(PyDate::new(naive_date))
    }

    // ------------------
    // date.fromisocalendar(year, week, day)
    //   ISO year, week (1..=53), day (1=Mon..7=Sun) -> Gregorian date.
    //   Mirrors CPython's exact error wording so SLT goldens cross-validate
    //   against Mantle.
    // ------------------
    fn fromisocalendar(args: &[Value]) -> Result<PyDate, Error> {
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

        // chrono returns None when week 53 doesn't exist for `year`.
        // Python's error in that case is "Invalid week: 53" — match it.
        let naive =
            NaiveDate::from_isoywd_opt(year as i32, week as u32, weekday).ok_or_else(|| {
                Error::new(ErrorKind::InvalidArgument, format!("Invalid week: {week}"))
            })?;

        Ok(PyDate::new(naive))
    }

    fn fromisoformat(args: &[Value]) -> Result<PyDate, Error> {
        let iso_str = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                "fromisoformat() requires a string in ISO format",
            )
        })?;

        let naive_date = NaiveDate::parse_from_str(iso_str, "%Y-%m-%d").map_err(|e| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("Invalid ISO date format: {iso_str}: {e}"),
            )
        })?;

        Ok(PyDate::new(naive_date))
    }
}

impl Object for PyDateClass {
    // This is called when the user does:  {{ date(...) }}
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        // Convert PyDate to Value here
        Self::create_date(args).map(Value::from_object)
    }

    // This is how the user does: {{ date.today() }}, {{ date.fromtimestamp(...) }}, etc.
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "today" => Self::date_today(args).map(Value::from_object),
            "fromtimestamp" => Self::from_timestamp(args).map(Value::from_object),
            "fromordinal" => Self::date_from_ordinal(args).map(Value::from_object),
            "fromisoformat" => Self::fromisoformat(args).map(Value::from_object),
            "fromisocalendar" => Self::fromisocalendar(args).map(Value::from_object),
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("date has no method named '{method}'"),
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub struct PyDate {
    pub date: NaiveDate,
}

impl PyDate {
    pub fn new(date: NaiveDate) -> Self {
        PyDate { date }
    }

    /// strftime(format)
    pub fn strftime(&self, args: &[Value]) -> Result<Value, Error> {
        let fmt = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
            Error::new(
                ErrorKind::MissingArgument,
                "strftime requires one string argument",
            )
        })?;

        // Convert to datetime to allow format codes for hours, minutes and seconds like python does.
        // See https://docs.python.org/3/library/datetime.html#datetime.date.strftime:
        // "Format codes referring to hours, minutes or seconds will see 0 values"
        let datetime: NaiveDateTime = self.date.into();

        // Python's `%f` is six zero-padded microsecond digits (always zero here);
        // chrono's `%f` is nine nanosecond digits. Format the surrounding pieces
        // with chrono and write the microseconds ourselves.
        let mut formatted = String::new();
        let mut remaining = fmt;
        while let Some(pos) = find_microsecond_directive(remaining) {
            formatted.push_str(&datetime.format(&remaining[..pos]).to_string());
            formatted.push_str("000000");
            remaining = &remaining[pos + "%f".len()..];
        }
        formatted.push_str(&datetime.format(remaining).to_string());

        Ok(Value::from(formatted))
    }

    /// Handle date + timedelta or date - timedelta or date - date operations
    fn add_op(&self, args: &[Value], is_add: bool) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let rhs: Value = parser.next_positional()?;

        // Case 1: date + timedelta or date - timedelta => date
        if let Some(delta) = rhs.downcast_object_ref::<PyTimeDelta>() {
            // Choose direction based on operation
            let duration = if is_add {
                delta.duration
            } else {
                -delta.duration
            };

            // Apply the duration to create a new date
            let date_time = self.date.and_hms_opt(0, 0, 0).unwrap() + duration;
            let new_date = date_time.date();

            return Ok(Value::from_object(PyDate::new(new_date)));
        }
        // Case 2: date - date => timedelta (only for subtraction)
        else if !is_add {
            if let Some(other_date) = rhs.downcast_object_ref::<PyDate>() {
                // Calculate days between dates
                let self_dt = self.date.and_hms_opt(0, 0, 0).unwrap();
                let other_dt = other_date.date.and_hms_opt(0, 0, 0).unwrap();

                let diff = self_dt.signed_duration_since(other_dt);
                return Ok(Value::from_object(PyTimeDelta::new(diff)));
            }
        }

        Err(Error::new(
            ErrorKind::InvalidOperation,
            if is_add {
                "Cannot add this type to a date object"
            } else {
                "Cannot subtract this type from a date object"
            },
        ))
    }

    fn replace(&self, args: &[Value]) -> Result<PyDate, Error> {
        let mut parser = ArgParser::new(args, None);

        // Start with the current date's values
        let mut year = self.date.year();
        let mut month = self.date.month();
        let mut day = self.date.day();

        // Apply any changes specified in kwargs
        if let Some(y) = parser.consume_optional_only_from_kwargs::<i32>("year") {
            year = y;
        }

        if let Some(m) = parser.consume_optional_only_from_kwargs::<u32>("month") {
            month = m;
        }

        if let Some(d) = parser.consume_optional_only_from_kwargs::<u32>("day") {
            day = d;
        }

        // Create a new date with the updated values
        match NaiveDate::from_ymd_opt(year, month, day) {
            Some(new_date) => Ok(PyDate::new(new_date)),
            None => Err(Error::new(
                ErrorKind::InvalidArgument,
                format!("Invalid date: year={year}, month={month}, day={day}"),
            )),
        }
    }

    fn isoformat(&self, _args: &[Value]) -> Result<Value, Error> {
        Ok(Value::from(self.date.format("%Y-%m-%d").to_string()))
    }

    fn weekday(&self, _args: &[Value]) -> Result<Value, Error> {
        // 0 = Monday, 6 = Sunday
        let weekday = self.date.weekday().num_days_from_monday();
        Ok(Value::from(weekday))
    }

    fn isoweekday(&self, _args: &[Value]) -> Result<Value, Error> {
        // 1 = Monday, 7 = Sunday
        let isoweekday = self.date.weekday().num_days_from_sunday() + 1;
        Ok(Value::from(isoweekday))
    }

    fn isocalendar(&self, _args: &[Value]) -> Result<Value, Error> {
        let iso_week = self.date.iso_week();
        let iso_year = iso_week.year();
        let iso_week_number = iso_week.week() as i32;
        let iso_weekday = self.date.weekday().number_from_monday() as i32; // 1 = Monday, 7 = Sunday

        // Return the tuple (ISO year, ISO week number, ISO weekday)
        Ok(Value::from(vec![iso_year, iso_week_number, iso_weekday]))
    }

    fn cmp_op(&self, args: &[Value], cmp: DateCmp) -> Result<Value, Error> {
        let iter = ArgsIter::new(
            match cmp {
                DateCmp::Eq => "__eq__",
                DateCmp::Neq => "__ne__",
                DateCmp::Lt => "__lt__",
                DateCmp::Le => "__le__",
                DateCmp::Gt => "__gt__",
                DateCmp::Ge => "__ge__",
            },
            &["other"],
            args,
        );
        let rhs = iter.next_arg::<Value>()?;
        iter.finish()?;
        if let Some(other_date) = rhs.downcast_object_ref::<PyDate>() {
            let o = self.date.cmp(&other_date.date);
            match cmp {
                DateCmp::Eq => Ok(Value::from(o == Ordering::Equal)),
                DateCmp::Neq => Ok(Value::from(o != Ordering::Equal)),
                DateCmp::Lt => Ok(Value::from(matches!(o, Ordering::Less))),
                DateCmp::Le => Ok(Value::from(matches!(o, Ordering::Less | Ordering::Equal))),
                DateCmp::Gt => Ok(Value::from(matches!(o, Ordering::Greater))),
                DateCmp::Ge => Ok(Value::from(matches!(
                    o,
                    Ordering::Greater | Ordering::Equal
                ))),
            }
        } else {
            match cmp {
                DateCmp::Eq => Ok(Value::from(false)),
                DateCmp::Neq => Ok(Value::from(true)),
                _ => Err(Error::new(
                    ErrorKind::InvalidArgument,
                    "Can only compare date objects",
                )),
            }
        }
    }
}

impl Object for PyDate {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn is_true(self: &Arc<Self>) -> bool {
        true
    }

    fn custom_cmp(self: &Arc<Self>, other: &DynObject) -> Option<Ordering> {
        other
            .downcast_ref::<Self>()
            .map(|other| self.date.cmp(&other.date))
    }
    // If someone does: {{ some_date.attribute }} in a template,
    // you can provide direct read access:
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            // If you want to let templates read "some_date.year" or "some_date.day":
            "year" => Some(Value::from(self.date.year())),
            "month" => Some(Value::from(self.date.month())),
            "day" => Some(Value::from(self.date.day())),
            _ => None,
        }
    }

    // When rendered directly, e.g. {{ some_date }}, produce YYYY-MM-DD.
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.date.format("%Y-%m-%d"))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "strftime" => Self::strftime(self, args),
            "replace" => Self::replace(self, args).map(Value::from_object),
            "isoformat" => Self::isoformat(self, args),
            "weekday" => Self::weekday(self, args),
            "isoweekday" => Self::isoweekday(self, args),
            "isocalendar" => Self::isocalendar(self, args),

            // Add arithmetic operations
            "__add__" => self.add_op(args, true),
            "__sub__" => self.add_op(args, false),

            // Comparison
            "__eq__" => self.cmp_op(args, DateCmp::Eq),
            "__ne__" => self.cmp_op(args, DateCmp::Neq),
            "__lt__" => self.cmp_op(args, DateCmp::Lt),
            "__le__" => self.cmp_op(args, DateCmp::Le),
            "__gt__" => self.cmp_op(args, DateCmp::Gt),
            "__ge__" => self.cmp_op(args, DateCmp::Ge),

            "today" => PyDateClass::date_today(args).map(Value::from_object),
            "fromtimestamp" => PyDateClass::from_timestamp(args).map(Value::from_object),
            "fromordinal" => PyDateClass::date_from_ordinal(args).map(Value::from_object),
            "fromisoformat" => PyDateClass::fromisoformat(args).map(Value::from_object),
            "fromisocalendar" => PyDateClass::fromisocalendar(args).map(Value::from_object),

            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("date has no method named '{method}'"),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::py_datetime::timedelta::PyTimeDelta;
    use minijinja::args;
    use minijinja::context;
    use minijinja::Environment;

    #[test]
    fn test_date_strftime() {
        // Test the direct strftime method implementation
        let date = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 15).unwrap());
        let date_arc = Arc::new(date);

        // Test with different format strings
        let result = date_arc.strftime(&[Value::from("%Y-%m-%d")]).unwrap();
        assert_eq!(result.to_string(), "2023-05-15");

        let result = date_arc.strftime(&[Value::from("%d/%m/%Y")]).unwrap();
        assert_eq!(result.to_string(), "15/05/2023");

        let result = date_arc.strftime(&[Value::from("%A, %B %d, %Y")]).unwrap();
        assert_eq!(result.to_string(), "Monday, May 15, 2023");

        let result = date_arc.strftime(&[Value::from("%H-%M-%S")]).unwrap();
        assert_eq!(result.to_string(), "00-00-00");

        // Test error case - missing format argument
        let error = date_arc.strftime(&[]).unwrap_err();
        assert!(error
            .to_string()
            .contains("strftime requires one string argument"));
    }

    #[test]
    fn test_date_strftime_in_template() {
        // Test strftime through the template engine
        let mut env = Environment::new();

        // Register the PyDateModule to make date accessible in templates
        env.add_global("date", Value::from_object(PyDateClass));

        // Create a template that uses date and strftime
        let template = env
            .template_from_str("{{ date(2023, 5, 15).strftime('%Y-%m-%d') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "2023-05-15");

        // Test with a different format
        let template = env
            .template_from_str("{{ date(2023, 5, 15).strftime('%d/%m/%Y') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "15/05/2023");
    }

    #[test]
    fn test_date_addition() {
        // Create a date and a timedelta
        let date = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 15).unwrap());
        let date_arc = Arc::new(date);

        // Adding 5 days
        let delta_5days = PyTimeDelta::new(chrono::Duration::days(5));
        let args = [Value::from_object(delta_5days)];

        let result = date_arc.add_op(&args, true).unwrap();
        let new_date = result.downcast_object_ref::<PyDate>().unwrap();
        assert_eq!(new_date.date.to_string(), "2023-05-20");

        // Adding negative days (equivalent to subtraction)
        let delta_neg3days = PyTimeDelta::new(chrono::Duration::days(-3));
        let args = [Value::from_object(delta_neg3days)];

        let result = date_arc.add_op(&args, true).unwrap();
        let new_date = result.downcast_object_ref::<PyDate>().unwrap();
        assert_eq!(new_date.date.to_string(), "2023-05-12");

        // Test month boundary
        let date_end_month = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 31).unwrap());
        let date_end_month_arc = Arc::new(date_end_month);
        let delta_1day = PyTimeDelta::new(chrono::Duration::days(1));
        let args = [Value::from_object(delta_1day)];

        let result = date_end_month_arc.add_op(&args, true).unwrap();
        let new_date = result.downcast_object_ref::<PyDate>().unwrap();
        assert_eq!(new_date.date.to_string(), "2023-06-01");
    }

    #[test]
    fn test_date_subtraction() {
        // Create a date and a timedelta
        let date = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 15).unwrap());
        let date_arc = Arc::new(date);

        // Subtracting 5 days
        let delta_5days = PyTimeDelta::new(chrono::Duration::days(5));
        let args = [Value::from_object(delta_5days)];

        let result = date_arc.add_op(&args, false).unwrap();
        let new_date = result.downcast_object_ref::<PyDate>().unwrap();
        assert_eq!(new_date.date.to_string(), "2023-05-10");

        // Test month boundary
        let date_start_month = PyDate::new(NaiveDate::from_ymd_opt(2023, 6, 1).unwrap());
        let date_start_month_arc = Arc::new(date_start_month);
        let delta_1day = PyTimeDelta::new(chrono::Duration::days(1));
        let args = [Value::from_object(delta_1day)];

        let result = date_start_month_arc.add_op(&args, false).unwrap();
        let new_date = result.downcast_object_ref::<PyDate>().unwrap();
        assert_eq!(new_date.date.to_string(), "2023-05-31");
    }

    #[test]
    fn test_date_subtraction_from_date() {
        // Create two dates
        let date1 = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 20).unwrap());
        let date1_arc = Arc::new(date1.clone());

        let date2 = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 15).unwrap());

        // Subtract date2 from date1 (should be 5 days)
        let args = [Value::from_object(date2.clone())];

        let result = date1_arc.add_op(&args, false).unwrap();
        let delta = result.downcast_object_ref::<PyTimeDelta>().unwrap();
        assert_eq!(delta.duration.num_days(), 5);

        // Now reverse (should be -5 days)
        let date2_arc = Arc::new(date2);
        let args = [Value::from_object(date1)];

        let result = date2_arc.add_op(&args, false).unwrap();
        let delta = result.downcast_object_ref::<PyTimeDelta>().unwrap();
        assert_eq!(delta.duration.num_days(), -5);
    }

    #[test]
    fn test_date_arithmetic_in_template() {
        // Test date arithmetic through the template engine
        let mut env = Environment::new();

        // Register the modules
        env.add_global("date", Value::from_object(PyDateClass));
        // We also need to register the timedelta module
        env.add_global(
            "timedelta",
            Value::from_object(crate::modules::py_datetime::timedelta::PyTimeDeltaClass),
        );

        // Test adding days
        let template = env
            .template_from_str("{{ (date(2023, 5, 15) + timedelta(days=5)).strftime('%Y-%m-%d') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "2023-05-20");

        // Test subtracting days
        let template = env
            .template_from_str("{{ (date(2023, 5, 15) - timedelta(days=3)).strftime('%Y-%m-%d') }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "2023-05-12");

        // Test date subtraction
        let template = env
            .template_from_str("{{ (date(2023, 5, 20) - date(2023, 5, 15)).days }}")
            .unwrap();
        let result = template.render(context!(), &[]).unwrap();
        assert_eq!(result, "5");
    }

    #[test]
    fn test_date_replace() {
        let date = PyDate::new(NaiveDate::from_ymd_opt(2023, 5, 15).unwrap());
        let date_arc = Arc::new(date);
        println!("date_arc: {date_arc:?}");
        // Test replacing year
        let result = date_arc.replace(args!(year => 2024)).unwrap();
        assert_eq!(result.date.year(), 2024);
        assert_eq!(result.date.month(), 5);
        assert_eq!(result.date.day(), 15);

        // Test replacing month
        let result = date_arc.replace(args!(month => 10)).unwrap();
        assert_eq!(result.date.year(), 2023);
        assert_eq!(result.date.month(), 10);
        assert_eq!(result.date.day(), 15);

        // Test replacing day
        let result = date_arc.replace(args!(day => 20)).unwrap();
        assert_eq!(result.date.year(), 2023);
        assert_eq!(result.date.month(), 5);
        assert_eq!(result.date.day(), 20);

        // Test replacing multiple fields
        let result = date_arc
            .replace(args!(year => 2024, month => 10, day => 20))
            .unwrap();
        assert_eq!(result.date.year(), 2024);
        assert_eq!(result.date.month(), 10);
        assert_eq!(result.date.day(), 20);

        // Test invalid date
        let result = date_arc.replace(args!(month => 13));
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Invalid date: year=2023, month=13, day=15"));
    }

    // ── date.fromisocalendar ────────────────────────────────────────────
    //
    // Error wordings mirror CPython exactly so SLT happy-path goldens cross-
    // validate against Mantle and the error messages users see match what
    // they'd see in dbt-core.

    fn fromisocalendar_err(args: &[Value]) -> String {
        PyDateClass::fromisocalendar(args)
            .unwrap_err()
            .detail()
            .unwrap_or_default()
            .to_string()
    }

    #[test]
    fn test_date_fromisocalendar_invalid_week_zero() {
        assert_eq!(fromisocalendar_err(args!(2024, 0, 1)), "Invalid week: 0");
    }

    #[test]
    fn test_date_fromisocalendar_invalid_week_too_high() {
        assert_eq!(fromisocalendar_err(args!(2024, 54, 1)), "Invalid week: 54");
    }

    #[test]
    fn test_date_fromisocalendar_week_53_in_non_53_year() {
        // 2024 has only 52 ISO weeks; CPython errors with "Invalid week: 53".
        assert_eq!(fromisocalendar_err(args!(2024, 53, 1)), "Invalid week: 53");
    }

    #[test]
    fn test_date_fromisocalendar_invalid_day() {
        assert_eq!(
            fromisocalendar_err(args!(2024, 1, 0)),
            "Invalid day: 0 (range is [1, 7])"
        );
        assert_eq!(
            fromisocalendar_err(args!(2024, 1, 8)),
            "Invalid day: 8 (range is [1, 7])"
        );
    }

    #[test]
    fn test_date_fromisocalendar_year_out_of_range() {
        assert_eq!(
            fromisocalendar_err(args!(0, 1, 1)),
            "Year is out of range: 0"
        );
        assert_eq!(
            fromisocalendar_err(args!(10000, 1, 1)),
            "Year is out of range: 10000"
        );
    }
}
