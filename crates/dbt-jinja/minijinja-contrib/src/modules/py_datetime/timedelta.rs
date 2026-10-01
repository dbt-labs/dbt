use chrono::Duration;
use minijinja::{arg_utils::ArgParser, value::Object, Error, ErrorKind, Value};
use std::fmt;
use std::sync::Arc;

use super::{
    date::PyDate,
    datetime::{DateTimeState, PyDateTime},
    time::PyTime,
};

#[derive(Clone, Debug)]
pub(crate) struct PyTimeDeltaClass;

impl PyTimeDeltaClass {
    fn timedelta_new(args: &[Value]) -> Result<PyTimeDelta, Error> {
        let mut parser = ArgParser::new(args, None);
        let days: i64 = parser.get("days").unwrap_or(0);
        let seconds: i64 = parser.get("seconds").unwrap_or(0);
        let microseconds: i64 = parser.get("microseconds").unwrap_or(0);
        let milliseconds: i64 = parser.get("milliseconds").unwrap_or(0);
        let minutes: i64 = parser.get("minutes").unwrap_or(0);
        let hours: i64 = parser.get("hours").unwrap_or(0);
        let weeks: i64 = parser.get("weeks").unwrap_or(0);

        let duration = Duration::weeks(weeks)
            + Duration::days(days)
            + Duration::hours(hours)
            + Duration::minutes(minutes)
            + Duration::seconds(seconds)
            + Duration::milliseconds(milliseconds)
            + Duration::microseconds(microseconds);
        Ok(PyTimeDelta::new(duration))
    }

    fn min() -> PyTimeDelta {
        PyTimeDelta::new(Duration::days(-999_999_999))
    }

    fn max() -> PyTimeDelta {
        PyTimeDelta::new(
            Duration::days(999_999_999) + Duration::seconds(86399) + Duration::microseconds(999999),
        )
    }

    fn resolution() -> PyTimeDelta {
        PyTimeDelta::new(Duration::microseconds(1))
    }
}

impl Object for PyTimeDeltaClass {
    fn call(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        Self::timedelta_new(args).map(Value::from_object)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "min" => Some(Value::from_object(Self::min())),
            "max" => Some(Value::from_object(Self::max())),
            "resolution" => Some(Value::from_object(Self::resolution())),
            _ => None,
        }
    }
}

// ----------------------------------------------------------------
// PyTimeDelta definition
// ----------------------------------------------------------------
#[derive(Clone, Debug)]
pub(crate) struct PyTimeDelta {
    pub duration: Duration,
}

impl PyTimeDelta {
    pub fn new(duration: Duration) -> Self {
        PyTimeDelta { duration }
    }

    /// Split the duration the way Python normalizes `timedelta`: the sign lives entirely in
    /// `days`, with `0 <= seconds < 86400` and `0 <= microseconds < 1_000_000`.
    ///
    /// Deliberately avoids `Duration::num_microseconds()` on the whole duration: that overflows
    /// (returns `None`) past ~292,471 years, and `timedelta.min` / `timedelta.max` are
    /// ±999,999,999 days, which is well beyond it. Only the sub-second remainder is converted to
    /// microseconds, and that always fits.
    fn normalized(&self) -> (i64, i64, i64) {
        // Truncates toward zero, so for negative durations this is the *ceiling*.
        let trunc_secs = self.duration.num_seconds();
        let sub_us = (self.duration - Duration::seconds(trunc_secs))
            .num_microseconds()
            .unwrap_or(0);

        // Convert truncation into flooring so the remainder is never negative.
        let (total_secs, micros) = if sub_us < 0 {
            (trunc_secs - 1, sub_us + 1_000_000)
        } else {
            (trunc_secs, sub_us)
        };

        (
            total_secs.div_euclid(86400),
            total_secs.rem_euclid(86400),
            micros,
        )
    }

    // Instance attributes
    pub fn days(&self) -> Option<Value> {
        Some(Value::from(self.normalized().0))
    }

    pub fn seconds(&self) -> Option<Value> {
        Some(Value::from(self.normalized().1))
    }

    pub fn microseconds(&self) -> Option<Value> {
        Some(Value::from(self.normalized().2))
    }

