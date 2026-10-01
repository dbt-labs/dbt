//! Data types define how data should be imported during the creation of a
//! :class:`.Table`.
//!
//! If column types are not explicitly specified when a :class:`.Table` is created,
//! agate will attempt to guess them. The :class:`.TypeTester` class can be used to
//! control how types are guessed.
//!
//! https://github.com/wireservice/agate/tree/5ebea8dd0b9c7cd0f795e53695aa4d782b95e40c/agate/data_types

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::Object;
use minijinja::{Error, ErrorKind, State, Value};

/// Default values which will be automatically cast to :code:`None`
pub const DEFAULT_NULL_VALUES: [&str; 6] = ["", "na", "n/a", "none", "null", "."];

/// A sequence of values which should be cast to [None] when encountered by this data type.
///
/// The Python agate implementation applies a .lower() transformation on creation, but we
/// do case-insensitive comparison on [NullValues::contains] instead.
#[derive(Debug, Clone)]
pub enum NullValues<'a> {
    Borrowed(&'a [&'a str]),
    Owned(Vec<Value>),
}

impl<'a> NullValues<'a> {
    /// Check if a value is considered null.
    pub fn contains(&self, value: &Value) -> bool {
        match self {
            NullValues::Borrowed(nulls) => nulls.iter().any(|&n| match value.as_str() {
                Some(s) => n.eq_ignore_ascii_case(s),
                None => n.eq_ignore_ascii_case(&value.to_string()),
            }),
            NullValues::Owned(nulls) => nulls.iter().any(|n| match (n.as_str(), value.as_str()) {
                (Some(ns), Some(s)) => ns.eq_ignore_ascii_case(s),
                (Some(ns), None) => ns.eq_ignore_ascii_case(&value.to_string()),
                (None, Some(s)) => n.to_string().eq_ignore_ascii_case(s),
                (None, None) => n.to_string().eq_ignore_ascii_case(&value.to_string()),
            }),
        }
    }
}

/// dyn-compatible [DataType] representation.
pub trait DataTypeRepr: fmt::Debug + Send + Sync {
    /// A sequence of values which should be cast to [None] when encountered by this data type.
    fn null_values(&self) -> &NullValues<'static>;

    /// The name of the [DataType] implementation.
    fn type_name(&self) -> &str;

    /// Test, for purposes of type inference, if a value could possibly be
    /// coerced to this data type.
    ///
    /// This is really just a thin wrapper around :meth:`DataType.cast`.
    fn test(&self, d: &dyn DataTypeRepr) -> bool {
        self.cast(d).is_ok()
    }

    /// Coerce a given string value into this column's data type.
    fn cast(&self, d: &dyn DataTypeRepr) -> Result<Value, Error> {
        // raise NotImplementedError
        let err = Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "cast() method not implemented for the {} data type",
                d.type_name()
            ),
        );
        Err(err)
    }

    /// Format a given native value for CSV serialization.
    fn csvify(&self, d: &Value) -> Value {
        if d.is_none() || d.is_undefined() {
            Value::from(())
        } else {
            Value::from(d.to_string())
        }
    }

    /// Format a given native value for JSON serialization.
    fn jsonify(&self, d: &Value) -> Value {
        if d.is_none() || d.is_undefined() {
            Value::from(())
        } else {
            Value::from(d.to_string())
        }
    }

    /// Compare this [DataType] representation (virtually-dispatched).
    fn eq_repr(&self, other: &dyn DataTypeRepr) -> bool;
}

#[derive(Debug, Clone)]
pub struct DataType(pub(crate) Arc<dyn DataTypeRepr>);

impl DataType {
    // XXX: remove this ctor once the full type hierarchy is implemented
    pub fn new(type_name: String) -> Self {
        let repr = DataTypeReprImpl::new(type_name);
        Self(Arc::new(repr))
    }

    pub fn type_name(&self) -> &str {
        self.0.type_name()
    }

    pub fn test(&self, d: &dyn DataTypeRepr) -> bool {
        self.0.test(d)
    }

    pub fn cast(&self, d: &dyn DataTypeRepr) -> Result<Value, Error> {
        self.0.cast(d)
    }

    pub fn csvify(&self, d: &Value) -> Value {
        self.0.csvify(d)
    }

    pub fn jsonify(&self, d: &Value) -> Value {
        self.0.jsonify(d)
    }
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<class 'agate.data_types.{}'>", self.type_name())
    }
}

impl Eq for DataType {}

