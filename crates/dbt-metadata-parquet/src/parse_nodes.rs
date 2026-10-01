//! Read-only access to `target/private/metadata/parse/nodes/` parquet files.
//!
//! Only the fields needed by `dbt agent schema` are read:
//! `unique_id`, `name`, `resource_type`, `original_path`, `depends_on`.
//!
//! Uses column projection so the large `payload` column is never decoded.

use std::collections::HashMap;
use std::path::Path;

use parquet::arrow::{ProjectionMask, arrow_reader::ParquetRecordBatchReaderBuilder};
use serde::Deserialize;

use crate::epoch_io;

const VERSION_PREFIX: &str = "v1_";

/// Minimal view of a parse-state node row — only the fields useful for schema output.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ParseNodeRow {
    pub unique_id: String,
    pub name: String,
    pub resource_type: String,
    pub original_path: String,
    /// Node-level description from schema YAML (promoted field, None when absent).
    pub description: Option<String>,
    /// Direct upstream dependencies (`model.*` and `source.*` entries).
    pub depends_on: Vec<String>,
}

/// Columns to project — omits large fields like `payload`, `fqn`, `tags`.
const PROJECTED: &[&str] = &[
    "unique_id",
    "name",
    "resource_type",
    "original_path",
    "description",
    "depends_on",
];

fn read_file(path: &std::path::PathBuf) -> Vec<ParseNodeRow> {
    let Ok(file) = std::fs::File::open(path) else {
        return vec![];
    };
    let Ok(builder) = ParquetRecordBatchReaderBuilder::try_new(file) else {
        return vec![];
    };
    // Roots-based projection is required for list columns (depends_on).
    let arrow_schema = builder.schema();
    let col_indices: Vec<usize> = PROJECTED
        .iter()
        .filter_map(|n| arrow_schema.index_of(n).ok())
        .collect();
    let mask = ProjectionMask::roots(builder.parquet_schema(), col_indices);
    let Ok(reader) = builder.with_projection(mask).build() else {
        return vec![];
    };
    let mut out = Vec::new();
    for batch in reader.flatten() {
        if let Ok(mut rows) = serde_arrow::from_record_batch::<Vec<ParseNodeRow>>(&batch) {
            out.append(&mut rows);
        }
    }
    out
}

/// Read parse-node rows from all epochs under `dir` (e.g. `target/private/metadata/parse/nodes/`).
/// Latest-wins per `unique_id` — highest `ingested_at` epoch file wins.
/// If only one epoch file exists the single-file fast path is taken.
pub fn read_parse_nodes(dir: &Path) -> Vec<ParseNodeRow> {
    let epochs = epoch_io::existing_epochs(dir, VERSION_PREFIX);
    if epochs.is_empty() {
        return vec![];
    }
    if epochs.len() == 1 {
        return read_file(&epochs[0].1);
    }
    // Multiple epochs: last writer wins per unique_id (epochs are sorted ascending).
    let mut by_id: HashMap<String, ParseNodeRow> = HashMap::new();
    for (_, path) in &epochs {
        for row in read_file(path) {
            by_id.insert(row.unique_id.clone(), row);
        }
    }
    by_id.into_values().collect()
}
