//! Converters from Arrow to minijinja values.
//!
//!

use arrow::array::{AsArray as _, PrimitiveArray};
use arrow::buffer::{BooleanBuffer, NullBuffer, OffsetBuffer, ScalarBuffer};
use arrow::compute::{CastOptions, cast_with_options};
use arrow::datatypes::*;
use arrow::util::display::FormatOptions;
use arrow_array::{
    Array, ArrowPrimitiveType, BooleanArray, GenericByteArray, GenericByteViewArray,
    GenericListArray, MapArray, OffsetSizeTrait, StructArray,
};
use arrow_buffer::i256;
use arrow_schema::ArrowError;
use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use minijinja::{Value, value::ValueMap};
use minijinja_contrib::modules::py_datetime::date::PyDate;
use minijinja_contrib::modules::py_datetime::datetime::PyDateTime;
use minijinja_contrib::modules::py_datetime::time::PyTime;
use minijinja_contrib::modules::pytz::PytzTimezone;
use std::str::FromStr;
use std::sync::Arc;

use crate::decimal::DecimalValue;

/// Converts the i-th element of an Arrow array to a minijinja Value.
pub trait ArrayConverter: Send + Sync {
    fn to_value(&self, idx: usize) -> Value;
}

// Boolean, Integers, and Floats {{{
struct BooleanArrayConverter {
    values: BooleanBuffer,
    nulls: Option<NullBuffer>,
}

impl BooleanArrayConverter {
    pub fn new(array: &BooleanArray) -> Self {
        Self {
            values: array.values().clone(),
            nulls: array.nulls().cloned(),
        }
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }
}

impl ArrayConverter for BooleanArrayConverter {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let value: bool = self.values.value(idx);
            Value::from(value)
        } else {
            Value::from(())
        }
    }
}

struct PrimitiveArrayConverter<T: ArrowPrimitiveType> {
    values: ScalarBuffer<T::Native>,
    nulls: Option<NullBuffer>,
}

impl<T: ArrowPrimitiveType> PrimitiveArrayConverter<T> {
    pub fn new(array: &PrimitiveArray<T>) -> Self {
        Self {
            values: array.values().clone(),
            nulls: array.nulls().cloned(),
        }
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }
}

macro_rules! make_primitive_array_converter {
    ($arrow_data_ty:ty) => {
        impl ArrayConverter for PrimitiveArrayConverter<$arrow_data_ty> {
            fn to_value(&self, idx: usize) -> Value {
                if self.is_valid(idx) {
                    self.values[idx].into()
                } else {
                    Value::from(())
                }
            }
        }
    };
}

make_primitive_array_converter!(Int8Type);
make_primitive_array_converter!(Int16Type);
make_primitive_array_converter!(Int32Type);
make_primitive_array_converter!(Int64Type);
make_primitive_array_converter!(UInt8Type);
make_primitive_array_converter!(UInt16Type);
make_primitive_array_converter!(UInt32Type);
make_primitive_array_converter!(UInt64Type);
make_primitive_array_converter!(Float32Type);
make_primitive_array_converter!(Float64Type);
// }}}

// Decimals {{{
struct DecimalArrayConverter<T: DecimalType> {
    values: ScalarBuffer<T::Native>,
    nulls: Option<NullBuffer>,
    precision: u8,
    scale: i8,
}

impl<T: DecimalType> DecimalArrayConverter<T> {
    pub fn new(array: &PrimitiveArray<T>) -> Self {
        Self {
            values: array.values().clone(),
            nulls: array.nulls().cloned(),
            precision: array.precision(),
            scale: array.scale(),
        }
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }
}

trait ConvertibleToI128 {
    fn to_i128(self) -> Option<i128>;
}

impl ConvertibleToI128 for i32 {
    fn to_i128(self) -> Option<i128> {
        Some(self as i128)
    }
}

impl ConvertibleToI128 for i64 {
    fn to_i128(self) -> Option<i128> {
        Some(self as i128)
    }
}

impl ConvertibleToI128 for i128 {
    fn to_i128(self) -> Option<i128> {
        Some(self)
    }
}

impl ConvertibleToI128 for i256 {
    fn to_i128(self) -> Option<i128> {
        // This is a copy of [arrow_buffer::bigint::i256::to_i128] that
        // unfortunately we can't use here because it's private.
        let (low, high) = self.to_parts();
        let as_i128 = low as i128;

        let high_negative = high < 0;
        let low_negative = as_i128 < 0;
        let high_valid = high == -1 || high == 0;

        (high_negative == low_negative && high_valid).then_some(low as i128)
    }
}

impl<T: DecimalType> ArrayConverter for DecimalArrayConverter<T>
where
    T::Native: ConvertibleToI128,
{
    fn to_value(&self, idx: usize) -> Value {
        const DECIMAL64_MAX_PRECISION: u8 = 19; // log10(2^64) ~= 19.265

        if self.is_valid(idx) {
            let value_bits = self.values[idx];
            // Is this decimal just an integer?
            if self.scale == 0 {
                // Does this integer fit in 64 bits?
                if self.precision <= DECIMAL64_MAX_PRECISION {
                    return Value::from(value_bits.to_i64());
                }
                // Does this integer fit in a 128-bit minijinja integer?
                if T::BYTE_LENGTH == 16 || self.precision <= Decimal128Type::MAX_PRECISION {
                    let value_bits128 = value_bits.to_i128().unwrap();
                    return Value::from(value_bits128);
                }
            }
            let value = DecimalValue::<T>::new(value_bits, self.precision, self.scale);
            Value::from_object(value)
        } else {
            Value::from(())
        }
    }
}
// }}}

// Date and Time {{{
/// Number of days between 0001-01-01 and 1970-01-01
const EPOCH_DAYS_FROM_CE: i32 = 719_163;
/// Number of milliseconds in a day
const NUM_MILLIS_PER_DAY: i64 = 86_400_000; // 24 * 60 * 60 * 1000

