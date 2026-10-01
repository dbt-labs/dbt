#![allow(clippy::let_and_return)]

use core::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::Mutex;

use im::HashSet;

use minijinja::arg_utils::ArgsIter;
use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, ObjectRepr, ValueKind};
use minijinja::{ErrorKind, State, Value, assert_nullary_args};

mod column;
mod columns;
mod converters;
pub mod data_type; // TODO: rename to data_types
mod decimal;
pub mod grouper;
pub mod hashers;
pub mod join;
mod print_table;
mod row;
mod rows;
mod table;
mod table_set;
/// Fixtures for testing within this crate and by users of it.
pub mod test_fixtures;

pub(crate) mod flat_record_batch;
mod vec_of_rows;

pub use column::Column;
pub use columns::Columns;
pub use data_type::DataType;
pub use row::Row;
pub use rows::Rows;
pub use table::AgateTable;
pub use table_set::TableSet;

/// Agate uses Python tuples to represent sequences of values.
///
/// Unlike Python lists, tuples are immutable and have a smaller interface.
trait TupleRepr: fmt::Debug + Send + Sync {
    /// Get a value from the tuple by index.
    fn get_item_by_index(&self, idx: isize) -> Option<Value>;

    /// Get the length of the tuple.
    fn len(&self) -> usize;

    /// Implement the `count` method for tuples.
    fn count_occurrences_of(&self, value: &Value) -> usize;

    /// Implement the `index` method for tuples.
    fn index_of(&self, value: &Value) -> Option<usize>;

    /// Clone this tuple representation (virtually-dispatched).
    fn clone_repr(&self) -> Box<dyn TupleRepr>;

    /// Clone this tuple representation (virtually-dispatched) but exclude the given index.
    ///
    /// Default implementation provided.
    fn excluding(&self, idx: usize) -> Box<dyn TupleRepr> {
        Box::new(ExcludedTupleRepr {
            inner: self.clone_repr(),
            excluded: HashSet::unit(idx),
        })
    }

    /// Compare this tuple representation (virtually-dispatched).
    ///
    /// Can be specialized on specific tuple representations to
    /// avoid copying every value.
    fn eq_repr(&self, other: &dyn TupleRepr) -> bool {
        if self.len() != other.len() {
            return false;
        }
        for i in 0..self.len() {
            if self.get_item_by_index(i as isize) != other.get_item_by_index(i as isize) {
                return false;
            }
        }
        true
    }
}

/// A tuple object that behaves like a Python tuple.
///
/// We need a custom implementation to avoid materializing the tuple in memory
/// by relying on sharing references to the underlying data from a table.
#[derive(Debug)]
pub struct Tuple(Box<dyn TupleRepr>);

impl Tuple {
    /// Get a value from the tuple by index.
    pub fn get(&self, idx: isize) -> Option<Value> {
        self.0.get_item_by_index(idx)
    }

    /// Get the length of the tuple.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Check if the tuple is empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Count the number of occurrences of a value in the tuple.
    pub fn count(&self, value: &Value) -> usize {
        self.0.count_occurrences_of(value)
    }

    /// Find the index of a value in the tuple.
    pub fn index(&self, value: &Value) -> Option<usize> {
        self.0.index_of(value)
    }

    /// Create a new Tuple with a fast clone
    fn clone_repr(&self) -> Tuple {
        Tuple(self.0.clone_repr())
    }

    /// Create a new Tuple with a fast clone excluding the given index
    fn excluding(&self, idx: usize) -> Tuple {
        Tuple(self.0.excluding(idx))
    }
}

/// A TupleRepr that wraps another TupleRepr and excludes certain indices.
///
/// This allows "mutation" by exclusion without materializing a new Vec<Value>.
/// The excluded set uses `im::HashSet` for efficient cloning.
#[derive(Debug)]
pub(crate) struct ExcludedTupleRepr {
    /// The underlying tuple representation.
    inner: Box<dyn TupleRepr>,
    /// The set of excluded indices (in terms of the original inner tuple).
    excluded: HashSet<usize>,
}

impl ExcludedTupleRepr {
    /// Map a visible index to the underlying inner index.
    /// Returns None if the visible index is out of bounds.
    fn visible_to_inner(&self, visible_idx: usize) -> Option<usize> {
        let mut visible_count = 0;
        for inner_idx in 0..self.inner.len() {
            if self.excluded.contains(&inner_idx) {
                continue;
            }
            if visible_count == visible_idx {
                return Some(inner_idx);
            }
            visible_count += 1;
        }
        None
    }
}

