use std::pin::Pin;
use std::sync::Arc;

use adbc_core::options::OptionStatement;
use arrow::array::RecordBatch;
use arrow_schema::{Schema, SchemaRef};
use datafusion_expr::LogicalPlan;
use dbt_adapter_core::AdapterType;
use dbt_adbc::Connection;
use dbt_common::hashing::code_hash;
use dbt_common::tracing::span_info::record_current_span_status_from_attrs;
use dbt_common::{ErrorCode, FsError, FsResult, create_debug_span, err, fs_err};
use dbt_df_providers::delayed_table::is_schema_compat;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_scheduler::instructions::Instruction;
use dbt_schemas::schemas::telemetry::{QueryExecuted, QueryOutcome};
use dbt_tasks_core::AdhocRunner;
use dbt_tasks_core::task::TaskOp;

/// Runs queries remotely against the warehouse via an adapter connection.
pub struct RemoteAdhocRunner {
    pub env: Arc<JinjaEnv>,
    pub adapter_type: AdapterType,
    /// `show --query-id <id>`: when set, `run_adhoc` ignores `instruction`/
    /// `rendered_sql` entirely and fetches this already-completed LakeCompute
    /// query's result directly instead. See `EvalArgs::query_id`.
    pub query_id: Option<String>,
}

impl AdhocRunner for RemoteAdhocRunner {
    fn run_adhoc<'a>(
        self: Arc<Self>,
        instruction: &'a Instruction,
        rendered_sql: &'a str,
        _unique_id: Option<&'a str>,
        connection: &'a mut Option<Box<dyn Connection>>,
    ) -> Pin<Box<dyn Future<Output = FsResult<(Vec<RecordBatch>, SchemaRef)>> + Send + 'a>> {
        Box::pin(async move {
            if let Some(query_id) = &self.query_id {
                return fetch_query_result_with_connection(query_id, &self.env, connection).await;
            }
            run_remote_adhoc_with_connection(
                instruction,
                rendered_sql,
                &self.env,
                self.adapter_type,
                connection,
            )
            .await
        })
    }
}

/// Reuse `conn_box`'s connection if present, otherwise open and cache one
/// from the workspace's base adapter engine. Shared by the ordinary
/// `--inline` path and the `--query-id` result-fetch path below.
///
/// Must run on a `dbt_runtime` worker: opening a connection blocks, and
/// `AdbcEngine::new_connection` asserts the caller is a pool worker.
fn get_or_open_connection<'a>(
    env: &JinjaEnv,
    conn_box: &'a mut Option<Box<dyn Connection>>,
) -> FsResult<&'a mut dyn Connection> {
    if conn_box.is_some() {
        return Ok(conn_box.as_mut().unwrap().as_mut());
    }
    let adapter_engine = env.get_base_adapter().map(|a| Arc::clone(a.engine()));
    let Some(engine) = adapter_engine else {
        return err!(
            ErrorCode::RemoteError,
            "No adapter engine configured in workspace"
        );
    };
    let conn = engine.new_connection(None, None)?;
    conn_box.replace(conn);
    Ok(conn_box.as_mut().unwrap().as_mut())
}

/// `show --query-id <id>`: fetch an already-completed LakeCompute query's result
/// directly, by setting `RESULT_QUERY_ID` on a fresh statement instead of a SQL
/// query. No submission, no worker/Temporal round trip -- the driver turns
/// this into a `Client::status()` lookup plus the same object-store fetch a
/// normal export uses (see `adbc_driver_dbt::statement::ComputeStatement::
/// fetch_existing_result`).
async fn fetch_query_result_with_connection(
    query_id: &str,
    env: &Arc<JinjaEnv>,
    conn_box: &mut Option<Box<dyn Connection>>,
) -> FsResult<(Vec<RecordBatch>, SchemaRef)> {
    let env = Arc::clone(env);
    let query_id = query_id.to_owned();
    let mut conn = conn_box.take();
    let (conn, result) = TaskOp::Blocking(Box::new(move || {
        let result = fetch_query_result_blocking(&query_id, &env, &mut conn);
        (conn, result)
    }))
    .run()
    .await?;
    *conn_box = conn;
    result
}

