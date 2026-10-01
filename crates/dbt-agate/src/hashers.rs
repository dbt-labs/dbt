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
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize);
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
}

impl ColumnHasher for BooleanColumnHasher {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.is_valid(row_idx) as u32;
        let value = self.values.value(row_idx) as u32;
        let buf4: u32 = (is_valid & value) | (is_valid << 8);
        hasher.write_u32(buf4);
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
        impl ColumnHasher for PrimitiveColumnHasher<$arrow_data_ty> {
            fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
                let is_valid = self.is_valid(row_idx);
                let value = self.values[row_idx];
                if mem::size_of_val(&value) == 8 {
                    let value_as_u64: u64 = if is_valid {
                        // hash f64 as u64 by transmuting the bits
                        unsafe { core::mem::transmute_copy(&value) }
                    } else {
                        0u64
                    };
                    is_valid.hash(hasher);
                    value_as_u64.hash(hasher);
                } else {
                    let mut value_as_u64: u64 = if is_valid {
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
                    } else {
                        0u64
                    };
                    // embed the is_valid boolean into the u64
                    value_as_u64 |= ((is_valid as u64) << 32);
                    value_as_u64.hash(hasher);
                }
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
}

impl<T: DecimalType> ColumnHasher for DecimalColumnHasher<T> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, idx: usize) {
        let is_valid = self.is_valid(idx);
        let value_bits = if is_valid {
            self.values[idx]
        } else {
            T::Native::default()
        };

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
// }}}

// String and Binary {{{
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
        let value = self.array.value(row_idx);
        let str = if is_valid { value } else { "" };
        is_valid.hash(hasher);
        str.hash(hasher);
    }
}

impl<O: OffsetSizeTrait> ColumnHasher for GenericByteColumnHasher<GenericBinaryType<O>> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = self.array.value(row_idx);
        let bytes = if is_valid { value } else { b"" };
        is_valid.hash(hasher);
        bytes.hash(hasher);
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
        let value = self.array.value(row_idx);
        let str = if is_valid { value } else { "" };
        is_valid.hash(hasher);
        str.hash(hasher);
    }
}

impl ColumnHasher for ByteViewColumnHasher<BinaryViewType> {
    fn write_row(&self, hasher: &mut sip128::SipHasher13, row_idx: usize) {
        let is_valid = self.array.is_valid(row_idx);
        let value = self.array.value(row_idx);
        let bytes = if is_valid { value } else { b"" };
        is_valid.hash(hasher);
        bytes.hash(hasher);
    }
}

type StringViewColumnHasher = ByteViewColumnHasher<StringViewType>;
type BinaryViewColumnHasher = ByteViewColumnHasher<BinaryViewType>;
// }}}

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

pub fn make_column_hasher(array: &dyn Array) -> Result<Box<dyn ColumnHasher>, ArrowError> {
    let converter: Box<dyn ColumnHasher> = match array.data_type() {
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
        BinaryArray, BinaryViewArray, LargeBinaryArray, LargeStringArray, StringArray,
        StringViewArray,
    };

    fn hash_first_row(array: &dyn Array) -> u64 {
        let column_hasher = make_column_hasher(array).unwrap();
        let mut hasher = sip128::SipHasher13::new();
        column_hasher.write_row(&mut hasher, 0);
        hasher.finish()
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
