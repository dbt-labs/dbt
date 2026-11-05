use std::sync::Arc;
use std::thread;
use std::time::Duration;

use arrow::array::RecordBatch;
use minijinja::State;

use dbt_adbc::*;
use dbt_common::AdapterResult;
use dbt_common::cancellation::CancellationToken;

pub use dbt_adapter_engine::noop_connection::NoopConnection;
pub use dbt_adapter_engine::query_comment;
pub use dbt_adapter_engine::retry;
pub use dbt_adapter_engine::sidecar_client::ColumnInfo;
pub use dbt_adapter_engine::sidecar_client::SidecarClient;

mod adapter_engine;
pub use adapter_engine::AdapterEngine;
pub use adapter_engine::Options;

mod adbc;
pub use adbc::AdbcEngine;
pub(crate) use adbc::resolve_connection_config;

mod record_replay;
pub use record_replay::RecordReplayEngine;

mod sidecar;
pub use sidecar::SidecarEngine;

/// Execute query and retry in case of an error. Retry is done (up to
/// the given limit) regardless of the error encountered.
///
/// https://github.com/dbt-labs/dbt-adapters/blob/996a302fa9107369eb30d733dadfaf307023f33d/dbt-adapters/src/dbt/adapters/sql/connections.py#L84
#[allow(clippy::too_many_arguments)]
pub fn execute_query_with_retry(
    engine: Arc<dyn AdapterEngine>,
    state: Option<&State>,
    conn: &'_ mut dyn Connection,
    ctx: &QueryCtx,
    sql: &str,
    retry_limit: u32,
    options: &Options,
    fetch: bool,
    token: CancellationToken,
) -> AdapterResult<RecordBatch> {
    let mut attempt = 0;
    let mut last_error = None;

    while attempt < retry_limit {
        match engine.execute_with_options(
            state,
            ctx,
            conn,
            sql,
            options.clone(),
            fetch,
            token.clone(),
        ) {
            Ok(result) => return Ok(result),
            Err(err) => {
                last_error = Some(err.clone());
                thread::sleep(Duration::from_secs(1));
                attempt += 1;
            }
        }
    }

    if let Some(err) = last_error {
        Err(err)
    } else {
        unreachable!("last_error should not be None if we exit the loop")
    }
}
