//! Epoch-append parquet for parse-time declared column metadata.
//!
//! Files land at:
//! ```text
//! target/
//!   metadata/parse/columns/v1_{N}.parquet   ← epoch-append, latest-wins by unique_id
//! ```
//!
//! ## Design
//! * Mirrors `compile/columns` but written at **parse** time from YAML declarations.
//! * Contains: name, description, declared_type, is_primary_key, constraints,
//!   meta, tags, granularity — everything the user wrote in schema YAML.
//! * `compile/columns` adds inferred_type and column_index on top; dbt-index
//!   reads parse first, compile second — compile wins on overlap.
//! * Sources and disabled models that are never compiled still get column metadata.
//! * Latest-wins per `unique_id`: a re-parse replaces the whole column set.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, TimeUnit};
use dbt_common::{FsResult, stdfs};
use serde::{Deserialize, Serialize};

use crate::epoch_io;

// ── constants ─────────────────────────────────────────────────────────────────

const VERSION_PREFIX: &str = "v1_";

// ── row schema ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParseColumnRow {
    pub unique_id: String,
    pub column_name: String,
    pub description: Option<String>,
    /// User-declared type from YAML `data_type:`.
    pub declared_type: Option<String>,
    /// True when a `type: primary_key` constraint is on this column.
    pub is_primary_key: bool,
    /// Full constraints JSON: [{type, name, expression, ...}]
    pub constraints: Option<String>,
    /// Column-level meta JSON object.
    pub meta: Option<String>,
    /// Column-level tags.
    pub tags: Vec<String>,
    /// Time granularity (non-null implies time dimension).
    pub granularity: Option<String>,
    pub ingested_at: i64,
}

fn column_fields() -> Vec<Field> {
    vec![
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("column_name", DataType::Utf8, false),
        Field::new("description", DataType::Utf8, true),
        Field::new("declared_type", DataType::Utf8, true),
        Field::new("is_primary_key", DataType::Boolean, false),
        Field::new("constraints", DataType::Utf8, true),
        Field::new("meta", DataType::Utf8, true),
        Field::new(
            "tags",
            DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            false,
        ),
        Field::new("granularity", DataType::Utf8, true),
        Field::new(
            "ingested_at",
            DataType::Timestamp(TimeUnit::Microsecond, Some(Arc::from("UTC"))),
            false,
        ),
    ]
}

// ── epoch helpers ─────────────────────────────────────────────────────────────

fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    epoch_io::existing_epochs(dir, VERSION_PREFIX)
}

// ── compaction ────────────────────────────────────────────────────────────────