/// The blocking half of [`fetch_query_result_with_connection`], run on a worker.
fn fetch_query_result_blocking(
    query_id: &str,
    env: &JinjaEnv,
    conn_box: &mut Option<Box<dyn Connection>>,
) -> FsResult<(Vec<RecordBatch>, SchemaRef)> {
    let conn = get_or_open_connection(env, conn_box)?;
    let mut stmt = conn.new_statement().map_err(from_adbc_error)?;
    stmt.set_option(
        OptionStatement::Other(dbt_adbc::lake_compute::RESULT_QUERY_ID.to_string()),
        adbc_core::options::OptionValue::String(query_id.to_string()),
    )
    .map_err(from_adbc_error)?;

    let mut reader = stmt.execute().map_err(from_adbc_error)?;
    let schema = reader.schema();
    let records: Vec<RecordBatch> = reader.by_ref().collect::<Result<_, _>>()?;
    drop(reader);
    warn_if_last_warnings(stmt.as_ref());
    let schema = attach_last_query_id(stmt.as_ref(), schema);

    Ok((records, schema))
}

async fn run_remote_adhoc_with_connection(
    instruction: &Instruction,
    rendered_sql: &str,
    env: &Arc<JinjaEnv>,
    adapter_type: AdapterType,
    conn_box: &mut Option<Box<dyn Connection>>,
) -> FsResult<(Vec<RecordBatch>, SchemaRef)> {
    if let Some(result) = replay_run_remote_adhoc_result() {
        return result;
    }

    // Reading the expected schema off the instruction needs no connection, so
    // it stays here and only what the query itself needs moves to the worker.
    let expected_schema = expected_schema_of(instruction);
    let fqn = instruction.fqn().join(".");
    let env = Arc::clone(env);
    let rendered_sql = rendered_sql.to_owned();
    let mut conn = conn_box.take();

    // Opening the connection and executing the query both block, so they run
    // on a `dbt_runtime` worker instead of on this async thread. The connection
    // travels with the closure and comes back with the result, so a subsequent
    // adhoc query on the same `conn_box` still reuses it.
    let (conn, result) = TaskOp::Blocking(Box::new(move || {
        let result = run_remote_adhoc_blocking(
            &rendered_sql,
            &env,
            adapter_type,
            expected_schema.as_ref(),
            &fqn,
            &mut conn,
        );
        (conn, result)
    }))
    .run()
    .await?;
    *conn_box = conn;
    result
}

/// The schema an `Lp` instruction's plan promises, as an Arrow schema so the
/// blocking half needs no datafusion types.
fn expected_schema_of(instruction: &Instruction) -> Option<SchemaRef> {
    match instruction {
        Instruction::Sql(_) => None,
        Instruction::Lp(lp_instruction) => match &lp_instruction.plan {
            LogicalPlan::Ddl(..)
            | LogicalPlan::Dml(..)
            | LogicalPlan::Copy(..)
            | LogicalPlan::Repartition(..)
            | LogicalPlan::Statement(..)
            | LogicalPlan::Explain(..)
            | LogicalPlan::Analyze(..)
            | LogicalPlan::DescribeTable(..) => None,
            _ => Some(Arc::new(lp_instruction.plan.schema().as_arrow().clone())),
        },
    }
}

