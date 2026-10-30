use std::collections::BTreeMap;
use std::sync::Arc;

use minijinja::Value;
use minijinja::value::mutable_vec::MutableVec;

use crate::Column;

pub fn none_value() -> Value {
    Value::from(())
}

pub fn empty_string_value() -> Value {
    Value::from("")
}

pub fn empty_vec_value() -> Value {
    Value::from(Vec::<Value>::new())
}

pub fn empty_mutable_vec_value() -> Value {
    Value::from(MutableVec::<Value>::new())
}

pub fn empty_map_value() -> Value {
    Value::from(BTreeMap::<Value, Value>::new())
}

pub fn columns_to_value(columns: Vec<Arc<Column>>) -> Value {
    let vec = MutableVec::from_iter(columns.into_iter().map(Arc::unwrap_or_clone));
    Value::from(vec)
}
