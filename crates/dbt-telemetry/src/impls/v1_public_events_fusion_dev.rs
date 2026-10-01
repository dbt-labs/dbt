use crate::proto::v1::public::events::fusion::dev::DebugValue;
use crate::proto::v1::public::events::fusion::dev::debug_value::Value;
use dbt_tracing::DebugValue as TracingDebugValue;

const MAX_EXACT_I64_IN_F64: i64 = 1i64 << f64::MANTISSA_DIGITS;

fn fits_exact_in_f64(n: i64) -> bool {
    // All integers with |n| <= 2^53 are exactly representable.
    (-MAX_EXACT_I64_IN_F64..=MAX_EXACT_I64_IN_F64).contains(&n)
}

impl From<i64> for DebugValue {
    fn from(value: i64) -> Self {
        if fits_exact_in_f64(value) {
            DebugValue {
                value: Some(Value::Float64(value as f64)),
            }
        } else {
            DebugValue {
                value: Some(Value::String(value.to_string())),
            }
        }
    }
}

impl From<u64> for DebugValue {
    fn from(value: u64) -> Self {
        if value <= MAX_EXACT_I64_IN_F64 as u64 {
            DebugValue {
                value: Some(Value::Float64(value as f64)),
            }
        } else {
            DebugValue {
                value: Some(Value::String(value.to_string())),
            }
        }
    }
}

impl From<f64> for DebugValue {
    fn from(value: f64) -> Self {
        DebugValue {
            value: Some(Value::Float64(value)),
        }
    }
}

impl From<bool> for DebugValue {
    fn from(value: bool) -> Self {
        DebugValue {
            value: Some(Value::Bool(value)),
        }
    }
}

impl From<String> for DebugValue {
    fn from(value: String) -> Self {
        DebugValue {
            value: Some(Value::String(value)),
        }
    }
}

impl From<&str> for DebugValue {
    fn from(value: &str) -> Self {
        DebugValue {
            value: Some(Value::String(value.to_string())),
        }
    }
}

impl From<Vec<u8>> for DebugValue {
    fn from(value: Vec<u8>) -> Self {
        DebugValue {
            value: Some(Value::Bytes(value)),
        }
    }
}

impl From<&[u8]> for DebugValue {
    fn from(value: &[u8]) -> Self {
        DebugValue {
            value: Some(Value::Bytes(value.into())),
        }
    }
}

impl From<TracingDebugValue> for DebugValue {
    fn from(value: TracingDebugValue) -> Self {
        DebugValue {
            value: Some(match value {
                TracingDebugValue::Float64(value) => Value::Float64(value),
                TracingDebugValue::Bool(value) => Value::Bool(value),
                TracingDebugValue::String(value) => Value::String(value),
                TracingDebugValue::Bytes(value) => Value::Bytes(value),
            }),
        }
    }
}
