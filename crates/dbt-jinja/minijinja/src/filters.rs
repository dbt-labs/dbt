//! Filter functions and abstractions.
//!
//! MiniJinja inherits from Jinja2 the concept of filter functions.  These are functions
//! which are applied to values to modify them.  For example the expression `{{ 42|filter(23) }}`
//! invokes the filter `filter` with the arguments `42` and `23`.
//!
//! MiniJinja comes with some built-in filters that are listed below. To create a
//! custom filter write a function that takes at least a value, then registers it
//! with [`add_filter`](crate::Environment::add_filter).
//!
//! # Using Filters
//!
//! Using filters in templates is possible in all places an expression is permitted.
//! This means they are not just used for printing but also are useful for iteration
//! or similar situations.
//!
//! Motivating example:
//!
//! ```jinja
//! <dl>
//! {% for key, value in config|items %}
//!   <dt>{{ key }}
//!   <dd><pre>{{ value|tojson }}</pre>
//! {% endfor %}
//! </dl>
//! ```
//!
//! # Custom Filters
//!
//! A custom filter is just a simple function which accepts its inputs
//! as parameters and then returns a new value.  For instance the following
//! shows a filter which takes an input value and replaces whitespace with
//! dashes and converts it to lowercase:
//!
//! ```
//! # use minijinja::Environment;
//! # let mut env = Environment::new();
//! fn slugify(value: String) -> String {
//!     value.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
//! }
//!
//! env.add_filter("slugify", slugify);
//! ```
//!
//! MiniJinja will perform the necessary conversions automatically.  For more
//! information see the [`Filter`] trait.
//!
//! # Accessing State
//!
//! In some cases it can be necessary to access the execution [`State`].  Since a borrowed
//! state implements [`ArgType`] it's possible to add a parameter that holds the state.
//! For instance the following filter appends the current template name to the string:
//!
//! ```
//! # use minijinja::Environment;
//! # let mut env = Environment::new();
//! use minijinja::{Value, State};
//!
//! fn append_template(state: &State, value: &Value) -> String {
//!     format!("{}-{}", value, state.name())
//! }
//!
//! env.add_filter("append_template", append_template);
//! ```
//!
//! # Filter configuration
//!
//! The recommended pattern for filters to change their behavior is to leverage global
//! variables in the template.  For instance take a filter that performs date formatting.
//! You might want to change the default time format format on a per-template basis
//! without having to update every filter invocation.  In this case the recommended
//! pattern is to reserve upper case variables and look them up in the filter:
//!
//! ```
//! # use minijinja::Environment;
//! # let mut env = Environment::new();
//! # fn format_unix_timestamp(_: f64, _: &str) -> String { "".into() }
//! use minijinja::State;
//!
//! fn timeformat(state: &State, ts: f64) -> String {
//!     let configured_format = state.lookup("TIME_FORMAT");
//!     let format = configured_format
//!         .as_ref()
//!         .and_then(|x| x.as_str())
//!         .unwrap_or("HH:MM:SS");
//!     format_unix_timestamp(ts, format)
//! }
//!
//! env.add_filter("timeformat", timeformat);
//! ```
//!
//! This then later lets a user override the default either by using
//! [`add_global`](crate::Environment::add_global) or by passing it with the
//! [`context!`] macro or similar.
//!
//! ```
//! # use minijinja::context;
//! # let other_variables = context!{};
//! let ctx = context! {
//!     TIME_FORMAT => "HH:MM",
//!     ..other_variables
//! };
//! ```
//!
//! # Built-in Filters
//!
//! When the `builtins` feature is enabled a range of built-in filters are
//! automatically added to the environment.  These are also all provided in
//! this module.  Note though that these functions are not to be
//! called from Rust code as their exact interface (arguments and return types)
//! might change from one MiniJinja version to another.
//!
//! Some additional filters are available in the
//! [`minijinja-contrib`](https://crates.io/crates/minijinja-contrib) crate.
use std::sync::Arc;

use crate::error::Error;
use crate::utils::{write_escaped, SealedMarker};
use crate::value::{mutable_vec, ArgType, FunctionArgs, FunctionResult, Value};
use crate::vm::State;
use crate::{AutoEscape, Output};

type FilterFunc = dyn Fn(&State, &[Value]) -> Result<Value, Error> + Sync + Send + 'static;

#[derive(Clone)]
pub(crate) struct BoxedFilter(Arc<FilterFunc>);

/// A utility trait that represents filters.
///
/// This trait is used by the [`add_filter`](crate::Environment::add_filter) method to abstract over
/// different types of functions that implement filters.  Filters are functions
/// which at the very least accept the [`State`] by reference as first parameter
/// and the value that that the filter is applied to as second.  Additionally up to
/// 4 further parameters are supported.
///
/// A filter can return any of the following types:
///
/// * `Rv` where `Rv` implements `Into<Value>`
/// * `Result<Rv, Error>` where `Rv` implements `Into<Value>`
///
/// Filters accept one mandatory parameter which is the value the filter is
/// applied to and up to 4 extra parameters.  The extra parameters can be
/// marked optional by using `Option<T>`.  The last argument can also use
/// [`Rest<T>`](crate::value::Rest) to capture the remaining arguments.  All
/// types are supported for which [`ArgType`] is implemented.
///
/// For a list of built-in filters see [`filters`](crate::filters).
///
/// # Basic Example
///
/// ```
/// # use minijinja::Environment;
/// # let mut env = Environment::new();
/// use minijinja::State;
///
/// fn slugify(value: String) -> String {
///     value.to_lowercase().split_whitespace().collect::<Vec<_>>().join("-")
/// }
///
/// env.add_filter("slugify", slugify);
/// ```
///
/// ```jinja
/// {{ "Foo Bar Baz"|slugify }} -> foo-bar-baz
/// ```
///
/// # Arguments and Optional Arguments
///
/// ```
/// # use minijinja::Environment;
/// # let mut env = Environment::new();
/// fn substr(value: String, start: u32, end: Option<u32>) -> String {
///     let end = end.unwrap_or(value.len() as _);
///     value.get(start as usize..end as usize).unwrap_or_default().into()
/// }
///
/// env.add_filter("substr", substr);
/// ```
///
/// ```jinja
/// {{ "Foo Bar Baz"|substr(4) }} -> Bar Baz
/// {{ "Foo Bar Baz"|substr(4, 7) }} -> Bar
/// ```
///
/// # Variadic
///
/// ```
/// # use minijinja::Environment;
/// # let mut env = Environment::new();
/// use minijinja::value::Rest;
///
/// fn pyjoin(joiner: String, values: Rest<String>) -> String {
///     values.join(&joiner)
/// }
///
/// env.add_filter("pyjoin", pyjoin);
/// ```
///
/// ```jinja
/// {{ "|".join(1, 2, 3) }} -> 1|2|3
/// ```
pub trait Filter<Rv, Args>: Send + Sync + 'static {
    /// Applies a filter to value with the given arguments.
    ///
    /// The value is always the first argument.
    #[doc(hidden)]
    fn apply_to(&self, args: Args, _: SealedMarker) -> Rv;
}

macro_rules! tuple_impls {
    ( $( $name:ident )* ) => {
        impl<Func, Rv, $($name),*> Filter<Rv, ($($name,)*)> for Func
        where
            Func: Fn($($name),*) -> Rv + Send + Sync + 'static,
            Rv: FunctionResult,
            $($name: for<'a> ArgType<'a>,)*
        {
            fn apply_to(&self, args: ($($name,)*), _: SealedMarker) -> Rv {
                #[allow(non_snake_case)]
                let ($($name,)*) = args;
                (self)($($name,)*)
            }
        }
    };
}

