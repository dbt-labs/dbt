use std::collections::BTreeMap;
use std::sync::RwLock;

use chrono::{DateTime, Local, Utc};
use minijinja::Value;

pub mod bound_method;
pub mod date;
pub mod datetime;
pub mod strptime;
pub mod time;
pub mod timedelta;
pub mod tzinfo;

/// Instant that `datetime.now()/utcnow()/today()`, `date.today()` and `time.now()` report instead
/// of the system clock. `None` (the default) means the real wall clock.
static FROZEN_NOW: RwLock<Option<DateTime<Utc>>> = RwLock::new(None);

/// Pin the wall clock seen by the `modules.datetime` Jinja module, or unpin it with `None`.
///
/// Process-wide. Time Machine replay uses this to render wall-clock-derived SQL (for example a
/// `utcnow() - timedelta(days=1)` var) exactly as it was rendered when the recording was made.
pub fn freeze_now(at: Option<DateTime<Utc>>) {
    // A poisoned lock still holds a valid `Option<DateTime>`; keep using it.
    *FROZEN_NOW.write().unwrap_or_else(|e| e.into_inner()) = at;
}

/// Current instant: the pinned one if `freeze_now` was called, otherwise the system clock.
pub(crate) fn now_utc() -> DateTime<Utc> {
    let frozen = *FROZEN_NOW.read().unwrap_or_else(|e| e.into_inner());
    match frozen {
        Some(at) => at,
        None => Utc::now(),
    }
}

/// `now_utc` in the system's local timezone.
pub(crate) fn now_local() -> DateTime<Local> {
    now_utc().with_timezone(&Local)
}

/// Byte index of the first `%f` conversion in a `strftime` format string, or
/// `None` if it contains none.
///
/// Python's `%f` is six zero-padded microsecond digits, while chrono reads `%f`
/// as nine nanosecond digits. The `strftime` implementations therefore split the
/// format string on `%f` and substitute the microseconds themselves rather than
/// letting chrono handle it. Escaped percents are skipped, so `%%f` is the
/// literal text `%f` and not a conversion, matching Python.
pub(crate) fn find_microsecond_directive(fmt: &str) -> Option<usize> {
    let bytes = fmt.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'%' {
            if bytes[i + 1] == b'f' {
                return Some(i);
            }
            // Any other `%X` (including `%%`) consumes both bytes, so the `f` in
            // `%%f` is never mistaken for a conversion.
            i += 2;
        } else {
            i += 1;
        }
    }
    None
}

pub fn create_datetime_module() -> BTreeMap<String, Value> {
    let mut datetime_module = BTreeMap::new();

    datetime_module.insert(
        "datetime".to_string(),
        Value::from_object(datetime::PyDateTimeClass),
    );
    datetime_module.insert("date".to_string(), Value::from_object(date::PyDateClass));
    datetime_module.insert("time".to_string(), Value::from_object(time::PyTimeClass));
    datetime_module.insert(
        "timedelta".to_string(),
        Value::from_object(timedelta::PyTimeDeltaClass),
    );
    datetime_module.insert(
        "tzinfo".to_string(),
        Value::from_object(tzinfo::PyTzInfoClass),
    );
    datetime_module.insert(
        "timezone".to_string(),
        Value::from_object(tzinfo::PyTimezoneClass),
    );
    datetime_module
}

#[cfg(test)]
mod tests {
    use super::find_microsecond_directive;

    #[test]
    fn test_find_microsecond_directive_skips_escaped_percent() {
        assert_eq!(find_microsecond_directive("%f"), Some(0));
        assert_eq!(find_microsecond_directive("%S%f"), Some(2));
        assert_eq!(find_microsecond_directive("%Y-%m-%dT%H:%M:%S.%f"), Some(18));
        // `%%` is a literal percent, so the `f` after it is plain text.
        assert_eq!(find_microsecond_directive("%%f"), None);
        // ...but a real conversion after an escaped one is still found.
        assert_eq!(find_microsecond_directive("%%f%f"), Some(3));
        // chrono's `%.6f` is a different conversion and must be left to chrono.
        assert_eq!(find_microsecond_directive("%.6f"), None);
        assert_eq!(find_microsecond_directive("no directives"), None);
        // A trailing bare `%` is not a conversion and must not index past the end.
        assert_eq!(find_microsecond_directive("%S%"), None);
        assert_eq!(find_microsecond_directive(""), None);
    }
}
