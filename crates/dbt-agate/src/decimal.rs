use arrow::array::ArrowNativeTypeOp as _;
use arrow::datatypes::DecimalType;
use minijinja::value::{Object, ObjectRepr};
use minijinja::{Error, Value};
use std::fmt;
use std::sync::Arc;

// The interface is supposed to be similar to Python's decimal module [1][2],
// and the implementation is based on the code from the arrow-arith crate.
//
// Precision is limited to 38 (128 bits) or 76 (256 bits) decimal digits
// unlike Python's 999999999999999999 decimal digits limit due to its use
// of bigint arithmetic under the hood.
//
// [1] https://docs.python.org/3/library/decimal.html
// [2] https://github.com/python/cpython/blob/3.13/Lib/_pydecimal.py

/// Rounding modes.
#[allow(non_camel_case_types)]
#[allow(dead_code)]
pub enum Rounding {
    /// Round towards Infinity.
    ROUND_CEILING,
    /// Round towards zero.
    ROUND_DOWN,
    /// Round towards -Infinity.
    ROUND_FLOOR,
    /// Round to nearest with ties going towards zero.
    ROUND_HALF_DOWN,
    /// Round to nearest with ties going to nearest even integer.
    ROUND_HALF_EVEN,
    /// Round to nearest with ties going away from zero.
    ROUND_HALF_UP,
    // Round away from zero.
    ROUND_UP,
    /// Round away from zero if last digit after rounding towards zero would have been 0 or 5; otherwise round towards zero.
    ROUND_05UP,
}

/// See _pydecimal.py for available flags.
pub enum Flag {}

/// See _pydecimal.py for available traps.
pub enum Trap {}

/// Context for decimal operations.
///
/// Based on https://docs.python.org/3/library/decimal.html#decimal.Context
#[allow(dead_code)]
#[allow(non_snake_case)]
pub struct Context {
    /// Precision for arithmetic operations in the context.
    prec: u8,
    /// Rounding mode.
    rounding: Rounding,
    /// The `Emin` and `Emax` fields are integers specifying the outer limits allowable
    /// for exponents. `Emin` must be in the range `[MIN_EMIN, 0]`, `Emax` in the range
    /// `[0, MAX_EMAX]`.
    Emin: i32,
    /// The `Emin` and `Emax` fields are integers specifying the outer limits allowable
    /// for exponents. `Emin` must be in the range `[MIN_EMIN, 0]`, `Emax` in the range
    /// `[0, MAX_EMAX]`.
    Emax: i32,
    /// If set to 1, exponents are printed with a capital E; otherwise, a lowercase e is
    /// used: `Decimal('6.02e+23')`.
    capitals: bool,
    /// See Python docs.
    clamp: u8,
    /// See Python docs.
    flags: Vec<Flag>,
    /// See Python docs.
    traps: Vec<Trap>,
}

impl Context {
    const fn default() -> Self {
        Self {
            prec: 28,
            rounding: Rounding::ROUND_HALF_EVEN,
            Emin: -77,
            Emax: 77,
            capitals: true,
            clamp: 0,
            flags: vec![],
            traps: vec![],
        }
    }
}

impl Default for Context {
    fn default() -> Self {
        Self::default()
    }
}

/// See https://docs.python.org/3/library/decimal.html#decimal.DefaultContext
pub const DEFAULT_CONTEXT: Context = Context::default();

pub struct DecimalValue<T: DecimalType> {
    value: T::Native,
    precision: u8,
    scale: i8,
}

impl<T: DecimalType> DecimalValue<T> {
    pub fn new(value: T::Native, precision: u8, scale: i8) -> Self {
        #[cfg(debug_assertions)]
        {
            T::validate_decimal_precision(value, precision).unwrap();
        }
        Self {
            value,
            precision,
            scale,
        }
    }

    pub fn is_zero(&self) -> bool {
        self.value == T::Native::ZERO
    }

    // TODO: implement operations on decimal scalars like
    // the code in the arrow-arith crate
}

impl<T: DecimalType> fmt::Debug for DecimalValue<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO: use default context to align with Python's decimal module formatting
        let _ = &DEFAULT_CONTEXT;
        let text = T::format_decimal(self.value, self.precision, self.scale);
        write!(f, "{text}")
    }
}

impl<T: DecimalType> Object for DecimalValue<T> {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn is_true(self: &Arc<Self>) -> bool {
        !self.is_zero()
    }

    fn call_method(
        self: &Arc<Self>,
        state: &minijinja::State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, Error> {
        if let Some(value) = self.get_value(&Value::from(method)) {
            return value.call(state, args, listeners);
        }

        // TODO: implement decimal methods

        Err(Error::new(
            minijinja::ErrorKind::UnknownMethod,
            format!("DecimalValue has no method named {method}"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Decimal128Type;

    #[test]
    fn decimal_value_truthiness_tracks_zero_state() {
        let zero = Arc::new(DecimalValue::<Decimal128Type>::new(0, 10, 2));
        assert!(zero.is_zero());
        assert!(!zero.is_true());

        let value = Arc::new(DecimalValue::<Decimal128Type>::new(12345, 10, 2));
        assert!(!value.is_zero());
        assert!(value.is_true());
    }
}