impl TupleRepr for ExcludedTupleRepr {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let visible_len = self.len();
        let adjusted = adjusted_index(idx, visible_len)?;
        let inner_idx = self.visible_to_inner(adjusted)?;
        self.inner.get_item_by_index(inner_idx as isize)
    }

    fn len(&self) -> usize {
        self.inner.len() - self.excluded.len()
    }

    fn count_occurrences_of(&self, value: &Value) -> usize {
        let mut count = 0;
        for i in 0..self.inner.len() {
            if self.excluded.contains(&i) {
                continue;
            }
            if let Some(v) = self.inner.get_item_by_index(i as isize) {
                if v == *value {
                    count += 1;
                }
            }
        }
        count
    }

    fn index_of(&self, value: &Value) -> Option<usize> {
        let mut visible_idx = 0;
        for inner_idx in 0..self.inner.len() {
            if self.excluded.contains(&inner_idx) {
                continue;
            }
            if let Some(v) = self.inner.get_item_by_index(inner_idx as isize) {
                if v == *value {
                    return Some(visible_idx);
                }
            }
            visible_idx += 1;
        }
        None
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        Box::new(ExcludedTupleRepr {
            inner: self.inner.clone_repr(),
            excluded: self.excluded.clone(),
        })
    }

    fn excluding(&self, idx: usize) -> Box<dyn TupleRepr> {
        // idx is a visible index, we need to convert to inner index
        if let Some(inner_idx) = self.visible_to_inner(idx) {
            Box::new(ExcludedTupleRepr {
                inner: self.inner.clone_repr(),
                excluded: self.excluded.update(inner_idx),
            })
        } else {
            // idx out of bounds, just clone
            self.clone_repr()
        }
    }
}

impl fmt::Display for Tuple {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let len = self.len();

        write!(f, "(")?;
        for i in 0..len {
            if i > 0 {
                write!(f, ", ")?;
            }
            let value = self.get(i as isize).unwrap();
            // Match Python's tuple repr: string elements are quoted (fs#14243), everything
            // else renders the same as its own Display (numbers, bools, None, nested objects).
            if matches!(value.kind(), ValueKind::String) {
                write!(f, "{value:?}")?;
            } else {
                write!(f, "{value}")?;
            }
        }
        if len == 1 {
            write!(f, ",")?;
        }
        write!(f, ")")
    }
}

impl Eq for Tuple {}

impl PartialEq for Tuple {
    fn eq(&self, other: &Self) -> bool {
        self.0.eq_repr(&*other.0)
    }
}

impl Object for Tuple {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        if let Some(idx) = key.as_i64() {
            self.get(idx as isize)
        } else {
            None
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.len())
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        Some(self.len())
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "count" => {
                let iter = ArgsIter::for_unnamed_pos_args("tuple.count", 1, args);
                let value = iter.next_arg::<&Value>()?;
                iter.finish()?;
                let count = self.count(value);
                Ok(Value::from(count))
            }
            "index" => {
                let iter = ArgsIter::for_unnamed_pos_args("tuple.index", 1, args);
                let value = iter.next_arg::<&Value>()?;
                iter.finish()?;
                let idx = self.index(value);
                Ok(Value::from(idx))
            }
            _ => Err(minijinja::Error::from(ErrorKind::UnknownMethod)),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The equivalent of tuple(zip(first, second)) in Python.
#[derive(Debug)]
struct ZippedTupleRepr {
    /// The first tuple in the zipped tuple.
    first: Box<dyn TupleRepr>,
    /// The second tuple in the zipped tuple.
    second: Box<dyn TupleRepr>,
}

impl ZippedTupleRepr {
    /// Create a new zipped tuple from two tuples.
    pub fn new(first: Box<dyn TupleRepr>, second: Box<dyn TupleRepr>) -> Self {
        debug_assert!(first.len() == second.len());
        Self { first, second }
    }

    pub fn from_tuples(first: &Tuple, second: &Tuple) -> Self {
        Self::new(first.0.clone_repr(), second.0.clone_repr())
    }

    pub fn into_tuple(self) -> Tuple {
        Tuple(Box::new(self))
    }
}

fn value_as_pair(value: &Value) -> Option<(Value, Value)> {
    // value must be a tuple of length 2 to have a chance
    // of matching the pairs in the zipped tuple
    if value.len().unwrap_or(0) != 2 {
        return None;
    }
    let fst_value = value.get_item_by_index(0);
    let snd_value = value.get_item_by_index(1);
    if fst_value.is_err() || snd_value.is_err() {
        return None;
    }
    Some((fst_value.unwrap(), snd_value.unwrap()))
}

impl TupleRepr for ZippedTupleRepr {
    fn get_item_by_index(&self, idx: isize) -> Option<Value> {
        let fst = self.first.get_item_by_index(idx)?;
        let snd = self.second.get_item_by_index(idx)?;
        Some(Value::from_iter([fst, snd]))
    }

    fn len(&self) -> usize {
        // first and second have the same length
        self.first.len()
    }

    fn count_occurrences_of(&self, value: &Value) -> usize {
        match value_as_pair(value) {
            Some((fst_value, snd_value)) => {
                let mut count = 0;
                for i in 0..self.len() {
                    let fst = self.first.get_item_by_index(i as isize).unwrap();
                    let snd = self.second.get_item_by_index(i as isize).unwrap();
                    if fst == fst_value && snd == snd_value {
                        count += 1;
                    }
                }
                count
            }
            None => 0,
        }
    }

    fn index_of(&self, value: &Value) -> Option<usize> {
        match value_as_pair(value) {
            Some((fst_value, snd_value)) => {
                for i in 0..self.len() {
                    let fst = self.first.get_item_by_index(i as isize).unwrap();
                    let snd = self.second.get_item_by_index(i as isize).unwrap();
                    if fst == fst_value && snd == snd_value {
                        return Some(i);
                    }
                }
                None
            }
            None => None,
        }
    }

    fn clone_repr(&self) -> Box<dyn TupleRepr> {
        let first = self.first.clone_repr();
        let second = self.second.clone_repr();
        let repr = ZippedTupleRepr::new(first, second);
        Box::new(repr)
    }
}

#[derive(Debug)]
struct InnerOrderedDict {
    keys: Tuple,
    values: Tuple,
}

impl InnerOrderedDict {
    fn new(keys: Tuple, values: Tuple) -> Self {
        debug_assert!(keys.len() == values.len());
        Self { keys, values }
    }

