//! Hashers of Arrow rows.
//!
//!

use arrow::array::{AsArray as _, PrimitiveArray};
use arrow::buffer::{BooleanBuffer, NullBuffer, ScalarBuffer};
use arrow::datatypes::*;
use arrow_array::{
    Array, ArrowPrimitiveType, BooleanArray, GenericByteArray, GenericByteViewArray,
    OffsetSizeTrait,
};
use arrow_buffer::i256;
use arrow_schema::ArrowError;
use core::mem;
use siphasher::sip128;
use std::hash::{BuildHasher, Hash as _, Hasher};

/// Trait for hashing a single column's value for a given row index.
///
/// It uses a 128-bit SipHasher with 1 compression step and 3 rounds
/// (faster than the more common c=2 r=4 variant).
pub trait ColumnHasher: Send + Sync {
    /// Hashes the value of the column at the given row index.
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize);

    /// Produces the hash of a null row. `write_row(i)` should produce the same
    /// hash as `write_null()` if the row at index `i` is null.
    ///
    /// This is necessary because from some nested types (e.g. DictionaryArray),
    /// the data in the parent array can force a child array value to be logically
    /// null even when it isn't
    fn write_null(&self, hasher: &mut sip128::SipHasher13);
}

pub struct TableHasher {
    column_hashers: Vec<Box<dyn ColumnHasher>>,
}

impl TableHasher {
    pub fn try_new<'a>(columns: impl Iterator<Item = &'a dyn Array>) -> Result<Self, ArrowError> {
        let column_hashers = columns.map(make_column_hasher).collect::<Result<_, _>>()?;
        Ok(Self { column_hashers })
    }
}

impl ColumnHasher for TableHasher {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, idx: usize) {
        const MARKER: u64 = 0xFFFFFFFFFFFFFFFFu64;
        for column_hasher in self.column_hashers.iter() {
            column_hasher.write_row(hasher, idx);
            hasher.write_u64(MARKER);
        }
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        const MARKER: u64 = 0xFFFFFFFFFFFFFFFFu64;
        for column_hasher in self.column_hashers.iter() {
            column_hasher.write_null(hasher);
            hasher.write_u64(MARKER);
        }
    }
}

/// Every value of a `Null` array is null, so all rows hash identically.
struct NullColumnHasher;

impl ColumnHasher for NullColumnHasher {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, _row_idx: usize) {
        hasher.write_u32(0);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        hasher.write_u32(0);
    }
}

// Boolean, Integers, Floats, Date, and Time {{{
struct BooleanColumnHasher {
    values: BooleanBuffer,
    nulls: Option<NullBuffer>,
}

impl BooleanColumnHasher {
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

    #[inline(always)]
    fn write_value(hasher: &mut sip128::SipHasher13, is_valid: bool, value: bool) {
        let is_valid = is_valid as u32;
        let value = value as u32;
        let bu32: u32 = value | (is_valid << 8);
        hasher.write_u32(bu32);
    }
}

impl ColumnHasher for BooleanColumnHasher {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.is_valid(row_idx);
        let value = self.values.value(row_idx);
        Self::write_value(hasher, is_valid, is_valid && value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        Self::write_value(hasher, false, false);
    }
}

struct PrimitiveColumnHasher<T: ArrowPrimitiveType> {
    values: ScalarBuffer<T::Native>,
    nulls: Option<NullBuffer>,
}

impl<T: ArrowPrimitiveType> PrimitiveColumnHasher<T> {
    fn from_buffers(values: ScalarBuffer<T::Native>, nulls: Option<NullBuffer>) -> Self {
        Self { values, nulls }
    }

    pub fn new(array: &PrimitiveArray<T>) -> Self {
        Self::from_buffers(array.values().clone(), array.nulls().cloned())
    }

    #[inline(always)]
    pub fn is_valid(&self, idx: usize) -> bool {
        self.nulls.as_ref().is_none_or(|nulls| nulls.is_valid(idx))
    }
}