impl PartialEq for DataType {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_repr(&*other.0)
    }
}

impl Object for DataType {
    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let as_data_type = |x: &str, fname: &str, value: &Value| {
            value
                .downcast_object_ref::<&dyn DataTypeRepr>()
                .ok_or_else(|| {
                    let got = ArgsIter::type_name_of_value(Some(value));
                    Error::new(
                        ErrorKind::InvalidArgument,
                        format!(
                            "argument {} to {}() has incompatible type {}; expected DataType",
                            x,
                            fname,
                            got.as_ref(),
                        ),
                    )
                })
                .copied()
        };
        match name {
            // DataType methods
            "test" => {
                let args_iter = ArgsIter::new(name, &["d"], args);
                let d = args_iter.next_arg::<&Value>()?;
                args_iter.finish()?;

                let d = as_data_type("d", name, d)?;
                Ok(Value::from(self.0.test(d)))
            }
            "cast" => {
                let args_iter = ArgsIter::new(name, &["d"], args);
                let d = args_iter.next_arg::<&Value>()?;
                args_iter.finish()?;

                let d = as_data_type("d", name, d)?;
                self.0.cast(d)
            }
            "csvify" => {
                let args_iter = ArgsIter::new(name, &["d"], args);
                let d = args_iter.next_arg::<&Value>()?;
                args_iter.finish()?;

                Ok(self.0.csvify(d))
            }
            "jsonify" => {
                let args_iter = ArgsIter::new(name, &["d"], args);
                let d = args_iter.next_arg::<&Value>()?;
                args_iter.finish()?;

                Ok(self.0.jsonify(d))
            }
            _ => Err(Error::new(
                ErrorKind::UnknownMethod,
                format!("{} has no method named '{}'", self.0.type_name(), name),
            )),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

#[derive(Debug, Clone)]
struct DataTypeReprImpl {
    null_values: NullValues<'static>,
    /// The name of the data type ("Text", "Number", "Boolean", "Date", "DateTime", "TimeDelta")
    type_name: String,
}

impl DataTypeReprImpl {
    pub fn new(type_name: String) -> Self {
        Self {
            type_name,
            null_values: NullValues::Borrowed(&DEFAULT_NULL_VALUES),
        }
    }

    /// Specifies how values should be parsed when creating a :class:`.Table`.
    ///
    /// :param null_values: A sequence of values which should be cast to
    ///     :code:`None` when encountered by this data type.
    #[expect(dead_code)]
    pub fn new2(type_name: String, null_values: Vec<Value>) -> Self {
        Self {
            type_name,
            null_values: NullValues::Owned(null_values),
        }
    }
}

impl DataTypeRepr for DataTypeReprImpl {
    fn null_values(&self) -> &NullValues<'static> {
        &self.null_values
    }

    fn type_name(&self) -> &str {
        &self.type_name
    }

    /// Format a given native value for JSON serialization.
    fn jsonify(&self, d: &Value) -> Value {
        if d.is_none() || d.is_undefined() {
            return Value::from(());
        }
        match self.type_name.as_str() {
            "Number" | "Boolean" => d.clone(),
            _ => Value::from(d.to_string()),
        }
    }

    fn eq_repr(&self, other: &dyn DataTypeRepr) -> bool {
        self.type_name() == other.type_name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Environment;

    #[derive(Debug)]
    struct AcceptingDataType;

    static NO_NULL_VALUES: NullValues<'static> = NullValues::Borrowed(&[]);

    impl DataTypeRepr for AcceptingDataType {
        fn null_values(&self) -> &NullValues<'static> {
            &NO_NULL_VALUES
        }

        fn type_name(&self) -> &str {
            "Accepting"
        }

        fn cast(&self, d: &dyn DataTypeRepr) -> Result<Value, Error> {
            Ok(Value::from(format!("casted {}", d.type_name())))
        }

        fn eq_repr(&self, other: &dyn DataTypeRepr) -> bool {
            self.type_name() == other.type_name()
        }
    }

    impl Object for &'static dyn DataTypeRepr {}

    fn accepting_repr() -> &'static dyn DataTypeRepr {
        Box::leak(Box::new(AcceptingDataType))
    }

    fn jsonify(dt: &Arc<DataType>, value: Value) -> Value {
        let env = Environment::new();
        let state = env.empty_state();
        dt.call_method(&state, "jsonify", &[value], &[]).unwrap()
    }

    fn csvify(dt: &Arc<DataType>, value: Value) -> Value {
        let env = Environment::new();
        let state = env.empty_state();
        dt.call_method(&state, "csvify", &[value], &[]).unwrap()
    }

