//! Epoch-backed Jinja render traces.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use arrow::datatypes::{DataType, Field, Fields};
use dbt_common::{FsResult, stdfs};
use serde::{Deserialize, Serialize};

use crate::epoch_io;

const VERSION_PREFIX: &str = "v1_";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderTraceEventRow {
    pub kind: String,
    pub source_start: i64,
    pub source_end: i64,
    pub rendered_start: i64,
    pub rendered_end: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RenderTraceRow {
    pub protocol_version: i64,
    pub ingested_at: i64,
    pub dbt_version: String,
    pub invocation_id: String,
    pub unique_id: String,
    pub filename: String,
    pub source_path: String,
    pub render_stage: String,
    pub source: String,
    pub rendered: String,
    pub events: Vec<RenderTraceEventRow>,
}

pub fn fields() -> Vec<Field> {
    let event_fields = Fields::from(vec![
        Field::new("kind", DataType::Utf8, false),
        Field::new("source_start", DataType::Int64, false),
        Field::new("source_end", DataType::Int64, false),
        Field::new("rendered_start", DataType::Int64, false),
        Field::new("rendered_end", DataType::Int64, false),
    ]);
    vec![
        Field::new("protocol_version", DataType::Int64, false),
        Field::new("ingested_at", DataType::Int64, false),
        Field::new("dbt_version", DataType::Utf8, false),
        Field::new("invocation_id", DataType::Utf8, false),
        Field::new("unique_id", DataType::Utf8, false),
        Field::new("filename", DataType::Utf8, false),
        Field::new("source_path", DataType::Utf8, false),
        Field::new("render_stage", DataType::Utf8, false),
        Field::new("source", DataType::Utf8, false),
        Field::new("rendered", DataType::Utf8, false),
        Field::new(
            "events",
            DataType::List(Field::new("item", DataType::Struct(event_fields), false).into()),
            false,
        ),
    ]
}

fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    epoch_io::existing_epochs(dir, VERSION_PREFIX)
}

fn compact(
    dir: &Path,
    recomputed_ids: &HashSet<String>,
    written_ids: &HashSet<String>,
    valid_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    let epochs = existing_epochs(dir);
    let rows: Vec<_> = read_render_traces_latest(dir)
        .into_iter()
        .filter(|row| valid_ids.is_none_or(|ids| ids.contains(&row.unique_id)))
        .filter(|row| {
            !recomputed_ids.contains(&row.unique_id) || written_ids.contains(&row.unique_id)
        })
        .collect();
    let path = dir.join(format!("{VERSION_PREFIX}0.parquet"));
    epoch_io::write_rows(&path, &fields(), &rows)?;
    for (epoch, path) in epochs {
        if epoch != 0 {
            let _ = std::fs::remove_file(path);
        }
    }
    Ok(())
}

pub fn write_render_traces(
    dir: &Path,
    mut rows: Vec<RenderTraceRow>,
    recomputed_ids: &HashSet<String>,
    valid_ids: Option<&HashSet<String>>,
) -> FsResult<()> {
    let epochs_before = existing_epochs(dir);
    if rows.is_empty() && epochs_before.is_empty() {
        return Ok(());
    }
    let written_ids: HashSet<_> = rows.iter().map(|row| row.unique_id.clone()).collect();
    if !rows.is_empty() {
        stdfs::create_dir_all(dir)?;
    }
    rows.sort_by(|left, right| left.unique_id.cmp(&right.unique_id));
    if !rows.is_empty() {
        let epoch = epoch_io::next_epoch(dir, VERSION_PREFIX);
        let path = dir.join(format!("{VERSION_PREFIX}{epoch}.parquet"));
        epoch_io::write_rows(&path, &fields(), &rows)?;
    }

    let file_count = existing_epochs(dir).len();
    let missing_recomputed = !recomputed_ids.is_subset(&written_ids);
    let has_deleted = valid_ids.is_some_and(|ids| {
        read_render_traces_latest(dir)
            .iter()
            .any(|row| !ids.contains(&row.unique_id))
    });
    if missing_recomputed
        || has_deleted
        || epoch_io::should_compact(rows.len(), valid_ids.map_or(0, HashSet::len), file_count)
    {
        compact(dir, recomputed_ids, &written_ids, valid_ids)?;
    }
    Ok(())
}