macro_rules! make_primitive_array_hasher {
    ($arrow_data_ty:ty) => {
        impl PrimitiveColumnHasher<$arrow_data_ty> {
            #[inline(always)]
            fn write_value(
                hasher: &mut sip128::SipHasher13,
                is_valid: bool,
                value: <$arrow_data_ty as ArrowPrimitiveType>::Native,
            ) {
                if mem::size_of_val(&value) == 8 {
                    // hash f64 as u64 by transmuting the bits
                    let value_as_u64: u64 = unsafe { core::mem::transmute_copy(&value) };
                    is_valid.hash(hasher);
                    value_as_u64.hash(hasher);
                } else {
                    let mut value_as_u64: u64 = {
                        // hash f32 as u64 by transmuting the bits and zero-extending
                        let mut extended = [0u8; 8];
                        let value_bytes = value.to_le_bytes();
                        // SAFETY: size of value is less than 8 bytes and pointers are non-overlapping
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                value_bytes.as_ptr(),
                                extended.as_mut_ptr(),
                                mem::size_of_val(&value),
                            );
                        };
                        u64::from_le_bytes(extended)
                    };
                    // embed the is_valid boolean into the u64
                    value_as_u64 |= ((is_valid as u64) << 32);
                    value_as_u64.hash(hasher);
                }
            }
        }

        impl ColumnHasher for PrimitiveColumnHasher<$arrow_data_ty> {
            fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
                let zero = <$arrow_data_ty as ArrowPrimitiveType>::Native::ZERO;
                let is_valid = self.is_valid(row_idx);
                let value = if is_valid { self.values[row_idx] } else { zero };
                Self::write_value(hasher, is_valid, value);
            }

            fn write_null(&self, hasher: &mut sip128::SipHasher13) {
                let zero = <$arrow_data_ty as ArrowPrimitiveType>::Native::ZERO;
                Self::write_value(hasher, false, zero);
            }
        }
    };
}

// Int8Type is handled by transmuting to UInt8Type
// Int16Type is handled by transmuting to UInt16Type
// Int32Type is handled by transmuting to UInt32Type
// Int64Type is handled by transmuting to UInt64Type
make_primitive_array_hasher!(UInt8Type);
make_primitive_array_hasher!(UInt16Type);
make_primitive_array_hasher!(UInt32Type);
make_primitive_array_hasher!(UInt64Type);
// Float32Type is handled by transmuting to UInt32Type
// Float64Type is handled by transmuting to UInt64Type
// Date32Type is handled by transmuting to UInt32Type
// Date64Type is handled by transmuting to UInt64Type
// Time32SecondType is handled by transmuting to UInt32Type
// Time32MillisecondType is handled by transmuting to UInt32Type
// Time64MicrosecondType is handled by transmuting to UInt64Type
// Time64NanosecondType is handled by transmuting to UInt64Type
// TimestampSecondType is handled by transmuting to UInt64Type
// TimestampMillisecondType is handled by transmuting to UInt64Type
// TimestampMicrosecondType is handled by transmuting to UInt64Type
// TimestampNanosecondType is handled by transmuting to UInt64Type
// }}}

// Decimals {{{
struct DecimalColumnHasher<T: DecimalType> {
    values: ScalarBuffer<T::Native>,
    nulls: Option<NullBuffer>,
}

impl<T: DecimalType> DecimalColumnHasher<T> {
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

    #[inline(always)]
    fn write_value(hasher: &mut sip128::SipHasher13, is_valid: bool, value_bits: T::Native) {
        is_valid.hash(hasher);
        match size_of::<T::Native>() {
            16 => {
                let value_as_u128: u128 = unsafe { mem::transmute_copy(&value_bits) };
                value_as_u128.hash(hasher);
            }
            32 => {
                let value_as_i256: i256 = unsafe { mem::transmute_copy(&value_bits) };
                value_as_i256.hash(hasher);
            }
            8 => {
                let value_as_u64: u64 = unsafe { mem::transmute_copy(&value_bits) };
                value_as_u64.hash(hasher);
            }
            4 => {
                let value_as_u32: u32 = unsafe { mem::transmute_copy(&value_bits) };
                value_as_u32.hash(hasher);
            }
            _ => unreachable!(),
        }
    }
}