tuple_impls! {}
tuple_impls! { A }
tuple_impls! { A B }
tuple_impls! { A B C }
tuple_impls! { A B C D }
tuple_impls! { A B C D E }
tuple_impls! { A B C D E F }

impl BoxedFilter {
    /// Creates a new boxed filter.
    pub fn new<F, Rv, Args>(f: F) -> BoxedFilter
    where
        F: Filter<Rv, Args> + for<'a> Filter<Rv, <Args as FunctionArgs<'a>>::Output>,
        Rv: FunctionResult,
        Args: for<'a> FunctionArgs<'a>,
    {
        BoxedFilter(Arc::new(move |state, args| -> Result<Value, Error> {
            f.apply_to(ok!(Args::from_values(Some(state), args)), SealedMarker)
                .into_result()
        }))
    }

    /// Applies the filter to a value and argument.
    pub fn apply_to(&self, state: &State, args: &[Value]) -> Result<Value, Error> {
        (self.0)(state, args)
    }
}

/// Marks a value as safe.  This converts it into a string.
///
/// When a value is marked as safe, no further auto escaping will take place.
pub fn safe(v: String) -> Value {
    Value::from_safe_string(v)
}

/// Escapes a string.  By default to HTML.
///
/// By default this filter is also registered under the alias `e`.  Note that
/// this filter escapes with the format that is native to the format or HTML
/// otherwise.  This means that if the auto escape setting is set to
/// `Json` for instance then this filter will serialize to JSON instead.
pub fn escape(state: &State, v: &Value) -> Result<Value, Error> {
    if v.is_safe() {
        return Ok(v.clone());
    }

    // this tries to use the escaping flag of the current scope, then
    // of the initial state and if that is also not set it falls back
    // to HTML.
    let auto_escape = match state.auto_escape() {
        AutoEscape::None => match state.env().initial_auto_escape(state.name()) {
            AutoEscape::None => AutoEscape::Html,
            other => other,
        },
        other => other,
    };
    let mut rv = match v.as_str() {
        Some(s) => String::with_capacity(s.len()),
        None => String::new(),
    };
    let mut out = Output::with_string(&mut rv);
    ok!(write_escaped(&mut out, auto_escape, v));
    Ok(Value::from_safe_string(rv))
}

#[cfg(feature = "builtins")]
mod builtins {
    use super::*;

    use crate::arg_utils::ArgParser;
    use crate::error::ErrorKind;
    use crate::utils::splitn_whitespace;
    use crate::value::mutable_vec::MutableVec;
    use crate::value::ops::{self, as_f64};
    use crate::value::{Enumerator, Kwargs, Object, ObjectRepr, Rest, ValueKind, ValueRepr};
    use std::borrow::Cow;
    use std::cmp::Ordering;
    use std::fmt::Write;
    use std::mem;

