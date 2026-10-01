//! Live Snowflake regression for Ctrl+C-style cancellation of an in-flight
//! ADBC statement.
//!
//! This intentionally uses `TrackedStatement` and
//! `cancel_all_tracked_statements`, matching the adapter path used when a
//! dbt invocation receives Ctrl+C. This covers client cancellation while
//! `CALL SYSTEM$WAIT(...)` is being polled by the Snowflake driver.
//!
//! Run:
//! ```sh
//! caffeinate -dimsu cargo xtask test --llm --no-external-deps -p dbt-adapter \
//!   snowflake_client_cancel_does_not_panic_adbc -- --ignored
//! ```

use std::time::{Duration, Instant};

use adbc_core::error::{Error, Result};
use adbc_core::options::AdbcVersion;
use dbt_adbc::{
    Backend, Database, Statement, connection,
    database::{self, LogLevel},
    driver, snowflake,
};
use dbt_tracked_stmt::{TrackedStatement, cancel_all_tracked_statements};

const ADBC_VERSION: AdbcVersion = AdbcVersion::V110;
const WAIT_SECONDS: u64 = 300;
const DEFAULT_CANCEL_AFTER_SECS: u64 = 75;

fn open_database() -> Result<Box<dyn Database>> {
    let mut driver = driver::Builder::new(Backend::Snowflake, driver::LoadStrategy::CdnCache)
        .with_adbc_version(ADBC_VERSION)
        .try_load()?;

    let mut builder = database::Builder::from_snowsql_config()?;
    builder.with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::DEFAULT)?;
    builder.with_named_option(snowflake::LOG_TRACING, LogLevel::Warn.to_string())?;
    builder.with_named_option(snowflake::LOGIN_TIMEOUT, "60s")?;
    builder.with_named_option(snowflake::REQUEST_TIMEOUT, "600s")?;
    // Keep client HTTP timeout generous so any cancellation in the test window
    // is driven by Statement::cancel(), not gosnowflake's per-request timer.
    builder.with_named_option(snowflake::AUTH_CLIENT_TIMEOUT, "600s")?;

    builder.build(&mut driver)
}

fn drain(reader: Box<dyn arrow_array::RecordBatchReader + Send + '_>) -> Result<()> {
    for batch in reader {
        batch.map_err(Error::from)?;
    }
    Ok(())
}

#[ignore = "live Snowflake; ~75s wallclock; opt-in via -- --ignored"]
#[test]
fn snowflake_client_cancel_does_not_panic_adbc() {
    let cancel_after = std::env::var("SNOWFLAKE_CTRL_C_AFTER_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CANCEL_AFTER_SECS);

    let mut database = open_database().expect("open database");
    let mut conn = connection::Builder::default()
        .build(&mut database)
        .expect("open connection");

    let mut stmt = TrackedStatement::new(conn.new_statement().expect("new statement"));
    stmt.set_sql_query(&format!("CALL SYSTEM$WAIT({WAIT_SECONDS})"))
        .expect("set sql");

    let execute_started = Instant::now();
    let execute_handle = std::thread::spawn(move || {
        let result = stmt.execute().and_then(drain);
        (execute_started.elapsed(), result)
    });

    std::thread::sleep(Duration::from_secs(cancel_after));
    let cancel_report = cancel_all_tracked_statements(0);
    let (execute_elapsed, execute_result) = execute_handle.join().expect("join execute thread");

    eprintln!(
        "[cancel]  statements={} failures={}",
        cancel_report.stmt_count, cancel_report.fail_count
    );
    eprintln!("[execute] elapsed={execute_elapsed:?} result={execute_result:?}");

    assert!(
        cancel_report.stmt_count >= 1,
        "no tracked Snowflake statement was available to cancel"
    );
    assert_eq!(
        cancel_report.fail_count, 0,
        "tracked statement cancellation failed"
    );

    let err = match execute_result {
        Ok(()) => panic!(
            "execute() unexpectedly succeeded after {execute_elapsed:?}; \
             tracked statement cancellation did not interrupt SYSTEM$WAIT"
        ),
        Err(e) => e,
    };

    let msg = err.message.to_lowercase();

    for panic_marker in [
        "go panic in snowflake driver",
        "go panicked",
        "driver is in unknown state",
        "invalid memory address",
        "nil pointer dereference",
    ] {
        assert!(
            !msg.contains(panic_marker),
            "Snowflake driver panic marker '{panic_marker}' found in error: {}",
            err.message
        );
    }
    assert!(
        execute_elapsed < Duration::from_secs(cancel_after + 60),
        "execute did not exit within 60s of cancel (elapsed={execute_elapsed:?}); \
         the cancel path may be waiting out SYSTEM$WAIT. err={}",
        err.message
    );
    assert!(
        ["cancel", "interrupt", "aborted", "statement_canceled"]
            .iter()
            .any(|marker| msg.contains(marker)),
        "expected a cancellation error, got: {}",
        err.message
    );
}