    /// Count of entries.
    fn len(&self) -> usize {
        self.keys.len()
    }

    /// Pop a key from the ordered dict without any actual mutation.
    ///
    /// Returns:
    /// - `Some((value, new_inner))` if the key existed
    /// - `None` if the key was not found
    fn pop(&self, key: &Value) -> Option<(Value, Self)> {
        for i in 0..self.keys.len() {
            let k = self.keys.get(i as isize).unwrap_or_default();
            if k == *key {
                let value = self.values.get(i as isize).expect("value should exist");

                let new = Self::new(self.keys.excluding(i), self.values.excluding(i));

                return Some((value, new));
            }
        }
        None
    }
}

/// An object that behaves like a Python `OrderedDict`.
///
/// We need a custom implementation to avoid materializing the map in memory.
/// All operations are lazy and delegate to the underlying `TupleRepr` objects
/// produced by `MappedSequence`.
///
/// Mutations (like `pop`) are handled by swapping in a new inner dict with
/// excluded indices, using `im::HashSet` for efficient cloning.
#[derive(Debug)]
pub struct OrderedDict {
    /// The inner state, wrapped in a Mutex for safe swapping.
    inner: Mutex<InnerOrderedDict>,
}

impl OrderedDict {
    fn new(keys: Tuple, values: Tuple) -> Self {
        debug_assert!(keys.len() == values.len());
        Self {
            inner: Mutex::new(InnerOrderedDict::new(keys, values)),
        }
    }

    /// Retrieve the value for a given key.
    pub fn get(&self, key: &Value) -> Option<Value> {
        let inner = self.inner.lock().expect("lock poisoned");
        for i in 0..inner.keys.len() {
            let k = inner.keys.get(i as isize)?;
            if k == *key {
                return inner.values.get(i as isize);
            }
        }
        None
    }

    /// Return the keys as a Tuple.
    fn keys(&self) -> Tuple {
        let inner = self.inner.lock().expect("lock poisoned");
        inner.keys.clone_repr()
    }

    /// Return the values as a Tuple.
    fn values(&self) -> Tuple {
        let inner = self.inner.lock().expect("lock poisoned");
        inner.values.clone_repr()
    }

    /// The equivalent of tuple(zip(self.keys(), self.values())).
    pub fn items(&self) -> Option<Tuple> {
        let inner = self.inner.lock().expect("lock poisoned");
        let keys = inner.keys.0.clone_repr();
        let values = inner.values.0.clone_repr();
        let zipped = ZippedTupleRepr::new(keys, values);
        Some(Tuple(Box::new(zipped)))
    }

    /// Count of entries.
    #[allow(dead_code)]
    fn len(&self) -> usize {
        let inner = self.inner.lock().expect("lock poisoned");
        inner.len()
    }
}

impl fmt::Display for OrderedDict {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let inner = self.inner.lock().expect("lock poisoned");
        let len = inner.len();
        write!(f, "OrderedDict({{")?;
        for i in 0..len {
            let key = inner.keys.get(i as isize).unwrap();
            let value = inner.values.get(i as isize).unwrap();
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{key}: {value}")?;
        }
        write!(f, "}})")
    }
}

// TODO: any other methods we need to implement here?
impl Object for OrderedDict {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Map
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.get(key)
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            // --- read-only map-like methods ---
            "get" => {
                // get(key, default=None)
                let iter = ArgsIter::new("OrderedDict.get", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                let default = iter.next_kwarg::<Option<&Value>>("default")?;
                iter.finish()?;
                Ok(self
                    .get(key)
                    .unwrap_or_else(|| default.cloned().unwrap_or_else(|| Value::from(()))))
            }

            "keys" => {
                assert_nullary_args!("OrderedDict.keys", args)?;
                Ok(Value::from_object(self.keys()))
            }

            "values" => {
                assert_nullary_args!("OrderedDict.values", args)?;
                Ok(Value::from_object(self.values()))
            }

            "items" => {
                assert_nullary_args!("OrderedDict.items", args)?;
                Ok(Value::from_object(self.items().unwrap()))
            }

            "copy" => {
                assert_nullary_args!("OrderedDict.copy", args)?;
                // deep-ish copy: tuples are cheap to clone via clone_repr (uses Arc)
                let inner = self.inner.lock().expect("lock poisoned");
                let out = OrderedDict {
                    inner: Mutex::new(InnerOrderedDict::new(
                        inner.keys.clone_repr(),
                        inner.values.clone_repr(),
                    )),
                };
                Ok(Value::from_object(out))
            }

            "contains" => {
                // non-pythonic helper, but useful: contains(key) -> bool
                let iter = ArgsIter::for_unnamed_pos_args("OrderedDict.contains", 1, args);
                let key = iter.next_arg::<&Value>()?;
                iter.finish()?;
                Ok(Value::from(self.get(key).is_some()))
            }

            // --- mutating methods implemented via excluded ---
            "pop" => {
                // pop(key, default=())
                // If key exists: create new inner with excluded index and return value.
                // If missing: return default if provided, else error.
                let iter = ArgsIter::new("OrderedDict.pop", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                let default = iter.next_kwarg::<Option<&Value>>("default")?;
                iter.finish()?;

                let mut inner = self.inner.lock().expect("lock poisoned");
                if let Some((value, new)) = inner.pop(key) {
                    *inner = new;
                    Ok(value)
                } else if let Some(default) = default {
                    Ok(default.clone())
                } else {
                    Err(minijinja::Error::new(
                        ErrorKind::NonKey,
                        "pop(): key not found",
                    ))
                }
            }

            _ => Err(minijinja::Error::new(
                ErrorKind::UnknownMethod,
                format!("OrderedDict has no method named {name}"),
            )),
        }
    }
}