    /// Converts a value to uppercase.
    ///
    /// ```jinja
    /// <h1>{{ chapter.title|upper }}</h1>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn upper(v: Cow<'_, str>) -> String {
        v.to_uppercase()
    }

    /// Converts a value to lowercase.
    ///
    /// ```jinja
    /// <h1>{{ chapter.title|lower }}</h1>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn lower(v: Cow<'_, str>) -> String {
        v.to_lowercase()
    }

    /// Converts a value to title case.
    ///
    /// ```jinja
    /// <h1>{{ chapter.title|title }}</h1>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn title(v: Cow<'_, str>) -> String {
        let mut rv = String::new();
        let mut capitalize = true;
        for c in v.chars() {
            if c.is_ascii_punctuation() || c.is_whitespace() {
                rv.push(c);
                capitalize = true;
            } else if capitalize {
                write!(rv, "{}", c.to_uppercase()).unwrap();
                capitalize = false;
            } else {
                write!(rv, "{}", c.to_lowercase()).unwrap();
            }
        }
        rv
    }

    /// Convert the string with all its characters lowercased
    /// apart from the first char which is uppercased.
    ///
    /// ```jinja
    /// <h1>{{ chapter.title|capitalize }}</h1>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn capitalize(text: Cow<'_, str>) -> String {
        let mut chars = text.chars();
        match chars.next() {
            None => String::new(),
            Some(f) => f.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        }
    }

    /// Does a string replace.
    ///
    /// It replaces all occurrences of the first parameter with the second.
    /// If count is provided, only replaces up to that many occurrences.
    ///
    /// ```jinja
    /// {{ "Hello Hello World"|replace("Hello", "Goodbye", 1) }}
    ///   -> Goodbye Hello World
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn replace(
        _state: &State,
        v: Cow<'_, str>,
        from: Cow<'_, str>,
        to: Cow<'_, str>,
        count: Option<usize>,
    ) -> String {
        if let Some(n) = count {
            let mut result = v.into_owned();
            let mut replaced = 0;
            while let Some(pos) = result[replaced..].find(&from as &str) {
                if replaced >= n {
                    break;
                }
                let absolute_pos = replaced + pos;
                result.replace_range(absolute_pos..absolute_pos + from.len(), &to);
                replaced += 1;
            }
            result
        } else {
            v.replace(&from as &str, &to as &str)
        }
    }

    /// Returns the "length" of the value
    ///
    /// By default this filter is also registered under the alias `count`.
    ///
    /// ```jinja
    /// <p>Search results: {{ results|length }}
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn length(v: &Value) -> Result<usize, Error> {
        if v.is_undefined() || v.is_none() {
            return Ok(0);
        }
        v.len().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("cannot calculate length of value of type {}", v.kind()),
            )
        })
    }

    fn cmp_helper(a: &Value, b: &Value, case_sensitive: bool) -> Ordering {
        if !case_sensitive {
            if let (Some(a), Some(b)) = (a.as_str(), b.as_str()) {
                #[cfg(feature = "unicode")]
                {
                    return unicase::UniCase::new(a).cmp(&unicase::UniCase::new(b));
                }
                #[cfg(not(feature = "unicode"))]
                {
                    return a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase());
                }
            }
        }
        a.cmp(b)
    }

    /// Undefined operands reach `dictsort` / `items` when a parse-phase template chains
    /// through a missing attribute. `handle_undefined(true)` keeps Lenient and Strict erroring.
    fn undefined_pair_list(state: &State) -> Result<Value, Error> {
        ok!(state
            .undefined_behavior()
            .handle_undefined(true)
            .map_err(|err| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    "cannot convert value into pair list",
                )
                .with_source(err)
            }));
        Ok(Value::from(MutableVec::<Value>::new()))
    }

    /// Dict sorting functionality.
    ///
    /// This filter works like `|items` but sorts the pairs by key first.
    ///
    /// The filter accepts a few keyword arguments:
    ///
    /// * `case_sensitive`: set to `true` to make the sorting of strings case sensitive.
    /// * `by`: set to `"value"` to sort by value. Defaults to `"key"`.
    /// * `reverse`: set to `true` to sort in reverse.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn dictsort(
        state: &State,
        v: &Value,
        case_sensitive: Option<bool>,
        by_value: Option<Cow<'_, str>>,
        reverse: Option<bool>,
        kwargs: Kwargs,
    ) -> Result<Value, Error> {
        if v.kind() != ValueKind::Map {
            if v.is_undefined() {
                return undefined_pair_list(state);
            }
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "cannot convert value into pair list",
            ));
        }

        let by_value = matches!(
            ok!(kwargs.get::<Option<&str>>("by"))
                .or(by_value.as_deref())
                .unwrap_or("key"),
            "value"
        );
        let case_sensitive = ok!(kwargs.get::<Option<bool>>("case_sensitive"))
            .or(case_sensitive)
            .unwrap_or(false);
        let mut rv: Vec<_> = ok!(v.try_iter())
            .map(|key| (key.clone(), v.get_item(&key).unwrap_or(Value::UNDEFINED)))
            .collect();
        rv.sort_by(|a, b| {
            let (a, b) = if by_value { (&a.1, &b.1) } else { (&a.0, &b.0) };
            cmp_helper(a, b, case_sensitive)
        });
        if ok!(kwargs.get::<Option<bool>>("reverse"))
            .or(reverse)
            .unwrap_or(false)
        {
            rv.reverse();
        }
        kwargs.assert_all_used()?;
        // Always return a list (like Jinja2's dictsort)
        Ok(Value::from(
            rv.into_iter()
                .map(|(k, v)| Value::from(vec![k, v]))
                .collect::<MutableVec<_>>(),
        ))
    }

    /// Returns a list of pairs (items) from a mapping.
    ///
    /// This can be used to iterate over keys and values of a mapping
    /// at once.  Note that this will use the original order of the map
    /// which is typically arbitrary unless the `preserve_order` feature
    /// is used in which case the original order of the map is retained.
    /// It's generally better to use `|dictsort` which sorts the map by
    /// key before iterating.
    ///
    /// ```jinja
    /// <dl>
    /// {% for key, value in my_dict|items %}
    ///   <dt>{{ key }}
    ///   <dd>{{ value }}
    /// {% endfor %}
    /// </dl>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn items(state: &State, v: &Value) -> Result<Value, Error> {
        if v.kind() == ValueKind::Map {
            let rv = MutableVec::with_capacity(v.len().unwrap_or(0));
            let iter = ok!(v.try_iter());
            for key in iter {
                let value = v.get_item(&key).unwrap_or(Value::UNDEFINED);
                rv.push(Value::from(tuple![key, value]));
            }
            Ok(Value::from(rv))
        } else if v.is_undefined() {
            undefined_pair_list(state)
        } else {
            Err(Error::new(
                ErrorKind::InvalidOperation,
                "cannot convert value into pair list",
            ))
        }
    }

    /// Reverses an iterable or string
    ///
    /// ```jinja
    /// {% for user in users|reverse %}
    ///   <li>{{ user.name }}
    /// {% endfor %}
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn reverse(v: &Value) -> Result<Value, Error> {
        v.reverse()
    }

    /// Trims a value
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn trim(s: Cow<'_, str>, chars: Option<Cow<'_, str>>) -> String {
        match chars {
            Some(chars) => {
                let chars = chars.chars().collect::<Vec<_>>();
                s.trim_matches(&chars[..]).to_string()
            }
            None => s.trim().to_string(),
        }
    }

    /// Joins a sequence by a character
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn join(
        val: &Value,
        joiner: Option<Cow<'_, str>>,
        kwargs: Kwargs,
    ) -> Result<String, Error> {
        if val.is_undefined() || val.is_none() {
            return Ok(String::new());
        }

        let joiner = joiner.as_ref().unwrap_or(&Cow::Borrowed(""));
        let attr = ok!(kwargs.get::<Option<&str>>("attribute"));
        ok!(kwargs.assert_all_used());

        let iter = ok!(val.try_iter().map_err(|err| {
            Error::new(
                ErrorKind::InvalidOperation,
                format!("cannot join value of type {}", val.kind()),
            )
            .with_source(err)
        }));

        let mut rv = String::new();
        for item in iter {
            if !rv.is_empty() {
                rv.push_str(joiner);
            }

            let value_to_join = if let Some(attr) = attr {
                item.get_path_or_default(attr, &Value::UNDEFINED)
            } else {
                item
            };

            if let Some(s) = value_to_join.as_str() {
                rv.push_str(s);
            } else {
                write!(rv, "{value_to_join}").ok();
            }
        }
        Ok(rv)
    }

    /// Split a string into its substrings, using `split` as the separator string.
    ///
    /// If `split` is not provided or `none` the string is split at all whitespace
    /// characters and multiple spaces and empty strings will be removed from the
    /// result.
    ///
    /// The `maxsplits` parameter defines the maximum number of splits
    /// (starting from the left).  Note that this follows Python conventions
    /// rather than Rust ones so `1` means one split and two resulting items.
    ///
    /// ```jinja
    /// {{ "hello world"|split|list }}
    ///     -> ["hello", "world"]
    ///
    /// {{ "c,s,v"|split(",")|list }}
    ///     -> ["c", "s", "v"]
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn split(s: Arc<str>, split: Option<Arc<str>>, maxsplits: Option<i64>) -> Value {
        let maxsplits = maxsplits.and_then(|x| if x >= 0 { Some(x as usize + 1) } else { None });

        Value::make_object_iterable((s, split), move |(s, split)| match (split, maxsplits) {
            (None, None) => Box::new(s.split_whitespace().map(Value::from)),
            (Some(split), None) => Box::new(s.split(split as &str).map(Value::from)),
            (None, Some(n)) => Box::new(splitn_whitespace(s, n).map(Value::from)),
            (Some(split), Some(n)) => Box::new(s.splitn(n, split as &str).map(Value::from)),
        })
    }

    /// Splits a string into lines.
    ///
    /// The newline character is removed in the process and not retained.  This
    /// function supports both Windows and UNIX style newlines.
    ///
    /// ```jinja
    /// {{ "foo\nbar\nbaz"|lines }}
    ///     -> ["foo", "bar", "baz"]
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn lines(s: Arc<str>) -> Value {
        Value::from_iter(s.lines().map(|x| x.to_string()))
    }

    /// If the value is undefined or falsy (when boolean=true) it will return the passed default value,
    /// otherwise the value of the variable:
    ///
    /// ```jinja
    /// <p>{{ my_variable|default("my_variable was not defined") }}</p>
    /// <p>{{ my_variable|default("my_variable was falsy", true) }}</p>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn default(value: &Value, other: Option<Value>, use_default: Option<Value>) -> Value {
        let use_default = match use_default {
            Some(use_default) if use_default.is_true() => value.is_undefined() || !value.is_true(),
            _ => value.is_undefined(),
        };

        if use_default {
            other.unwrap_or_else(|| Value::from(""))
        } else {
            value.clone()
        }
    }

    /// Returns the absolute value of a number.
    ///
    /// ```jinja
    /// |a - b| = {{ (a - b)|abs }}
    ///   -> |2 - 4| = 2
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn abs(value: Value) -> Result<Value, Error> {
        match value.0 {
            ValueRepr::U64(_) | ValueRepr::U128(_) => Ok(value),
            ValueRepr::I64(x) => match x.checked_abs() {
                Some(rv) => Ok(Value::from(rv)),
                None => Ok(Value::from((x as i128).abs())), // this cannot overflow
            },
            ValueRepr::I128(x) => {
                x.0.checked_abs()
                    .map(Value::from)
                    .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "overflow on abs"))
            }
            ValueRepr::F64(x) => Ok(Value::from(x.abs())),
            _ => Err(Error::new(
                ErrorKind::InvalidOperation,
                "cannot get absolute value",
            )),
        }
    }

    /// Converts a value into an integer.
    ///
    /// ```jinja
    /// {{ "42"|int == 42 }} -> true
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn int(args: &[Value]) -> Result<Value, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let value: Value = arg_parser.next_positional()?;
        let default = arg_parser.get_optional("default").unwrap_or(0);
        let base = arg_parser.get_optional("base").unwrap_or(10);

        match &value.0 {
            ValueRepr::Undefined(_) => Err(Error::from(ErrorKind::UndefinedError)),
            ValueRepr::None => Ok(Value::from(default)),
            ValueRepr::Bool(x) => Ok(Value::from(*x as u64)),
            ValueRepr::U64(_) | ValueRepr::I64(_) | ValueRepr::U128(_) | ValueRepr::I128(_) => {
                Ok(value.clone())
            }
            ValueRepr::F64(v) => Ok(Value::from(*v as i128)),
            ValueRepr::String(..) | ValueRepr::SmallStr(_) => {
                let s = value.as_str().unwrap().trim();
                // Handle base prefixes
                let s_lower = s.to_lowercase();
                let (s, actual_base) = match s_lower.as_str() {
                    s if s.starts_with("0b") => (&s[2..], 2),
                    s if s.starts_with("0o") => (&s[2..], 8),
                    s if s.starts_with("0x") => (&s[2..], 16),
                    _ => (s, base),
                };

                if let Ok(i) = i128::from_str_radix(s, actual_base) {
                    Ok(Value::from(i))
                } else if let Ok(f) = s.parse::<f64>() {
                    Ok(Value::from(f as i128))
                } else {
                    Ok(Value::from(default))
                }
            }
            ValueRepr::Object(_) => {
                // Support Python's __int__ protocol for custom objects
                if let Some(int_val) = value.get_attr_fast("__int__") {
                    if let Some(i) = int_val.as_i64() {
                        return Ok(Value::from(i));
                    }
                }
                Ok(Value::from(default))
            }
            ValueRepr::Bytes(_) => Ok(Value::from(default)),
            ValueRepr::Invalid(_) => value.clone().validate(),
        }
    }

    /// Converts a value into a float.
    ///
    /// ```jinja
    /// {{ "42.5"|float == 42.5 }} -> true
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn float(value: &Value) -> Result<Value, Error> {
        match &value.0 {
            ValueRepr::Undefined(_) | ValueRepr::None => Ok(Value::from(0.0)),
            ValueRepr::Bool(x) => Ok(Value::from(*x as u64 as f64)),
            ValueRepr::String(..) | ValueRepr::SmallStr(_) => value
                .as_str()
                .unwrap()
                .parse::<f64>()
                .map(Value::from)
                .map_err(|err| Error::new(ErrorKind::InvalidOperation, err.to_string())),
            ValueRepr::Invalid(_) => value.clone().validate(),
            _ => as_f64(value, true).map(Value::from).ok_or_else(|| {
                Error::new(
                    ErrorKind::InvalidOperation,
                    format!("cannot convert {} to float", value.kind()),
                )
            }),
        }
    }

    /// Sums up all the values in a sequence.
    ///
    /// ```jinja
    /// {{ range(10)|sum }} -> 45
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn sum(state: &State, values: Value, start: Option<Value>) -> Result<Value, Error> {
        let mut rv = Value::from(0);
        let iter = ok!(state.undefined_behavior().try_iter(values));
        let start = if let Some(start) = start {
            if start.is_undefined() {
                Ok(0 as f64)
            } else if start.is_kwargs() {
                // check if start has key "start"
                let start = start.as_object().unwrap();
                if let Some(value) = start.get_value(&Value::from("start")) {
                    // first check if value is an empty list
                    if value == Value::from(Vec::<Value>::new()) {
                        // this sum(start=[]) means flatten the list
                        // https://stackoverflow.com/questions/31876069/is-it-possible-to-flatten-a-lists-of-lists-with-ansible-jinja2/37481293#37481293
                        let mut result = vec![];
                        for item in iter {
                            if item.is_undefined() {
                                ok!(state.undefined_behavior().handle_undefined(false));
                                continue;
                            }
                            if let Some(iter) = item.as_object().and_then(|x| x.try_iter()) {
                                for value in iter {
                                    result.push(value);
                                }
                            } else {
                                result.push(item);
                            }
                        }
                        return Ok(Value::from(result));
                    }

                    as_f64(&value, true).ok_or_else(|| {
                        Error::new(
                            ErrorKind::InvalidOperation,
                            format!("cannot convert {} to int", value.kind()),
                        )
                    })
                } else {
                    Ok(0 as f64)
                }
            } else {
                as_f64(&start, true).ok_or_else(|| {
                    Error::new(
                        ErrorKind::InvalidOperation,
                        format!("cannot convert {} to int", start.kind()),
                    )
                })
            }
        } else {
            Ok(0 as f64)
        }? as usize;
        for value in iter.skip(start) {
            if value.is_undefined() {
                ok!(state.undefined_behavior().handle_undefined(false));
                continue;
            } else if !value.is_number() {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("can only sum numbers, got {}", value.kind()),
                ));
            }
            rv = ok!(ops::add(&rv, &value));
        }

        Ok(rv)
    }

    /// Looks up an attribute.
    ///
    /// In MiniJinja this is the same as the `[]` operator.  In Jinja2 there is a
    /// small difference which is why this filter is sometimes used in Jinja2
    /// templates.  For compatibility it's provided here as well.
    ///
    /// ```jinja
    /// {{ value['key'] == value|attr('key') }} -> true
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn attr(value: &Value, key: &Value) -> Result<Value, Error> {
        value.get_item(key)
    }

    /// Round the number to a given precision.
    ///
    /// Round the number to a given precision. The first parameter specifies the
    /// precision (default is 0).
    ///
    /// ```jinja
    /// {{ 42.55|round }}
    ///   -> 43.0
    /// ```
    ///
    /// It also supports the `method` parameter to specify the rounding method.
    ///
    /// ```jinja
    /// {{ 42.55|round(0, method="ceil") }}
    ///   -> 43.0
    /// ```
    ///
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn round(args: &[Value]) -> Result<Value, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let value: Value = arg_parser.next_positional()?;
        let precision: Option<i32> = arg_parser.get_optional("precision");
        let method: Option<String> = arg_parser.get_optional("method");

        match value.0 {
            ValueRepr::I64(_) | ValueRepr::I128(_) | ValueRepr::U64(_) | ValueRepr::U128(_) => {
                Ok(value)
            }
            ValueRepr::F64(val) => {
                let x = 10f64.powi(precision.unwrap_or(0));
                let scaled_val = x * val;

                let rounded = match method.as_deref() {
                    Some("ceil") => scaled_val.ceil(),
                    Some("floor") => scaled_val.floor(),
                    Some("common") | None => scaled_val.round(), // default behavior
                    Some(other) => {
                        return Err(Error::new(
                            ErrorKind::InvalidOperation,
                            format!("invalid rounding method '{other}', expected 'ceil', 'floor', or 'common'"),
                        ));
                    }
                };

                Ok(Value::from(rounded / x))
            }
            _ => Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("cannot round value ({})", value.kind()),
            )),
        }
    }

    /// Returns the first item from an iterable.
    ///
    /// If the list is empty `undefined` is returned. Mirrors Python Jinja2:
    /// an undefined input yields undefined (no error), and a non-iterable
    /// input (None, number, ...) raises a `TypeError` of the form
    /// `'<typename>' object is not iterable`.
    ///
    /// ```jinja
    /// <dl>
    ///   <dt>primary email
    ///   <dd>{{ user.email_addresses|first|default('no user') }}
    /// </dl>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn first(value: &Value) -> Result<Value, Error> {
        if value.is_undefined() {
            return Ok(Value::UNDEFINED);
        }
        if let Some(s) = value.as_str() {
            return Ok(s.chars().next().map_or(Value::UNDEFINED, Value::from));
        }
        if let Some(mut iter) = value.as_object().and_then(|x| x.try_iter()) {
            return Ok(iter.next().unwrap_or(Value::UNDEFINED));
        }
        Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("'{}' object is not iterable", value.python_type_name()),
        ))
    }

    /// Returns the last item from an iterable.
    ///
    /// If the list is empty `undefined` is returned. Mirrors Python Jinja2:
    /// an undefined input yields undefined, dicts yield their last key in
    /// insertion order, and a non-iterable input (None, number, ...) raises
    /// a `TypeError` of the form `'<typename>' object is not reversible`
    /// (Jinja2's `last` filter calls `reversed(...)` under the hood).
    ///
    /// ```jinja
    /// <h2>Most Recent Update</h2>
    /// {% with update = updates|last %}
    ///   <dl>
    ///     <dt>Location
    ///     <dd>{{ update.location }}
    ///     <dt>Status
    ///     <dd>{{ update.status }}
    ///   </dl>
    /// {% endwith %}
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn last(value: Value) -> Result<Value, Error> {
        if value.is_undefined() {
            return Ok(Value::UNDEFINED);
        }
        if let Some(s) = value.as_str() {
            return Ok(s.chars().next_back().map_or(Value::UNDEFINED, Value::from));
        }
        if value.kind() == ValueKind::Map {
            return Ok(value
                .as_object()
                .and_then(|o| o.try_iter_pairs())
                .and_then(|iter| iter.last().map(|(k, _)| k))
                .unwrap_or(Value::UNDEFINED));
        }
        if matches!(value.kind(), ValueKind::Seq | ValueKind::Iterable) {
            let rev = ok!(value.reverse());
            let mut iter = ok!(rev.try_iter());
            return Ok(iter.next().unwrap_or_default());
        }
        Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("'{}' object is not reversible", value.python_type_name()),
        ))
    }

    /// Returns the smallest item from an iterable.
    ///
    /// ```jinja
    /// {{ [1, 2, 3, 4]|min }} -> 1
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn min(state: &State, value: Value) -> Result<Value, Error> {
        let iter = ok!(state.undefined_behavior().try_iter(value).map_err(|err| {
            Error::new(ErrorKind::InvalidOperation, "cannot convert value to list").with_source(err)
        }));
        Ok(iter.min().unwrap_or(Value::UNDEFINED))
    }

    /// Returns the largest item from an iterable.
    ///
    /// ```jinja
    /// {{ [1, 2, 3, 4]|max }} -> 4
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn max(state: &State, value: Value) -> Result<Value, Error> {
        let iter = ok!(state.undefined_behavior().try_iter(value).map_err(|err| {
            Error::new(ErrorKind::InvalidOperation, "cannot convert value to list").with_source(err)
        }));
        Ok(iter.max().unwrap_or(Value::UNDEFINED))
    }

    /// Returns the sorted version of the given list.
    ///
    /// The filter accepts a few keyword arguments:
    ///
    /// * `case_sensitive`: set to `true` to make the sorting of strings case sensitive.
    /// * `attribute`: can be set to an attribute or dotted path to sort by that attribute
    /// * `reverse`: set to `true` to sort in reverse.
    ///
    /// ```jinja
    /// {{ [1, 3, 2, 4]|sort }} -> [4, 3, 2, 1]
    /// {{ [1, 3, 2, 4]|sort(reverse=true) }} -> [1, 2, 3, 4]
    /// # Sort users by age attribute in descending order.
    /// {{ users|sort(attribute="age") }}
    /// # Sort users by age attribute in ascending order.
    /// {{ users|sort(attribute="age", reverse=true) }}
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn sort(state: &State, value: Value, kwargs: Kwargs) -> Result<Value, Error> {
        let mut items = ok!(state.undefined_behavior().try_iter(value).map_err(|err| {
            Error::new(ErrorKind::InvalidOperation, "cannot convert value to list").with_source(err)
        }))
        .collect::<Vec<_>>();
        let case_sensitive = ok!(kwargs.get::<Option<bool>>("case_sensitive")).unwrap_or(false);
        if let Some(attr) = ok!(kwargs.get::<Option<&str>>("attribute")) {
            items.sort_by(|a, b| match (a.get_path(attr), b.get_path(attr)) {
                (Ok(a), Ok(b)) => cmp_helper(&a, &b, case_sensitive),
                _ => Ordering::Equal,
            });
        } else {
            items.sort_by(|a, b| cmp_helper(a, b, case_sensitive))
        }
        if let Some(true) = ok!(kwargs.get("reverse")) {
            items.reverse();
        }
        ok!(kwargs.assert_all_used());

        // Always return a list (like Python's sorted())
        Ok(Value::from_object(mutable_vec::MutableVec::from(items)))
    }

    /// Converts the input value into a list.
    ///
    /// If the value is already a list, then it's returned unchanged.
    /// Applied to a map this returns the list of keys, applied to a
    /// string this returns the characters.  If the value is undefined
    /// an empty list is returned.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn list(state: &State, value: Value) -> Result<Value, Error> {
        let iter = ok!(state.undefined_behavior().try_iter(value).map_err(|err| {
            Error::new(ErrorKind::InvalidOperation, "cannot convert value to list").with_source(err)
        }));
        Ok(Value::from(iter.collect::<MutableVec<_>>()))
    }

    /// Converts a value into a string if it's not one already.
    ///
    /// If the string has been marked as safe, that value is preserved.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn string(value: &Value) -> Value {
        if value.kind() == ValueKind::String {
            value.clone()
        } else {
            value.to_string().into()
        }
    }

    /// Converts the value into a boolean value.
    ///
    /// This behaves the same as the if statement does with regards to
    /// handling of boolean values.
    ///
    /// ```jinja
    /// {{ 42|bool }} -> true
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn bool(value: &Value) -> bool {
        value.is_true()
    }

    /// Slice an iterable and return a list of lists containing
    /// those items.
    ///
    /// Useful if you want to create a div containing three ul tags that
    /// represent columns:
    ///
    /// ```jinja
    /// <div class="columnwrapper">
    /// {% for column in items|slice(3) %}
    ///   <ul class="column-{{ loop.index }}">
    ///   {% for item in column %}
    ///     <li>{{ item }}</li>
    ///   {% endfor %}
    ///   </ul>
    /// {% endfor %}
    /// </div>
    /// ```
    ///
    /// If you pass it a second argument it's used to fill missing values on the
    /// last iteration.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn slice(state: &State, args: &[Value]) -> Result<Value, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let value: Value = arg_parser.next_positional()?;
        let count: usize = arg_parser.next_positional()?;
        let fill_with: Option<Value> = arg_parser.get_optional("fill_with");

        if count == 0 {
            return Err(Error::new(ErrorKind::InvalidOperation, "count cannot be 0"));
        }
        let items = ok!(state.undefined_behavior().try_iter(value)).collect::<Vec<_>>();
        let len = items.len();
        let items_per_slice = len / count;
        let slices_with_extra = len % count;
        let mut offset = 0;
        let mut rv = Vec::with_capacity(count);

        for slice in 0..count {
            let start = offset + slice * items_per_slice;
            if slice < slices_with_extra {
                offset += 1;
            }
            let end = offset + (slice + 1) * items_per_slice;
            let tmp = &items[start..end];

            if let Some(ref filler) = fill_with {
                if slice >= slices_with_extra {
                    let mut tmp = tmp.to_vec();
                    tmp.push(filler.clone());
                    rv.push(Value::from(tmp));
                    continue;
                }
            }

            rv.push(Value::from(tmp.to_vec()));
        }

        Ok(Value::from(rv))
    }

    /// Batch items.
    ///
    /// This filter works pretty much like `slice` just the other way round. It
    /// returns a list of lists with the given number of items. If you provide a
    /// second parameter this is used to fill up missing items.
    ///
    /// ```jinja
    /// <table>
    ///   {% for row in items|batch(3, '&nbsp;') %}
    ///   <tr>
    ///   {% for column in row %}
    ///     <td>{{ column }}</td>
    ///   {% endfor %}
    ///   </tr>
    ///   {% endfor %}
    /// </table>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn batch(state: &State, args: &[Value]) -> Result<Value, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let value: Value = arg_parser.next_positional()?;
        let count: usize = arg_parser.next_positional()?;
        let fill_with: Option<Value> = arg_parser.get_optional("fill_with");

        if count == 0 {
            return Err(Error::new(ErrorKind::InvalidOperation, "count cannot be 0"));
        }
        let mut rv = Vec::with_capacity(value.len().unwrap_or(0) / count);
        let mut tmp = Vec::with_capacity(count);

        for item in ok!(state.undefined_behavior().try_iter(value)) {
            if tmp.len() == count {
                rv.push(Value::from(mem::replace(
                    &mut tmp,
                    Vec::with_capacity(count),
                )));
            }
            tmp.push(item);
        }

        if !tmp.is_empty() {
            if let Some(filler) = fill_with {
                for _ in 0..count - tmp.len() {
                    tmp.push(filler.clone());
                }
            }
            rv.push(Value::from(tmp));
        }

        Ok(Value::from(rv))
    }

    /// Converts a minijinja Value to serde_json::Value with sorted object keys.
    /// This ensures that dictionary keys are serialized in alphabetical order,
    /// matching Python's json.dumps() default behavior.
    #[cfg(feature = "json")]
    fn value_to_sorted_json(value: &Value) -> Result<serde_json::Value, serde_json::Error> {
        use serde_json::Value as JsonValue;

        // First serialize to serde_json::Value (which preserves order from minijinja)
        let json_value: JsonValue = serde_json::to_value(value)?;

        // Then recursively sort all object keys
        Ok(sort_json_keys(json_value))
    }

    /// Recursively sorts all object keys in a serde_json::Value
    #[cfg(feature = "json")]
    fn sort_json_keys(value: serde_json::Value) -> serde_json::Value {
        use serde_json::Value as JsonValue;
        use std::collections::BTreeMap;

        match value {
            JsonValue::Object(map) => {
                // Convert to BTreeMap to sort keys, then recursively sort nested values
                let sorted: BTreeMap<String, JsonValue> = map
                    .into_iter()
                    .map(|(k, v)| (k, sort_json_keys(v)))
                    .collect();
                JsonValue::Object(sorted.into_iter().collect())
            }
            JsonValue::Array(arr) => {
                // Recursively sort keys in array elements
                JsonValue::Array(arr.into_iter().map(sort_json_keys).collect())
            }
            other => other,
        }
    }

    /// A JSON formatter that matches Python's json.dumps() default style:
    /// - Compact (no newlines or indentation)
    /// - Space after colons (e.g., `{"key": "value"}` instead of `{"key":"value"}`)
    /// - Space after commas (e.g., `{"a": 1, "b": 2}` instead of `{"a": 1,"b": 2}`)
    #[cfg(feature = "json")]
    struct PythonStyleJsonFormatter;

    #[cfg(feature = "json")]
    impl serde_json::ser::Formatter for PythonStyleJsonFormatter {
        fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }

        fn begin_object_value<W>(&mut self, writer: &mut W) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            writer.write_all(b": ")
        }

        fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> std::io::Result<()>
        where
            W: ?Sized + std::io::Write,
        {
            if first {
                Ok(())
            } else {
                writer.write_all(b", ")
            }
        }
    }

    /// Dumps a value to JSON.
    ///
    /// This filter is only available if the `json` feature is enabled.  The resulting
    /// value is safe to use in HTML as well as it will not contain any special HTML
    /// characters.  The optional parameter to the filter can be set to `true` to enable
    /// pretty printing.  Not that the `"` character is left unchanged as it's the
    /// JSON string delimiter.  If you want to pass JSON serialized this way into an
    /// HTTP attribute use single quoted HTML attributes:
    ///
    /// ```jinja
    /// <script>
    ///   const GLOBAL_CONFIG = {{ global_config|tojson }};
    /// </script>
    /// <a href="#" data-info='{{ json_object|tojson }}'>...</a>
    /// ```
    ///
    /// The filter takes one argument `indent` (which can also be passed as keyword
    /// argument for compatibility with Jinja2) which can be set to `true` to enable
    /// pretty printing or an integer to control the indentation of the pretty
    /// printing feature.
    ///
    /// ```jinja
    /// <script>
    ///   const GLOBAL_CONFIG = {{ global_config|tojson(indent=2) }};
    /// </script>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(all(feature = "builtins", feature = "json"))))]
    #[cfg(feature = "json")]
    pub fn tojson(args: &[Value]) -> Result<Value, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let value: Value = arg_parser.next_positional()?;
        let indent: Option<Value> = arg_parser.get_optional("indent");
        let indent = match indent {
            None => None,
            Some(ref val) => match bool::try_from(val.clone()).ok() {
                Some(true) => Some(2),
                Some(false) => None,
                None => Some(ok!(usize::try_from(val.clone()))),
            },
        };
        ok!(arg_parser.assert_all_used());

        // Convert to serde_json::Value with sorted keys to match Python's json.dumps()
        let sorted_json = ok!(value_to_sorted_json(&value).map_err(|err| Error::new(
            ErrorKind::InvalidOperation,
            "cannot serialize to JSON"
        )
        .with_source(err)));

        if let Some(indent) = indent {
            let mut out = Vec::<u8>::new();
            let indentation = " ".repeat(indent);
            let formatter = serde_json::ser::PrettyFormatter::with_indent(indentation.as_bytes());
            let mut s = serde_json::Serializer::with_formatter(&mut out, formatter);
            serde::Serialize::serialize(&sorted_json, &mut s)
                .map(|_| unsafe { String::from_utf8_unchecked(out) })
        } else {
            // Use Python-style formatting (space after colon and comma)
            let mut out = Vec::<u8>::new();
            let mut s = serde_json::Serializer::with_formatter(&mut out, PythonStyleJsonFormatter);
            serde::Serialize::serialize(&sorted_json, &mut s)
                .map(|_| unsafe { String::from_utf8_unchecked(out) })
        }
        .map_err(|err| {
            Error::new(ErrorKind::InvalidOperation, "cannot serialize to JSON").with_source(err)
        })
        .map(|s| {
            // When this filter is used the return value is safe for both HTML and JSON
            let mut rv = String::with_capacity(s.len());
            for c in s.chars() {
                match c {
                    '<' => rv.push_str("\\u003c"),
                    '>' => rv.push_str("\\u003e"),
                    '&' => rv.push_str("\\u0026"),
                    '\'' => rv.push_str("\\u0027"),
                    _ => rv.push(c),
                }
            }
            Value::from_safe_string(rv)
        })
    }

    /// Indents Value with spaces
    ///
    /// The first optional parameter to the filter can be set to `true` to
    /// indent the first line. The parameter defaults to false.
    /// the second optional parameter to the filter can be set to `true`
    /// to indent blank lines. The parameter defaults to false.
    /// This filter is useful, if you want to template yaml-files
    ///
    /// ```jinja
    /// example:
    ///   config:
    /// {{ global_config|indent(2) }}          # does not indent first line
    /// {{ global_config|indent(2,true) }}     # indent whole Value with two spaces
    /// {{ global_config|indent(2,true,true)}} # indent whole Value and all blank lines
    /// ```
    #[cfg_attr(docsrs, doc(cfg(all(feature = "builtins"))))]
    pub fn indent(args: &[Value]) -> Result<String, Error> {
        let mut arg_parser = ArgParser::new(args, None);
        let mut value: String = arg_parser.next_positional()?;
        let width: usize = arg_parser
            .get_optional("width")
            .or_else(|| arg_parser.next_positional().ok())
            .unwrap_or(0);
        let indent_first_line: bool = arg_parser.get_optional("first").unwrap_or(false);
        let indent_blank_lines: bool = arg_parser.get_optional("blank").unwrap_or(false);

        ok!(arg_parser.assert_all_used());

        fn strip_trailing_newline(input: &mut String) {
            if input.ends_with('\n') {
                input.truncate(input.len() - 1);
            }
            if input.ends_with('\r') {
                input.truncate(input.len() - 1);
            }
        }

        strip_trailing_newline(&mut value);
        let indent_with = " ".repeat(width);
        let mut output = String::new();
        let mut iterator = value.split('\n');
        if !indent_first_line {
            output.push_str(iterator.next().unwrap());
            output.push('\n');
        }
        for line in iterator {
            if line.is_empty() {
                if indent_blank_lines {
                    output.push_str(&indent_with);
                }
            } else {
                write!(output, "{indent_with}{line}").ok();
            }
            output.push('\n');
        }
        strip_trailing_newline(&mut output);
        Ok(output)
    }

    /// URL encodes a value.
    ///
    /// If given a map it encodes the parameters into a query set, otherwise it
    /// encodes the stringified value.  If the value is none or undefined, an
    /// empty string is returned.
    ///
    /// ```jinja
    /// <a href="/search?{{ {"q": "my search", "lang": "fr"}|urlencode }}">Search</a>
    /// ```
    #[cfg_attr(docsrs, doc(cfg(all(feature = "builtins", feature = "urlencode"))))]
    #[cfg(feature = "urlencode")]
    pub fn urlencode(value: &Value) -> Result<String, Error> {
        const SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
            .remove(b'/')
            .remove(b'.')
            .remove(b'-')
            .remove(b'_')
            .add(b' ');

        if value.kind() == ValueKind::Map {
            let mut rv = String::new();
            for k in ok!(value.try_iter()) {
                let v = ok!(value.get_item(&k));
                if v.is_none() || v.is_undefined() {
                    continue;
                }
                if !rv.is_empty() {
                    rv.push('&');
                }
                write!(
                    rv,
                    "{}={}",
                    percent_encoding::utf8_percent_encode(&k.to_string(), SET),
                    percent_encoding::utf8_percent_encode(&v.to_string(), SET)
                )
                .unwrap();
            }
            Ok(rv)
        } else {
            match &value.0 {
                ValueRepr::None | ValueRepr::Undefined(_) => Ok("".into()),
                ValueRepr::Bytes(b) => Ok(percent_encoding::percent_encode(b, SET).to_string()),
                ValueRepr::String(..) | ValueRepr::SmallStr(_) => Ok(
                    percent_encoding::utf8_percent_encode(value.as_str().unwrap(), SET).to_string(),
                ),
                _ => Ok(percent_encoding::utf8_percent_encode(&value.to_string(), SET).to_string()),
            }
        }
    }

    fn select_or_reject(
        state: &State,
        invert: bool,
        value: Value,
        attr: Option<Cow<'_, str>>,
        test_name: Option<Cow<'_, str>>,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        let mut rv = vec![];
        let test = if let Some(test_name) = test_name {
            Some(ok!(state
                .env
                .get_test(&test_name)
                .ok_or_else(|| Error::from(ErrorKind::UnknownTest))))
        } else {
            None
        };
        for value in ok!(state.undefined_behavior().try_iter(value)) {
            let test_value = if let Some(ref attr) = attr {
                ok!(value.get_path(attr))
            } else {
                value.clone()
            };
            let passed = if let Some(test) = test {
                let new_args = Some(test_value)
                    .into_iter()
                    .chain(args.0.iter().cloned())
                    .collect::<Vec<_>>();
                ok!(test.perform(state, &new_args))
            } else {
                test_value.is_true()
            };
            if passed != invert {
                rv.push(value);
            }
        }
        Ok(rv)
    }

    /// Creates a new sequence of values that pass a test.
    ///
    /// Filters a sequence of objects by applying a test to each object.
    /// Only values that pass the test are included.
    ///
    /// If no test is specified, each object will be evaluated as a boolean.
    ///
    /// ```jinja
    /// {{ [1, 2, 3, 4]|select("odd") }} -> [1, 3]
    /// {{ [false, null, 42]|select }} -> [42]
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn select(
        state: &State,
        value: Value,
        test_name: Option<Cow<'_, str>>,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        select_or_reject(state, false, value, None, test_name, args)
    }

    /// Creates a new sequence of values of which an attribute passes a test.
    ///
    /// This functions like [`select`] but it will test an attribute of the
    /// object itself:
    ///
    /// ```jinja
    /// {{ users|selectattr("is_active") }} -> all users where x.is_active is true
    /// {{ users|selectattr("id", "even") }} -> returns all users with an even id
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn selectattr(
        state: &State,
        value: Value,
        attr: Cow<'_, str>,
        test_name: Option<Cow<'_, str>>,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        select_or_reject(state, false, value, Some(attr), test_name, args)
    }

    /// Creates a new sequence of values that don't pass a test.
    ///
    /// This is the inverse of [`select`].
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn reject(
        state: &State,
        value: Value,
        test_name: Option<Cow<'_, str>>,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        select_or_reject(state, true, value, None, test_name, args)
    }

    /// Creates a new sequence of values of which an attribute does not pass a test.
    ///
    /// This functions like [`select`] but it will test an attribute of the
    /// object itself:
    ///
    /// ```jinja
    /// {{ users|rejectattr("is_active") }} -> all users where x.is_active is false
    /// {{ users|rejectattr("id", "even") }} -> returns all users with an odd id
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn rejectattr(
        state: &State,
        value: Value,
        attr: Cow<'_, str>,
        test_name: Option<Cow<'_, str>>,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        select_or_reject(state, true, value, Some(attr), test_name, args)
    }

    /// Applies a filter to a sequence of objects or looks up an attribute.
    ///
    /// This is useful when dealing with lists of objects but you are really
    /// only interested in a certain value of it.
    ///
    /// The basic usage is mapping on an attribute. Given a list of users
    /// you can for instance quickly select the username and join on it:
    ///
    /// ```jinja
    /// {{ users|map(attribute='username')|join(', ') }}
    /// ```
    ///
    /// You can specify a `default` value to use if an object in the list does
    /// not have the given attribute.
    ///
    /// ```jinja
    /// {{ users|map(attribute="username", default="Anonymous")|join(", ") }}
    /// ```
    ///
    /// Alternatively you can have `map` invoke a filter by passing the name of the
    /// filter and the arguments afterwards. A good example would be applying a
    /// text conversion filter on a sequence:
    ///
    /// ```jinja
    /// Users on this page: {{ titles|map('lower')|join(', ') }}
    /// ```
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn map(
        state: &State,
        value: Value,
        args: crate::value::Rest<Value>,
    ) -> Result<Vec<Value>, Error> {
        let mut rv = Vec::with_capacity(value.len().unwrap_or(0));

        // attribute mapping
        let (args, kwargs): (&[Value], Kwargs) = crate::value::from_args(&args)?;

        if let Some(attr) = ok!(kwargs.get::<Option<Value>>("attribute")) {
            if !args.is_empty() {
                return Err(Error::from(ErrorKind::TooManyArguments));
            }
            let default = if kwargs.has("default") {
                ok!(kwargs.get::<Value>("default"))
            } else {
                Value::UNDEFINED
            };
            for value in ok!(state.undefined_behavior().try_iter(value)) {
                let sub_val = match attr.as_str() {
                    Some(path) => value.get_path(path),
                    None => value.get_item(&attr),
                };
                rv.push(match (sub_val, &default) {
                    (Ok(attr), _) => {
                        if attr.is_undefined() {
                            default.clone()
                        } else {
                            attr
                        }
                    }
                    (Err(_), default) if !default.is_undefined() => default.clone(),
                    (Err(err), _) => return Err(err),
                });
            }
            ok!(kwargs.assert_all_used());
            return Ok(rv);
        }

        // filter mapping
        let filter_name = ok!(args
            .first()
            .ok_or_else(|| Error::new(ErrorKind::InvalidOperation, "filter name is required")));
        let filter_name = ok!(filter_name.as_str().ok_or_else(|| {
            Error::new(ErrorKind::InvalidOperation, "filter name must be a string")
        }));

        let filter = ok!(state
            .env
            .get_filter(filter_name)
            .ok_or_else(|| Error::from(ErrorKind::UnknownFilter)));
        for value in ok!(state.undefined_behavior().try_iter(value)) {
            let new_args = Some(value.clone())
                .into_iter()
                .chain(args.iter().skip(1).cloned())
                .collect::<Vec<_>>();
            rv.push(ok!(filter.apply_to(state, &new_args)));
        }
        Ok(rv)
    }

    /// Group a sequence of objects by an attribute.
    ///
    /// The attribute can use dot notation for nested access, like `"address.city"``.
    /// The values are sorted first so only one group is returned for each unique value.
    /// The attribute can be passed as first argument or as keyword argument named
    /// `attribute`.
    ///
    /// For example, a list of User objects with a city attribute can be
    /// rendered in groups. In this example, grouper refers to the city value of
    /// the group.
    ///
    /// ```jinja
    /// <ul>{% for city, items in users|groupby("city") %}
    ///   <li>{{ city }}
    ///   <ul>{% for user in items %}
    ///     <li>{{ user.name }}
    ///   {% endfor %}</ul>
    /// </li>
    /// {% endfor %}</ul>
    /// ```    ///
    /// groupby yields named tuples of `(grouper, list)``, which can be used instead
    /// of the tuple unpacking above.  As such this example is equivalent:
    ///
    /// ```jinja
    /// <ul>{% for group in users|groupby(attribute="city") %}
    ///   <li>{{ group.grouper }}
    ///   <ul>{% for user in group.list %}
    ///     <li>{{ user.name }}
    ///   {% endfor %}</ul>
    /// </li>
    /// {% endfor %}</ul>
    /// ```    ///
    /// You can specify a default value to use if an object in the list does not
    /// have the given attribute.
    ///
    /// ```jinja
    /// <ul>{% for city, items in users|groupby("city", default="NY") %}
    ///   <li>{{ city }}: {{ items|map(attribute="name")|join(", ") }}</li>
    /// {% endfor %}</ul>
    /// ```
    ///
    /// Like the [`sort`] filter, sorting and grouping is case-insensitive by default.
    /// The key for each group will have the case of the first item in that group
    /// of values. For example, if a list of users has cities `["CA", "NY", "ca"]``,
    /// the "CA" group will have two values.  This can be disabled by passing
    /// `case_sensitive=True`.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn groupby(value: Value, attribute: Option<&str>, kwargs: Kwargs) -> Result<Value, Error> {
        let default = ok!(kwargs.get::<Option<Value>>("default")).unwrap_or_default();
        let case_sensitive = ok!(kwargs.get::<Option<bool>>("case_sensitive")).unwrap_or(false);
        let attr = match attribute {
            Some(attr) => attr,
            None => ok!(kwargs.get::<&str>("attribute")),
        };
        let mut items: Vec<Value> = ok!(value.try_iter()).collect();
        items.sort_by(|a, b| {
            let a = a.get_path_or_default(attr, &default);
            let b = b.get_path_or_default(attr, &default);
            cmp_helper(&a, &b, case_sensitive)
        });
        ok!(kwargs.assert_all_used());

        #[derive(Debug)]
        pub struct GroupTuple {
            grouper: Value,
            list: Vec<Value>,
        }

        impl Object for GroupTuple {
            fn repr(self: &Arc<Self>) -> ObjectRepr {
                ObjectRepr::Seq
            }

            fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
                match (key.as_usize(), key.as_str()) {
                    (Some(0), None) | (None, Some("grouper")) => Some(self.grouper.clone()),
                    (Some(1), None) | (None, Some("list")) => {
                        Some(Value::make_object_iterable(self.clone(), |this| {
                            Box::new(this.list.iter().cloned())
                                as Box<dyn Iterator<Item = _> + Send + Sync>
                        }))
                    }
                    _ => None,
                }
            }

            fn enumerate(self: &Arc<Self>) -> Enumerator {
                Enumerator::Seq(2)
            }
        }

        let mut rv = Vec::new();
        let mut grouper = None::<Value>;
        let mut list = Vec::new();

        for item in items {
            let group_by = item.get_path_or_default(attr, &default);
            if let Some(ref last_grouper) = grouper {
                if cmp_helper(last_grouper, &group_by, case_sensitive) != Ordering::Equal {
                    rv.push(Value::from_object(GroupTuple {
                        grouper: last_grouper.clone(),
                        list: std::mem::take(&mut list),
                    }));
                }
            }
            grouper = Some(group_by);
            list.push(item);
        }

        if !list.is_empty() {
            rv.push(Value::from_object(GroupTuple {
                grouper: grouper.unwrap(),
                list,
            }));
        }

        Ok(Value::from_object(rv))
    }

    /// Returns a list of unique items from the given iterable.
    ///
    /// ```jinja
    /// {{ ['foo', 'bar', 'foobar', 'foobar']|unique|list }}
    ///   -> ['foo', 'bar', 'foobar']
    /// ```
    ///
    /// The unique items are yielded in the same order as their first occurrence
    /// in the iterable passed to the filter.  The filter will not detect
    /// duplicate objects or arrays, only primitives such as strings or numbers.
    ///
    /// Optionally the `attribute` keyword argument can be used to make the filter
    /// operate on an attribute instead of the value itself.  In this case only
    /// one city per state would be returned:
    ///
    /// ```jinja
    /// {{ list_of_cities|unique(attribute='state') }}
    /// ```
    ///
    /// Like the [`sort`] filter this operates case-insensitive by default.
    /// For example, if a list has the US state codes `["CA", "NY", "ca"]``,
    /// the resulting list will have `["CA", "NY"]`.  This can be disabled by
    /// passing `case_sensitive=True`.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn unique(state: &State, values: Value, kwargs: Kwargs) -> Result<Value, Error> {
        use std::collections::BTreeSet;

        let attr = ok!(kwargs.get::<Option<&str>>("attribute"));
        let case_sensitive = ok!(kwargs.get::<Option<bool>>("case_sensitive")).unwrap_or(false);
        ok!(kwargs.assert_all_used());

        let mut rv = Vec::new();
        let mut seen = BTreeSet::new();

        let iter = ok!(state.undefined_behavior().try_iter(values));
        for item in iter {
            let value_to_compare = if let Some(attr) = attr {
                item.get_path_or_default(attr, &Value::UNDEFINED)
            } else {
                item.clone()
            };
            let memorized_value = if case_sensitive {
                value_to_compare.clone()
            } else if let Some(s) = value_to_compare.as_str() {
                Value::from(s.to_lowercase())
            } else {
                value_to_compare.clone()
            };

            if seen.insert(memorized_value) {
                rv.push(item);
            }
        }

        Ok(Value::from(rv))
    }

    /// Pretty print a variable.
    ///
    /// This is useful for debugging as it better shows what's inside an object.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn pprint(value: &Value) -> String {
        format!("{value:#?}")
    }

    /// Formats a string using printf-style formatting.
    ///
    /// ```jinja
    /// {{ "Hello %s"|format("World") }} -> Hello World
    /// {{ "Value: %d"|format(42) }} -> Value: 42
    /// {{ "Float: %f"|format(3.14) }} -> Float: 3.14
    /// ```
    ///
    /// Supports basic printf format specifiers:
    /// * %s - String formatting
    /// * %d - Integer formatting
    /// * %f - Float formatting
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn format(_state: &State, value: Cow<'_, str>, args: Rest<Value>) -> Result<Value, Error> {
        crate::value::ops::format_string(&value, args.as_slice())
    }

    /// Returns true if the value is a list.
    #[cfg_attr(docsrs, doc(cfg(feature = "builtins")))]
    pub fn is_list(value: &Value) -> bool {
        value.kind() == ValueKind::Seq || value.kind() == ValueKind::Iterable
    }
}

#[cfg(feature = "builtins")]
pub use self::builtins::*;
