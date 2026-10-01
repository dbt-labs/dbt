use std::collections::BTreeMap;

use minijinja::Value;
use minijinja::value::mutable_vec::MutableVec;

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
