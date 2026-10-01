//! Projection: source record batches -> information schema record batches.
//!
//! Output Arrow types come from the source field, so a rename never changes a
//! column's type. Output fields are always declared nullable: a projected array
//! keeps its own null buffer, and a nullable field accepts an array with no
//! nulls, so this is always sound and avoids restating nullability per column.

use std::collections::HashMap;
use std::sync::Arc;

use arrow_array::{Array, RecordBatch, StringArray, UInt32Array, new_null_array};
use arrow_schema::{Field, Schema, SchemaRef};

use crate::IndexError;
use crate::parquet::schema_for;

use super::spec::{Filter, Src, TableSpec};

/// Resolve the source field for an output column, honouring join side order.
fn source_field(spec: &TableSpec, src_name: &str) -> Option<Field> {
    let lookup = |table: &str| -> Option<Field> {
        let s = schema_for(table);
        s.field_with_name(src_name).ok().cloned()
    };
    match spec.src {
        Src::Table(t) => lookup(t),
        Src::Join { left, right, .. } => lookup(left).or_else(|| lookup(right)),
        Src::Own => None,
    }
}

/// The Arrow schema of an output table. Pure — no file access, so it is
/// identical whether or not the source table was ever written.
pub fn out_schema(spec: &TableSpec) -> Result<SchemaRef, IndexError> {
    let mut fields: Vec<Field> = Vec::with_capacity(spec.cols.len());
    for col in spec.cols {
        let field = match col.ty {
            Some(ty) => Field::new(col.out, ty.data_type(), true),
            None => {
                let src = source_field(spec, col.src).ok_or_else(|| {
                    IndexError::Other(format!(
                        "info schema: {} column '{}' has no source column '{}'",
                        spec.qualified_name(),
                        col.out,
                        col.src
                    ))
                })?;
                Field::new(col.out, src.data_type().clone(), true)
            }
        };
        fields.push(field);
    }
    Ok(Arc::new(Schema::new(fields)))
}

/// Row mask for `spec.filter`. `None` means "keep everything".
fn filter_mask(spec: &TableSpec, batch: &RecordBatch) -> Option<arrow_array::BooleanArray> {
    let types = match spec.filter {
        Filter::All => return None,
        Filter::ResourceTypeIn(types) => types,
    };
    let idx = batch.schema().index_of("resource_type").ok()?;
    let col = batch.column(idx).as_any().downcast_ref::<StringArray>()?;
    let keep: Vec<bool> = (0..batch.num_rows())
        .map(|i| !col.is_null(i) && types.contains(&col.value(i)))
        .collect();
    Some(arrow_array::BooleanArray::from(keep))
}

/// Take one output column out of an already-aligned source batch.
fn column_from(batch: &RecordBatch, src_name: &str, out_field: &Field) -> arrow_array::ArrayRef {
    match batch.schema().index_of(src_name) {
        Ok(i) => Arc::clone(batch.column(i)),
        // The source table exists but predates this column: emit nulls rather
        // than failing, so an older target directory stays readable.
        Err(_) => new_null_array(out_field.data_type(), batch.num_rows()),
    }
}

/// Project `batches` of a single source table through `spec`.
pub fn project_table(
    spec: &TableSpec,
    batches: &[RecordBatch],
    out: &SchemaRef,
) -> Result<Vec<RecordBatch>, IndexError> {
    let mut result = Vec::with_capacity(batches.len());
    for batch in batches {
        let batch = match filter_mask(spec, batch) {
            Some(mask) => arrow_select::filter::filter_record_batch(batch, &mask)
                .map_err(|e| IndexError::Other(format!("info schema filter: {e}")))?,
            None => batch.clone(),
        };
        if batch.num_rows() == 0 {
            continue;
        }
        let cols: Vec<arrow_array::ArrayRef> = spec
            .cols
            .iter()
            .zip(out.fields())
            .map(|(col, field)| match col.ty {
                Some(_) => new_null_array(field.data_type(), batch.num_rows()),
                None => column_from(&batch, col.src, field),
            })
            .collect();
        result.push(
            RecordBatch::try_new(Arc::clone(out), cols)
                .map_err(|e| IndexError::Other(format!("info schema batch: {e}")))?,
        );
    }
    Ok(result)
}

