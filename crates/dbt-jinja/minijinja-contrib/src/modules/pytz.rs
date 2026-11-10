mod timezones;

use chrono::{DateTime, LocalResult, NaiveDateTime, TimeZone};
use chrono_tz::{Tz, TZ_VARIANTS};
use minijinja::arg_utils::ArgsIter;
use minijinja::value::mutable_set::MutableSet;
use minijinja::value::mutable_vec::MutableVec;
use minijinja::value::{Enumerator, Object, ObjectRepr};
use minijinja::{Error, ErrorKind, Value};
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, OnceLock};
use timezones::{common_timezones, country_names, country_timezones};

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

    /// Return an offset from UTC as a `chrono::Duration` for the given local DateTime in this tz.
    pub fn utcoffset(&self, _local_dt: &DateTime<Tz>) -> Value {
        // Get the seconds offset directly from the datetime's offset
        Value::from("TODO IMPLEMENT")
    }

    /// Return the DST offset if any. If you just want to return None or 0, do so.
    pub fn dst(&self, _local_dt: &DateTime<Tz>) -> Option<chrono::Duration> {
        // Real DST logic is more nuanced, and Chrono doesn't directly expose it.
        // If you prefer to always return None or Some(0), do so:
        None
    }

    /// Return the timezone name for the given local DateTime. This might be "EST", "EDT", or a raw offset, etc.
    pub fn tzname(&self, local_dt: &DateTime<Tz>) -> String {
        local_dt.offset().to_string()
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
    pytz_module.insert(
        "all_timezones".to_string(),
        Value::from_object(TimezoneList(TimezoneNames::All)),
    );
    pytz_module.insert(
        "all_timezones_set".to_string(),
        Value::from_object(TimezoneSet::new(TimezoneNames::All)),
    );
    pytz_module.insert(
        "common_timezones".to_string(),
        Value::from_object(TimezoneList(TimezoneNames::Common)),
    );
    pytz_module.insert(
        "common_timezones_set".to_string(),
        Value::from_object(TimezoneSet::new(TimezoneNames::Common)),
    );
    pytz_module.insert(
        "country_timezones".to_string(),
        Value::from_object(CountryMap::Timezones),
    );
    pytz_module.insert(
        "country_names".to_string(),
        Value::from_object(CountryMap::Names),
    );

    pytz_module
}

/// The static name lists behind `pytz.all_timezones` and `pytz.common_timezones`.
/// `TZ_VARIANTS` is already alphabetized like the pytz list.
#[derive(Debug, Clone, Copy)]
enum TimezoneNames {
    All,
    Common,
}

impl TimezoneNames {
    fn len(self) -> usize {
        match self {
            TimezoneNames::All => TZ_VARIANTS.len(),
            TimezoneNames::Common => common_timezones().len(),
        }
    }

    fn get(self, index: usize) -> Option<&'static str> {
        match self {
            TimezoneNames::All => TZ_VARIANTS.get(index).map(|tz| tz.name()),
            TimezoneNames::Common => common_timezones().get(index).copied(),
        }
    }
}

/// `pytz.all_timezones` and `pytz.common_timezones`: a read-only list view over a static list.
///
/// XXX: This implementation avoids copying the static list into a new list, but it
/// does mean that the list is read-only and does not support methods like `.pop()`.
/// Since these are pytz constants, we're considering this a preferable tradeoff.
/// If users end up needing to mutate `pytz.all_timezones` or `pytz.common_timezones`
/// for some reason, we can swap this implementation to match `TimezoneSet`.
#[derive(Debug)]
struct TimezoneList(TimezoneNames);

impl Object for TimezoneList {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    // Reported as mutable so that it renders and concatenates as a list, not a tuple.
    fn is_mutable(self: &Arc<Self>) -> bool {
        true
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.0.get(key.as_usize()?).map(Value::from)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.0.len())
    }
}

/// `pytz.all_timezones_set` and `pytz.common_timezones_set`: like pytz, the set is
/// only populated once it is used.
#[derive(Debug)]
struct TimezoneSet {
    names: TimezoneNames,
    set: OnceLock<Arc<MutableSet>>,
}

impl TimezoneSet {
    fn new(names: TimezoneNames) -> Self {
        TimezoneSet {
            names,
            set: OnceLock::new(),
        }
    }

    fn set(&self) -> &Arc<MutableSet> {
        self.set.get_or_init(|| {
            Arc::new(
                (0..self.names.len())
                    .filter_map(|index| self.names.get(index))
                    .collect(),
            )
        })
    }
}

impl Object for TimezoneSet {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn is_mutable(self: &Arc<Self>) -> bool {
        true
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        self.set().enumerate()
    }

    fn call_method(
        self: &Arc<Self>,
        state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.set().call_method(state, method, args, listeners)
    }
}

/// `pytz.country_timezones` and `pytz.country_names`: read-only maps keyed by ISO 3166
/// country code. Like pytz, a lookup uppercases the code.
#[derive(Debug)]
enum CountryMap {
    Timezones,
    Names,
}

impl CountryMap {
    fn lookup(&self, code: &str) -> Option<Value> {
        let code = code.to_ascii_uppercase();
        match self {
            CountryMap::Timezones => country_timezones().get(code.as_str()).map(|zones| {
                Value::from_object(MutableVec::from(
                    zones.iter().copied().map(Value::from).collect::<Vec<_>>(),
                ))
            }),
            CountryMap::Names => country_names()
                .get(code.as_str())
                .map(|name| Value::from(*name)),
        }
    }
}