    /// `num_seconds()` alone truncates to whole seconds and drops the fractional part, so add the
    /// sub-second remainder back. Computed off the truncated seconds rather than `normalized()` so
    /// the f64 keeps full precision for large durations.
    fn total_seconds_f64(&self) -> f64 {
        let trunc_secs = self.duration.num_seconds();
        let sub_us = (self.duration - Duration::seconds(trunc_secs))
            .num_microseconds()
            .unwrap_or(0);
        trunc_secs as f64 + sub_us as f64 / 1_000_000.0
    }

    pub fn total_seconds(&self) -> Result<Value, Error> {
        Ok(Value::from(self.total_seconds_f64()))
    }

    // ----------------------------------------------------------------
    // __add__(rhs)
    //
    //  1) timedelta + timedelta => timedelta
    //  2) timedelta + datetime  => datetime
    //  3) timedelta + time => time
    //  4) timedelta + date => date
    //  5) otherwise error
    // ----------------------------------------------------------------
    fn add(&self, args: &[Value]) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let rhs: Value = parser.next_positional()?;

        // 1) timedelta + timedelta = timedelta
        if let Some(other_delta) = rhs.downcast_object_ref::<PyTimeDelta>() {
            let new_duration = self.duration + other_delta.duration;
            return Ok(Value::from_object(PyTimeDelta::new(new_duration)));
        }
        // 2) timedelta + datetime = datetime
        else if let Some(dt) = rhs.downcast_object_ref::<PyDateTime>() {
            match &dt.state {
                // If dt is naive, produce a naive result
                DateTimeState::Naive(ndt) => {
                    let new_naive = *ndt + self.duration;
                    return Ok(Value::from_object(PyDateTime::new_naive(new_naive)));
                }
                // If dt is aware, produce an aware result with the same Tz
                DateTimeState::Aware(adt) => {
                    let new_aware = *adt + self.duration; // chrono::DateTime<Tz> + chrono::Duration
                    return Ok(Value::from_object(PyDateTime::new_aware(
                        new_aware,
                        dt.tzinfo.clone(),
                    )));
                }
                // If dt has a fixed offset, preserve it
                DateTimeState::FixedOffset(fdt) => {
                    let new_fixed = *fdt + self.duration;
                    return Ok(Value::from_object(PyDateTime {
                        state: DateTimeState::FixedOffset(new_fixed),
                        tzinfo: dt.tzinfo.clone(),
                    }));
                }
            }
        }
        // 3) timedelta + time = time
        else if let Some(time) = rhs.downcast_object_ref::<PyTime>() {
            let new_time = time.time + self.duration;
            return Ok(Value::from_object(PyTime::new(new_time, None)));
        }
        // 4) timedelta + date = date
        else if let Some(date) = rhs.downcast_object_ref::<PyDate>() {
            let new_date = date.date + self.duration;
            return Ok(Value::from_object(PyDate::new(new_date)));
        }

        Err(Error::new(
            ErrorKind::InvalidOperation,
            "Cannot add timedelta to this type",
        ))
    }

    // ----------------------------------------------------------------
    // __sub__(rhs)
    //
    //  1) timedelta - timedelta => timedelta
    //  (In Python, there's no direct 'timedelta - datetime' or such.)
    // ----------------------------------------------------------------
    fn sub(&self, args: &[Value]) -> Result<Value, Error> {
        let mut parser = ArgParser::new(args, None);
        let rhs: Value = parser.next_positional()?;

        // 1) timedelta - timedelta = timedelta
        if let Some(other_delta) = rhs.downcast_object_ref::<PyTimeDelta>() {
            let new_duration = self.duration - other_delta.duration;
            return Ok(Value::from_object(PyTimeDelta::new(new_duration)));
        }

        // Python doesn't allow `timedelta - datetime`.
        // "datetime" - "timedelta" is fine, but that would be handled by
        // the PyDateTime's __sub__ method, not here.

        Err(Error::new(
            ErrorKind::InvalidOperation,
            "Cannot subtract this type from a timedelta",
        ))
    }
}