pub fn read_render_traces_latest(dir: &Path) -> Vec<RenderTraceRow> {
    let mut latest = HashMap::<String, RenderTraceRow>::new();
    for (_, path) in existing_epochs(dir) {
        for row in epoch_io::read_rows::<RenderTraceRow>(&path) {
            latest
                .entry(row.unique_id.clone())
                .and_modify(|current| {
                    if row.ingested_at >= current.ingested_at {
                        *current = row.clone();
                    }
                })
                .or_insert(row);
        }
    }
    let mut rows: Vec<_> = latest.into_values().collect();
    rows.sort_by(|left, right| left.unique_id.cmp(&right.unique_id));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::Schema;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;

    fn row(unique_id: &str, invocation_id: &str, ingested_at: i64, kind: &str) -> RenderTraceRow {
        RenderTraceRow {
            protocol_version: 2,
            ingested_at,
            dbt_version: "0.0.0-test".to_owned(),
            invocation_id: invocation_id.to_owned(),
            unique_id: unique_id.to_owned(),
            filename: "models/example.sql".to_owned(),
            source_path: "models/example.sql".to_owned(),
            render_stage: "jinja_before_ephemeral_injection".to_owned(),
            source: "select {{ value }}".to_owned(),
            rendered: "select 1".to_owned(),
            events: vec![RenderTraceEventRow {
                kind: kind.to_owned(),
                source_start: 0,
                source_end: 18,
                rendered_start: 0,
                rendered_end: 8,
            }],
        }
    }

    #[test]
    fn nested_events_roundtrip_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut trace = row("model.example.trace", "first", 1, "literal");
        trace.events.push(RenderTraceEventRow {
            kind: "macro".to_owned(),
            source_start: 7,
            source_end: 18,
            rendered_start: 7,
            rendered_end: 8,
        });
        write_render_traces(dir.path(), vec![trace.clone()], &HashSet::new(), None).unwrap();
        assert_eq!(read_render_traces_latest(dir.path()), vec![trace]);

        let file = std::fs::File::open(dir.path().join("v1_0.parquet")).unwrap();
        let reader = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let expected_events = Fields::from(vec![
            Field::new("kind", DataType::Utf8, false),
            Field::new("source_start", DataType::Int64, false),
            Field::new("source_end", DataType::Int64, false),
            Field::new("rendered_start", DataType::Int64, false),
            Field::new("rendered_end", DataType::Int64, false),
        ]);
        let expected = Schema::new(vec![
            Field::new("protocol_version", DataType::Int64, false),
            Field::new("ingested_at", DataType::Int64, false),
            Field::new("dbt_version", DataType::Utf8, false),
            Field::new("invocation_id", DataType::Utf8, false),
            Field::new("unique_id", DataType::Utf8, false),
            Field::new("filename", DataType::Utf8, false),
            Field::new("source_path", DataType::Utf8, false),
            Field::new("render_stage", DataType::Utf8, false),
            Field::new("source", DataType::Utf8, false),
            Field::new("rendered", DataType::Utf8, false),
            Field::new(
                "events",
                DataType::List(Field::new("item", DataType::Struct(expected_events), false).into()),
                false,
            ),
        ]);
        assert_eq!(reader.schema().as_ref(), &expected);
    }

    #[test]
    fn later_epoch_wins_and_compaction_prunes_dead_nodes() {
        let dir = tempfile::tempdir().unwrap();
        write_render_traces(
            dir.path(),
            vec![
                row("model.example.keep", "first", 1, "literal"),
                row("model.example.remove", "first", 1, "literal"),
            ],
            &HashSet::new(),
            None,
        )
        .unwrap();
        let mut changed = vec![row("model.example.keep", "second", 2, "macro")];
        changed.extend((0..10).map(|index| {
            row(
                &format!("model.example.extra_{index}"),
                "second",
                2,
                "literal",
            )
        }));
        write_render_traces(
            dir.path(),
            changed,
            &HashSet::new(),
            Some(&HashSet::from(["model.example.keep".to_owned()])),
        )
        .unwrap();
        let rows = read_render_traces_latest(dir.path());
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].invocation_id, "second");
        assert_eq!(rows[0].events[0].kind, "macro");
    }

    #[test]
    fn timestamp_wins_even_when_written_in_an_earlier_epoch() {
        let dir = tempfile::tempdir().unwrap();
        write_render_traces(
            dir.path(),
            vec![row("model.example.trace", "newer", 2, "macro")],
            &HashSet::new(),
            None,
        )
        .unwrap();
        write_render_traces(
            dir.path(),
            vec![row("model.example.trace", "older", 1, "literal")],
            &HashSet::new(),
            None,
        )
        .unwrap();
        assert_eq!(
            read_render_traces_latest(dir.path())[0].invocation_id,
            "newer"
        );
    }

    #[test]
    fn missing_recomputed_trace_removes_stale_row() {
        let dir = tempfile::tempdir().unwrap();
        let unique_id = "model.example.trace".to_owned();
        write_render_traces(
            dir.path(),
            vec![row(&unique_id, "first", 1, "literal")],
            &HashSet::new(),
            None,
        )
        .unwrap();
        write_render_traces(dir.path(), Vec::new(), &HashSet::from([unique_id]), None).unwrap();
        assert!(read_render_traces_latest(dir.path()).is_empty());
    }

    #[test]
    fn empty_write_still_prunes_deleted_nodes() {
        let dir = tempfile::tempdir().unwrap();
        write_render_traces(
            dir.path(),
            vec![row("model.example.deleted", "first", 1, "literal")],
            &HashSet::new(),
            None,
        )
        .unwrap();
        write_render_traces(
            dir.path(),
            Vec::new(),
            &HashSet::new(),
            Some(&HashSet::new()),
        )
        .unwrap();
        assert!(read_render_traces_latest(dir.path()).is_empty());
    }
}
