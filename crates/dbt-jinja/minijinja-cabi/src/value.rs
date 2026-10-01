use std::collections::BTreeMap;
use std::ffi::{c_char, CStr, CString};
use std::mem::{transmute, ManuallyDrop};
use std::ops::Deref;
use std::sync::Arc;

use minijinja::value::{Object, ValueIter, ValueKind};
use minijinja::{Error, ErrorKind, Value};

use crate::utils::AbiResult;

/// Gives a mutable borrow into a value.
///
/// This is highly inefficient as it will today basically cause the
/// value to be cloned all the time, even if the refcount is just 1.
/// That's a limitation however that could be fixed by providing a
/// way for a `Value` of refcount 1 to expose a mutable reference to
/// the internal object.
fn with_cow<T, F>(slf: &mut mj_value, f: F) -> Result<(), Error>
where
    T: Default + Object + Clone + 'static,
    F: FnOnce(&mut T) -> Result<(), Error>,
{
    let mut map: Arc<T> = slf.downcast_object().ok_or_else(|| {
        let dummy = Value::from_object(T::default());
        Error::new(
            ErrorKind::InvalidOperation,
            format!("value is not a {}", dummy.kind()),
        )
    })?;
    f(Arc::make_mut(&mut map))?;
    unsafe {
        mj_value_decref(slf);
    }
    *slf = Value::from_dyn_object(map).into();
    Ok(())
}

/// Opaque value type.
#[repr(C)]
pub struct mj_value {
    // Motivation on the size here: The size of `Value` is really not
    // known and since cbindgen has no way to guarantee us a matching
    // size we have to be creative.  The dominating type size wise is
    // most likely going to be SmallStr which is a u8+[u8; 22] plus the
    // enum discriminant (u8).
    //
    // We are going with u64 here for alignment reasons which is likely
    // to be a good default across platforms.
    _opaque: [u64; 3],
}

impl mj_value {
    pub(crate) fn into_value(self) -> Value {
        unsafe { transmute(self._opaque) }
    }
}

impl From<Value> for mj_value {
    fn from(value: Value) -> Self {
        mj_value {
            _opaque: unsafe { transmute::<Value, [u64; 3]>(value) },
        }
    }
}

impl AbiResult for mj_value {
    fn err_value() -> Self {
        Self::from(Value::UNDEFINED)
    }
}

impl Deref for mj_value {
    type Target = Value;

    fn deref(&self) -> &Self::Target {
        unsafe { transmute(&self._opaque) }
    }
}

ffi_fn! {
    /// Creates a new none value.
    unsafe fn mj_value_new_none(_scope) -> mj_value {
        Value::from(()).into()
    }
}

ffi_fn! {
    /// Creates a new undefined value.
    unsafe fn mj_value_new_undefined(_scope) -> mj_value {
        Value::UNDEFINED.into()
    }
}

ffi_fn! {
    /// Creates a new string value
    unsafe fn mj_value_new_string(scope, s: *const c_char) -> mj_value {
        Value::from(scope.get_str(s)?).into()
    }
}

ffi_fn! {
    /// Creates a new boolean value
    unsafe fn mj_value_new_bool(_scope, value: bool) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new u32 value
    unsafe fn mj_value_new_u32(_scope, value: u32) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new i32 value
    unsafe fn mj_value_new_i32(_scope, value: i32) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new u64 value
    unsafe fn mj_value_new_u64(_scope, value: u64) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new i64 value
    unsafe fn mj_value_new_i64(_scope, value: i64) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new f32 value
    unsafe fn mj_value_new_f32(_scope, value: f32) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates a new f64 value
    unsafe fn mj_value_new_f64(_scope, value: f64) -> mj_value {
        Value::from(value).into()
    }
}

ffi_fn! {
    /// Creates an empty object value
    unsafe fn mj_value_new_object(_scope) -> mj_value {
        Value::from(BTreeMap::<Value, Value>::new()).into()
    }
}