/// Project a left join of two source tables through `spec`.
///
/// Left rows are filtered first, then matched against the right table by key.
/// Unmatched left rows are kept with the right-hand columns null, so a row that
/// has no detail record is never dropped.
pub fn project_join(
    spec: &TableSpec,
    left: &[RecordBatch],
    right: &[RecordBatch],
    out: &SchemaRef,
) -> Result<Vec<RecordBatch>, IndexError> {
    let (left_on, right_on, right_table) = match spec.src {
        Src::Join {
            left_on,
            right_on,
            right,
            ..
        } => (left_on, right_on, right),
        _ => return Err(IndexError::Other("project_join on a non-join spec".into())),
    };

    // Filtered, single-batch left side.
    let left_batches = {
        let mut v = Vec::new();
        for batch in left {
            let b = match filter_mask(spec, batch) {
                Some(mask) => arrow_select::filter::filter_record_batch(batch, &mask)
                    .map_err(|e| IndexError::Other(format!("info schema filter: {e}")))?,
                None => batch.clone(),
            };
            if b.num_rows() > 0 {
                v.push(b);
            }
        }
        v
    };
    if left_batches.is_empty() {
        return Ok(Vec::new());
    }
    let left_schema = left_batches[0].schema();
    let left = arrow_select::concat::concat_batches(&left_schema, &left_batches)
        .map_err(|e| IndexError::Other(format!("info schema concat left: {e}")))?;

    // Single-batch right side, plus key -> row index. Later rows win.
    let right_schema = right
        .first()
        .map(|b| b.schema())
        .unwrap_or_else(|| schema_for(right_table));
    let right_nonempty: Vec<RecordBatch> =
        right.iter().filter(|b| b.num_rows() > 0).cloned().collect();
    let right = if right_nonempty.is_empty() {
        RecordBatch::new_empty(Arc::clone(&right_schema))
    } else {
        arrow_select::concat::concat_batches(&right_schema, &right_nonempty)
            .map_err(|e| IndexError::Other(format!("info schema concat right: {e}")))?
    };

    let mut by_key: HashMap<&str, u32> = HashMap::with_capacity(right.num_rows());
    if let Ok(i) = right.schema().index_of(right_on) {
        if let Some(keys) = right.column(i).as_any().downcast_ref::<StringArray>() {
            for row in 0..right.num_rows() {
                if !keys.is_null(row) {
                    by_key.insert(keys.value(row), row as u32);
                }
            }
        }
    }

    // Per left row, the matching right row or null.
    let take_idx: UInt32Array = {
        let left_keys = left.schema().index_of(left_on).ok().and_then(|i| {
            left.column(i)
                .as_any()
                .downcast_ref::<StringArray>()
                .cloned()
        });
        match left_keys {
            Some(keys) => (0..left.num_rows())
                .map(|row| {
                    if keys.is_null(row) {
                        None
                    } else {
                        by_key.get(keys.value(row)).copied()
                    }
                })
                .collect(),
            None => (0..left.num_rows()).map(|_| None).collect(),
        }
    };

    // Take column by column rather than as a batch: an unmatched left row
    // yields a null, which a source column declared non-nullable would reject
    // if the taken columns were reassembled against the source schema.
    let mut right_taken: HashMap<String, arrow_array::ArrayRef> = HashMap::new();
    if right.num_rows() > 0 {
        for (i, field) in right.schema().fields().iter().enumerate() {
            let taken = arrow_select::take::take(right.column(i).as_ref(), &take_idx, None)
                .map_err(|e| IndexError::Other(format!("info schema take: {e}")))?;
            right_taken.insert(field.name().clone(), taken);
        }
    }

    let cols: Vec<arrow_array::ArrayRef> = spec
        .cols
        .iter()
        .zip(out.fields())
        .map(|(col, field)| {
            if col.ty.is_some() {
                return new_null_array(field.data_type(), left.num_rows());
            }
            // Left side wins; fall through to the right side.
            if left.schema().index_of(col.src).is_ok() {
                return column_from(&left, col.src, field);
            }
            match right_taken.get(col.src) {
                Some(arr) => Arc::clone(arr),
                None => new_null_array(field.data_type(), left.num_rows()),
            }
        })
        .collect();

    Ok(vec![RecordBatch::try_new(Arc::clone(out), cols).map_err(
        |e| IndexError::Other(format!("info schema join batch: {e}")),
    )?])
}