impl<T: DecimalType> ColumnHasher for DecimalColumnHasher<T> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, idx: usize) {
        let is_valid = self.is_valid(idx);
        let value = if is_valid {
            self.values[idx]
        } else {
            T::Native::ZERO
        };
        Self::write_value(hasher, is_valid, value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        Self::write_value(hasher, false, T::Native::ZERO);
    }
}
// }}}

// String and Binary {{{

/// Shared by the string and string view hashers so both hash a value identically.
#[inline(always)]
fn write_str_value(hasher: &mut sip128::SipHasher13, is_valid: bool, value: &str) {
    is_valid.hash(hasher);
    value.hash(hasher);
}

/// Shared by the binary and binary view hashers so both hash a value identically.
#[inline(always)]
fn write_bytes_value(hasher: &mut sip128::SipHasher13, is_valid: bool, value: &[u8]) {
    is_valid.hash(hasher);
    value.hash(hasher);
}

struct GenericByteColumnHasher<T: ByteArrayType> {
    array: GenericByteArray<T>,
}

impl<T: ByteArrayType> GenericByteColumnHasher<T> {
    pub fn new(array: &GenericByteArray<T>) -> Self {
        Self {
            array: array.clone(),
        }
    }
}

impl<O: OffsetSizeTrait> ColumnHasher for GenericByteColumnHasher<GenericStringType<O>> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = if is_valid {
            self.array.value(row_idx)
        } else {
            ""
        };
        write_str_value(hasher, is_valid, value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        write_str_value(hasher, false, "");
    }
}

impl<O: OffsetSizeTrait> ColumnHasher for GenericByteColumnHasher<GenericBinaryType<O>> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = if is_valid {
            self.array.value(row_idx)
        } else {
            b""
        };
        write_bytes_value(hasher, is_valid, value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        write_bytes_value(hasher, false, b"");
    }
}

type GenericStringColumnHasher<O> = GenericByteColumnHasher<GenericStringType<O>>;
type StringColumnHasher = GenericStringColumnHasher<i32>;
type LargeStringColumnHasher = GenericStringColumnHasher<i64>;

type GenericBinaryColumnHasher<O> = GenericByteColumnHasher<GenericBinaryType<O>>;
type BinaryColumnHasher = GenericBinaryColumnHasher<i32>;
type LargeBinaryColumnHasher = GenericBinaryColumnHasher<i64>;
// }}}

// StringView and BinaryView {{{
struct ByteViewColumnHasher<T: ByteViewType> {
    array: GenericByteViewArray<T>,
}

impl<T: ByteViewType> ByteViewColumnHasher<T> {
    pub fn new(array: &GenericByteViewArray<T>) -> Self {
        Self {
            array: array.clone(),
        }
    }
}

impl ColumnHasher for ByteViewColumnHasher<StringViewType> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = if is_valid {
            self.array.value(row_idx)
        } else {
            ""
        };
        write_str_value(hasher, is_valid, value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        write_str_value(hasher, false, "");
    }
}

impl ColumnHasher for ByteViewColumnHasher<BinaryViewType> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = if is_valid {
            self.array.value(row_idx)
        } else {
            b""
        };
        write_bytes_value(hasher, is_valid, value);
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        write_bytes_value(hasher, false, b"");
    }
}

type StringViewColumnHasher = ByteViewColumnHasher<StringViewType>;
type BinaryViewColumnHasher = ByteViewColumnHasher<BinaryViewType>;
// }}}

struct DictionaryColumnHasher<K: ArrowPrimitiveType> {
    keys: PrimitiveArray<K>,
    values: Box<dyn ColumnHasher>,
}

impl<K: ArrowPrimitiveType> DictionaryColumnHasher<K> {
    pub fn new(keys: PrimitiveArray<K>, values: Box<dyn ColumnHasher>) -> Self {
        Self { keys, values }
    }
}