ffi_fn! {
    /// Creates an empty list value
    unsafe fn mj_value_new_list(_scope) -> mj_value {
        Value::from(Vec::<Value>::new()).into()
    }
}

ffi_fn! {
    /// Inserts a string key into an object value.
    ///
    /// The value is moved into the object.
    unsafe fn mj_value_set_string_key(
        scope,
        slf: &mut mj_value,
        key: *const c_char,
        value: mj_value
    ) -> bool {
        mj_value_set_key(slf, Value::from(scope.get_str(key)?).into(), value)
    }
}

ffi_fn! {
    /// Inserts a key into an object value.
    ///
    /// The value is moved into the object.
    unsafe fn mj_value_set_key(
        _scope,
        slf: &mut mj_value,
        key: mj_value,
        value: mj_value
    ) -> bool {
        // TODO: make this work with other ValueMap types too.
        with_cow(slf, |map: &mut BTreeMap<Value, Value>| {
            map.insert(key.into_value(), value.into_value());
            Ok(())
        })?;
        true
    }
}

ffi_fn! {
    /// Appends a value to a list
    ///
    /// The value is moved into the list.
    unsafe fn mj_value_append(
        _scope,
        slf: &mut mj_value,
        value: mj_value,
    ) -> bool {
        with_cow(slf, |seq: &mut Vec<Value>| {
            seq.push(value.into_value());
            Ok(())
        })?;
        true
    }
}

/// The kind of a value.
#[repr(C)]
pub enum mj_value_kind {
    MJ_VALUE_KIND_UNDEFINED,
    MJ_VALUE_KIND_NONE,
    MJ_VALUE_KIND_BOOL,
    MJ_VALUE_KIND_NUMBER,
    MJ_VALUE_KIND_STRING,
    MJ_VALUE_KIND_BYTES,
    MJ_VALUE_KIND_SEQ,
    MJ_VALUE_KIND_MAP,
    MJ_VALUE_KIND_ITERABLE,
    MJ_VALUE_KIND_PLAIN,
    MJ_VALUE_KIND_INVALID,
}

impl AbiResult for mj_value_kind {
    fn err_value() -> Self {
        mj_value_kind::MJ_VALUE_KIND_INVALID
    }
}

impl TryFrom<ValueKind> for mj_value_kind {
    type Error = ();

    fn try_from(value: ValueKind) -> Result<Self, Self::Error> {
        Ok(match value {
            ValueKind::Undefined => mj_value_kind::MJ_VALUE_KIND_UNDEFINED,
            ValueKind::None => mj_value_kind::MJ_VALUE_KIND_NONE,
            ValueKind::Bool => mj_value_kind::MJ_VALUE_KIND_BOOL,
            ValueKind::Number => mj_value_kind::MJ_VALUE_KIND_NUMBER,
            ValueKind::String => mj_value_kind::MJ_VALUE_KIND_STRING,
            ValueKind::Bytes => mj_value_kind::MJ_VALUE_KIND_BYTES,
            ValueKind::Seq => mj_value_kind::MJ_VALUE_KIND_SEQ,
            ValueKind::Map => mj_value_kind::MJ_VALUE_KIND_MAP,
            ValueKind::Iterable => mj_value_kind::MJ_VALUE_KIND_ITERABLE,
            ValueKind::Plain => mj_value_kind::MJ_VALUE_KIND_PLAIN,
            ValueKind::Invalid => mj_value_kind::MJ_VALUE_KIND_INVALID,
            _ => return Err(()),
        })
    }
}

ffi_fn! {
    /// Returns the value kind.
    unsafe fn mj_value_get_kind(_scope, value: mj_value) -> mj_value_kind {
        value.kind().try_into().unwrap_or(mj_value_kind::MJ_VALUE_KIND_INVALID)
    }
}