impl ArrayConverter for PrimitiveArrayConverter<Date32Type> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let num_days_from_epoch = self.values[idx];
            let num_days_from_ce = num_days_from_epoch + EPOCH_DAYS_FROM_CE;
            let naive_date_opt = NaiveDate::from_num_days_from_ce_opt(num_days_from_ce);
            debug_assert!(
                naive_date_opt.is_some(),
                "out-of-range date32 value: {num_days_from_epoch}"
            );
            match naive_date_opt {
                Some(naive_date) => Value::from_object(PyDate::new(naive_date)),
                None => {
                    // Handle out-of-range dates gracefully, but out-of-range
                    // date32 value is most likely a bug somewhere, hence the
                    // debug_assert above.
                    Value::from(())
                }
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for PrimitiveArrayConverter<Date64Type> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let num_millis_from_epoch = self.values[idx];
            let num_days_from_epoch = num_millis_from_epoch / NUM_MILLIS_PER_DAY;
            let num_days_from_ce = num_days_from_epoch as i32 + EPOCH_DAYS_FROM_CE;
            let naive_date_opt = NaiveDate::from_num_days_from_ce_opt(num_days_from_ce);
            debug_assert!(
                naive_date_opt.is_some(),
                "out-of-range date64 value: {num_millis_from_epoch}"
            );
            match naive_date_opt {
                Some(naive_date) => Value::from_object(PyDate::new(naive_date)),
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for PrimitiveArrayConverter<Time32SecondType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let naive_time_opt = u32::try_from(self.values[idx]).ok().and_then(|seconds| {
                let naive_time_opt = NaiveTime::from_num_seconds_from_midnight_opt(seconds, 0);
                debug_assert!(
                    naive_time_opt.is_some(),
                    "out-of-range time32 (seconds) value: {seconds}"
                );
                naive_time_opt
            });
            match naive_time_opt {
                Some(naive_time) => Value::from_object(PyTime::new(naive_time, None)),
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for PrimitiveArrayConverter<Time32MillisecondType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let naive_time_opt = u32::try_from(self.values[idx]).ok().and_then(|millis| {
                let secs = millis / 1000;
                let nano = (millis % 1000) * 1_000_000;
                let naive_time_opt = NaiveTime::from_num_seconds_from_midnight_opt(secs, nano);
                debug_assert!(
                    naive_time_opt.is_some(),
                    "out-of-range time32 (milliseconds) value: {millis}"
                );
                naive_time_opt
            });
            match naive_time_opt {
                Some(naive_time) => Value::from_object(PyTime::new(naive_time, None)),
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for PrimitiveArrayConverter<Time64MicrosecondType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let naive_time_opt = u64::try_from(self.values[idx]).ok().and_then(|micros| {
                let secs = (micros / 1_000_000) as u32;
                let nano = ((micros % 1_000_000) * 1_000) as u32;
                let naive_time_opt = NaiveTime::from_num_seconds_from_midnight_opt(secs, nano);
                debug_assert!(
                    naive_time_opt.is_some(),
                    "out-of-range time64 (microseconds) value: {micros}"
                );
                naive_time_opt
            });
            match naive_time_opt {
                Some(naive_time) => Value::from_object(PyTime::new(naive_time, None)),
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for PrimitiveArrayConverter<Time64NanosecondType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let naive_time_opt = u64::try_from(self.values[idx]).ok().and_then(|nanos| {
                let secs = (nanos / 1_000_000_000) as u32;
                let nano_frac = (nanos % 1_000_000_000) as u32;
                let naive_time_opt = NaiveTime::from_num_seconds_from_midnight_opt(secs, nano_frac);
                debug_assert!(
                    naive_time_opt.is_some(),
                    "out-of-range time64 (nanoseconds) value: {nanos}"
                );
                naive_time_opt
            });
            match naive_time_opt {
                Some(naive_time) => Value::from_object(PyTime::new(naive_time, None)),
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

pub struct TimestampArrayConverter<T: ArrowPrimitiveType> {
    array: PrimitiveArray<T>,
    tz: Option<Tz>,
    py_tz: Option<PytzTimezone>,
}

impl<T: ArrowPrimitiveType<Native = i64>> TimestampArrayConverter<T> {
    pub fn new(array: PrimitiveArray<T>, maybe_tz_str: Option<Arc<str>>) -> Self {
        let tz_opt = maybe_tz_str.as_ref().and_then(|tz_str| {
            let parse_result = Tz::from_str(tz_str);
            debug_assert!(
                parse_result.is_ok(),
                "unexpected timezone string from Arrow {tz_str}: {parse_result:?}"
            );
            parse_result.ok()
        });

        let py_tz_opt = tz_opt.as_ref().map(|tz| PytzTimezone { tz: *tz });

        Self {
            array,
            tz: tz_opt,
            py_tz: py_tz_opt,
        }
    }

    fn from_arrow_timestamp(raw: i64, time_unit: TimeUnit) -> Option<NaiveDateTime> {
        match time_unit {
            TimeUnit::Second => DateTime::<Utc>::from_timestamp(raw, 0).map(|dt| dt.naive_utc()),
            TimeUnit::Millisecond => {
                DateTime::<Utc>::from_timestamp_millis(raw).map(|dt| dt.naive_utc())
            }
            TimeUnit::Microsecond => {
                DateTime::<Utc>::from_timestamp_micros(raw).map(|dt| dt.naive_utc())
            }
            TimeUnit::Nanosecond => DateTime::<Utc>::from_timestamp_nanos(raw)
                .naive_utc()
                .into(),
        }
    }

    fn do_to_value(&self, idx: usize, time_unit: TimeUnit) -> Value {
        if self.array.is_valid(idx) {
            let raw = self.array.value(idx);

            let naive_opt = Self::from_arrow_timestamp(raw, time_unit);
            debug_assert!(
                naive_opt.is_some(),
                "out-of-range timestamp ({time_unit:?}) value: {raw}"
            );

            match naive_opt {
                Some(naive) => {
                    let py_dt = self
                        .tz
                        .map(|tz| tz.from_utc_datetime(&naive))
                        .map(|aware| {
                            debug_assert!(
                                self.py_tz.is_some(),
                                "py_tz must be some when tz is some"
                            );
                            PyDateTime::new_aware(aware, self.py_tz.clone())
                        })
                        .unwrap_or_else(|| PyDateTime::new_naive(naive));

                    Value::from_object(py_dt)
                }
                None => Value::from(()),
            }
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for TimestampArrayConverter<TimestampSecondType> {
    fn to_value(&self, idx: usize) -> Value {
        self.do_to_value(idx, TimeUnit::Second)
    }
}

impl ArrayConverter for TimestampArrayConverter<TimestampMillisecondType> {
    fn to_value(&self, idx: usize) -> Value {
        self.do_to_value(idx, TimeUnit::Millisecond)
    }
}

impl ArrayConverter for TimestampArrayConverter<TimestampMicrosecondType> {
    fn to_value(&self, idx: usize) -> Value {
        self.do_to_value(idx, TimeUnit::Microsecond)
    }
}

impl ArrayConverter for TimestampArrayConverter<TimestampNanosecondType> {
    fn to_value(&self, idx: usize) -> Value {
        self.do_to_value(idx, TimeUnit::Nanosecond)
    }
}
// }}}

// String and Binary {{{
struct GenericByteArrayConverter<T: ByteArrayType> {
    array: GenericByteArray<T>,
}

impl<T: ByteArrayType> GenericByteArrayConverter<T> {
    pub fn new(array: &GenericByteArray<T>) -> Self {
        Self {
            array: array.clone(),
        }
    }
}

impl<O: OffsetSizeTrait> ArrayConverter for GenericByteArrayConverter<GenericStringType<O>> {
    fn to_value(&self, idx: usize) -> Value {
        if self.array.is_valid(idx) {
            let value = self.array.value(idx);
            Value::from(value)
        } else {
            Value::from(())
        }
    }
}

impl<O: OffsetSizeTrait> ArrayConverter for GenericByteArrayConverter<GenericBinaryType<O>> {
    fn to_value(&self, idx: usize) -> Value {
        if self.array.is_valid(idx) {
            let value = self.array.value(idx);
            Value::from(value)
        } else {
            Value::from(())
        }
    }
}

type GenericStringArrayConverter<O> = GenericByteArrayConverter<GenericStringType<O>>;
type StringArrayConverter = GenericStringArrayConverter<i32>;
type LargeStringArrayConverter = GenericStringArrayConverter<i64>;

type GenericBinaryArrayConverter<O> = GenericByteArrayConverter<GenericBinaryType<O>>;
type BinaryArrayConverter = GenericBinaryArrayConverter<i32>;
type LargeBinaryArrayConverter = GenericBinaryArrayConverter<i64>;
// }}}

// StringView and BinaryView {{{
struct ByteViewArrayConverter<T: ByteViewType> {
    array: GenericByteViewArray<T>,
}

impl<T: ByteViewType> ByteViewArrayConverter<T> {
    pub fn new(array: &GenericByteViewArray<T>) -> Self {
        Self {
            array: array.clone(),
        }
    }
}

impl ArrayConverter for ByteViewArrayConverter<StringViewType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.array.is_valid(idx) {
            let value = self.array.value(idx);
            Value::from(value)
        } else {
            Value::from(())
        }
    }
}

impl ArrayConverter for ByteViewArrayConverter<BinaryViewType> {
    fn to_value(&self, idx: usize) -> Value {
        if self.array.is_valid(idx) {
            let value = self.array.value(idx);
            Value::from(value)
        } else {
            Value::from(())
        }
    }
}

type StringViewArrayConverter = ByteViewArrayConverter<StringViewType>;
type BinaryViewArrayConverter = ByteViewArrayConverter<BinaryViewType>;
// }}}

// List {{{
struct GenericListArrayConverter<O: OffsetSizeTrait> {
    offsets: OffsetBuffer<O>,
    child: Box<dyn ArrayConverter>,
    nulls: Option<NullBuffer>,
}

impl<O: OffsetSizeTrait> GenericListArrayConverter<O> {
    pub fn new(list_array: &GenericListArray<O>) -> Result<Self, ArrowError> {
        let child_array = list_array.values();
        let child = make_array_converter(child_array.as_ref())?;
        Ok(Self {
            offsets: list_array.offsets().clone(),
            child,
            nulls: list_array.nulls().cloned(),
        })
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }

    #[inline(always)]
    fn range_for(&self, idx: usize) -> std::ops::Range<usize> {
        let start = self.offsets[idx].as_usize();
        let end = self.offsets[idx + 1].as_usize();
        start..end
    }
}

impl<O: OffsetSizeTrait> ArrayConverter for GenericListArrayConverter<O> {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let range = self.range_for(idx);
            let mut elems = Vec::with_capacity(range.len());

            for child_idx in range {
                elems.push(self.child.to_value(child_idx));
            }
            Value::from(elems)
        } else {
            Value::from(())
        }
    }
}

type ListArrayConverter = GenericListArrayConverter<i32>;
type LargeListArrayConverter = GenericListArrayConverter<i64>;
// }}}

// Map {{{
struct MapConverter {
    keys: Box<dyn ArrayConverter>,
    values: Box<dyn ArrayConverter>,
    offsets: OffsetBuffer<i32>,
    nulls: Option<NullBuffer>,
}

impl MapConverter {
    pub fn new(array: &MapArray) -> Result<Self, ArrowError> {
        let keys = make_array_converter(array.keys().as_ref())?;
        let values = make_array_converter(array.values().as_ref())?;
        Ok(Self {
            keys,
            values,
            offsets: array.offsets().clone(),
            nulls: array.nulls().cloned(),
        })
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }

    #[inline(always)]
    fn range_for(&self, idx: usize) -> std::ops::Range<usize> {
        let start = self.offsets[idx].as_usize();
        let end = self.offsets[idx + 1].as_usize();
        start..end
    }
}

impl ArrayConverter for MapConverter {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let npairs = self.range_for(idx);
            let mut elems = ValueMap::with_capacity(npairs.len());

            for idx in npairs {
                elems.insert(self.keys.to_value(idx), self.values.to_value(idx));
            }
            Value::from(elems)
        } else {
            Value::from(())
        }
    }
}

// }}}

// Struct {{{
struct StructConverter {
    fields: Fields,
    field_converters: Vec<Box<dyn ArrayConverter>>,
    nulls: Option<NullBuffer>,
}

impl StructConverter {
    pub fn new(array: &StructArray) -> Result<Self, ArrowError> {
        debug_assert_eq!(
            array.fields().len(),
            array.columns().len(),
            "struct field count must match column count"
        );
        let mut field_converters = Vec::with_capacity(array.num_columns());
        for column in array.columns() {
            field_converters.push(make_array_converter(column.as_ref())?);
        }
        Ok(Self {
            fields: array.fields().clone(),
            field_converters,
            nulls: array.nulls().cloned(),
        })
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }
}

impl ArrayConverter for StructConverter {
    fn to_value(&self, idx: usize) -> Value {
        if self.is_valid(idx) {
            let mut elems = ValueMap::with_capacity(self.fields.len());
            for (field, conv) in self.fields.iter().zip(self.field_converters.iter()) {
                elems.insert(Value::from(field.name().as_str()), conv.to_value(idx));
            }
            Value::from(elems)
        } else {
            Value::from(())
        }
    }
}
// }}}

pub fn make_array_converter(array: &dyn Array) -> Result<Box<dyn ArrayConverter>, ArrowError> {
    let converter: Box<dyn ArrayConverter> = match array.data_type() {
        DataType::Boolean => Box::new(BooleanArrayConverter::new(array.as_boolean())),
        DataType::Int8 => Box::new(PrimitiveArrayConverter::<Int8Type>::new(
            array.as_primitive::<Int8Type>(),
        )),
        DataType::Int16 => Box::new(PrimitiveArrayConverter::<Int16Type>::new(
            array.as_primitive::<Int16Type>(),
        )),
        DataType::Int32 => Box::new(PrimitiveArrayConverter::<Int32Type>::new(
            array.as_primitive::<Int32Type>(),
        )),
        DataType::Int64 => Box::new(PrimitiveArrayConverter::<Int64Type>::new(
            array.as_primitive::<Int64Type>(),
        )),
        DataType::UInt8 => Box::new(PrimitiveArrayConverter::<UInt8Type>::new(
            array.as_primitive::<UInt8Type>(),
        )),
        DataType::UInt16 => Box::new(PrimitiveArrayConverter::<UInt16Type>::new(
            array.as_primitive::<UInt16Type>(),
        )),
        DataType::UInt32 => Box::new(PrimitiveArrayConverter::<UInt32Type>::new(
            array.as_primitive::<UInt32Type>(),
        )),
        DataType::UInt64 => Box::new(PrimitiveArrayConverter::<UInt64Type>::new(
            array.as_primitive::<UInt64Type>(),
        )),
        DataType::Float32 => Box::new(PrimitiveArrayConverter::<Float32Type>::new(
            array.as_primitive::<Float32Type>(),
        )),
        DataType::Float64 => Box::new(PrimitiveArrayConverter::<Float64Type>::new(
            array.as_primitive::<Float64Type>(),
        )),
        DataType::Decimal32(_, _) => Box::new(DecimalArrayConverter::<Decimal32Type>::new(
            array.as_primitive::<Decimal32Type>(),
        )),
        DataType::Decimal64(_, _) => Box::new(DecimalArrayConverter::<Decimal64Type>::new(
            array.as_primitive::<Decimal64Type>(),
        )),
        DataType::Decimal128(_, _) => Box::new(DecimalArrayConverter::<Decimal128Type>::new(
            array.as_primitive::<Decimal128Type>(),
        )),
        DataType::Decimal256(_, _) => Box::new(DecimalArrayConverter::<Decimal256Type>::new(
            array.as_primitive::<Decimal256Type>(),
        )),
        DataType::Utf8 => Box::new(StringArrayConverter::new(array.as_string())),
        DataType::LargeUtf8 => Box::new(LargeStringArrayConverter::new(array.as_string::<i64>())),
        DataType::Binary => Box::new(BinaryArrayConverter::new(array.as_binary())),
        DataType::LargeBinary => Box::new(LargeBinaryArrayConverter::new(array.as_binary::<i64>())),
        DataType::Utf8View => Box::new(StringViewArrayConverter::new(array.as_string_view())),
        DataType::BinaryView => Box::new(BinaryViewArrayConverter::new(array.as_binary_view())),
        DataType::Date32 => Box::new(PrimitiveArrayConverter::<Date32Type>::new(
            array.as_primitive::<Date32Type>(),
        )),
        DataType::Date64 => Box::new(PrimitiveArrayConverter::<Date64Type>::new(
            array.as_primitive::<Date64Type>(),
        )),
        DataType::Time32(TimeUnit::Second) => {
            Box::new(PrimitiveArrayConverter::<Time32SecondType>::new(
                array.as_primitive::<Time32SecondType>(),
            ))
        }
        DataType::Time32(TimeUnit::Millisecond) => {
            Box::new(PrimitiveArrayConverter::<Time32MillisecondType>::new(
                array.as_primitive::<Time32MillisecondType>(),
            ))
        }
        DataType::Time64(TimeUnit::Microsecond) => {
            Box::new(PrimitiveArrayConverter::<Time64MicrosecondType>::new(
                array.as_primitive::<Time64MicrosecondType>(),
            ))
        }
        DataType::Time64(TimeUnit::Nanosecond) => {
            Box::new(PrimitiveArrayConverter::<Time64NanosecondType>::new(
                array.as_primitive::<Time64NanosecondType>(),
            ))
        }
        DataType::Timestamp(TimeUnit::Second, maybe_tz) => {
            Box::new(TimestampArrayConverter::<TimestampSecondType>::new(
                array.as_primitive::<TimestampSecondType>().clone(),
                maybe_tz.clone(),
            ))
        }
        DataType::Timestamp(TimeUnit::Millisecond, maybe_tz) => {
            Box::new(TimestampArrayConverter::<TimestampMillisecondType>::new(
                array.as_primitive::<TimestampMillisecondType>().clone(),
                maybe_tz.clone(),
            ))
        }
        DataType::Timestamp(TimeUnit::Microsecond, maybe_tz) => {
            Box::new(TimestampArrayConverter::<TimestampMicrosecondType>::new(
                array.as_primitive::<TimestampMicrosecondType>().clone(),
                maybe_tz.clone(),
            ))
        }
        DataType::Timestamp(TimeUnit::Nanosecond, maybe_tz) => {
            Box::new(TimestampArrayConverter::<TimestampNanosecondType>::new(
                array.as_primitive::<TimestampNanosecondType>().clone(),
                maybe_tz.clone(),
            ))
        }
        DataType::List(_field) => Box::new(ListArrayConverter::new(array.as_list::<i32>())?),
        DataType::LargeList(_field) => {
            Box::new(LargeListArrayConverter::new(array.as_list::<i64>())?)
        }
        DataType::Map(_field, _is_sorted) => Box::new(MapConverter::new(array.as_map())?),
        DataType::Struct(_fields) => Box::new(StructConverter::new(array.as_struct())?),
        _ => {
            // FALLBACK: Turn every Arrow value into a [minijinja::Value] string.
            let format_options = FormatOptions::new().with_null("None");
            let cast_options = CastOptions {
                safe: true,
                format_options,
            };
            let string_array = cast_with_options(array, &DataType::Utf8, &cast_options)?;
            Box::new(StringArrayConverter::new(string_array.as_string()))
        }
    };
    Ok(converter)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::compute::kernels::cast_utils::Parser as _;
    use arrow_array::{
        ArrayRef, BinaryViewArray, Date32Array, Date64Array, Decimal128Array, Decimal256Array,
        Float64Array, Int32Array, Int64Array, LargeBinaryArray, LargeListArray, LargeStringArray,
        ListArray, StringArray, StringViewArray, Time32MillisecondArray, Time32SecondArray,
        Time64MicrosecondArray, Time64NanosecondArray, TimestampMicrosecondArray,
        TimestampMillisecondArray, TimestampNanosecondArray, TimestampSecondArray, UInt64Array,
        builder::{Int32Builder, ListBuilder, MapBuilder, StringBuilder, StructBuilder},
    };

    use arrow_buffer::Buffer;
    use arrow_data::ArrayData;
    use arrow_data::decimal::MAX_DECIMAL128_FOR_EACH_PRECISION;
    use minijinja::Value;
    use std::sync::Arc;

    const MAX_DECIMAL128: i128 = MAX_DECIMAL128_FOR_EACH_PRECISION[38];

    fn arrow_to_values(array: &dyn Array) -> Result<Vec<Value>, ArrowError> {
        let converter = make_array_converter(array)?;
        let nrows = array.len();
        let mut values = Vec::with_capacity(nrows);
        for idx in 0..nrows {
            values.push(converter.to_value(idx));
        }
        Ok(values)
    }

    #[test]
    fn test_int32_values() {
        let array: ArrayRef = Arc::new(Int32Array::from(vec![Some(1), None, Some(3)]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from(1), Value::from(()), Value::from(3)]
        );
    }

    #[test]
    fn test_int64_values() {
        let array: ArrayRef = Arc::new(Int64Array::from(vec![Some(100), Some(-200), None]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from(100), Value::from(-200), Value::from(())]
        );
    }

    #[test]
    fn test_uint64_values() {
        let array: ArrayRef = Arc::new(UInt64Array::from(vec![Some(100), Some(200), None]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from(100), Value::from(200), Value::from(())]
        );
    }

    #[test]
    fn test_f64_values() {
        let array: ArrayRef = Arc::new(Float64Array::from(vec![Some(100.00), Some(200.05), None]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from(100.00), Value::from(200.05), Value::from(())]
        );
    }

    #[test]
    fn test_decimal128_38_0_values() {
        let buffer_data = vec![
            123456789,
            1337, // NULL (see .nulls())
            -42,
            MAX_DECIMAL128,
        ];
        let data = ArrayData::builder(DataType::Decimal128(38, 0))
            .len(4)
            .add_buffer(Buffer::from_vec(buffer_data))
            .nulls(Some(NullBuffer::from(vec![true, false, true, true])))
            .build()
            .unwrap();
        let decimal_array = Decimal128Array::from(data);
        let array: ArrayRef = Arc::new(decimal_array);
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![
                Value::from(123456789),
                Value::from(()),
                Value::from(-42),
                Value::from(MAX_DECIMAL128),
            ]
        );
    }

    #[test]
    fn test_decimal256_76_0_values() {
        let buffer_data = vec![
            i256::from_i128(123456789),
            i256::from_i128(1337), // NULL (see .nulls())
            i256::from_i128(-42),
            i256::from_i128(MAX_DECIMAL128),
            i256::from_i128(MAX_DECIMAL128 + i64::MAX as i128),
        ];
        let array_data = ArrayData::builder(DataType::Decimal256(76, 0))
            .len(5)
            .add_buffer(Buffer::from_vec(buffer_data))
            .nulls(Some(NullBuffer::from(vec![true, false, true, true, true])))
            .build()
            .unwrap();
        let decimal_array = Decimal256Array::from(array_data);
        let array: ArrayRef = Arc::new(decimal_array);
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(result[0].to_string(), "123456789");
        assert_eq!(result[1], Value::from(()));
        assert_eq!(result[2].to_string(), "-42");
        assert_eq!(
            result[3].to_string(),
            "99999999999999999999999999999999999999"
        );
        assert_eq!(
            result[4].to_string(),
            "100000000000000000009223372036854775806"
        );
    }

    #[test]
    fn test_decimal128_38_2_values() {
        let buffer_data = vec![
            123456789,
            133700, // NULL (see .nulls())
            -4250,
            MAX_DECIMAL128,
        ];
        let data = ArrayData::builder(DataType::Decimal128(38, 2))
            .len(4)
            .add_buffer(Buffer::from_vec(buffer_data))
            .nulls(Some(NullBuffer::from(vec![true, false, true, true])))
            .build()
            .unwrap();
        let decimal_array = Decimal128Array::from(data);
        let array: ArrayRef = Arc::new(decimal_array);
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(result[0].to_string(), "1234567.89");
        assert_eq!(result[1], Value::from(()));
        assert_eq!(result[2].to_string(), "-42.50");
        assert!(result[3].to_string().ends_with("9999.99"))
    }

    #[test]
    fn test_decimal256_76_2_values() {
        let buffer_data = vec![
            i256::from_i128(123456789),
            i256::from_i128(133700), // NULL (see .nulls())
            i256::from_i128(-4250),
            i256::from_i128(MAX_DECIMAL128),
            i256::from_parts(u128::MAX, i64::MAX as i128),
        ];
        let data = ArrayData::builder(DataType::Decimal256(76, 2))
            .len(5)
            .add_buffer(Buffer::from_vec(buffer_data))
            .nulls(Some(NullBuffer::from(vec![true, false, true, true, true])))
            .build()
            .unwrap();
        let decimal_array = Decimal256Array::from(data);
        let array: ArrayRef = Arc::new(decimal_array);
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(result[0].to_string(), "1234567.89");
        assert_eq!(result[1], Value::from(()));
        assert_eq!(result[2].to_string(), "-42.50");
        assert_eq!(
            result[3].to_string(),
            "999999999999999999999999999999999999.99"
        );
        assert_eq!(
            result[4].to_string(),
            "31385508676933403819178947116038332080511777222320172564.47"
        );
    }

    #[test]
    fn test_string_values() {
        let array: ArrayRef = Arc::new(StringArray::from(vec![Some("Hello"), Some("World"), None]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from("Hello"), Value::from("World"), Value::from(())]
        );
    }

    #[test]
    fn test_large_string_values() {
        let array: ArrayRef = Arc::new(LargeStringArray::from(vec![
            Some("Hello"),
            Some("World"),
            None,
        ]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from("Hello"), Value::from("World"), Value::from(())]
        );
    }

    #[test]
    fn test_large_binary_values() {
        let array: ArrayRef = Arc::new(LargeBinaryArray::from(vec![
            Some(b"ab".as_ref()),
            None,
            Some(b"cd".as_ref()),
        ]));
        // LargeBinary and Binary should convert identically to their i32 counterparts.
        let small: ArrayRef = Arc::new(arrow_array::BinaryArray::from(vec![
            Some(b"ab".as_ref()),
            None,
            Some(b"cd".as_ref()),
        ]));
        assert_eq!(
            arrow_to_values(&array).unwrap(),
            arrow_to_values(&small).unwrap()
        );
    }

    #[test]
    fn test_string_view_values() {
        let array: ArrayRef = Arc::new(StringViewArray::from(vec![
            Some("Hello"),
            Some("World"),
            None,
        ]));
        let result = arrow_to_values(&array).unwrap();
        assert_eq!(
            result,
            vec![Value::from("Hello"), Value::from("World"), Value::from(())]
        );
    }

    #[test]
    fn test_binary_view_values() {
        let array: ArrayRef = Arc::new(BinaryViewArray::from(vec![
            Some(b"ab".as_ref()),
            None,
            Some(b"cd".as_ref()),
        ]));
        // BinaryView should convert identically to Binary.
        let small: ArrayRef = Arc::new(arrow_array::BinaryArray::from(vec![
            Some(b"ab".as_ref()),
            None,
            Some(b"cd".as_ref()),
        ]));
        assert_eq!(
            arrow_to_values(&array).unwrap(),
            arrow_to_values(&small).unwrap()
        );
    }

    macro_rules! test_date {
        ($date_type:ty, $array_type:ty) => {
            // Input Arrow array with date32/64 values
            let array = {
                let date0 = <$date_type>::parse("2025-05-28").unwrap();
                let date1 = <$date_type>::parse("2025-05-29").unwrap();
                <$array_type>::from(vec![Some(date0), Some(date1), None])
            };
            // Expected output minijinja values
            let date0 =
                Value::from_object(PyDate::new(NaiveDate::from_ymd_opt(2025, 5, 28).unwrap()));
            let date1 =
                Value::from_object(PyDate::new(NaiveDate::from_ymd_opt(2025, 5, 29).unwrap()));
            assert_eq!(date0.to_string(), "2025-05-28");
            assert_eq!(date1.to_string(), "2025-05-29");

            // Convert Arrow array to minijinja values and assert the result.
            let result = arrow_to_values(&array).unwrap();
            assert_eq!(result, vec![date0, date1, Value::from(())]);

            // Ensure strftime can be called on the values.
            let env = minijinja::Environment::new();
            let state = env.empty_state();

            let date0 = &result[0];
            let date1 = &result[1];

            let res0 = date0.call_method(&state, "strftime", &[Value::from("%Y/%m/%d")], &[]);
            assert_eq!(res0.unwrap().to_string(), "2025/05/28");

            let res1 = date1.call_method(&state, "strftime", &[Value::from("%Y/%m/%d")], &[]);
            assert_eq!(res1.unwrap().to_string(), "2025/05/29");
        };
    }

    #[test]
    fn test_date32_values() {
        test_date!(Date32Type, Date32Array);
    }

    #[test]
    fn test_date64_values() {
        test_date!(Date64Type, Date64Array);
    }

    macro_rules! test_time {
        ($time_type:ty, $array_type:ty) => {
            // Input Arrow array with time32/64 values
            let array = {
                let time0 = <$time_type>::parse("13:37:00").unwrap();
                let time1 = <$time_type>::parse("23:59:59").unwrap();
                <$array_type>::from(vec![Some(time0), Some(time1), None])
            };
            // Expected output minijinja values
            let time0 = Value::from_object(PyTime::new(
                NaiveTime::from_hms_opt(13, 37, 0).unwrap(),
                None,
            ));
            let time1 = Value::from_object(PyTime::new(
                NaiveTime::from_hms_opt(23, 59, 59).unwrap(),
                None,
            ));

            // Convert Arrow array to minijinja values and assert the result.
            let result = arrow_to_values(&array).unwrap();
            assert_eq!(result, vec![time0, time1, Value::from(())]);

            // Ensure strftime can be called on the values.
            let env = minijinja::Environment::new();
            let state = env.empty_state();

            let time0 = &result[0];
            let time1 = &result[1];

            let res0 =
                time0.call_method(&state, "strftime", &[Value::from("h=%H, m=%M, s=%S")], &[]);
            assert_eq!(res0.unwrap().to_string(), "h=13, m=37, s=00");

            let res1 =
                time1.call_method(&state, "strftime", &[Value::from("h=%H, m=%M, s=%S")], &[]);
            assert_eq!(res1.unwrap().to_string(), "h=23, m=59, s=59");
        };
    }

    #[test]
    fn test_time32_seconds_values() {
        test_time!(Time32SecondType, Time32SecondArray);
    }

    #[test]
    fn test_time32_milliseconds_values() {
        test_time!(Time32MillisecondType, Time32MillisecondArray);
    }

    #[test]
    fn test_time64_microseconds_values() {
        test_time!(Time64MicrosecondType, Time64MicrosecondArray);
    }

    #[test]
    fn test_time64_nanoseconds_values() {
        test_time!(Time64NanosecondType, Time64NanosecondArray);
    }

    const TEST_TIMESTAMPS: [Option<i64>; 4] = [Some(0), None, Some(123_123_123), Some(-101)];
    const TEST_TZ_NAME: &str = "America/New_York";

    fn scale_timestamps(input: &[Option<i64>], scale: i64) -> Vec<Option<i64>> {
        input.iter().map(|opt| opt.map(|s| s * scale)).collect()
    }

    fn value_from_timestamps_in_secs(s: i64) -> Value {
        let dt = DateTime::<Utc>::from_timestamp(s, 0).unwrap().naive_utc();
        Value::from_object(PyDateTime::new_naive(dt))
    }

    fn tz_aware_value_from_timestamp_in_secs(s: i64, tz: Tz) -> Value {
        let dt = DateTime::<Utc>::from_timestamp(s, 0).unwrap().naive_utc();
        let aware = tz.from_utc_datetime(&dt);
        Value::from_object(PyDateTime::new_aware(aware, Some(PytzTimezone { tz })))
    }

    macro_rules! assert_timestamp_values {
        ($array_type:ident, $scale:expr) => {{
            let tz = Tz::from_str(TEST_TZ_NAME).unwrap();

            let to_strs = |v: &[Value]| v.iter().map(ToString::to_string).collect::<Vec<_>>();

            // === Naive Test ===
            let input_naive = scale_timestamps(&TEST_TIMESTAMPS, $scale);
            let array: ArrayRef = Arc::new($array_type::from(input_naive));
            let result = arrow_to_values(&array).unwrap();

            let expected: Vec<Value> = TEST_TIMESTAMPS
                .iter()
                .map(|opt| opt.map_or_else(|| Value::from(()), value_from_timestamps_in_secs))
                .collect();

            assert_eq!(to_strs(&result), to_strs(&expected));

            // === Aware Test ===
            let aware_secs: Vec<i64> = TEST_TIMESTAMPS.iter().copied().flatten().collect();
            let aware_input: Vec<Option<i64>> = aware_secs.iter().copied().map(Some).collect();
            let input_aware = scale_timestamps(&aware_input, $scale);
            let array_aware: ArrayRef =
                Arc::new($array_type::from(input_aware).with_timezone(TEST_TZ_NAME));
            let result_aware = arrow_to_values(&array_aware).unwrap();

            let expected_aware: Vec<Value> = aware_secs
                .iter()
                .copied()
                .map(|s| tz_aware_value_from_timestamp_in_secs(s, tz))
                .collect();

            assert_eq!(to_strs(&result_aware), to_strs(&expected_aware));
        }};
    }

    #[test]
    fn test_timestamp_second_values() {
        assert_timestamp_values!(TimestampSecondArray, 1);
    }

    #[test]
    fn test_timestamp_millis_values() {
        assert_timestamp_values!(TimestampMillisecondArray, 1_000);
    }

    #[test]
    fn test_timestamp_micros_values() {
        assert_timestamp_values!(TimestampMicrosecondArray, 1_000_000);
    }

    #[test]
    fn test_timestamp_nanos_values() {
        assert_timestamp_values!(TimestampNanosecondArray, 1_000_000_000);
    }

    #[test]
    fn test_list_values() {
        let data = vec![Some(vec![Some(1), Some(2)]), Some(vec![Some(3)]), None];

        let array: ArrayRef = Arc::new(ListArray::from_iter_primitive::<Int32Type, _, _>(data));

        let result = arrow_to_values(&array).unwrap();

        assert_eq!(
            result,
            vec![
                Value::from(vec![Value::from(1), Value::from(2)]),
                Value::from(vec![Value::from(3)]),
                Value::from(()), // null row
            ]
        );
    }

    #[test]
    fn test_large_list_values() {
        let data = vec![Some(vec![Some(1), Some(2)]), Some(vec![Some(3)]), None];

        let array: ArrayRef =
            Arc::new(LargeListArray::from_iter_primitive::<Int32Type, _, _>(data));

        let result = arrow_to_values(&array).unwrap();

        assert_eq!(
            result,
            vec![
                Value::from(vec![Value::from(1), Value::from(2)]),
                Value::from(vec![Value::from(3)]),
                Value::from(()), // null row
            ]
        );
    }

    #[test]
    fn test_nested_list() {
        let inner_values_builder = ListBuilder::new(Int32Builder::new());
        let mut outer_builder = ListBuilder::new(inner_values_builder);

        {
            let inner_builder = outer_builder.values();

            inner_builder.values().append_value(1);
            inner_builder.append(true);

            inner_builder.values().append_value(2);
            inner_builder.append(true);

            outer_builder.append(true);
        }

        outer_builder.append(false);

        let array: ArrayRef = Arc::new(outer_builder.finish());
        let result = arrow_to_values(&array).unwrap();

        assert_eq!(
            result,
            vec![
                Value::from(vec![vec![Value::from(1)], vec![Value::from(2)],]),
                Value::from(()), // null row
            ]
        );
    }

    #[test]
    fn test_map_column_with_nulls() {
        let string_builder = StringBuilder::new();
        let int_builder = Int32Builder::with_capacity(4);

        // {"joe": 1}, {"blogs": 2, "foo": 4}, {}, null]
        let mut builder = MapBuilder::new(None, string_builder, int_builder);

        builder.keys().append_value("joe");
        builder.values().append_value(1);
        builder.append(true).unwrap();

        builder.keys().append_value("blogs");
        builder.values().append_value(2);
        builder.keys().append_value("foo");
        builder.values().append_value(4);
        builder.append(true).unwrap();
        builder.append(true).unwrap();
        builder.append(false).unwrap();

        let array = builder.finish();
        let result = arrow_to_values(&array).unwrap();
        let mut m1 = ValueMap::new();
        m1.insert(Value::from("joe"), Value::from(1));

        let mut m2 = ValueMap::new();
        m2.insert(Value::from("blogs"), Value::from(2));
        m2.insert(Value::from("foo"), Value::from(4));

        assert_eq!(
            result,
            vec![
                Value::from(m1),
                Value::from(m2),
                Value::from(ValueMap::new()), // empty dict
                Value::from(())               // None
            ]
        );
    }

    #[test]
    fn test_struct_values() {
        // struct<a:int, b:string> -> dict, with field nulls preserved.
        let a: ArrayRef = Arc::new(Int32Array::from(vec![Some(1), Some(2)]));
        let b: ArrayRef = Arc::new(StringArray::from(vec![Some("x"), None]));
        let array = StructArray::from(vec![
            (Arc::new(Field::new("a", DataType::Int32, true)), a),
            (Arc::new(Field::new("b", DataType::Utf8, true)), b),
        ]);

        let result = arrow_to_values(&array).unwrap();

        let mut row0 = ValueMap::new();
        row0.insert(Value::from("a"), Value::from(1));
        row0.insert(Value::from("b"), Value::from("x"));
        let mut row1 = ValueMap::new();
        row1.insert(Value::from("a"), Value::from(2));
        row1.insert(Value::from("b"), Value::from(()));

        assert_eq!(result, vec![Value::from(row0), Value::from(row1)]);
    }

    #[test]
    fn test_struct_values_with_offset() {
        // A *sliced* struct array (non-zero offset) must convert the right rows.
        // The converter indexes struct children by row index, relying on Arrow
        // propagating slice offset/length into the child arrays.
        let a: ArrayRef = Arc::new(Int32Array::from(vec![Some(1), Some(2), Some(3)]));
        let b: ArrayRef = Arc::new(StringArray::from(vec![Some("x"), Some("y"), Some("z")]));
        let array: ArrayRef = Arc::new(StructArray::from(vec![
            (Arc::new(Field::new("a", DataType::Int32, true)), a),
            (Arc::new(Field::new("b", DataType::Utf8, true)), b),
        ]));

        // Drop the first row -> remaining rows {a:2,b:"y"}, {a:3,b:"z"}.
        let sliced = array.slice(1, 2);
        let result = arrow_to_values(sliced.as_ref()).unwrap();

        let mut row0 = ValueMap::new();
        row0.insert(Value::from("a"), Value::from(2));
        row0.insert(Value::from("b"), Value::from("y"));
        let mut row1 = ValueMap::new();
        row1.insert(Value::from("a"), Value::from(3));
        row1.insert(Value::from("b"), Value::from("z"));

        assert_eq!(result, vec![Value::from(row0), Value::from(row1)]);
    }

    #[test]
    fn test_map_with_struct_values() {
        // map<string, struct<a:int>> -- renders as a nested dict.
        let value_fields = vec![Field::new("a", DataType::Int32, true)];
        let mut builder = MapBuilder::new(
            None,
            StringBuilder::new(),
            StructBuilder::from_fields(value_fields, 1),
        );

        // {"k": {"a": 1}}
        builder.keys().append_value("k");
        {
            let vb = builder.values();
            vb.field_builder::<Int32Builder>(0).unwrap().append_value(1);
            vb.append(true);
        }
        builder.append(true).unwrap();

        let array = builder.finish();
        let result = arrow_to_values(&array).unwrap();

        let mut inner = ValueMap::new();
        inner.insert(Value::from("a"), Value::from(1));
        let mut outer = ValueMap::new();
        outer.insert(Value::from("k"), Value::from(inner));

        assert_eq!(result, vec![Value::from(outer)]);
    }
}
