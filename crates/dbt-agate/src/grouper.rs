use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use arrow_array::RecordBatch;
use arrow_schema::ArrowError;
use siphasher::sip128;

use crate::hashers::ColumnHasher;
use crate::hashers::TableHasher;

pub struct Grouper {
    /// Initial hasher state based on the schema of the grouping/join keys
    initial_hasher: sip128::SipHasher13,
    /// Table hasher for computing row hashes
    table_hasher: TableHasher,
    /// Number of rows in the table/projection
    num_rows: usize,
}

impl Grouper {
    #[inline(never)]
    pub fn from_record_batch_columns(
        record_batch: &RecordBatch,
        column_indices: &[usize],
    ) -> Result<Self, ArrowError> {
        const MARKER: u64 = 0xFFFFFFFFFFFFFFFFu64;
        let initial_hasher = {
            let schema_ref = record_batch.schema_ref();
            let mut initial_hasher = sip128::SipHasher13::new();
            for column_idx in column_indices {
                schema_ref
                    .field(*column_idx)
                    .data_type()
                    .hash(&mut initial_hasher);
                initial_hasher.write_u64(MARKER);
            }
            initial_hasher
        };
        let columns = column_indices
            .iter()
            .map(|&i| record_batch.column(i).as_ref());
        let table_hasher = TableHasher::try_new(columns)?;
        Ok(Self {
            initial_hasher,
            table_hasher,
            num_rows: record_batch.num_rows(),
        })
    }

    /// Produce a unique fingerprint for the row values in the projection.
    ///
    /// For a grouper with `column_indices = [0, 2]`, the row hash is computed as:
    ///
    /// ```text
    /// hash(row) = hash(columns[0][row], columns[2][row])
    /// ```
    #[inline]
    pub(crate) fn hash_row(&self, row_idx: usize) -> SipHash128 {
        use sip128::Hasher128 as _;
        let mut hasher = self.initial_hasher;
        self.table_hasher.write_row(&mut hasher, row_idx);
        hasher.finish128().into()
    }

    pub fn iter(&self) -> GroupIterator<'_> {
        GroupIterator::new(self)
    }
}

/// A wrapper around [sip128::Hash128] to implement [Hash] and [Eq].
#[derive(PartialEq, Eq)]
pub(crate) struct SipHash128 {
    pub h1: u64,
    pub h2: u64,
}

impl From<sip128::Hash128> for SipHash128 {
    fn from(value: sip128::Hash128) -> Self {
        Self {
            h1: value.h1,
            h2: value.h2,
        }
    }
}

impl Hash for SipHash128 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // SipHash128 is already a hash, so any 64 bits from it will do
        state.write_u64(self.h1);
    }
}

/// Produces a stream of dense group IDs starting from 0 for each row in the table.
pub struct GroupIterator<'a> {
    grouper: &'a Grouper,
    row_idx: usize,
    groups: HashMap<SipHash128, usize>,
    next_group_id: usize,
}

impl<'a> GroupIterator<'a> {
    pub fn new(grouper: &'a Grouper) -> Self {
        Self {
            grouper,
            row_idx: 0,
            groups: HashMap::new(),
            next_group_id: 0,
        }
    }
}

impl Iterator for GroupIterator<'_> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.row_idx < self.grouper.num_rows {
            let hash: SipHash128 = self.grouper.hash_row(self.row_idx);
            self.row_idx += 1;
            let group_id = self.groups.entry(hash).or_insert_with(|| {
                let group_id = self.next_group_id;
                self.next_group_id += 1;
                group_id
            });
            Some(*group_id)
        } else {
            None
        }
    }
}
