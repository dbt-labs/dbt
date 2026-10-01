use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use adbc_core::options::{OptionStatement, OptionValue};
use arrow_array::RecordBatch;
use arrow_schema::Schema;
use dbt_adbc::bigquery::QUERY_LABELS;
use dbt_adbc::{Backend, Connection, QueryCtx, Statement};
use dbt_auth::AdapterConfig;
use dbt_common::behavior_flags::Behavior;
use dbt_common::cancellation::CancellationToken;
use dbt_common::hashing::code_hash;
use dbt_common::tracing::span_info::{
    read_current_span_start_info, record_current_span_status_from_attrs,
};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult, Cancellable, create_debug_span};
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_sql_utils::snowflake_terminal_flow_statement;
use dbt_telemetry::{QueryExecuted, QueryOutcome};
use dbt_tracked_stmt::TrackedStatement;
use indexmap::IndexMap;
use minijinja::State;
use tracy_client::span;

use crate::AdapterType;
use crate::cache::RelationCache;
use crate::engine::concat_batches::concat_batches_widened;
use crate::engine::databricks_query_tags::query_tags_from_state;
use crate::engine::query_comment::QueryCommentConfig;
use crate::engine::sidecar_client::SidecarClient;
use crate::errors::adbc_error_to_adapter_error;
use crate::record_batch::{ROWS_AFFECTED_META, RecordBatchExt, SchemaExt};
use crate::response::query_id_from_record_batch;
use crate::sql::normalize::strip_sql_comments;
use crate::sql_types::TypeOps;
use crate::statement::*;
use crate::stmt_splitter::StmtSplitter;

pub type Options = Vec<(String, OptionValue)>;

/// Normalize result column names for Snowflake commands whose output schema is
/// defined as lowercase.
///
/// Snowflake `SHOW` and `DESCRIBE` output columns are lowercase, but the ADBC
/// driver can report them in uppercase. Ordinary query results retain the
/// driver-reported casing.
fn normalize_result_column_names(
    adapter_type: AdapterType,
    sql: &str,
    batch: RecordBatch,
) -> RecordBatch {
    if adapter_type != AdapterType::Snowflake {
        return batch;
    }

    let result_statement = snowflake_terminal_flow_statement(sql);
    let normalized_sql = strip_sql_comments(result_statement);
    let first_keyword = normalized_sql.split_whitespace().next();
    if first_keyword.is_some_and(|keyword| {
        keyword.eq_ignore_ascii_case("show")
            || keyword.eq_ignore_ascii_case("describe")
            || keyword.eq_ignore_ascii_case("desc")
    }) {
        batch.lowercase_column_names()
    } else {
        batch
    }
}

/// A trait abstracting the layer between the adapter layer and database drivers.
///
/// Each concrete engine type (XDBC with live/mock/record/replay modes, sidecar)
/// implements this trait directly. This is the internal adapter service for other
/// Rust modules in Fusion as the adapter layer interface is forced to abide by
/// what is expected for consumption from Jinja code.
pub trait AdapterEngine: Send + Sync {
    /// Get the adapter type for this engine
    fn adapter_type(&self) -> AdapterType;

    /// Get the ADBC backend for this engine
    fn backend(&self) -> Backend;

    /// Get the resolved quoting policy
    fn quoting(&self) -> ResolvedQuoting;

    /// Get the statement splitter for this engine
    fn splitter(&self) -> &dyn StmtSplitter;

    /// Get the type operations for this engine
    fn type_ops(&self) -> &Arc<dyn TypeOps>;

    /// Get the query comment config for this engine
    fn query_comment(&self) -> &QueryCommentConfig;

    /// Get a config value by key
    fn config(&self, key: &str) -> Option<Cow<'_, str>>;

    /// Get the full config object
    fn get_config(&self) -> &AdapterConfig;

    /// Get a reference to the relation cache
    fn relation_cache(&self) -> &Arc<RelationCache>;