/// A generic container for immutable data that can be accessed either by
/// numeric index or by key. This is similar to a Python `OrderedDict` except
/// that the keys are optional and iteration over it returns the values instead
/// of keys.
///
/// Implementors should delegate Object and fmt::Display methods to this trait.
pub trait MappedSequence {
    // https://github.com/wireservice/agate/blob/master/agate/mapped_sequence.py

    /// The equivalent of Python's type(self).__name__.
    fn type_name(&self) -> &str {
        "MappedSequence"
    }

    /// Values as a Python tuple.
    ///
    /// __iter__ should iterate over the values in this sequence.
    /// enumerate(self) should enumerate the values in this sequence.
    /// __len__ should return the number of values in this sequence.
    /// __eq__, __ne__, and __contains__ should use these values.
    fn values(&self) -> Tuple;

    /// A Python list of the keys in the sequence (optional).
    fn keys(&self) -> Option<Tuple>;

    /// A tuple of (key, value) pairs in this [`MappedSequence`] (optional).
    fn items(&self) -> Option<Tuple> {
        let dict = self.dict()?;
        dict.items()
    }

    /// Retrieve the value for a given key, or a default value if the key is not
    /// present.
    fn get(self: &Arc<Self>, key: &Value, default: Option<&Value>) -> Value {
        let value = self.get_value(key);
        match value {
            Some(value) => value,
            None => default.cloned().unwrap_or_else(|| Value::from(())),
        }
    }

    /// Retrieve the contents of this sequence as an ordered dict.
    ///
    /// If keys() are not defined, this is also not defined.
    fn dict(&self) -> Option<OrderedDict> {
        self.keys()
            .map(|keys| OrderedDict::new(keys, self.values()))
    }

    // impl of the Object trait for MappedSequence objects ---------------------

    /// See [`minijinja::Value::repr`].
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Seq
    }

    /// Retrieve values from this array by index, slice or key.
    ///
    /// Based on MappedSequence.__getitem__.
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        if let Some(idx) = key.as_i64() {
            self.values().get(idx as isize)
        } else {
            let dict = self.dict()?;
            dict.get(key)
        }
    }

    /// See [`minijinja::Object::enumerate`].
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        Enumerator::Seq(self.values().len())
    }

    /// See [`minijinja::Object::call_method`].
    fn call_method(
        self: &Arc<Self>,
        state: &State,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            // MappedSequence methods
            "values" => {
                assert_nullary_args!("MappedSequence.values", args)?;
                let values = self.values();
                Ok(Value::from_object(values))
            }
            "keys" => {
                assert_nullary_args!("MappedSequence.keys", args)?;
                let keys = self
                    .keys()
                    .map(Value::from_object)
                    .unwrap_or_else(|| Value::from(()));
                Ok(keys)
            }
            "items" => {
                assert_nullary_args!("MappedSequence.items", args)?;
                if let Some(items) = self.items() {
                    Ok(Value::from_object(items))
                } else {
                    // trying to approximate a `raise KeyError`
                    Err(minijinja::Error::new(
                        ErrorKind::NonKey,
                        format!("{} type does not define keys()", self.type_name()),
                    ))
                }
            }
            "get" => {
                // def get(self, key, default=None)
                let iter = ArgsIter::new("MappedSequence.get", &["key"], args);
                let key = iter.next_arg::<&Value>()?;
                let default = iter.next_kwarg::<Option<&Value>>("default")?;
                iter.finish()?;
                let value = self.get(key, default);
                Ok(value)
            }
            "dict" => {
                assert_nullary_args!("MappedSequence.items", args)?;
                if let Some(dict) = self.dict() {
                    Ok(Value::from_object(dict))
                } else {
                    // trying to approximate a `raise KeyError`
                    Err(minijinja::Error::new(
                        ErrorKind::NonKey,
                        format!("{} type does not define keys()", self.type_name()),
                    ))
                }
            }
            _ => {
                if let Some(value) = self.get_value(&Value::from(name)) {
                    return value.call(state, args, listeners);
                }
                Err(minijinja::Error::new(
                    ErrorKind::UnknownMethod,
                    format!("{} has no method named {}", self.type_name(), name),
                ))
            }
        }
    }

    /// See [`minijinja::Object::render`].
    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.fmt(f)
    }

    // impl of the fmt::Display trait for MappedSequence objects -------------------

    /// Used to implement the equivalent of __unicode__, __str__, and __repr__ in Python.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let values = self.values();
        let len = values.len();
        write!(f, "<agate.{}: (", self.type_name())?;
        for i in 0..len.min(5) {
            if let Some(value) = values.get(i as isize) {
                write!(f, "{value}")?;
                if i < len - 1 {
                    write!(f, ", ")?;
                }
            }
        }
        if len > 5 {
            write!(f, ", ...")?;
        }
        write!(f, ")>")?;
        Ok(())
    }
}