impl<K: ArrowPrimitiveType> ColumnHasher for DictionaryColumnHasher<K> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid_key = self.keys.is_valid(row_idx);
        if is_valid_key {
            let key = self.keys.value(row_idx).as_usize();
            self.values.write_row(hasher, key);
        } else {
            self.values.write_null(hasher);
        }
    }

    fn write_null(&self, hasher: &mut sip128::SipHasher13) {
        self.values.write_null(hasher);
    }
}

/// Creates a new PrimitiveArray<U> by transmuting the underlying buffer of a PrimitiveArray<T>.
///
/// This is useful when you have two primitive types with the same memory layout
/// and wants to reduce the number of specializations necessary to implement certain
/// operations that are agnostic to the actual primitive type (e.g. hashing).
///
/// Example: a hashing algorithm that works for both Int32 and UInt32 can be implemented
/// by transmuting the Int32Array to a UInt32Array and reusing the same specialization.
/// To get different hash values for Int32 and UInt32 values of the same bit pattern,
/// we can seed the initial hasher state differently with the hash of the data type.
///
/// This is a zero-copy operation.
///
/// # Panics
///
/// Panics if the size of T and U are not the same and/or alignment requirements are not met.
fn transmuted_primitive<T: ArrowPrimitiveType, U: ArrowPrimitiveType>(
    array: &PrimitiveArray<T>,
) -> PrimitiveArray<U> {
    // cheap clones that amount to bumping a ref-count
    // in the Arc<Bytes> in the underlying buffers
    let values = {
        let inner_buffer = array.values().inner().clone();
        // ScalarBuffer::from() performs dynamic checks on size and alignment
        ScalarBuffer::<U::Native>::from(inner_buffer)
    };
    let nulls = array.nulls().cloned();
    PrimitiveArray::<U>::new(values, nulls)
}

fn cast_primitive<T: ArrowPrimitiveType>(
    array: &dyn Array,
    options: &arrow::compute::CastOptions,
) -> Result<PrimitiveArray<T>, ArrowError> {
    let value_type = T::DATA_TYPE;
    let cast_array = arrow::compute::cast_with_options(array, &value_type, options)?;
    let primitive_array = cast_array
        .as_any()
        .downcast_ref::<PrimitiveArray<T>>()
        .expect("cast array");
    Ok(primitive_array.clone())
}

/// Helper for situations where we can transmute from signed to unsigned and
/// then cast to another unsigned integer (of higher width). Used well,
/// this reduces the number of specializations of `transmuted_primitive` and
/// `cast_primitive` that will be instantiated in the binary.
fn transmute_and_cast<T: ArrowPrimitiveType, U: ArrowPrimitiveType, V: ArrowPrimitiveType>(
    array: &dyn Array,
    options: &arrow::compute::CastOptions,
) -> Result<PrimitiveArray<V>, ArrowError> {
    let transmuted = transmuted_primitive::<T, U>(array.as_primitive());
    cast_primitive::<V>(&transmuted, options)
}