/// The blocking half of [`run_remote_adhoc_with_connection`], run on a worker.
fn run_remote_adhoc_blocking(
    rendered_sql: &str,
    env: &JinjaEnv,
    adapter_type: AdapterType,
    expected_schema: Option<&SchemaRef>,
    fqn: &str,
    conn_box: &mut Option<Box<dyn Connection>>,
) -> FsResult<(Vec<RecordBatch>, SchemaRef)> {
    let conn = get_or_open_connection(env, conn_box)?;
    let mut stmt = conn.new_statement().map_err(from_adbc_error)?;
    stmt.set_sql_query(rendered_sql).map_err(from_adbc_error)?;

    let (schema, mut reader) = {
        let sql_hash = code_hash(rendered_sql);
        let _query_span_guard = create_debug_span(QueryExecuted::start(
            rendered_sql.to_string(),
            sql_hash,
            adapter_type.as_ref().to_owned(),
            None,
            Some("dbt run query".to_string()),
        ))
        .entered();

        let reader = match stmt.execute() {
            Ok(r) => r,
            Err(e) => {
                record_current_span_status_from_attrs(|attrs| {
                    if let Some(attrs) = attrs.downcast_mut::<QueryExecuted>() {
                        attrs.dbt_core_event_code = "E017".to_string();
                        attrs.set_query_outcome(QueryOutcome::Error);
                        attrs.query_error_adapter_message = Some(format!("{:?}", e));
                    }
                });

                if let Some(recorder) = dbt_adapter::time_machine::global_recorder() {
                    recorder.record_run_remote_adhoc(
                        rendered_sql,
                        &[],
                        &Arc::new(Schema::empty()),
                        false,
                        Some(format!("{:?}", e)),
                    );
                }

                return Err(from_adbc_error(e));
            }
        };

        let schema = reader.schema();

        record_current_span_status_from_attrs(|attrs| {
            if let Some(attrs) = attrs.downcast_mut::<QueryExecuted>() {
                attrs.dbt_core_event_code = "E017".to_string();
                attrs.set_query_outcome(QueryOutcome::Success);
            }
        });

        (schema, reader)
    };

    if let Some(expected_schema) = expected_schema
        && !is_schema_compat(schema.as_ref(), expected_schema.as_ref())
    {
        return err!(
            ErrorCode::RemoteError,
            "Detected schema mismatch for {}: \
                this is likely because your local workspace has changes that are not \
                yet reflected in the remote database, \
                you may need to (re)build the workspace",
            fqn
        );
    }

    let records: Vec<RecordBatch> = reader.by_ref().collect::<Result<_, _>>()?;

    // `reader` holds `stmt` borrowed; drop it before reading the statement's
    // `LAST_WARNINGS` option below, mirroring the sequencing
    // `adapter_engine.rs`'s `adbc_execute_with_options` already relies on for
    // the same option (the driver may only populate it once the reader is
    // fully drained).
    drop(reader);
    warn_if_last_warnings(stmt.as_ref());
    let schema = attach_last_query_id(stmt.as_ref(), schema);

    if let Some(recorder) = dbt_adapter::time_machine::global_recorder() {
        recorder.record_run_remote_adhoc(rendered_sql, &records, &schema, true, None);
    }

    Ok((records, schema))
}

fn replay_run_remote_adhoc_result() -> Option<FsResult<(Vec<RecordBatch>, SchemaRef)>> {
    let replayer = dbt_adapter::time_machine::global_replayer()?;

    Some(match replayer.get_run_remote_adhoc_result() {
        Some(result) => result.map_err(|e| fs_err!(ErrorCode::RemoteError, "{}", e)),
        None => err!(
            ErrorCode::RemoteError,
            "Missing recorded run_remote_adhoc event during replay; refusing to execute a live adhoc query"
        ),
    })
}

fn from_adbc_error(err: adbc_core::error::Error) -> Box<FsError> {
    fs_err!(ErrorCode::Generic, "{}", err)
}

/// Surface any non-fatal backend warning attached to the just-executed
/// statement (e.g. LakeCompute's export-limit truncation notice) via
/// `tracing::warn!`, matching `compute_platform.rs`'s
/// `warn_if_response_has_message` so warning formatting is consistent across
/// both `show --inline` and normal model execution.
///
/// Not gated to a specific adapter: only the `LakeCompute` driver ever
/// populates `LAST_WARNINGS`, but calling this unconditionally is safe --
/// every concrete `Statement` implementation in this codebase (the
/// driver-manager's generic wrapper used by every non-lake-compute backend, and the
/// record/replay wrapper) overrides `get_option_string` and returns a clean
/// `Err` for an option it doesn't recognize, never the trait default's
/// `unimplemented!()` panic. This also avoids needing to determine "which
/// adapter is this statement actually running against" here at all -- no
/// adapter-type label available at this call site reliably reflects a
/// per-command `--adapter <type>` override (see git history for the bug this
/// replaced: gating on `RemoteAdhocRunner`'s static `adapter_type` field, and
/// later on `env.get_base_adapter()`'s reported type, both disagreed with
/// the connection actually in use).
fn warn_if_last_warnings(stmt: &dyn dbt_adbc::statement::Statement) {
    let warning = stmt
        .get_option_string(OptionStatement::Other(
            dbt_adbc::lake_compute::LAST_WARNINGS.to_string(),
        ))
        .unwrap_or_default();
    if !warning.is_empty() {
        tracing::warn!("{warning}");
    }
}

