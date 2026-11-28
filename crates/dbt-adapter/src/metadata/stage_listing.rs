//! The newest last modified time of the files in Snowflake stages, from
//! `LIST`.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow::array::{Array, RecordBatch, StringArray};
use arrow::compute::{CastOptions, cast_with_options};
use arrow_schema::DataType;
use dbt_adapter_engine::MapReduce;
use dbt_adbc::QueryCtx;
use dbt_common::ErrorCode;
use dbt_common::cancellation::{Cancellable, CancellationToken};
use dbt_common::tracing::dbt_emit::emit_warn_log_message;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::connection::{AdapterConnectionFactory, ConnectionGuard};
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult};

/// A stage to list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageListing {
    /// The name the result is keyed by.
    pub name: String,
    /// The `LIST` statement that returns the files the stage reference reads,
    /// for example `LIST @"DB"."S"."STG"/2024/ PATTERN = '.*csv'`.
    pub list_sql: String,
}

/// The newest `last_modified` of the files `LIST` returned, in epoch
/// milliseconds, or `None` when there are no files.
///
/// `last_modified` is text such as `Mon, 11 Sep 2023 16:57:43 GMT`.
pub(crate) fn newest_last_modified(batch: &RecordBatch) -> AdapterResult<Option<i64>> {
    if batch.num_rows() == 0 {
        return Ok(None);
    }
    let unexpected =
        |message: String| AdapterError::new(AdapterErrorKind::UnexpectedResult, message);
    let schema = batch.schema();
    let (index, _) = schema
        .column_with_name("last_modified")
        .or_else(|| schema.column_with_name("LAST_MODIFIED"))
        .ok_or_else(|| unexpected("LIST returned no last_modified column".to_string()))?;
    let options = CastOptions {
        safe: false,
        ..Default::default()
    };
    let column = cast_with_options(batch.column(index), &DataType::Utf8, &options)
        .map_err(|e| unexpected(format!("LIST returned an unreadable last_modified: {e}")))?;
    let values = column
        .as_any()
        .downcast_ref::<StringArray>()
        .ok_or_else(|| unexpected("LIST returned an unreadable last_modified".to_string()))?;
    let mut newest = None;
    for value in values.iter().flatten() {
        let epoch_ms = chrono::DateTime::parse_from_rfc2822(value)
            .map_err(|e| unexpected(format!("LIST returned last_modified {value:?}: {e}")))?
            .timestamp_millis();
        newest = newest.max(Some(epoch_ms));
    }
    Ok(newest)
}

fn list_stage(
    adapter: &AdapterImpl,
    conn: &mut ConnectionGuard<'_>,
    stage: &StageListing,
    token: CancellationToken,
) -> AdapterResult<Option<i64>> {
    let ctx = QueryCtx::default().with_desc("dbt State stage last modified");
    let (_response, table) = adapter.query(&ctx, conn, &stage.list_sql, None, token)?;
    newest_last_modified(&table.original_record_batch())
}

/// Lists each stage in parallel. A stage whose `LIST` fails maps to `None`,
/// which dbt State treats as modified, and emits a warning. Cancellation and
/// missing replay data still fail the whole call.
pub(crate) fn list_stages(
    adapter: &AdapterImpl,
    stages: Vec<StageListing>,
    token: CancellationToken,
) -> AsyncAdapterResult<'static, BTreeMap<String, Option<i64>>> {
    type Acc = BTreeMap<String, Option<i64>>;
    let factory = Box::new(AdapterConnectionFactory::new(adapter.engine().clone()));
    let adapter = adapter.clone();
    let token_clone = token.clone();
    let map_f =
        move |conn: &mut ConnectionGuard<'_>, stage: &StageListing| -> AdapterResult<Option<i64>> {
            list_stage(&adapter, conn, stage, token_clone.clone())
        };
    let reduce_f = move |acc: &mut Acc,
                         stage: StageListing,
                         result: AdapterResult<Option<i64>>|
          -> Result<(), Cancellable<AdapterError>> {
        let epoch_ms = match result {
            Ok(epoch_ms) => epoch_ms,
            Err(err)
                if matches!(
                    err.kind(),
                    AdapterErrorKind::Cancelled | AdapterErrorKind::ReplayDataMissing
                ) =>
            {
                return Err(Cancellable::Error(err));
            }
            Err(err) => {
                emit_warn_log_message(
                    ErrorCode::StateStageListFailed,
                    format!(
                        "dbt State failed to list files in stage {}: {err}; treating it as modified",
                        stage.name
                    ),
                );
                None
            }
        };
        acc.insert(stage.name, epoch_ms);
        Ok(())
    };
    MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None).run(Arc::new(stages), token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{Field, Schema};

    fn batch(column: &str, values: Vec<Option<&str>>) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("name", DataType::Utf8, true),
                Field::new(column, DataType::Utf8, true),
            ])),
            vec![
                Arc::new(StringArray::from(vec![Some("f"); values.len()])),
                Arc::new(StringArray::from(values)),
            ],
        )
        .unwrap()
    }

    #[test]
    fn newest_last_modified_is_the_latest_file() {
        let listed = batch(
            "last_modified",
            vec![
                Some("Mon, 11 Sep 2023 16:57:43 GMT"),
                Some("Tue, 12 Sep 2023 08:00:00 GMT"),
                None,
            ],
        );
        assert_eq!(newest_last_modified(&listed).unwrap(), Some(1694505600000));
    }

    #[test]
    fn newest_last_modified_of_no_files_is_none() {
        assert_eq!(
            newest_last_modified(&batch("last_modified", vec![])).unwrap(),
            None
        );
    }

    #[test]
    fn newest_last_modified_rejects_unreadable_results() {
        assert!(newest_last_modified(&batch("size", vec![Some("1")])).is_err());
        assert!(newest_last_modified(&batch("last_modified", vec![Some("yesterday")])).is_err());
    }
}