    /// Get the resolved behavior object with user overrides applied
    fn behavior(&self) -> &Arc<Behavior>;

    /// Get the user overrides for behavior flags
    fn behavior_flag_overrides(&self) -> &BTreeMap<String, bool>;

    /// Create a new connection to the warehouse.
    fn new_connection(
        &self,
        state: Option<&State>,
        node_id: Option<String>,
    ) -> AdapterResult<Box<dyn Connection>>;

    /// Create a new connection to the warehouse with the given config.
    fn new_connection_with_config(
        &self,
        config: &AdapterConfig,
    ) -> AdapterResult<Box<dyn Connection>>;

    fn has_query_cache(&self) -> bool {
        false
    }

    fn new_query_cache_statement(
        &self,
        _stmt: Box<dyn Statement>,
    ) -> AdapterResult<Box<dyn Statement>> {
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "Query cache not supported",
        ))
    }

    fn set_query_cache_reverse_deps(
        &self,
        _deps: BTreeMap<String, BTreeSet<String>>,
    ) -> AdapterResult<()> {
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "Query cache not supported",
        ))
    }

    /// Execute the given SQL query or statement with options.
    ///
    /// The default implementation uses ADBC to execute queries. Engines that
    /// route execution differently (e.g. sidecar) should override this.
    #[allow(clippy::too_many_arguments)]
    fn execute_with_options(
        &self,
        state: Option<&State>,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        options: Options,
        fetch: bool,
        token: CancellationToken,
    ) -> AdapterResult<RecordBatch> {
        adbc_execute_with_options(self, state, ctx, conn, sql, options, fetch, token)
    }

    // -- Methods with default implementations ---------------------------------

    /// The `threads` configuration value from the dbt profile.
    ///
    /// Used to derive connection concurrency limits in metadata adapters.
    /// Returns `None` when the setting is not available (mock, sidecar, etc.).
    fn threads(&self) -> Option<usize> {
        None
    }

    fn is_mock(&self) -> bool {
        false
    }

    /// Whether this is a sidecar engine (subprocess-based execution)
    fn is_sidecar(&self) -> bool {
        false
    }

    fn is_replay(&self) -> bool {
        false
    }

    /// Returns the config fingerprint of this engine's connections.
    ///
    /// The connection pool uses this to reuse a connection only among engines
    /// with an identical connection configuration. Override this in engines
    /// that create real connections so different configurations are not reused.
    fn fingerprint(&self) -> u64 {
        0
    }

    /// Fingerprints the connection `config` would open, without opening one.
    /// The pool reuses a connection only when this matches the connection's
    /// own fingerprint; a mismatch forces a new connection.
    fn fingerprint_for_config(&self, _config: &AdapterConfig) -> AdapterResult<u64> {
        Ok(self.fingerprint())
    }

    /// Get the physical execution backend for sidecar engines.
    ///
    /// Returns the actual database backend (DuckDB, Snowflake, etc.) that SQL
    /// will execute against. This differs from [`adapter_type()`] which returns
    /// the logical adapter type.
    fn physical_backend(&self) -> Option<Backend> {
        None
    }

    /// Get a reference to the sidecar client, if this is a sidecar engine.
    fn sidecar_client(&self) -> Option<&dyn SidecarClient> {
        None
    }

    /// Execute the given SQL query or statement (convenience wrapper).
    fn execute(
        &self,
        state: Option<&State>,
        conn: &'_ mut dyn Connection,
        ctx: &QueryCtx,
        sql: &str,
        token: CancellationToken,
    ) -> AdapterResult<RecordBatch> {
        self.execute_with_options(state, ctx, conn, sql, Options::new(), true, token)
    }

    fn get_configured_database_name(&self) -> Option<Cow<'_, str>> {
        self.config("database")
    }
}

/// Logs a perf-debugging step duration at DEBUG level (`RUST_LOG=debug`).
fn log_step_duration(label: &str, elapsed: std::time::Duration) {
    tracing::debug!("{label} took {elapsed:?}");
}