impl Object for CountryMap {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.lookup(key.as_str()?)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        match **self {
            CountryMap::Timezones => Enumerator::Iter(Box::new(
                country_timezones().keys().map(|code| Value::from(*code)),
            )),
            CountryMap::Names => Enumerator::Iter(Box::new(
                country_names().keys().map(|code| Value::from(*code)),
            )),
        }
    }

    // `country_timezones('us')` is the older pytz spelling of `country_timezones['us']`.
    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        if let CountryMap::Names = **self {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "object is not callable",
            ));
        }
        let iter = ArgsIter::new("country_timezones", &["iso3166_code"], args);
        let code = iter.next_arg::<&str>()?;
        iter.finish()?;
        self.lookup(code).ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidArgument,
                format!("Unknown country code: {code}"),
            )
        })
    }
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
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("Timezone object has no method named '{method}'"),
            )),
        }
    }

    // pytz keeps one timezone object per zone name, so two timezones are equal
    // only when they name the same zone: `US/Eastern` != `America/New_York`.
    fn custom_cmp(
        self: &Arc<Self>,
        other: &minijinja::value::DynObject,
    ) -> Option<std::cmp::Ordering> {
        other
            .downcast_ref::<Self>()
            .map(|other| self.tz.name().cmp(other.tz.name()))
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
    use std::collections::HashSet;

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

    fn render_pytz_expression(expression: &str) -> String {
        let mut env = minijinja::Environment::new();
        env.add_global("pytz", Value::from(create_pytz_namespace()));
        let source = format!("{{{{ {expression} }}}}");
        let template = env.template_from_str(&source).unwrap();
        template.render(minijinja::context!(), &[]).unwrap()
    }

    #[test]
    fn test_timezones_with_same_name_are_equal() {
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('America/New_York') == pytz.timezone('America/New_York')"
            ),
            "True"
        );
        // Lookup is case-insensitive, so both spellings are the same zone
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('america/new_york') == pytz.timezone('America/New_York')"
            ),
            "True"
        );
        assert_eq!(
            render_pytz_expression("pytz.timezone('UTC') == pytz.utc"),
            "True"
        );
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('America/New_York') != pytz.timezone('America/New_York')"
            ),
            "False"
        );
    }

    #[test]
    fn test_timezones_for_different_zones_are_not_equal() {
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('America/New_York') == pytz.timezone('Europe/Paris')"
            ),
            "False"
        );
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('America/New_York') != pytz.timezone('Europe/Paris')"
            ),
            "True"
        );
    }

    #[test]
    fn test_timezones_with_alias_names_are_not_equal() {
        // pytz keeps one object per name, so an alias is a different timezone
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('US/Eastern') == pytz.timezone('America/New_York')"
            ),
            "False"
        );
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('US/Eastern') != pytz.timezone('America/New_York')"
            ),
            "True"
        );
        assert_eq!(
            render_pytz_expression("pytz.timezone('Etc/UTC') == pytz.utc"),
            "False"
        );
    }

    #[test]
    fn test_timezone_membership_compares_zones() {
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('America/New_York') in [pytz.timezone('America/New_York')]"
            ),
            "True"
        );
        assert_eq!(
            render_pytz_expression(
                "pytz.timezone('US/Eastern') in [pytz.timezone('America/New_York')]"
            ),
            "False"
        );
    }

    #[test]
    fn test_tz_variants_are_alphabetized() {
        // `all_timezones` exposes `TZ_VARIANTS` as is and relies on this order.
        assert!(TZ_VARIANTS.windows(2).all(|w| w[0].name() < w[1].name()));
    }

    #[test]
    fn test_timezone_sets_are_lazy() {
        for (names, len) in [
            (TimezoneNames::All, TZ_VARIANTS.len()),
            (TimezoneNames::Common, common_timezones().len()),
        ] {
            let lazy = Arc::new(TimezoneSet::new(names));
            assert!(lazy.set.get().is_none());

            assert_eq!(lazy.enumerator_len(), Some(len));
            assert!(lazy.set.get().is_some());
        }
    }

    #[test]
    fn test_common_timezones_are_sorted_known_zones() {
        assert_eq!(common_timezones().len(), 433);
        assert!(common_timezones().windows(2).all(|w| w[0] < w[1]));
        let all: HashSet<&str> = TZ_VARIANTS.iter().map(|tz| tz.name()).collect();
        let unknown: Vec<&str> = common_timezones()
            .iter()
            .copied()
            .filter(|name| !all.contains(name))
            .collect();
        assert!(unknown.is_empty(), "not in TZ_VARIANTS: {unknown:?}");
    }

    #[test]
    fn test_country_timezones_are_known_zones() {
        let all: HashSet<&str> = TZ_VARIANTS.iter().map(|tz| tz.name()).collect();
        let unknown: Vec<&str> = country_timezones()
            .values()
            .flatten()
            .copied()
            .filter(|name| !all.contains(name))
            .collect();
        assert!(unknown.is_empty(), "not in TZ_VARIANTS: {unknown:?}");
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
}
