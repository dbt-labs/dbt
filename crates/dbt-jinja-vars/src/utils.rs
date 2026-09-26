use std::collections::BTreeMap;

use chrono::{FixedOffset, NaiveDate, NaiveTime, TimeZone as _};
use dbt_yaml::Value as YmlValue;
use minijinja::Value as MinijinjaValue;
use minijinja_contrib::modules::py_datetime::{date::PyDate, datetime::PyDateTime};

/// Converts a resolved YAML 1.1 timestamp into a Jinja date or datetime
/// object, matching dbt Core. A date-only timestamp becomes a `date`, a
/// timestamp with a zone suffix becomes an aware `datetime`, and any other
/// becomes a naive `datetime`. Falls back to the canonical string form if a
/// component is out of range.
pub fn yml_timestamp_to_minijinja(timestamp: &dbt_yaml::Timestamp) -> MinijinjaValue {
    let (year, month, day) = timestamp.date();
    let Some(date) = NaiveDate::from_ymd_opt(year, month as u32, day as u32) else {
        return MinijinjaValue::from(timestamp.to_string());
    };

    let Some(time) = timestamp.time() else {
        return MinijinjaValue::from_object(PyDate::new(date));
    };
    let Some(time) = NaiveTime::from_hms_nano_opt(
        time.hour as u32,
        time.minute as u32,
        time.second as u32,
        time.nanosecond,
    ) else {
        return MinijinjaValue::from(timestamp.to_string());
    };
    let datetime = date.and_time(time);

    match timestamp.tz_minutes() {
        Some(minutes) => match FixedOffset::east_opt(minutes * 60) {
            Some(offset) => MinijinjaValue::from_object(PyDateTime::new_fixed_offset(
                offset.from_local_datetime(&datetime).unwrap(),
            )),
            None => MinijinjaValue::from(timestamp.to_string()),
        },
        None => MinijinjaValue::from_object(PyDateTime::new_naive(datetime)),
    }
}

/// Convert YmlValue to minijinja::Value
///
/// Unlike a direct `Deserialize` conversion, this method is able to preserve
/// `Timestamp` typed values.
///
/// TODO: make this the default behavior for `YmlValue` -> `MinijinjaValue`
/// deserialize conversions.
pub fn yml_value_to_minijinja(value: &YmlValue) -> MinijinjaValue {
    match value {
        YmlValue::Null(_) => MinijinjaValue::from(None::<()>),
        YmlValue::Bool(b, _) => MinijinjaValue::from(*b),
        YmlValue::String(s, _) => MinijinjaValue::from(s),
        YmlValue::Number(n, _) => {
            if let Some(i) = n.as_i64() {
                MinijinjaValue::from(i)
            } else if let Some(f) = n.as_f64() {
                MinijinjaValue::from(f)
            } else {
                MinijinjaValue::from(n.to_string())
            }
        }
        YmlValue::Sequence(seq, _) => {
            let items: Vec<MinijinjaValue> = seq.iter().map(yml_value_to_minijinja).collect();
            MinijinjaValue::from(items)
        }
        YmlValue::Mapping(map, _) => {
            let result = map
                .iter()
                .filter_map(|(k, v)| match k {
                    YmlValue::String(key, _) => Some((key.to_string(), yml_value_to_minijinja(v))),
                    _ => None,
                })
                .collect::<BTreeMap<String, MinijinjaValue>>();
            MinijinjaValue::from_object(result)
        }
        YmlValue::Tagged(tagged, _) => {
            // For tagged values, convert the inner value
            yml_value_to_minijinja(&tagged.value)
        }
        YmlValue::Timestamp(timestamp, _) => yml_timestamp_to_minijinja(timestamp),
    }
}