fn compact_epochs(dir: &Path, valid_ids: Option<&HashSet<String>>) -> FsResult<()> {
    let epochs = existing_epochs(dir);
    if epochs.is_empty() {
        return Ok(());
    }

    let mut best: HashMap<String, (i64, Vec<ParseColumnRow>)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<ParseColumnRow>(path) {
            let entry = best.entry(row.unique_id.clone()).or_insert((0, Vec::new()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1.clear();
                entry.1.push(row);
            } else if row.ingested_at == entry.0 {
                entry.1.push(row);
            }
        }
    }

    let mut merged: Vec<ParseColumnRow> = Vec::new();
    for (uid, (_, rows)) in best {
        if let Some(valid) = valid_ids {
            if !valid.contains(&uid) {
                continue;
            }
        }
        merged.extend(rows);
    }
    merged.sort_by(|a, b| {
        a.unique_id
            .cmp(&b.unique_id)
            .then(a.column_name.cmp(&b.column_name))
    });

    let consolidated = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    let tmp = dir.join(format!("{VERSION_PREFIX}.tmp.parquet"));
    epoch_io::write_rows(&tmp, &column_fields(), &merged)?;
    stdfs::rename(&tmp, &consolidated)?;

    for (n, path) in &epochs {
        if *n != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

// ── public API ────────────────────────────────────────────────────────────────

/// Write parse-time declared column rows for one invocation.
///
/// `recomputed_nodes`: if Some, only these nodes were re-parsed (delta write).
/// If None, this is a full parse (epoch 0).
pub fn write_parse_columns(
    dir: &Path,
    rows: Vec<ParseColumnRow>,
    recomputed_nodes: Option<&HashSet<String>>,
    alive_node_count: Option<usize>,
    valid_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let epoch = if recomputed_nodes.is_some() {
        epoch_io::next_epoch(dir, VERSION_PREFIX)
    } else {
        0
    };
    let path = dir.join(format!("{VERSION_PREFIX}{epoch}.parquet"));
    epoch_io::write_rows(&path, &column_fields(), &rows)?;

    if recomputed_nodes.is_none() {
        for (n, p) in existing_epochs(dir) {
            if n != 0 {
                let _ = std::fs::remove_file(p);
            }
        }
    }

    let epochs = existing_epochs(dir);
    if epoch_io::should_compact(rows.len(), alive_node_count.unwrap_or(0), epochs.len()) {
        compact_epochs(dir, valid_ids)?;
    }
    Ok(())
}

/// Read all parse-column rows (latest-wins per unique_id).
pub fn read_parse_columns(dir: &Path) -> Vec<ParseColumnRow> {
    let epochs = existing_epochs(dir);
    let mut best: HashMap<String, (i64, Vec<ParseColumnRow>)> = HashMap::new();
    for (_, path) in &epochs {
        for row in epoch_io::read_rows::<ParseColumnRow>(path) {
            let entry = best.entry(row.unique_id.clone()).or_insert((0, Vec::new()));
            if row.ingested_at > entry.0 {
                entry.0 = row.ingested_at;
                entry.1.clear();
                entry.1.push(row);
            } else if row.ingested_at == entry.0 {
                entry.1.push(row);
            }
        }
    }
    let mut result: Vec<ParseColumnRow> = best.into_values().flat_map(|(_, rows)| rows).collect();
    result.sort_by(|a, b| {
        a.unique_id
            .cmp(&b.unique_id)
            .then(a.column_name.cmp(&b.column_name))
    });
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(uid: &str, col: &str, desc: &str, ts: i64) -> ParseColumnRow {
        ParseColumnRow {
            unique_id: uid.to_string(),
            column_name: col.to_string(),
            description: Some(desc.to_string()),
            declared_type: Some("VARCHAR".to_string()),
            is_primary_key: false,
            constraints: None,
            meta: None,
            tags: vec![],
            granularity: None,
            ingested_at: ts,
        }
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![
            make_row("model.pkg.users", "id", "Primary key", 100),
            make_row("model.pkg.users", "email", "User email", 100),
        ];
        write_parse_columns(dir.path(), rows, None, None, None).unwrap();
        let back = read_parse_columns(dir.path());
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].column_name, "email");
        assert_eq!(back[1].column_name, "id");
    }

    #[test]
    fn latest_wins_per_node() {
        let dir = tempfile::tempdir().unwrap();
        let mut targets = HashSet::new();
        targets.insert("model.pkg.a".to_string());

        // First parse: 2 columns
        let rows1 = vec![
            make_row("model.pkg.a", "x", "old", 1),
            make_row("model.pkg.a", "y", "old", 1),
        ];
        write_parse_columns(dir.path(), rows1, Some(&targets), None, None).unwrap();

        // Re-parse: 1 column (y removed, x updated)
        let rows2 = vec![make_row("model.pkg.a", "x", "new", 2)];
        write_parse_columns(dir.path(), rows2, Some(&targets), None, None).unwrap();

        let back = read_parse_columns(dir.path());
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].description.as_deref(), Some("new"));
    }

    #[test]
    fn primary_key_flag() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![ParseColumnRow {
            unique_id: "model.pkg.orders".to_string(),
            column_name: "order_id".to_string(),
            description: None,
            declared_type: None,
            is_primary_key: true,
            constraints: Some(r#"[{"type":"primary_key"}]"#.to_string()),
            meta: None,
            tags: vec![],
            granularity: None,
            ingested_at: 1,
        }];
        write_parse_columns(dir.path(), rows, None, None, None).unwrap();
        let back = read_parse_columns(dir.path());
        assert!(back[0].is_primary_key);
    }
}