// ----------------------------------------------------------------
// Implementation of Object for PyTimeDelta
// ----------------------------------------------------------------
impl Object for PyTimeDelta {
    fn is_true(self: &Arc<Self>) -> bool {
        // Python: any non-zero timedelta is truthy, down to a single microsecond.
        !self.duration.is_zero()
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str()? {
            "days" => self.days(),
            "seconds" => self.seconds(),
            "microseconds" => self.microseconds(),
            "total_seconds" => Some(Value::from(
                "<built-in method total_seconds of datetime.timedelta object>",
            )),
            _ => None,
        }
    }

    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        match method {
            "__add__" => self.add(args),
            "__sub__" => self.sub(args),
            "total_seconds" => self.total_seconds(),
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("timedelta object has no method named '{method}'"),
            )),
        }
    }

    /// Mirrors CPython's `timedelta.__str__`: e.g. `"2 days, 3:30:00.123456"`, `"0:00:00"`,
    /// `"-1 day, 23:59:58.500000"`. Hours are unpadded, the microseconds suffix is omitted when
    /// zero, and the sign is carried by the (normalized) day count rather than prefixed.
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (days, seconds, microseconds) = self.normalized();
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;
        let seconds = seconds % 60;

        if days != 0 {
            let plural = if days.abs() == 1 { "" } else { "s" };
            write!(f, "{days} day{plural}, ")?;
        }
        write!(f, "{hours}:{minutes:02}:{seconds:02}")?;
        if microseconds != 0 {
            write!(f, ".{microseconds:06}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::args;
    use minijinja::Environment;
    use minijinja::Value;

    #[test]
    fn test_timedelta_creation() {
        let td = PyTimeDeltaClass::timedelta_new(&[]).unwrap();
        assert_eq!(td.duration.num_seconds(), 0);

        let td =
            PyTimeDeltaClass::timedelta_new(args!(days => 2, hours => 3, minutes => 30)).unwrap();
        assert_eq!(td.duration.num_days(), 2);
        assert_eq!(td.duration.num_hours() % 24, 3);
        assert_eq!(td.duration.num_minutes() % 60, 30);
    }

    #[test]
    fn test_timedelta_attributes() {
        let td = PyTimeDelta::new(
            Duration::days(2)
                + Duration::hours(3)
                + Duration::minutes(30)
                + Duration::microseconds(123456),
        );
        assert_eq!(td.days().unwrap().as_i64().unwrap(), 2);
        assert_eq!(td.seconds().unwrap().as_i64().unwrap(), 12600); // 3h30m = 12600s
        assert_eq!(td.microseconds().unwrap().as_i64().unwrap(), 123456);
    }

    #[test]
    fn test_timedelta_arithmetic() {
        let td1 = PyTimeDelta::new(Duration::days(2));
        let td2 = PyTimeDelta::new(Duration::days(1));
        let td1_arc = Arc::new(td1);

        // Test addition
        let binding = td1_arc.add(&[Value::from_object(td2.clone())]).unwrap();
        let result = binding.downcast_object_ref::<PyTimeDelta>().unwrap();
        assert_eq!(result.duration.num_days(), 3);

        // Test subtraction
        let binding = td1_arc.sub(&[Value::from_object(td2)]).unwrap();
        let result = binding.downcast_object_ref::<PyTimeDelta>().unwrap();
        assert_eq!(result.duration.num_days(), 1);
    }

    #[test]
    fn test_timedelta_in_template() {
        let mut env = Environment::new();
        env.add_global("timedelta", Value::from_object(PyTimeDeltaClass));

        // Test creation and attributes
        let template = env
            .template_from_str(
                "{{ timedelta(days=2, hours=3).days }}, {{ timedelta(minutes=90).seconds }}",
            )
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "2, 5400");

        // Test positional arguments creation
        let template = env.template_from_str("{{ timedelta(4).days }}").unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "4");

        // Test arithmetic
        let template = env
            .template_from_str("{{ (timedelta(days=2) + timedelta(days=1)).days }}")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "3");

        // Test the total_seconds() method
        let template = env
            .template_from_str("{{ timedelta(4).total_seconds() }}")
            .unwrap();
        let result = template.render(minijinja::context!(), &[]).unwrap();
        assert_eq!(result, "345600.0");
    }

    /// Renders `{{ td }}` / `{{ td.<attr> }}` through a template the way user Jinja would, so the
    /// assertions cover the `Object` impl rather than just the inherent methods.
    fn render_expr(expr: &str) -> String {
        let mut env = Environment::new();
        env.add_global("timedelta", Value::from_object(PyTimeDeltaClass));
        env.template_from_str(expr)
            .unwrap()
            .render(minijinja::context!(), &[])
            .unwrap()
    }

    /// dbt-labs/dbt-core#15756: `total_seconds()` truncated to whole seconds, so every sub-second
    /// duration came back as `0.0`. Expected values are CPython 3.12 `timedelta` output.
    #[test]
    fn test_total_seconds_keeps_sub_second_precision() {
        // The reported case: an 11.27 ms interval.
        assert_eq!(
            render_expr("{{ timedelta(milliseconds=11, microseconds=270).total_seconds() }}"),
            "0.01127"
        );
        // Non-zero seconds *and* a fraction — catches dropping only the fractional part, which a
        // whole-second-only case cannot distinguish from a correct implementation.
        assert_eq!(
            render_expr("{{ timedelta(seconds=2, milliseconds=500).total_seconds() }}"),
            "2.5"
        );
        // Largest sub-second value: nothing is being rounded to nearest.
        assert_eq!(
            render_expr("{{ timedelta(microseconds=999999).total_seconds() }}"),
            "0.999999"
        );
        assert_eq!(
            render_expr("{{ timedelta(microseconds=1).total_seconds() }}"),
            "0.000001"
        );
        // Whole seconds still render with the trailing `.0`, as Python does.
        assert_eq!(
            render_expr("{{ timedelta(4).total_seconds() }}"),
            "345600.0"
        );
        assert_eq!(render_expr("{{ timedelta(0).total_seconds() }}"), "0.0");
        assert_eq!(
            render_expr(
                "{{ timedelta(days=2, hours=3, minutes=30, microseconds=123456).total_seconds() }}"
            ),
            "185400.123456"
        );
        assert_eq!(
            render_expr("{{ timedelta(seconds=-1, microseconds=-500000).total_seconds() }}"),
            "-1.5"
        );
        assert_eq!(
            render_expr("{{ timedelta(seconds=-90).total_seconds() }}"),
            "-90.0"
        );
    }

    /// A sub-second duration must be truthy; `is_true` also tested whole seconds.
    #[test]
    fn test_sub_second_timedelta_is_truthy() {
        assert_eq!(
            render_expr("{{ 'yes' if timedelta(milliseconds=11) else 'no' }}"),
            "yes"
        );
        assert_eq!(
            render_expr("{{ 'yes' if timedelta(microseconds=1) else 'no' }}"),
            "yes"
        );
        assert_eq!(render_expr("{{ 'yes' if timedelta(0) else 'no' }}"), "no");
        // Still truthy when negative and sub-second.
        assert_eq!(
            render_expr("{{ 'yes' if timedelta(microseconds=-1) else 'no' }}"),
            "yes"
        );
    }

    /// Python carries the sign in `days` and keeps `seconds` / `microseconds` non-negative.
    /// Rust's `%` truncates toward zero, which produced negative values in all three fields.
    #[test]
    fn test_negative_durations_normalize_like_python() {
        let cases: &[(&str, i64, i64, i64)] = &[
            // (constructor args, days, seconds, microseconds) — from CPython 3.12
            ("seconds=-1, microseconds=-500000", -1, 86398, 500000),
            ("days=-1", -1, 0, 0),
            ("days=-2, seconds=-1", -3, 86399, 0),
            ("seconds=-90", -1, 86310, 0),
            ("microseconds=-1", -1, 86399, 999999),
            // Positive values are unaffected.
            (
                "days=2, hours=3, minutes=30, microseconds=123456",
                2,
                12600,
                123456,
            ),
            ("hours=25", 1, 3600, 0),
            ("seconds=0", 0, 0, 0),
        ];

        for (args, days, seconds, micros) in cases {
            assert_eq!(
                render_expr(&format!(
                    "{{{{ timedelta({args}).days }}}},{{{{ timedelta({args}).seconds }}}},{{{{ timedelta({args}).microseconds }}}}"
                )),
                format!("{days},{seconds},{micros}"),
                "timedelta({args})"
            );
        }
    }

    /// `str(timedelta)` parity with CPython: unpadded hours, `.microseconds` suffix only when
    /// non-zero, singular `day`, and the sign carried by the normalized day count.
    #[test]
    fn test_render_matches_python_str() {
        let cases: &[(&str, &str)] = &[
            ("0", "0:00:00"),
            ("microseconds=1", "0:00:00.000001"),
            ("milliseconds=11, microseconds=270", "0:00:00.011270"),
            ("seconds=2, milliseconds=500", "0:00:02.500000"),
            ("microseconds=999999", "0:00:00.999999"),
            (
                "seconds=-1, microseconds=-500000",
                "-1 day, 23:59:58.500000",
            ),
            (
                "days=2, hours=3, minutes=30, microseconds=123456",
                "2 days, 3:30:00.123456",
            ),
            ("days=-1", "-1 day, 0:00:00"),
            ("days=-2, seconds=-1", "-3 days, 23:59:59"),
            ("seconds=-90", "-1 day, 23:58:30"),
            ("hours=25", "1 day, 1:00:00"),
        ];

        for (args, expected) in cases {
            assert_eq!(
                render_expr(&format!("{{{{ timedelta({args}) }}}}")),
                *expected,
                "timedelta({args})"
            );
        }
    }

    /// `timedelta.min` / `timedelta.max` are ±999,999,999 days, which overflows
    /// `Duration::num_microseconds()`. The fix must not silently degrade to `0` there.
    #[test]
    fn test_extreme_constants_do_not_overflow_to_zero() {
        let max = Arc::new(PyTimeDeltaClass::max());
        assert_eq!(max.days().unwrap().as_i64().unwrap(), 999_999_999);
        assert_eq!(max.seconds().unwrap().as_i64().unwrap(), 86399);
        assert_eq!(max.microseconds().unwrap().as_i64().unwrap(), 999999);
        let max_total = max.total_seconds_f64();
        assert!(
            max_total > 8.6e13,
            "timedelta.max.total_seconds() collapsed to {max_total}"
        );

        let min = Arc::new(PyTimeDeltaClass::min());
        assert_eq!(min.days().unwrap().as_i64().unwrap(), -999_999_999);
        assert_eq!(min.seconds().unwrap().as_i64().unwrap(), 0);
        assert_eq!(min.microseconds().unwrap().as_i64().unwrap(), 0);
        assert!(min.total_seconds_f64() < -8.6e13);
    }

    /// The end-to-end shape from the issue: time an interval, then read it back both ways.
    #[test]
    fn test_datetime_difference_reports_sub_second_elapsed() {
        let t0 = chrono::NaiveDate::from_ymd_opt(2026, 7, 31)
            .unwrap()
            .and_hms_micro_opt(12, 0, 0, 0)
            .unwrap();
        let t1 = chrono::NaiveDate::from_ymd_opt(2026, 7, 31)
            .unwrap()
            .and_hms_micro_opt(12, 0, 0, 11270)
            .unwrap();
        let elapsed = Arc::new(PyTimeDelta::new(t1 - t0));

        assert_eq!(elapsed.total_seconds_f64(), 0.011270_f64);
        // The workaround users adopted must agree with the now-correct total_seconds().
        let by_hand = elapsed.seconds().unwrap().as_i64().unwrap() as f64
            + elapsed.microseconds().unwrap().as_i64().unwrap() as f64 / 1_000_000.0;
        assert_eq!(elapsed.total_seconds_f64(), by_hand);
    }

    #[test]
    fn test_timedelta_constants() {
        let min_td = PyTimeDeltaClass::min();
        assert_eq!(min_td.duration.num_days(), -999_999_999);

        let max_td = PyTimeDeltaClass::max();
        assert_eq!(max_td.duration.num_days(), 999_999_999);
        assert_eq!(max_td.duration.num_seconds() % 86400, 86399);

        let resolution = PyTimeDeltaClass::resolution();
        assert_eq!(resolution.duration.num_microseconds().unwrap(), 1);
    }
}