pub fn adjusted_index(idx: isize, len: usize) -> Option<usize> {
    // Convert len to isize for consistent comparisons
    let len = len as isize;

    // Handle negative indices (e.g., -1 means last element)
    let adjusted = if idx < 0 { len + idx } else { idx };

    // Check if the adjusted index is within bounds
    if adjusted >= 0 && adjusted < len {
        Some(adjusted as usize)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use minijinja::Environment;
    use minijinja::value::Kwargs;

    use super::*;

    #[derive(Debug, Clone)]
    struct TestTupleRepr {
        values: Arc<Vec<Value>>,
    }

    impl TestTupleRepr {
        pub fn new(values: Arc<Vec<Value>>) -> Self {
            Self { values }
        }
    }

    impl TupleRepr for TestTupleRepr {
        fn get_item_by_index(&self, idx: isize) -> Option<Value> {
            self.values.get(idx as usize).cloned()
        }

        fn len(&self) -> usize {
            self.values.len()
        }

        fn count_occurrences_of(&self, value: &Value) -> usize {
            let mut count = 0;
            for v in self.values.iter() {
                if v == value {
                    count += 1;
                }
            }
            count
        }

        fn index_of(&self, value: &Value) -> Option<usize> {
            for (i, v) in self.values.iter().enumerate() {
                if v == value {
                    return Some(i);
                }
            }
            None
        }

        fn clone_repr(&self) -> Box<dyn TupleRepr> {
            Box::new(TestTupleRepr {
                values: Arc::clone(&self.values),
            })
        }
    }

    #[derive(Debug)]
    struct TestMappedSequence {
        keys: Arc<Vec<Value>>,
        values: Arc<Vec<Value>>,
    }

    impl TestMappedSequence {
        pub fn new(keys: Arc<Vec<Value>>, values: Arc<Vec<Value>>) -> Self {
            debug_assert!(keys.is_empty() || keys.len() == values.len());
            Self { keys, values }
        }
    }

    impl MappedSequence for TestMappedSequence {
        fn values(&self) -> Tuple {
            let repr = Box::new(TestTupleRepr::new(Arc::clone(&self.values)));
            Tuple(repr)
        }

        fn keys(&self) -> Option<Tuple> {
            if self.keys.is_empty() {
                None
            } else {
                let repr = Box::new(TestTupleRepr::new(Arc::clone(&self.keys)));
                Some(Tuple(repr))
            }
        }
    }

    impl fmt::Display for TestMappedSequence {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            MappedSequence::fmt(self, f)
        }
    }

    fn tuple(values: Vec<Value>) -> Tuple {
        Tuple(Box::new(TestTupleRepr::new(Arc::new(values))))
    }

    fn assert_tuple_values(tuple: &Tuple, expected: Vec<Value>) {
        assert_eq!(tuple.len(), expected.len());
        for (idx, expected) in expected.into_iter().enumerate() {
            assert_eq!(tuple.get(idx as isize), Some(expected));
        }
    }

    #[test]
    fn tuple_display_matches_python_tuple_punctuation() {
        assert_eq!(tuple(vec![]).to_string(), "()");
        assert_eq!(tuple(vec![Value::from(1)]).to_string(), "(1,)");
        assert_eq!(
            tuple(vec![Value::from(1), Value::from(2), Value::from(3)]).to_string(),
            "(1, 2, 3)"
        );
    }

    /// Regression for fs#14243: `query_to_list`-style macros stringify a single-column query
    /// result tuple directly (e.g. via a `|replace(",)", ")")` filter) to build SQL like
    /// `('2026-08-24 08:31:03.684529+00')`. Python's tuple `repr()` quotes string elements;
    /// this must too, or the generated SQL is missing its string-literal quotes entirely.
    #[test]
    fn tuple_display_quotes_string_elements_like_python_repr() {
        assert_eq!(
            tuple(vec![Value::from("2026-08-24 08:31:03.684529+00")]).to_string(),
            "('2026-08-24 08:31:03.684529+00',)"
        );
        // Non-string elements are unaffected -- numbers/bools/None still render bare.
        assert_eq!(
            tuple(vec![Value::from("a"), Value::from(1), Value::from(true)]).to_string(),
            "('a', 1, True)"
        );
        // A string containing a single quote switches to double quotes, matching
        // `ValueRepr`'s own Debug (Python repr) behavior for strings.
        assert_eq!(tuple(vec![Value::from("it's")]).to_string(), "(\"it's\",)");
        // A string containing both quote characters can't just switch quote style --
        // the embedded `'` must be escaped instead, or the literal breaks (fs#14245).
        assert_eq!(
            tuple(vec![Value::from("it's \"great\"")]).to_string(),
            r#"('it\'s "great"',)"#
        );
    }

    #[test]
    fn test_tuple() {
        let values = vec![Value::from(2), Value::from("biscoito")];
        let tuple = Arc::new(Tuple(Box::new(TestTupleRepr::new(Arc::new(values)))));

        let env = Environment::new();
        let state = env.empty_state();

        // tuple.count(2) => 1
        // tuple.count("biscoito") => 1
        // tuple.count(42) => 0
        // tuple.count("cookie") => 0
        let count = tuple
            .call_method(&state, "count", &[Value::from(2)], &[])
            .unwrap();
        assert_eq!(count, Value::from(1));
        let count = tuple
            .call_method(&state, "count", &[Value::from("biscoito")], &[])
            .unwrap();
        assert_eq!(count, Value::from(1));
        let count = tuple
            .call_method(&state, "count", &[Value::from(42)], &[])
            .unwrap();
        assert_eq!(count, Value::from(0));
        let count = tuple
            .call_method(&state, "count", &[Value::from("cookie")], &[])
            .unwrap();
        assert_eq!(count, Value::from(0));

        // tuple.index(2) => 0
        // tuple.index("biscoito") => 1
        // tuple.index(42) => None
        // tuple.index("cookie") => None
        let index = tuple
            .call_method(&state, "index", &[Value::from(2)], &[])
            .unwrap();
        assert_eq!(index, Value::from(0));
        let index = tuple
            .call_method(&state, "index", &[Value::from("biscoito")], &[])
            .unwrap();
        assert_eq!(index, Value::from(1));
        let index = tuple
            .call_method(&state, "index", &[Value::from(42)], &[])
            .unwrap();
        assert_eq!(index, Value::from(()));
        let index = tuple
            .call_method(&state, "index", &[Value::from("cookie")], &[])
            .unwrap();
        assert_eq!(index, Value::from(()));
    }

    #[test]
    fn tuple_repr_exclusion_and_object_methods() {
        let sample = tuple(vec![
            Value::from("a"),
            Value::from("b"),
            Value::from("a"),
            Value::from("c"),
        ]);

        assert!(!sample.is_empty());
        assert_eq!(sample.to_string(), "('a', 'b', 'a', 'c')");
        assert_eq!(
            Value::from_dyn_object(Arc::new(sample.clone_repr())).to_string(),
            "('a', 'b', 'a', 'c')"
        );
        assert_eq!(sample.count(&Value::from("a")), 2);
        assert_eq!(sample.count(&Value::from("missing")), 0);
        assert_eq!(sample.index(&Value::from("c")), Some(3));
        assert_eq!(sample.index(&Value::from("missing")), None);
        assert_eq!(
            sample,
            tuple(vec![
                Value::from("a"),
                Value::from("b"),
                Value::from("a"),
                Value::from("c"),
            ])
        );
        assert_ne!(sample, tuple(vec![Value::from("a"), Value::from("b")]));

        let excluded = sample.excluding(1);
        assert_tuple_values(
            &excluded,
            vec![Value::from("a"), Value::from("a"), Value::from("c")],
        );
        assert_eq!(excluded.count(&Value::from("a")), 2);
        assert_eq!(excluded.count(&Value::from("b")), 0);
        assert_eq!(excluded.index(&Value::from("c")), Some(2));

        let excluded_again = excluded.excluding(1);
        assert_tuple_values(&excluded_again, vec![Value::from("a"), Value::from("c")]);
        let out_of_bounds = excluded_again.excluding(99);
        assert_tuple_values(&out_of_bounds, vec![Value::from("a"), Value::from("c")]);

        let object_tuple = Arc::new(tuple(vec![Value::from(10), Value::from(20)]));
        assert_eq!(object_tuple.enumerator_len(), Some(2));
        assert_eq!(
            object_tuple.get_value(&Value::from(1)),
            Some(Value::from(20))
        );
        assert_eq!(object_tuple.get_value(&Value::from("not-an-index")), None);

        let empty = tuple(vec![]);
        assert!(empty.is_empty());
        assert_eq!(empty.to_string(), "()");
    }

    #[test]
    fn zipped_tuple_counts_indices_and_rejects_non_pairs() {
        let first = tuple(vec![Value::from("a"), Value::from("b"), Value::from("a")]);
        let second = tuple(vec![Value::from(1), Value::from(2), Value::from(1)]);
        let zipped = ZippedTupleRepr::from_tuples(&first, &second).into_tuple();

        let a_one = Value::from_iter([Value::from("a"), Value::from(1)]);
        let b_two = Value::from_iter([Value::from("b"), Value::from(2)]);
        let a_two = Value::from_iter([Value::from("a"), Value::from(2)]);

        assert_tuple_values(&zipped, vec![a_one.clone(), b_two.clone(), a_one.clone()]);
        assert_eq!(zipped.count(&a_one), 2);
        assert_eq!(zipped.count(&b_two), 1);
        assert_eq!(zipped.count(&a_two), 0);
        assert_eq!(zipped.index(&a_one), Some(0));
        assert_eq!(zipped.index(&b_two), Some(1));
        assert_eq!(zipped.index(&a_two), None);

        let not_a_pair = Value::from_iter([Value::from("a")]);
        assert_eq!(zipped.count(&not_a_pair), 0);
        assert_eq!(zipped.index(&not_a_pair), None);
    }

    #[test]
    fn test_mapped_sequence() {
        let keys = Arc::new(vec![Value::from("count"), Value::from("name")]);
        let values = Arc::new(vec![Value::from(2), Value::from("biscoito")]);
        let sequence = Arc::new(TestMappedSequence::new(
            Arc::clone(&keys),
            Arc::clone(&values),
        ));

        let keys_repr = TestTupleRepr::new(Arc::clone(&keys));
        let values_repr = TestTupleRepr::new(Arc::clone(&values));
        let expected_keys = Tuple(Box::new(keys_repr.clone()));
        let expected_values = Tuple(Box::new(values_repr.clone()));
        let expected_items = Tuple(Box::new(ZippedTupleRepr::new(
            Box::new(keys_repr),
            Box::new(values_repr),
        )));

        let env = Environment::new();
        let state = env.empty_state();

        // sequence.keys() => ("count", "name")
        let found_keys_as_value = sequence.call_method(&state, "keys", &[], &[]).unwrap();
        let found_keys = found_keys_as_value.downcast_object_ref::<Tuple>().unwrap();
        assert_eq!(*found_keys, expected_keys);

        // sequence.values() => (2, "biscoito")
        let found_values_as_value = sequence.call_method(&state, "values", &[], &[]).unwrap();
        let found_values = found_values_as_value
            .downcast_object_ref::<Tuple>()
            .unwrap();
        assert_eq!(*found_values, expected_values);

        // sequence.items() => (("count", 2), ("name", "biscoito"))
        let found_items_as_value = sequence.call_method(&state, "items", &[], &[]).unwrap();
        let found_items = found_items_as_value.downcast_object_ref::<Tuple>().unwrap();
        assert_eq!(*found_items, expected_items);

        // sequence.get("count") => 2
        // sequence.get("name") => "biscoito"
        // sequence.get("unknown") => ()
        let count_value = sequence
            .call_method(&state, "get", &[Value::from("count")], &[])
            .unwrap();
        assert_eq!(count_value, Value::from(2));
        let name_value = sequence
            .call_method(&state, "get", &[Value::from("name")], &[])
            .unwrap();
        assert_eq!(name_value, Value::from("biscoito"));
        let unknown_value = sequence
            .call_method(&state, "get", &[Value::from("unknown")], &[])
            .unwrap();
        assert_eq!(unknown_value, Value::from(()));

        // sequence.get("unknown", 42) => 42
        // sequence.get("unknown", default=1337) => 1337
        let default_value = sequence
            .call_method(
                &state,
                "get",
                &[Value::from("unknown"), Value::from(42)],
                &[],
            )
            .unwrap();
        assert_eq!(default_value, Value::from(42));
        let default_value = sequence
            .call_method(
                &state,
                "get",
                &[
                    Value::from("unknown"),
                    Value::from(Kwargs::from_iter(vec![("default", Value::from(1337))])),
                ],
                &[],
            )
            .unwrap();
        assert_eq!(default_value, Value::from(1337));

        // sequence.dict() => OrderedDict({"count": 2, "name": "biscoito"})
        let dict_value = sequence.call_method(&state, "dict", &[], &[]).unwrap();
        let dict = dict_value.downcast_object_ref::<OrderedDict>().unwrap();
        assert_eq!(dict.to_string(), "OrderedDict({count: 2, name: biscoito})");
    }

    #[test]
    fn mapped_sequence_without_keys_has_no_items_or_dict() {
        let sequence = Arc::new(TestMappedSequence::new(
            Arc::new(vec![]),
            Arc::new(vec![Value::from(2), Value::from("biscoito")]),
        ));

        let env = Environment::new();
        let state = env.empty_state();

        assert_eq!(sequence.type_name(), "MappedSequence");
        assert_eq!(sequence.get_value(&Value::from(0)), Some(Value::from(2)));
        assert_eq!(
            sequence.get_value(&Value::from(1)),
            Some(Value::from("biscoito"))
        );
        assert_eq!(sequence.get_value(&Value::from("count")), None);
        assert_eq!(
            sequence.get(&Value::from("missing"), Some(&Value::from(42))),
            Value::from(42)
        );

        assert_eq!(
            sequence.call_method(&state, "keys", &[], &[]).unwrap(),
            Value::from(())
        );
        let items_err = sequence.call_method(&state, "items", &[], &[]).unwrap_err();
        assert!(items_err.to_string().contains("does not define keys()"));
        let dict_err = sequence.call_method(&state, "dict", &[], &[]).unwrap_err();
        assert!(dict_err.to_string().contains("does not define keys()"));
    }

    #[test]
    fn adjusted_index_handles_negative_and_out_of_bounds_indices() {
        assert_eq!(adjusted_index(-1, 3), Some(2));
        assert_eq!(adjusted_index(-3, 3), Some(0));
        assert_eq!(adjusted_index(-4, 3), None);
        assert_eq!(adjusted_index(0, 3), Some(0));
        assert_eq!(adjusted_index(3, 3), None);
        assert_eq!(adjusted_index(0, 0), None);
    }

    #[test]
    fn ordered_dict_read_methods_copy_and_contains() {
        let keys = tuple(vec![Value::from("a"), Value::from("b")]);
        let values = tuple(vec![Value::from(1), Value::from(2)]);
        let dict = Arc::new(OrderedDict::new(keys, values));

        let env = Environment::new();
        let state = env.empty_state();

        assert_eq!(dict.len(), 2);
        assert_eq!(dict.get_value(&Value::from("a")), Some(Value::from(1)));
        assert_eq!(dict.get_value(&Value::from("missing")), None);
        assert_eq!(
            dict.call_method(&state, "get", &[Value::from("a")], &[])
                .unwrap(),
            Value::from(1)
        );
        assert_eq!(
            dict.call_method(
                &state,
                "get",
                &[
                    Value::from("missing"),
                    Value::from(Kwargs::from_iter(vec![("default", Value::from(99))])),
                ],
                &[],
            )
            .unwrap(),
            Value::from(99)
        );

        let keys = dict
            .call_method(&state, "keys", &[], &[])
            .unwrap()
            .downcast_object::<Tuple>()
            .unwrap();
        assert_tuple_values(&keys, vec![Value::from("a"), Value::from("b")]);
        let values = dict
            .call_method(&state, "values", &[], &[])
            .unwrap()
            .downcast_object::<Tuple>()
            .unwrap();
        assert_tuple_values(&values, vec![Value::from(1), Value::from(2)]);
        let items = dict
            .call_method(&state, "items", &[], &[])
            .unwrap()
            .downcast_object::<Tuple>()
            .unwrap();
        assert_tuple_values(
            &items,
            vec![
                Value::from_iter([Value::from("a"), Value::from(1)]),
                Value::from_iter([Value::from("b"), Value::from(2)]),
            ],
        );

        assert_eq!(
            dict.call_method(&state, "contains", &[Value::from("b")], &[])
                .unwrap(),
            Value::from(true)
        );
        assert_eq!(
            dict.call_method(&state, "contains", &[Value::from("missing")], &[])
                .unwrap(),
            Value::from(false)
        );

        let copied = dict.call_method(&state, "copy", &[], &[]).unwrap();
        assert_eq!(
            copied
                .downcast_object_ref::<OrderedDict>()
                .unwrap()
                .get(&Value::from("a")),
            Some(Value::from(1))
        );
        assert_eq!(
            copied
                .downcast_object_ref::<OrderedDict>()
                .unwrap()
                .get(&Value::from("b")),
            Some(Value::from(2))
        );
        assert_eq!(
            copied
                .call_method(&state, "pop", &[Value::from("a")], &[])
                .unwrap(),
            Value::from(1)
        );
        assert_eq!(
            copied
                .downcast_object_ref::<OrderedDict>()
                .unwrap()
                .get(&Value::from("a")),
            None
        );
        assert_eq!(
            copied.downcast_object_ref::<OrderedDict>().unwrap().len(),
            1
        );
        assert_eq!(
            copied
                .downcast_object_ref::<OrderedDict>()
                .unwrap()
                .get(&Value::from("b")),
            Some(Value::from(2))
        );
        assert_eq!(dict.to_string(), "OrderedDict({a: 1, b: 2})");
    }

    #[test]
    fn test_ordered_dict_pop() {
        // Create an OrderedDict with 3 key-value pairs
        let keys = Arc::new(vec![Value::from("a"), Value::from("b"), Value::from("c")]);
        let values = Arc::new(vec![Value::from(1), Value::from(2), Value::from(3)]);
        let keys_tuple = Tuple(Box::new(TestTupleRepr::new(keys)));
        let values_tuple = Tuple(Box::new(TestTupleRepr::new(values)));
        let dict = Arc::new(OrderedDict::new(keys_tuple, values_tuple));

        let env = Environment::new();
        let state = env.empty_state();

        // Initial state: {"a": 1, "b": 2, "c": 3}
        assert_eq!(dict.to_string(), "OrderedDict({a: 1, b: 2, c: 3})");

        // pop("b") should return 2 and remove "b"
        let popped_value = dict
            .call_method(&state, "pop", &[Value::from("b")], &[])
            .unwrap();
        assert_eq!(popped_value, Value::from(2));
        assert_eq!(dict.to_string(), "OrderedDict({a: 1, c: 3})");

        // After pop, "b" should not be accessible
        assert!(dict.get(&Value::from("b")).is_none());
        assert_eq!(dict.get(&Value::from("a")), Some(Value::from(1)));
        assert_eq!(dict.get(&Value::from("c")), Some(Value::from(3)));

        // pop("a") should return 1 and remove "a"
        let popped_value = dict
            .call_method(&state, "pop", &[Value::from("a")], &[])
            .unwrap();
        assert_eq!(popped_value, Value::from(1));
        assert_eq!(dict.to_string(), "OrderedDict({c: 3})");

        // pop("c") should return 3 and leave dict empty
        let popped_value = dict
            .call_method(&state, "pop", &[Value::from("c")], &[])
            .unwrap();
        assert_eq!(popped_value, Value::from(3));
        assert_eq!(dict.to_string(), "OrderedDict({})");

        // pop on missing key with default should return default
        let popped_value = dict
            .call_method(
                &state,
                "pop",
                &[
                    Value::from("missing"),
                    Value::from(Kwargs::from_iter(vec![("default", Value::from(42))])),
                ],
                &[],
            )
            .unwrap();
        assert_eq!(popped_value, Value::from(42));
    }

    /// Regression test for calling `__ne__` on `dbt_agate::Tuple`.
    ///
    /// Expected behavior:
    /// - Pre-fix: this test aborts the process with a stack overflow (infinite recursion).
    /// - Post-fix: this test completes successfully.
    #[test]
    fn tuple_ne_operator_does_not_stack_overflow() {
        let tuple = Arc::new(Tuple(Box::new(TestTupleRepr::new(Arc::new(vec![
            Value::from(1),
        ])))));

        let env = Environment::new();
        let ctx = minijinja::context! { t => Value::from_dyn_object(tuple) };

        // `!=` lowers to a `__ne__` method lookup on the left operand in MiniJinja.
        // Pre-fix this triggers infinite recursion in `Tuple::call_method`.
        let _out = env
            .render_str("{{ t != (1,) }}", ctx, &[])
            .expect("render should succeed without stack overflow");
    }
}