ffi_fn! {
    /// Converts the value into a string.
    ///
    /// The returned string needs to be freed with `mj_str_free`.
    unsafe fn mj_value_to_str(_scope, value: mj_value) -> *mut c_char {
        CString::new(value.to_string()).map_err(|_| {
            Error::new(ErrorKind::InvalidOperation, "string contains null bytes")
        })?.into_raw()
    }
}

ffi_fn! {
    /// Extracts an integer from the value
    unsafe fn mj_value_as_i64(_scope, value: mj_value) -> i64 {
        value.as_i64().unwrap_or_default()
    }
}

ffi_fn! {
    /// Extracts an unsigned integer from the value
    unsafe fn mj_value_as_u64(_scope, value: mj_value) -> u64 {
        u64::try_from((*value).clone()).unwrap_or_default()
    }
}

ffi_fn! {
    /// Extracts a float from the value
    unsafe fn mj_value_as_f64(_scope, value: mj_value) -> f64 {
        f64::try_from((*value).clone()).unwrap_or_default()
    }
}

ffi_fn! {
    /// Checks if the value is truthy
    unsafe fn mj_value_is_true(_scope, value: mj_value) -> bool {
        value.is_true()
    }
}

ffi_fn! {
    /// Checks if the value is numeric
    unsafe fn mj_value_is_number(_scope, value: mj_value) -> bool {
        value.is_number()
    }
}

ffi_fn! {
    /// Returns the length of the object
    unsafe fn mj_value_len(_scope, value: mj_value) -> u64 {
        value.len().unwrap_or(0) as _
    }
}

ffi_fn! {
    /// Looks up an element by an integer index in a list of object
    unsafe fn mj_value_get_by_index(_scope, value: mj_value, idx: u64) -> mj_value {
        value.get_item_by_index(idx as usize).unwrap_or_default().into()
    }
}

ffi_fn! {
    /// Looks up an element by a string index in an object.
    unsafe fn mj_value_get_by_str(_scope, value: mj_value, key: *const c_char) -> mj_value {
        let key = CStr::from_ptr(key);
        if let Ok(key) = key.to_str() {
            value.get_attr(key).unwrap_or_default()
        } else {
            Value::UNDEFINED
        }.into()
    }
}

ffi_fn! {
    /// Looks up an element by a value
    unsafe fn mj_value_get_by_value(_scope, value: mj_value, key: mj_value) -> mj_value {
        value.get_item(&key as &Value).unwrap_or_default().into()
    }
}

/// Helps iterating over a value.
pub struct mj_value_iter(ValueIter);

ffi_fn! {
    /// Iterates over the value.
    unsafe fn mj_value_try_iter(_scope, value: mj_value) -> *mut mj_value_iter {
        Box::into_raw(Box::new(mj_value_iter(value.try_iter()?)))
    }
}

ffi_fn! {
    /// Yields the next value from the iterator.
    unsafe fn mj_value_iter_next(
        _scope,
        iter: *mut mj_value_iter,
        val_out: *mut mj_value
    ) -> bool {
        if let Some(next) = (*iter).0.next() {
            *val_out = mj_value::from(next);
            true
        } else {
            false
        }
    }
}

ffi_fn! {
    /// Ends the iteration and deallocates the iterator
    unsafe fn mj_value_iter_free(_scope, iter: *mut mj_value_iter) {
        let _ = Box::from_raw(iter);
    }
}

ffi_fn! {
    /// Increments the refcount
    unsafe fn mj_value_incref(_scope, value: *mut mj_value) {
        let value: &Value = &*value;
        let _ = ManuallyDrop::new(value.clone());
    }
}

ffi_fn! {
    /// Decrements the refcount
    unsafe fn mj_value_decref(_scope, value: *mut mj_value) {
        let mut value: ManuallyDrop<Value> = transmute((*value)._opaque);
        ManuallyDrop::drop(&mut value);
    }
}

ffi_fn! {
    /// Debug prints a value to stderr
    unsafe fn mj_value_dbg(_scope, value: mj_value) {
        eprintln!("{:?}", &value as &Value);
    }
}