pub fn make_column_hasher(array: &dyn Array) -> Result<Box<dyn ColumnHasher>, ArrowError> {
    use arrow::compute::CastOptions;

    let converter: Box<dyn ColumnHasher> = match array.data_type() {
        DataType::Null => Box::new(NullColumnHasher),
        DataType::Dictionary(_, _) => {
            let cast_options = CastOptions::default();

            // Creates the hasher with the given keys array of type T.
            fn hasher0<T: ArrowPrimitiveType>(
                keys: &dyn Array,
                values_hasher: Box<dyn ColumnHasher>,
            ) -> Result<Box<dyn ColumnHasher>, ArrowError> {
                let keys = keys.as_primitive::<T>().clone();
                let hasher = Box::new(DictionaryColumnHasher::<T>::new(keys, values_hasher));
                Ok(hasher)
            }

            // Cast keys to T and make a hasher.
            fn hasher1<T: ArrowDictionaryKeyType>(
                keys: &dyn Array,
                cast_options: &CastOptions,
                values_hasher: Box<dyn ColumnHasher>,
            ) -> Result<Box<dyn ColumnHasher>, ArrowError> {
                let keys = cast_primitive::<T>(keys, cast_options)?;
                let hasher = Box::new(DictionaryColumnHasher::new(keys, values_hasher));
                Ok(hasher)
            }

            // Transmute keys from T to U and make a hasher.
            fn hasher2<T: ArrowPrimitiveType, U: ArrowPrimitiveType>(
                keys: &dyn Array,
                values_hasher: Box<dyn ColumnHasher>,
            ) -> Result<Box<dyn ColumnHasher>, ArrowError> {
                let keys = transmuted_primitive::<T, U>(keys.as_primitive());
                let hasher = Box::new(DictionaryColumnHasher::new(keys, values_hasher));
                Ok(hasher)
            }

            // Transmute keys from T to U, cast to V, and make a hasher.
            fn hasher3<T: ArrowPrimitiveType, U: ArrowPrimitiveType, V: ArrowPrimitiveType>(
                keys: &dyn Array,
                cast_options: &CastOptions,
                values_hasher: Box<dyn ColumnHasher>,
            ) -> Result<Box<dyn ColumnHasher>, ArrowError> {
                let keys = transmute_and_cast::<T, U, V>(keys, cast_options)?;
                let hasher = Box::new(DictionaryColumnHasher::new(keys, values_hasher));
                Ok(hasher)
            }

            let dict = array.as_any_dictionary();
            let keys = dict.keys();
            let values_hasher = {
                let values = dict.values();
                make_column_hasher(values.as_ref())?
            };

            // Every signed key type can be transmuted to the corresponding unsigned type.
            // Then we cast the unsigned type to either 32-bit or 64-bit unsigned integer.
            let hasher: Box<dyn ColumnHasher> = match keys.data_type() {
                DataType::UInt8 => hasher1::<UInt32Type>(keys, &cast_options, values_hasher)?,
                DataType::UInt16 => hasher1::<UInt32Type>(keys, &cast_options, values_hasher)?,
                DataType::UInt32 => hasher0::<UInt32Type>(keys, values_hasher)?,
                DataType::UInt64 => hasher0::<UInt64Type>(keys, values_hasher)?,
                DataType::Int8 => {
                    hasher3::<Int8Type, UInt8Type, UInt32Type>(keys, &cast_options, values_hasher)?
                }
                DataType::Int16 => hasher3::<Int16Type, UInt16Type, UInt32Type>(
                    keys,
                    &cast_options,
                    values_hasher,
                )?,
                DataType::Int32 => hasher2::<Int32Type, UInt32Type>(keys, values_hasher)?,
                DataType::Int64 => hasher2::<Int64Type, UInt64Type>(keys, values_hasher)?,
                _ => unreachable!("unexpected dictionary key type"),
            };
            hasher
        }
        DataType::Boolean => Box::new(BooleanColumnHasher::new(array.as_boolean())),
        DataType::Int8 => {
            let typed = transmuted_primitive::<Int8Type, UInt8Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt8Type>::new(&typed))
        }
        DataType::Int16 => {
            let typed = transmuted_primitive::<Int16Type, UInt16Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt16Type>::new(&typed))
        }
        DataType::Int32 => {
            let typed = transmuted_primitive::<Int32Type, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Int64 => {
            let typed = transmuted_primitive::<Int64Type, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::UInt8 => Box::new(PrimitiveColumnHasher::<UInt8Type>::new(
            array.as_primitive::<UInt8Type>(),
        )),
        DataType::UInt16 => Box::new(PrimitiveColumnHasher::<UInt16Type>::new(
            array.as_primitive::<UInt16Type>(),
        )),
        DataType::UInt32 => Box::new(PrimitiveColumnHasher::<UInt32Type>::new(
            array.as_primitive::<UInt32Type>(),
        )),
        DataType::UInt64 => Box::new(PrimitiveColumnHasher::<UInt64Type>::new(
            array.as_primitive::<UInt64Type>(),
        )),
        DataType::Float32 => {
            let typed = transmuted_primitive::<Float32Type, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Float64 => {
            let typed = transmuted_primitive::<Float64Type, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Decimal32(_, _) => {
            let typed = transmuted_primitive::<Decimal32Type, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Decimal64(_, _) => {
            let typed = transmuted_primitive::<Decimal64Type, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Decimal128(_, _) => Box::new(DecimalColumnHasher::<Decimal128Type>::new(
            array.as_primitive::<Decimal128Type>(),
        )),
        DataType::Decimal256(_, _) => Box::new(DecimalColumnHasher::<Decimal256Type>::new(
            array.as_primitive::<Decimal256Type>(),
        )),
        DataType::Utf8 => Box::new(StringColumnHasher::new(array.as_string())),
        DataType::LargeUtf8 => Box::new(LargeStringColumnHasher::new(array.as_string::<i64>())),
        DataType::Binary => Box::new(BinaryColumnHasher::new(array.as_binary())),
        DataType::LargeBinary => Box::new(LargeBinaryColumnHasher::new(array.as_binary::<i64>())),
        DataType::Utf8View => Box::new(StringViewColumnHasher::new(array.as_string_view())),
        DataType::BinaryView => Box::new(BinaryViewColumnHasher::new(array.as_binary_view())),
        DataType::Date32 => {
            let typed = transmuted_primitive::<Date32Type, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Date64 => {
            let typed = transmuted_primitive::<Date64Type, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Time32(TimeUnit::Second) => {
            let typed = transmuted_primitive::<Time32SecondType, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Time32(TimeUnit::Millisecond) => {
            let typed =
                transmuted_primitive::<Time32MillisecondType, UInt32Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt32Type>::new(&typed))
        }
        DataType::Time64(TimeUnit::Microsecond) => {
            let typed =
                transmuted_primitive::<Time64MicrosecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Time64(TimeUnit::Nanosecond) => {
            let typed =
                transmuted_primitive::<Time64NanosecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Timestamp(TimeUnit::Second, _) => {
            let typed =
                transmuted_primitive::<TimestampSecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Timestamp(TimeUnit::Millisecond, _) => {
            let typed =
                transmuted_primitive::<TimestampMillisecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Timestamp(TimeUnit::Microsecond, _) => {
            let typed =
                transmuted_primitive::<TimestampMicrosecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        DataType::Timestamp(TimeUnit::Nanosecond, _) => {
            let typed =
                transmuted_primitive::<TimestampNanosecondType, UInt64Type>(array.as_primitive());
            Box::new(PrimitiveColumnHasher::<UInt64Type>::new(&typed))
        }
        _ => {
            return Err(ArrowError::NotYetImplemented(format!(
                "Hasher not implemented for{:?} data type ",
                array.data_type()
            )));
        }
    };
    Ok(converter)
}

/// A [Hasher] for objects that are hashes themselves.
#[derive(Default)]
pub struct IdentityHasher {
    hash: u64,
    #[cfg(debug_assertions)]
    unexpected_call: bool,
}
impl Hasher for IdentityHasher {
    fn write(&mut self, _bytes: &[u8]) {
        #[cfg(debug_assertions)]
        {
            self.unexpected_call = true;
        }
    }
    fn write_u64(&mut self, i: u64) {
        self.hash = i;
    }
    fn finish(&self) -> u64 {
        #[cfg(debug_assertions)]
        {
            debug_assert!(!self.unexpected_call);
        }
        self.hash
    }
}

#[derive(Default)]
pub struct IdentityBuildHasher;
impl BuildHasher for IdentityBuildHasher {
    type Hasher = IdentityHasher;
    fn build_hasher(&self) -> Self::Hasher {
        IdentityHasher::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{
        ArrayRef, BinaryArray, BinaryViewArray, Date32Array, Decimal32Array, Decimal64Array,
        Decimal128Array, Decimal256Array, DictionaryArray, Float32Array, Float64Array, Int8Array,
        Int16Array, Int32Array, Int64Array, LargeBinaryArray, LargeStringArray, StringArray,
        StringViewArray, TimestampMicrosecondArray, UInt8Array, UInt16Array, UInt32Array,
        UInt64Array, make_array,
    };
    use std::sync::Arc;

    fn hash_row(column_hasher: &dyn ColumnHasher, row_idx: usize) -> u64 {
        let mut hasher = sip128::SipHasher13::new();
        column_hasher.write_row(&mut hasher, row_idx);
        hasher.finish()
    }

    fn hash_first_row(array: &dyn Array) -> u64 {
        hash_row(make_column_hasher(array).unwrap().as_ref(), 0)
    }

    fn hash_rows(array: &dyn Array) -> Vec<u64> {
        let column_hasher = make_column_hasher(array).unwrap();
        (0..array.len())
            .map(|i| hash_row(column_hasher.as_ref(), i))
            .collect()
    }

    fn hash_null(array: &dyn Array) -> u64 {
        let column_hasher = make_column_hasher(array).unwrap();
        let mut hasher = sip128::SipHasher13::new();
        column_hasher.write_null(&mut hasher);
        hasher.finish()
    }

    /// Replaces the nulls buffer of `array` without touching its values, so
    /// the rows that become null keep whatever (non-zero) value they had.
    fn with_nulls(array: &dyn Array, validity: &[bool]) -> ArrayRef {
        assert_eq!(array.null_count(), 0, "expected an array without nulls");
        let data = array
            .to_data()
            .into_builder()
            .nulls(Some(NullBuffer::from(validity.to_vec())))
            .build()
            .unwrap();
        make_array(data)
    }

    /// `array` holds 5 values: `[value, garbage, zero, garbage, other value]`.
    /// Rows 1 and 3 are made null without clearing the garbage under them.
    fn assert_null_rows_ignore_the_values(array: ArrayRef) {
        let name = array.data_type().clone();
        let array = with_nulls(&array, &[true, false, true, false, true]);
        let hashes = hash_rows(&array);
        let null = hash_null(&array);
        assert_eq!(hashes[1], null, "{name}: null row 1 vs write_null");
        assert_eq!(hashes[3], null, "{name}: null row 3 vs write_null");
        assert_ne!(hashes[0], null, "{name}: value vs null");
        assert_ne!(hashes[2], null, "{name}: zero value vs null");
        assert_ne!(hashes[0], hashes[4], "{name}: different values");
    }

    #[test]
    fn null_rows_ignore_the_values_under_the_nulls_buffer() {
        let long = "a string long enough to not be inlined in a view";
        let arrays: Vec<ArrayRef> = vec![
            Arc::new(BooleanArray::from(vec![true, true, false, true, false])),
            Arc::new(Int8Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Int16Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Int32Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Int64Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(UInt8Array::from(vec![1, 7, 0, 3, 2])),
            Arc::new(UInt16Array::from(vec![1, 7, 0, 3, 2])),
            Arc::new(UInt32Array::from(vec![1, 7, 0, 3, 2])),
            Arc::new(UInt64Array::from(vec![1, 7, 0, 3, 2])),
            Arc::new(Float32Array::from(vec![1.5, 7.25, 0.0, -3.0, 2.5])),
            Arc::new(Float64Array::from(vec![1.5, 7.25, 0.0, -3.0, 2.5])),
            Arc::new(Date32Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(TimestampMicrosecondArray::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Decimal32Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Decimal64Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Decimal128Array::from(vec![1, 7, 0, -3, 2])),
            Arc::new(Decimal256Array::from(
                [1, 7, 0, -3, 2].map(i256::from_i128).to_vec(),
            )),
            Arc::new(StringArray::from(vec!["a", "garbage", "", long, "b"])),
            Arc::new(LargeStringArray::from(vec!["a", "garbage", "", long, "b"])),
            Arc::new(StringViewArray::from(vec!["a", "garbage", "", long, "b"])),
            Arc::new(BinaryArray::from_iter_values([
                "a", "garbage", "", long, "b",
            ])),
            Arc::new(LargeBinaryArray::from_iter_values([
                "a", "garbage", "", long, "b",
            ])),
            Arc::new(BinaryViewArray::from_iter_values([
                "a", "garbage", "", long, "b",
            ])),
        ];
        for array in arrays {
            assert_null_rows_ignore_the_values(array);
        }
    }

    /// Rows: "b", null (null value), "a", null (null key over the key of "b"),
    /// "b" (another key), null (null key over the key of "a").
    ///
    /// The null value and the null keys keep non-zero values under them.
    fn dictionary<K: ArrowDictionaryKeyType>() -> DictionaryArray<K> {
        let keys = PrimitiveArray::<K>::from_iter_values(
            [0, 1, 2, 0, 3, 2].map(|i| K::Native::from_usize(i).unwrap()),
        );
        let keys = with_nulls(&keys, &[true, true, true, false, true, false]);
        let values = StringArray::from(vec!["b", "garbage", "a", "b"]);
        let values = with_nulls(&values, &[true, false, true, true]);
        DictionaryArray::try_new(keys.as_primitive::<K>().clone(), values).unwrap()
    }

    fn assert_dictionary_hashes<K: ArrowDictionaryKeyType>() {
        let name = K::DATA_TYPE;
        let dict = dictionary::<K>();
        let hashes = hash_rows(&dict);
        let null = hash_null(&dict);
        assert_eq!(hashes[0], hashes[4], "{name}: equal values, different keys");
        assert_ne!(hashes[0], hashes[2], "{name}: different values");
        assert_eq!(hashes[1], null, "{name}: null value vs write_null");
        assert_eq!(hashes[3], null, "{name}: null key over \"b\" vs write_null");
        assert_eq!(hashes[5], null, "{name}: null key over \"a\" vs write_null");
        assert_ne!(hashes[0], null, "{name}: value vs null");
        assert_ne!(hashes[2], null, "{name}: value vs null");
    }

    #[test]
    fn dictionary_hashes_values_through_the_keys() {
        assert_dictionary_hashes::<Int8Type>();
        assert_dictionary_hashes::<Int16Type>();
        assert_dictionary_hashes::<Int32Type>();
        assert_dictionary_hashes::<Int64Type>();
        assert_dictionary_hashes::<UInt8Type>();
        assert_dictionary_hashes::<UInt16Type>();
        assert_dictionary_hashes::<UInt32Type>();
        assert_dictionary_hashes::<UInt64Type>();
    }

    #[test]
    fn dictionary_hashes_are_independent_of_the_key_type() {
        assert_eq!(
            hash_rows(&dictionary::<Int8Type>()),
            hash_rows(&dictionary::<UInt64Type>())
        );
    }

    #[test]
    fn large_utf8_hashes_like_utf8() {
        let small = StringArray::from(vec![Some("hello")]);
        let large = LargeStringArray::from(vec![Some("hello")]);
        assert_eq!(hash_first_row(&small), hash_first_row(&large));
    }

    #[test]
    fn large_binary_hashes_like_binary() {
        let small = BinaryArray::from(vec![Some(b"hello".as_ref())]);
        let large = LargeBinaryArray::from(vec![Some(b"hello".as_ref())]);
        assert_eq!(hash_first_row(&small), hash_first_row(&large));
    }

    #[test]
    fn string_view_hashes_like_utf8() {
        let small = StringArray::from(vec![Some("hello")]);
        let view = StringViewArray::from(vec![Some("hello")]);
        assert_eq!(hash_first_row(&small), hash_first_row(&view));
    }

    #[test]
    fn binary_view_hashes_like_binary() {
        let small = BinaryArray::from(vec![Some(b"hello".as_ref())]);
        let view = BinaryViewArray::from(vec![Some(b"hello".as_ref())]);
        assert_eq!(hash_first_row(&small), hash_first_row(&view));
    }

    #[test]
    fn string_view_hashes_differ_by_validity() {
        let value = StringViewArray::from(vec![Some("hello")]);
        let null = StringViewArray::from(vec![None::<&str>]);
        assert_ne!(hash_first_row(&value), hash_first_row(&null));
    }
}