/// Read the LakeCompute job id of the just-executed statement (e.g. from
/// `show --inline`) and stamp it onto `schema`'s metadata, the only way to
/// later recover this result via LakeCompute's `/download_credentials`
/// endpoint. This function's caller talks to the raw ADBC `Statement`
/// directly (unlike model materializations, which go through
/// `AdapterEngine::execute_with_options`/`adapter_engine.rs`'s
/// `attach_lake_compute_metadata`), so the id has to be attached here for
/// `showable/mod.rs::run_show` to append it to the "Query <name>" title line.
/// Also logged at debug level for troubleshooting. Unconditional for the same
/// reason as `warn_if_last_warnings` above: only the LakeCompute driver ever
/// populates `LAST_QUERY_ID`, and every other `Statement` impl returns a
/// clean `Err` for an option it doesn't recognize.
fn attach_last_query_id(stmt: &dyn dbt_adbc::statement::Statement, schema: SchemaRef) -> SchemaRef {
    let query_id = stmt
        .get_option_string(OptionStatement::Other(
            dbt_adbc::lake_compute::LAST_QUERY_ID.to_string(),
        ))
        .unwrap_or_default();
    if query_id.is_empty() {
        return schema;
    }
    tracing::debug!("show --inline completed via LakeCompute (query_id={query_id})");
    let mut metadata = schema.metadata().clone();
    metadata.insert(
        dbt_adbc::lake_compute::schema_metadata::QUERY_ID.to_string(),
        query_id,
    );
    Arc::new(Schema::new_with_metadata(schema.fields().clone(), metadata))
}

#[cfg(test)]
mod warn_if_last_warnings_tests {
    use super::*;

    struct FakeStatement {
        warning: Result<String, adbc_core::error::Error>,
    }

    impl dbt_adbc::statement::Statement for FakeStatement {
        fn bind(&mut self, _batch: RecordBatch) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn bind_stream(
            &mut self,
            _reader: Box<dyn arrow::array::RecordBatchReader + Send>,
        ) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn execute<'a>(
            &'a mut self,
        ) -> adbc_core::error::Result<Box<dyn arrow::array::RecordBatchReader + Send + 'a>>
        {
            unimplemented!()
        }
        fn execute_update(&mut self) -> adbc_core::error::Result<Option<i64>> {
            unimplemented!()
        }
        fn execute_schema(&mut self) -> adbc_core::error::Result<Schema> {
            unimplemented!()
        }
        fn execute_partitions(&mut self) -> adbc_core::error::Result<adbc_core::PartitionedResult> {
            unimplemented!()
        }
        fn get_parameter_schema(&self) -> adbc_core::error::Result<Schema> {
            unimplemented!()
        }
        fn prepare(&mut self) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn set_sql_query(&mut self, _sql: &str) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn set_substrait_plan(&mut self, _plan: &[u8]) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn cancel(&mut self) -> adbc_core::error::Result<()> {
            unimplemented!()
        }
        fn get_option_string(&self, _key: OptionStatement) -> adbc_core::error::Result<String> {
            self.warning.clone()
        }
    }

    #[test]
    fn unsupported_option_error_does_not_panic() {
        // Proves an `Err` from `get_option_string` (the real behavior of every
        // non-lake-compute backend's driver-manager wrapper for an option it doesn't
        // recognize) is handled gracefully, not propagated/panicked on.
        let stmt = FakeStatement {
            warning: Err(adbc_core::error::Error::with_message_and_status(
                "unrecognized option",
                adbc_core::error::Status::InvalidArguments,
            )),
        };
        warn_if_last_warnings(&stmt);
    }

    #[test]
    fn empty_warning_does_not_panic() {
        let stmt = FakeStatement {
            warning: Ok(String::new()),
        };
        warn_if_last_warnings(&stmt);
    }

    #[test]
    fn non_empty_warning_does_not_panic() {
        let stmt = FakeStatement {
            warning: Ok("matched 50001 rows, which exceeds the 50000-row export limit".to_string()),
        };
        warn_if_last_warnings(&stmt);
    }
}