/// Default ADBC-based execute_with_options implementation.
///
/// Used by engines whose connections implement the full ADBC protocol
/// (AdbcEngine in Live, Record, and Replay modes).
#[allow(clippy::too_many_arguments)]
pub(crate) fn adbc_execute_with_options(
    engine: &(impl AdapterEngine + ?Sized),
    state: Option<&State>,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    sql: &str,
    options: Options,
    fetch: bool,
    token: CancellationToken,
) -> AdapterResult<RecordBatch> {
    assert!(!sql.is_empty() || !options.is_empty());

    let maybe_query_comment = state
        .map(|s| engine.query_comment().resolve_comment(s))
        .transpose()?;

    let sql = match &maybe_query_comment {
        Some(comment) => {
            let sql = engine.query_comment().add_comment(sql, comment);
            Cow::Owned(sql)
        }
        None => Cow::Borrowed(sql),
    };

    let adapter_type = engine.adapter_type();
    let mut options = options;
    match (state, adapter_type) {
        (_, AdapterType::Databricks) => {
            options.extend(query_tags_from_state(state)?.into_statement_options())
        }
        (_, AdapterType::ClickHouse) => {
            options.extend(crate::metadata::clickhouse::statement_options())
        }
        (Some(state), AdapterType::Bigquery) => {
            let mut job_labels =
                maybe_query_comment
                    .as_ref()
                    .map_or_else(IndexMap::new, |comment| {
                        engine
                            .query_comment()
                            .get_job_labels_from_query_comment(comment)
                    });
            if let Some(invocation_id_label) = state
                .lookup("invocation_id", &[])
                .and_then(|value| value.as_str().map(|label| label.to_owned()))
            {
                job_labels.insert("dbt_invocation_id".to_string(), invocation_id_label);
            }

            let job_label_option =
                serde_json::to_string(&job_labels).expect("Should be able to serialize job labels");
            options.push((
                QUERY_LABELS.to_owned(),
                OptionValue::String(job_label_option),
            ));
        }
        _ => {}
    }

    type ExecuteOutput = (Arc<Schema>, Vec<RecordBatch>, Option<i64>);
    let do_execute = |conn: &'_ mut dyn Connection| -> Result<
        ExecuteOutput,
        Cancellable<adbc_core::error::Error>,
    > {
        use dbt_adbc::statement::Statement as _;

        let mut stmt = if engine.has_query_cache() {
            let stmt = conn.new_statement()?;
            engine.new_query_cache_statement(stmt).map_err(|e| {
                Cancellable::Error(adbc_core::error::Error::with_message_and_status(
                    e.message(),
                    adbc_core::error::Status::Internal,
                ))
            })?
        } else {
            conn.new_statement()?
        };
        if let Some(node_id) = ctx.node_id() {
            stmt.set_option(
                OptionStatement::Other(DBT_NODE_ID.to_string()),
                OptionValue::String(node_id.clone()),
            )?;
        }
        if let Some(p) = ctx.phase() {
            stmt.set_option(
                OptionStatement::Other(DBT_EXECUTION_PHASE.to_string()),
                OptionValue::String(p.to_string()),
            )?;
        }
        if adapter_type == AdapterType::LakeCompute {
            if let Some(project) = ctx.project_name() {
                stmt.set_option(
                    OptionStatement::Other(DBT_PROJECT.to_string()),
                    OptionValue::String(project.clone()),
                )?;
            }
            if let Some(model) = ctx.model_name() {
                stmt.set_option(
                    OptionStatement::Other(DBT_MODEL.to_string()),
                    OptionValue::String(model.clone()),
                )?;
            }
            if let Some(run_id) = ctx.run_id() {
                stmt.set_option(
                    OptionStatement::Other(DBT_RUN_ID.to_string()),
                    OptionValue::String(run_id.clone()),
                )?;
            }
        }
        stmt.set_option(
            OptionStatement::Other(DBT_METADATA.to_string()),
            OptionValue::Int(ctx.is_metadata() as i64),
        )?;
        stmt.set_option(
            OptionStatement::Other(DBT_FETCH.to_string()),
            OptionValue::Int(fetch as i64),
        )?;
        if adapter_type == AdapterType::Snowflake
            && let Some(traceparent) = read_current_span_start_info(|info| {
                format!("00-{:032x}-{:016x}-01", info.trace_id, info.span_id)
            })
        {
            stmt.set_option(
                OptionStatement::Other("adbc.telemetry.trace_parent".to_string()),
                OptionValue::String(traceparent),
            )?;
        }
        options
            .into_iter()
            .try_for_each(|(key, value)| stmt.set_option(OptionStatement::Other(key), value))?;
        stmt.set_sql_query(sql.as_ref())?;

        // Make sure we don't create more statements after global cancellation.
        token.check_cancellation()?;

        // Track the statement so execution can be cancelled
        // when the user Ctrl-C's the process.
        let mut stmt = TrackedStatement::new(stmt);

        // Lake compute: every statement compute_platform.rs sends is DDL/DML
        // whose result is never read (it always passes fetch=false -- models
        // only ever create/drop/write, they don't read query results back).
        // `stmt.execute()` below calls `reader.schema()` unconditionally, which
        // for this driver forces a real export round trip (download_credentials
        // + list_files, ~2-3s) even though nothing will ever consume that
        // export. `execute_update()` skips export setup server-side entirely
        // and never touches the schema. This is only safe because dbt-compute
        // doesn't execute tests today (see `selects_lake_compute` routing in
        // dbt-tasks-sa/src/task.rs, keyed off the models table) -- a test
        // needs its result rows, so a future test-execution path over lake compute must
        // pass fetch=true and must not hit this branch.
        if adapter_type == AdapterType::LakeCompute && !fetch {
            let rows_affected = stmt.execute_update()?;
            token.check_cancellation()?;
            // Surface non-fatal backend warnings (e.g. an export-limit
            // truncation notice) and the job id via schema metadata, since
            // this function's return type has no other slot for
            // statement-level side channels. `AdapterResponse::from_record_batch`
            // reads both back; the job id is the only way to later recover
            // this statement's result via dbt-compute's `/download_credentials`.
            let warnings = stmt
                .get_option_string(OptionStatement::Other(
                    dbt_adbc::lake_compute::LAST_WARNINGS.to_string(),
                ))
                .unwrap_or_default();
            let query_id = stmt
                .get_option_string(OptionStatement::Other(
                    dbt_adbc::lake_compute::LAST_QUERY_ID.to_string(),
                ))
                .unwrap_or_default();
            let mut metadata = std::collections::HashMap::new();
            if !warnings.is_empty() {
                metadata.insert(
                    dbt_adbc::lake_compute::schema_metadata::WARNINGS.to_string(),
                    warnings,
                );
            }
            if !query_id.is_empty() {
                metadata.insert(
                    dbt_adbc::lake_compute::schema_metadata::QUERY_ID.to_string(),
                    query_id,
                );
            }
            let schema = if metadata.is_empty() {
                Arc::new(Schema::empty())
            } else {
                Arc::new(Schema::new_with_metadata(
                    Vec::<arrow_schema::Field>::new(),
                    metadata,
                ))
            };
            return Ok((schema, Vec::new(), rows_affected));
        }

        // Redshift-only: other adapters need execute()'s schema metadata
        // (query_id, BigQuery row counts) that this path drops.
        if adapter_type == AdapterType::Redshift && !fetch {
            let rows_affected = stmt.execute_update()?;
            token.check_cancellation()?;
            return Ok((Arc::new(Schema::empty()), Vec::new(), rows_affected));
        }

        let t_exec = std::time::Instant::now();
        let reader = stmt.execute()?;
        log_step_duration(
            "stmt.execute() (submit+wait_for_completion, returns reader)",
            t_exec.elapsed(),
        );
        let t_schema = std::time::Instant::now();
        let schema = reader.schema();
        log_step_duration("reader.schema()", t_schema.elapsed());
        let mut batches = Vec::with_capacity(1);

        // Surface non-fatal backend warnings (e.g. an export-limit truncation
        // notice) the same way the `!fetch` lake compute branch above does: via schema
        // metadata, since this closure's return type has no other slot for
        // statement-level side channels. `stmt.execute()` (quack's
        // `ComputeStatement::do_execute`) already stashed them internally, so
        // this is a local option read, not a second round trip -- but it must
        // happen after `reader` (which holds `stmt` borrowed) is dropped.
        let attach_lake_compute_metadata = |stmt: &TrackedStatement, schema: Arc<Schema>| {
            if adapter_type != AdapterType::LakeCompute {
                return schema;
            }
            let warnings = stmt
                .get_option_string(OptionStatement::Other(
                    dbt_adbc::lake_compute::LAST_WARNINGS.to_string(),
                ))
                .unwrap_or_default();
            let query_id = stmt
                .get_option_string(OptionStatement::Other(
                    dbt_adbc::lake_compute::LAST_QUERY_ID.to_string(),
                ))
                .unwrap_or_default();
            if warnings.is_empty() && query_id.is_empty() {
                return schema;
            }
            let mut metadata = schema.metadata().clone();
            if !warnings.is_empty() {
                metadata.insert(
                    dbt_adbc::lake_compute::schema_metadata::WARNINGS.to_string(),
                    warnings,
                );
            }
            if !query_id.is_empty() {
                metadata.insert(
                    dbt_adbc::lake_compute::schema_metadata::QUERY_ID.to_string(),
                    query_id,
                );
            }
            Arc::new(Schema::new_with_metadata(schema.fields().clone(), metadata))
        };

        // Snowflake DML (MERGE/INSERT/UPDATE/DELETE) returns a one-row metadata batch
        // with columns like "number of rows inserted". AdapterResponse needs that batch
        // to compute rows_affected correctly, so we must drain even when fetch=false.
        if !fetch && !schema.has_dml_columns(engine.adapter_type()) {
            drop(reader);
            let schema = attach_lake_compute_metadata(&stmt, schema);
            return Ok((schema, batches, None));
        }

        // This loop has been discovered to inexplicably hang in some circumstances
        // See PR https://github.com/dbt-labs/fs/pull/7755
        let t_loop = std::time::Instant::now();
        for res in reader {
            let batch = res.map_err(adbc_core::error::Error::from)?;
            batches.push(batch);
            // Check for cancellation before processing the next batch
            // or concatenating the batches produced so far.
            token.check_cancellation()?;
        }
        log_step_duration("batch-consume loop (for res in reader)", t_loop.elapsed());
        let schema = attach_lake_compute_metadata(&stmt, schema);
        Ok((schema, batches, None))
    };
    let _span = span!("SqlEngine::execute");

    let sql_hash = code_hash(sql.as_ref());
    let t_span_create = std::time::Instant::now();
    let query_span_guard = create_debug_span(QueryExecuted::start(
        sql.to_string(),
        sql_hash,
        adapter_type.as_ref().to_owned(),
        ctx.node_id().cloned(),
        ctx.desc().cloned(),
    ))
    .entered();
    log_step_duration("create_debug_span(...).entered()", t_span_create.elapsed());

    let t_do_execute = std::time::Instant::now();
    let (schema, batches, rows_affected) = match do_execute(conn) {
        Ok(res) => res,
        Err(err @ (Cancellable::Cancelled | Cancellable::Error(_))) => {
            let cancelled = || {
                AdapterError::new(
                    AdapterErrorKind::Cancelled,
                    "SQL statement execution was cancelled",
                )
            };
            let (adapter_error, error_message, vendor_code) = match err {
                Cancellable::Cancelled => (cancelled(), None, None),
                // Statements that were running while cancellation was triggered
                // fail with an error here. But that error is a consequence of a
                // forced cancellation, so we check the `CancellationToken` and
                // treat the error as a cancellation and that makes the terminal
                // output much better for users. Nothing went wrong with the SQL
                // execution, it was just killed because the user asked for
                // cancellation.
                Cancellable::Error(_) if token.is_cancelled() => (cancelled(), None, None),
                Cancellable::Error(e) => {
                    let error_message = Some(format!("{:?}: {}", e.status, e.message));
                    let vendor_code = Some(e.vendor_code);
                    let adapter_error = adbc_error_to_adapter_error(e);
                    (adapter_error, error_message, vendor_code)
                }
            };
            let outcome = if adapter_error.kind() == AdapterErrorKind::Cancelled {
                QueryOutcome::Canceled
            } else {
                QueryOutcome::Error
            };
            record_current_span_status_from_attrs(move |attrs| {
                if let Some(attrs) = attrs.downcast_mut::<QueryExecuted>() {
                    attrs.dbt_core_event_code = "E017".to_string();
                    attrs.set_query_outcome(outcome);
                    attrs.query_error_adapter_message = error_message.clone();
                    attrs.query_error_vendor_code = vendor_code;
                }
            });
            return Err(adapter_error);
        }
    };
    log_step_duration(
        "do_execute(conn) (closure: stmt.execute + schema + batch loop)",
        t_do_execute.elapsed(),
    );
    let t_post = std::time::Instant::now();
    let schema = match rows_affected {
        Some(rows) => {
            let mut metadata = schema.metadata().clone();
            metadata.insert(ROWS_AFFECTED_META.to_string(), rows.to_string());
            Arc::new(Schema::new_with_metadata(schema.fields().clone(), metadata))
        }
        None => schema,
    };
    let total_batch = concat_batches_widened(schema, batches)?;
    let total_batch = normalize_result_column_names(adapter_type, sql.as_ref(), total_batch);
    log_step_duration(
        "concat_batches + normalize_result_column_names",
        t_post.elapsed(),
    );

    let t_status = std::time::Instant::now();
    record_current_span_status_from_attrs(|attrs| {
        if let Some(attrs) = attrs.downcast_mut::<QueryExecuted>() {
            attrs.dbt_core_event_code = "E017".to_string();
            attrs.set_query_outcome(QueryOutcome::Success);
            attrs.query_id = query_id_from_record_batch(&total_batch, adapter_type);
        }
    });
    log_step_duration("record_current_span_status_from_attrs", t_status.elapsed());

    let t_guard_drop = std::time::Instant::now();
    drop(query_span_guard);
    log_step_duration(
        "drop(query_span_guard) (span exit/export)",
        t_guard_drop.elapsed(),
    );

    Ok(total_batch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_array::{ArrayRef, StringArray};
    use arrow_schema::{DataType, Field};
    use minijinja::{Environment, Value};

    fn uppercase_constraint_batch() -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![
                Field::new("COLUMN_NAME", DataType::Utf8, false),
                Field::new("CONSTRAINT_NAME", DataType::Utf8, false),
                Field::new("RELY", DataType::Utf8, false),
            ])),
            vec![
                Arc::new(StringArray::from(vec!["id", "account_id"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["pk_orders", "pk_orders"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["Y", "Y"])) as ArrayRef,
            ],
        )
        .unwrap()
    }

    fn single_column_batch(column_name: &str) -> RecordBatch {
        RecordBatch::try_new(
            Arc::new(Schema::new(vec![Field::new(
                column_name,
                DataType::Utf8,
                false,
            )])),
            vec![Arc::new(StringArray::from(vec!["value"])) as ArrayRef],
        )
        .unwrap()
    }

    #[test]
    fn normalize_result_column_names_supports_dbt_constraints_access_pattern() {
        let batch = normalize_result_column_names(
            AdapterType::Snowflake,
            "/* dbt query comment */ SHOW UNIQUE KEYS IN TABLE orders",
            uppercase_constraint_batch(),
        );
        assert_eq!(
            batch
                .schema()
                .fields()
                .iter()
                .map(|field| field.name().as_str())
                .collect::<Vec<_>>(),
            vec!["column_name", "constraint_name", "rely"]
        );

        let table = dbt_agate::AgateTable::from_record_batch(Arc::new(batch)).into_value();
        let columns = table.get_attr("columns").unwrap();
        let column = columns.get_item(&Value::from("column_name")).unwrap();
        let env = Environment::new();
        let state = env.empty_state();
        let values = column.call_method(&state, "values", &[], &[]).unwrap();
        assert_eq!(values.len(), Some(2));
        assert_eq!(values.get_item_by_index(0).unwrap(), Value::from("id"));
        assert_eq!(
            values.get_item_by_index(1).unwrap(),
            Value::from("account_id")
        );

        let row = table
            .get_attr("rows")
            .unwrap()
            .get_item_by_index(0)
            .unwrap();
        assert_eq!(
            row.get_item(&Value::from("constraint_name")).unwrap(),
            Value::from("pk_orders")
        );
        assert_eq!(
            row.get_item(&Value::from("column_name")).unwrap(),
            Value::from("id")
        );
        assert_eq!(
            row.get_item(&Value::from("rely")).unwrap(),
            Value::from("Y")
        );
    }

    #[test]
    fn normalize_result_column_names_handles_snowflake_describe() {
        for sql in [
            "DESCRIBE TABLE orders",
            "desc table orders",
            "-- comment\nDESC TABLE orders",
        ] {
            let batch = normalize_result_column_names(
                AdapterType::Snowflake,
                sql,
                uppercase_constraint_batch(),
            );
            assert_eq!(batch.schema().field(0).name(), "column_name");
        }
    }

    #[test]
    fn normalize_result_column_names_preserves_snowflake_select() {
        let batch = normalize_result_column_names(
            AdapterType::Snowflake,
            "SELECT 1 AS a",
            uppercase_constraint_batch(),
        );
        assert_eq!(batch.schema().field(0).name(), "COLUMN_NAME");
    }

    #[test]
    fn normalize_result_column_names_uses_terminal_snowflake_flow_statement() {
        for (sql, column_name) in [
            (
                r#"SHOW TABLES ->> SELECT "name" AS TABLE_NAME FROM $1"#,
                "TABLE_NAME",
            ),
            (
                r#"SHOW TABLES ->> /* result query */ SELECT "name" AS "MixedCase" FROM $1"#,
                "MixedCase",
            ),
        ] {
            let batch = normalize_result_column_names(
                AdapterType::Snowflake,
                sql,
                single_column_batch(column_name),
            );
            assert_eq!(batch.schema().field(0).name(), column_name);
        }

        let batch = normalize_result_column_names(
            AdapterType::Snowflake,
            "SELECT 1 ->> SHOW TABLES",
            single_column_batch("NAME"),
        );
        assert_eq!(batch.schema().field(0).name(), "name");
    }

    #[test]
    fn normalize_result_column_names_ignores_flow_operators_in_snowflake_literals() {
        for sql in [
            "SHOW TABLES LIKE '->>'",
            "SHOW TABLES LIKE $$->>$$",
            "SHOW /* ->> SELECT 1 */ TABLES",
        ] {
            let batch = normalize_result_column_names(
                AdapterType::Snowflake,
                sql,
                single_column_batch("NAME"),
            );
            assert_eq!(batch.schema().field(0).name(), "name");
        }
    }

    #[test]
    fn normalize_result_column_names_preserves_other_adapters() {
        let batch = normalize_result_column_names(
            AdapterType::Bigquery,
            "SHOW PRIMARY KEYS IN TABLE orders",
            uppercase_constraint_batch(),
        );
        assert_eq!(batch.schema().field(0).name(), "COLUMN_NAME");
    }
}