    #[test]
    fn null_values_match_case_insensitively_for_borrowed_and_owned_values() {
        let borrowed = NullValues::Borrowed(&DEFAULT_NULL_VALUES);
        assert!(borrowed.contains(&Value::from("NA")));
        assert!(borrowed.contains(&Value::from(".")));
        assert!(!borrowed.contains(&Value::from("available")));

        let owned = NullValues::Owned(vec![Value::from("Missing"), Value::from(0)]);
        assert!(owned.contains(&Value::from("missing")));
        assert!(owned.contains(&Value::from("0")));
        assert!(owned.contains(&Value::from(0)));
        assert!(!owned.contains(&Value::from(1)));
    }

    #[test]
    fn data_type_default_cast_csvify_and_jsonify_are_observable() {
        let text = Arc::new(DataType::new("Text".to_string()));
        let number = DataType::new("Number".to_string());
        let other = DataTypeReprImpl::new("Number".to_string());

        assert!(!text.test(&other));
        let err = text.cast(&other).unwrap_err();
        assert!(err.to_string().contains("cast() method not implemented"));

        assert_eq!(text.csvify(&Value::from("abc")), Value::from("abc"));
        assert_eq!(text.csvify(&Value::from(42)), Value::from("42"));
        assert_eq!(text.csvify(&Value::from(())), Value::from(()));
        assert_eq!(text.csvify(&Value::UNDEFINED), Value::from(()));

        assert_eq!(text.jsonify(&Value::from("abc")), Value::from("abc"));
        assert_eq!(text.jsonify(&Value::from(())), Value::from(()));
        assert_eq!(text.jsonify(&Value::UNDEFINED), Value::from(()));
        assert_eq!(number.jsonify(&Value::from(42)), Value::from(42));
        assert_eq!(
            DataType::new("Boolean".to_string()).jsonify(&Value::from(true)),
            Value::from(true)
        );
    }

    #[test]
    fn data_type_default_test_jsonify_and_object_dispatch_are_observable() {
        let accepting = Arc::new(DataType(Arc::new(AcceptingDataType)));
        let arg = accepting_repr();

        assert!(!arg.null_values().contains(&Value::from("not-null")));
        assert!(arg.test(arg));
        assert!(accepting.test(arg));
        assert_eq!(
            accepting.cast(arg).unwrap(),
            Value::from("casted Accepting")
        );

        assert_eq!(arg.jsonify(&Value::from(42)), Value::from("42"));
        assert_eq!(arg.jsonify(&Value::from(())), Value::from(()));
        assert_eq!(arg.jsonify(&Value::UNDEFINED), Value::from(()));

        let env = Environment::new();
        let state = env.empty_state();
        let arg_value = Value::from_object(arg);
        assert_eq!(
            accepting
                .call_method(&state, "test", std::slice::from_ref(&arg_value), &[])
                .unwrap(),
            Value::from(true)
        );
        assert_eq!(
            accepting
                .call_method(&state, "cast", &[arg_value], &[])
                .unwrap(),
            Value::from("casted Accepting")
        );
    }

    #[test]
    fn test_data_type_jsonify_text() {
        let dt = Arc::new(DataType::new("Text".to_string()));
        let result = jsonify(&dt, Value::from("hello"));
        assert_eq!(result, Value::from("hello"));
    }

    #[test]
    fn test_data_type_jsonify_number() {
        let dt = Arc::new(DataType::new("Number".to_string()));
        let result = jsonify(&dt, Value::from(42));
        assert_eq!(result, Value::from(42));
    }

    #[test]
    fn test_data_type_jsonify_boolean() {
        let dt = Arc::new(DataType::new("Boolean".to_string()));
        let result = jsonify(&dt, Value::from(true));
        assert_eq!(result, Value::from(true));
    }

    #[test]
    fn test_data_type_jsonify_null() {
        let dt = Arc::new(DataType::new("Text".to_string()));
        let result = jsonify(&dt, Value::from(()));
        assert_eq!(result, Value::from(()));
    }

    #[test]
    fn test_data_type_csvify() {
        let dt = Arc::new(DataType::new("Text".to_string()));
        assert_eq!(csvify(&dt, Value::from("hello")), Value::from("hello"));
        assert_eq!(csvify(&dt, Value::from(42)), Value::from("42"));
        assert_eq!(csvify(&dt, Value::from(())), Value::from(()));
    }
}
