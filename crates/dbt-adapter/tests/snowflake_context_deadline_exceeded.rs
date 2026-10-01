//! Deterministic local repro for the "context deadline exceeded (Client.Timeout
//! exceeded while awaiting headers)"
//!
//! Strategy: stand up a slow TCP listener on 127.0.0.1 that accepts the
//! connection, optionally reads bytes, but never responds with HTTP headers
//! within the test's window. Point the Snowflake ADBC driver (and through it
//! gosnowflake) at it via `host`/`port`/`protocol=http`. Observe:
//!
//!   1. What the gosnowflake driver returns when `LOGIN_TIMEOUT="1s"` (current
//!      fs production setting).
//!   2. How long the failed call took.
//!   3. Whether wrapping the same call in our `ConnectionRetryPolicy` actually
//!      changes the wall-clock and the final error string the user observes.
//!
//! Use `-- --ignored --nocapture` to inspect the prints. No external network or
//! credentials required.
//!
//! Run:
//! ```sh
//! cargo xtask test --llm --no-external-deps -p dbt-adapter \
//!   snowflake_login_deadline -- --ignored --nocapture
//! ```
//!
//! What this test is for: pinning down which clock is actually firing (gosnowflake's
//! retry budget, gosnowflake's per-request `Client.Timeout`, or something
//! upstream) so any subsequent timeout change is grounded in measurement, not
//! guesswork.

use std::io::Read;
use std::net::TcpListener;
use std::thread;

use adbc_core::options::AdbcVersion;
use dbt_adbc::{
    Backend, Database, connection,
    database::{self, LogLevel},
    driver, snowflake,
};

const ADBC_VERSION: AdbcVersion = AdbcVersion::V110;

/// Bind a localhost TCP listener that accepts connections, drains their bytes,
/// and then holds them open forever (or until the test exits). Returns the
/// bound port plus a connection counter that the test can read.
///
/// Key invariant: the socket is never closed from this side, so the HTTP
/// client on the other end sees no EOF — only the absence of headers. That
/// matches "Snowflake auth endpoint is slow/unresponsive", which is the
/// real-world condition behind the user-reported error.
fn spawn_blackhole_listener() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1:0");
    let port = listener.local_addr().expect("local_addr").port();
    thread::spawn(move || {
        for incoming in listener.incoming() {
            match incoming {
                Ok(mut stream) => {
                    thread::spawn(move || {
                        // Drain whatever the client writes so its write side
                        // doesn't fail with EPIPE. Read with no timeout — when
                        // the client gives up and FINs, read returns 0 and
                        // we exit. Crucially we never write back, and we never
                        // close from our side.
                        let mut buf = [0u8; 4096];
                        loop {
                            match stream.read(&mut buf) {
                                Ok(0) | Err(_) => break,
                                Ok(_) => continue,
                            }
                        }
                    });
                }
                Err(_) => break,
            }
        }
    });
    port
}

/// Build a Snowflake [`Database`] aimed at a local black-hole server.
///
/// No real credentials are needed — the goal is to trigger
/// `context deadline exceeded (Client.Timeout exceeded while awaiting headers)`.
///
/// - `login_timeout` sets ADBC `login_timeout` → gosnowflake `Config.LoginTimeout`,
///   which becomes `retryHTTP.totalTimeout` for auth (the login retry budget, not
///   per-request wall clock; see gosnowflake `retry.go`).
/// - `AUTH_CLIENT_TIMEOUT` is fixed at `1ms` so each `http.Client.Do` fails fast instead
///   of blocking on gosnowflake's 900s default.
fn build_stub_database_with(port: u16, login_timeout: &str) -> Box<dyn Database> {
    let mut driver = driver::Builder::new(Backend::Snowflake, driver::LoadStrategy::CdnCache)
        .with_adbc_version(ADBC_VERSION)
        .try_load()
        .expect("load Snowflake ADBC driver");

    let mut builder = database::Builder::new(Backend::Snowflake);
    builder
        .with_named_option(snowflake::ACCOUNT, "stub-account")
        .unwrap()
        .with_named_option(snowflake::HOST, "127.0.0.1")
        .unwrap()
        .with_named_option(snowflake::PORT, port.to_string())
        .unwrap()
        .with_named_option(snowflake::PROTOCOL, "http")
        .unwrap()
        .with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::DEFAULT)
        .unwrap()
        .with_username("stub-user")
        .with_password("stub-password")
        .with_named_option(snowflake::LOGIN_TIMEOUT, login_timeout)
        .unwrap()
        .with_named_option(snowflake::LOG_TRACING, LogLevel::Warn.to_string())
        .unwrap()
        // always set a shorter per http request timeout to speed up retrying
        .with_named_option(snowflake::AUTH_CLIENT_TIMEOUT, "1ms")
        .unwrap();

    builder
        .build(&mut driver)
        .expect("build database (config-only)")
}

#[test]
fn snowflake_context_deadline_exceeded_login_timeout_1s() {
    let port = spawn_blackhole_listener();
    let mut database = build_stub_database_with(port, "1s");

    let result = connection::Builder::default().build(&mut database);
    match result {
        Ok(_) => panic!("unexpected success against black-hole server"),
        Err(err) => {
            let msg = err.message;
            assert!(
                msg.contains(
                    "context deadline exceeded (Client.Timeout exceeded while awaiting headers)"
                ),
                "expected error message to contain 'context deadline exceeded (Client.Timeout exceeded while awaiting headers)', but got: {}",
                msg
            );
        }
    }
}
