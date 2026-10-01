/// Generic debug value holder for unstructured TRACE span fields.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugValue {
    Float64(f64),
    Bool(bool),
    String(String),
    Bytes(Vec<u8>),
}

const MAX_EXACT_I64_IN_F64: i64 = 1i64 << f64::MANTISSA_DIGITS;

fn fits_exact_in_f64(n: i64) -> bool {
    // All integers with |n| <= 2^53 are exactly representable.
    (-MAX_EXACT_I64_IN_F64..=MAX_EXACT_I64_IN_F64).contains(&n)
}

impl From<i64> for DebugValue {
    fn from(value: i64) -> Self {
        if fits_exact_in_f64(value) {
            Self::Float64(value as f64)
        } else {
            Self::String(value.to_string())
        }
    }
}

impl From<u64> for DebugValue {
    fn from(value: u64) -> Self {
        if value <= MAX_EXACT_I64_IN_F64 as u64 {
            Self::Float64(value as f64)
        } else {
            Self::String(value.to_string())
        }
    }
}

impl From<f64> for DebugValue {
    fn from(value: f64) -> Self {
        Self::Float64(value)
    }
}

impl From<bool> for DebugValue {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl From<String> for DebugValue {
    fn from(value: String) -> Self {
        Self::String(value)
    }
}

impl From<&str> for DebugValue {
    fn from(value: &str) -> Self {
        Self::String(value.to_string())
    }
}

impl From<Vec<u8>> for DebugValue {
    fn from(value: Vec<u8>) -> Self {
        Self::Bytes(value)
    }
}

impl From<&[u8]> for DebugValue {
    fn from(value: &[u8]) -> Self {
        Self::Bytes(value.into())
    }
}
