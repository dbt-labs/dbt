use std::time::Duration;

use dbt_adapter_core::AdapterType;
use dbt_adbc::Connection;
use dbt_adbc::duration::parse_duration;
use dbt_auth::AdapterConfig;
use dbt_common::Cancellable;
use dbt_common::tracing::dbt_emit::emit_info_log_message;

mod databricks;
mod redshift;
mod snowflake;

use crate::errors::is_bridge_adbc_error;

#[derive(Debug)]
pub enum BackoffStrategy {
    /// Quadratic backoff: `attempt * attempt seconds`.
    ///
    /// Matches dbt-snowflake (Python)'s `exponential_backoff(attempt) = attempt * attempt`
    /// (the Python name is misleading; the formula is quadratic).
    Quadratic,
    /// Doubling backoff capped at 60s: 1, 2, 4, 8, 16, 32, 60, 60, ...
    Exponential,
}

impl BackoffStrategy {
    /// Defines how long to sleep before the `attempt`-th retry (1-indexed).
    ///
    /// `attempt=1` is the first retry. May return [`Duration::ZERO`] for instant retry.
    fn delay_before_next_attempt(&self, attempt: u32) -> Duration {
        match self {
            BackoffStrategy::Quadratic => {
                let attempt = u64::from(attempt);
                Duration::from_secs(attempt.saturating_mul(attempt))
            }
            BackoffStrategy::Exponential => {
                let seconds = 2u64.saturating_pow(attempt.saturating_sub(1));
                Duration::from_secs(seconds.min(60))
            }
        }
    }
}

/// Policy for retrying a connection-establishment failure.
#[derive(Debug)]
pub struct ConnectionRetryPolicy {
    adapter_type: AdapterType,
    /// Retries on top of the initial attempt. `0` is effectively no-retry.
    pub max_retries: u32,
    /// Per-iteration sleep between retry attempts, sourced from the profile
    /// field `connect_timeout`. `None` means "use [`BackoffStrategy`]".
    /// Matches the role of `retry_timeout` in Python's
    /// `dbt-adapters/base/connections.py::retry_connection` (Snowflake and
    /// Databricks pass `connect_timeout if not None else exponential_backoff`).
    pub retry_sleep: Option<Duration>,
    /// See [`BackoffStrategy`]. Used only when `retry_sleep` is `None`.
    pub backoff: BackoffStrategy,
}

impl ConnectionRetryPolicy {
    /// Build a connection-establishment retry policy from a profile ([AdapterConfig]).
    ///
    /// No retries will happen if `max_retries` is zero or [ConnectionRetryPolicy::is_retryable]
    /// always returns `false`.
    ///
    /// Profile fields (all optional):
    /// - `connect_retries: int` — retries on top of the initial attempt
    ///   (default per [`default_connect_retries`])
    /// - `connect_timeout: int (seconds) | duration string` — per-iteration
    ///   sleep between attempts. When unset, [`BackoffStrategy::Quadratic`]
    ///   provides the delay sequence (1s, 4s, 9s, …). Matches Python's
    ///   `retry_timeout` semantics in `dbt-adapters::retry_connection`.
    /// - `retry_all: bool = false` — catch every error class
    /// - `retry_on_database_errors: bool = false` — broaden the retryable set
    ///   to include database-level / unknown errors
    pub fn new(adapter_type: AdapterType, config: &AdapterConfig) -> ConnectionRetryPolicy {
        let retries_key = match adapter_type {
            // https://github.com/dbt-labs/dbt-adapters/blob/802d40f0f0eb663c17e5be08d1e3fb2e55b9ee34/dbt-redshift/src/dbt/adapters/redshift/connections.py#L149-L150
            AdapterType::Redshift => "retries",
            _ => "connect_retries",
        };
        let max_retries = config
            .get_string(retries_key)
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or_else(|| Self::default_connect_retries(adapter_type));

        // `connect_timeout` is typed as `Option<i64>` in the profile schema
        // (see `dbt-schemas/src/schemas/profiles.rs`). Accept both bare
        // integer seconds and duration strings (e.g. "30s", "2m") for
        // symmetry with `parse_duration`. Per Python's
        // `dbt-adapters::retry_connection` semantics, this is the
        // per-iteration sleep between attempts — not a cumulative budget
        // and not a per-attempt driver deadline.
        let retry_sleep = config.get_string("connect_timeout").and_then(|v| {
            let s = v.as_ref();
            s.parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| parse_duration(s).ok())
        });

        ConnectionRetryPolicy {
            adapter_type,
            max_retries,
            retry_sleep,
            backoff: BackoffStrategy::Quadratic,
        }
    }

    /// Execute a connection attempt with retry according to the policy.
    ///
    /// `connect_fn` is called for each attempt. On retryable failures, the
    /// per-iteration sleep [`retry_sleep`](Self::retry_sleep) (or the
    /// quadratic [`backoff`](Self::backoff) when unset) is applied before
    /// the next attempt. The final error after exhausting retries is the
    /// error from the last attempt.
    ///
    /// The loop terminates when any of:
    /// - the connect succeeds,
    /// - `attempt >= self.max_retries`, or
    /// - the error is not retryable per [`is_retryable`](Self::is_retryable).
    pub fn execute(
        &self,
        config: &AdapterConfig,
        connect_fn: impl FnMut() -> adbc_core::error::Result<Box<dyn Connection>>,
    ) -> adbc_core::error::Result<Box<dyn Connection>> {
        let total_attempts = self.max_retries.saturating_add(1);
        retry_loop(
            self.max_retries,
            connect_fn,
            |err| self.is_retryable(config, err),
            |attempt| match self.retry_sleep {
                Some(t) => t,
                None => self.backoff.delay_before_next_attempt(attempt),
            },
            |attempt, delay, err| {
                // INFO, not WARN: a coded warning can be promoted to an error under
                // `--warn-error`, which would fail a run whose connection recovered.
                emit_info_log_message(format!(
                    "{} connection attempt {attempt} of {total_attempts} failed, \
                     retrying in {delay:?}: {}",
                    self.adapter_type, err.message
                ));
            },
        )
    }

    /// Per-adapter default for the `connect_retries` profile field.
    ///
    /// Snowflake intentionally diverges from dbt 1.X's `dbt-snowflake`
    /// default of `1` overall, but it is more tolerant; see `UNCAPPED_MAX_RETRY_COUNT`.
    fn default_connect_retries(adapter_type: AdapterType) -> u32 {
        use AdapterType::*;
        match adapter_type {
            Snowflake => 0,
            // XXX: expand if customization is required
            _ => 1,
        }
    }

    fn is_retryable(&self, config: &AdapterConfig, err: &adbc_core::error::Error) -> bool {
        use AdapterType::*;
        // The Bridge driver already redials once when the service is unreachable,
        // and its errors say nothing about the warehouse login, so `retry_all`
        // doesn't apply to them.
        if is_bridge_adbc_error(err) {
            return false;
        }
        match self.adapter_type {
            Snowflake => snowflake::is_retryable_snowflake_login_error(config, err),
            Databricks => databricks::is_retryable_databricks_error(config, err),
            Redshift => redshift::is_retryable_redshift_login_error(config, err),
            // Other adapters don't have a retryable-error criteria implemented here yet.
            _ => false,
        }
    }
}

/// Shared retry-loop skeleton for [`ConnectionRetryPolicy`] and [`QueryRetryPolicy`].
#[inline(always)]
fn retry_loop<T, E>(
    max_retries: u32,
    mut attempt_fn: impl FnMut() -> Result<T, E>,
    is_retryable: impl Fn(&E) -> bool,
    delay_for: impl Fn(u32) -> Duration,
    on_retry: impl Fn(u32, Duration, &E),
) -> Result<T, E> {
    let mut attempt: u32 = 0;
    loop {
        match attempt_fn() {
            Ok(v) => return Ok(v),
            Err(err) => {
                if attempt >= max_retries || !is_retryable(&err) {
                    return Err(err);
                }
                let delay = delay_for(attempt + 1);
                on_retry(attempt + 1, delay, &err);
                if !delay.is_zero() {
                    std::thread::sleep(delay);
                }
                attempt += 1;
            }
        }
    }
}

/// Message substrings for transport-layer transient failures common to every Go-based ADBC driver
const GO_NET_TRANSIENT_PATTERNS: &[&str] = &[
    // Go net dial / DNS / TCP-level
    "i/o timeout",
    "connection refused",
    "connection reset",
    "no such host",
    // Go `context` cancellation propagating up from a WithTimeout/WithDeadline
    "context deadline exceeded",
];

pub(crate) fn matches_go_net_transient(msg_lowercase: &str) -> bool {
    GO_NET_TRANSIENT_PATTERNS
        .iter()
        .any(|p| msg_lowercase.contains(p))
}

/// Retries a transient query-*execution* failure (as opposed to
/// [`ConnectionRetryPolicy`], which retries connection-open). Redshift-only.
///
/// Retries on the same connection: Fusion runs Redshift autocommit-only, so
/// there's no aborted transaction to clear, and the driver re-`Prepare`s
/// fresh on every call -- enough to recover from a stale-OID or
/// missing-relation catalog race.
#[derive(Debug, Clone)]
pub struct QueryRetryPolicy {
    adapter_type: AdapterType,
    max_retries: u32,
    retry_all: bool,
}

impl QueryRetryPolicy {
    /// Reads Redshift's actual profile field names (`retries`, not `connect_retries`).
    pub fn new(adapter_type: AdapterType, config: &AdapterConfig) -> QueryRetryPolicy {
        let max_retries = config
            .get_string("retries")
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(1);
        let retry_all = config.get_bool("retry_all").unwrap_or(false);
        QueryRetryPolicy {
            adapter_type,
            max_retries,
            retry_all,
        }
    }

    /// Cancellation is never retried.
    pub fn execute<T>(
        &self,
        mut query_fn: impl FnMut() -> Result<T, Cancellable<adbc_core::error::Error>>,
    ) -> Result<T, Cancellable<adbc_core::error::Error>> {
        if self.adapter_type != AdapterType::Redshift {
            return query_fn();
        }
        retry_loop(
            self.max_retries,
            query_fn,
            |err| match err {
                Cancellable::Cancelled => false,
                Cancellable::Error(e) => self.is_retryable(e),
            },
            |attempt| BackoffStrategy::Exponential.delay_before_next_attempt(attempt),
            |_, _, _| {},
        )
    }

    fn is_retryable(&self, err: &adbc_core::error::Error) -> bool {
        // A lost Bridge session can't be retried on the same connection, and
        // an `08R07` call may already have run.
        !is_bridge_adbc_error(err)
            && redshift::is_retryable_redshift_query_error(self.retry_all, &err.message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adbc_core::error::{Error as AdbcError, Status};
    use std::time::Instant;

    pub(crate) fn adbc_err(status: Status, msg: &str) -> AdbcError {
        AdbcError::with_message_and_status(msg, status)
    }

    pub(crate) fn cfg_default() -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::Mapping::new())
    }

    pub(crate) fn cfg_retry_all() -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::Mapping::from_iter([(
            "retry_all".into(),
            true.into(),
        )]))
    }

    fn bridge_err(status: Status, sqlstate: &[u8; 5]) -> AdbcError {
        let mut err = adbc_err(status, "[Bridge] i/o timeout");
        err.sqlstate = sqlstate.map(|b| b as _);
        err
    }

    #[test]
    fn bridge_errors_are_never_retried() {
        for adapter_type in [
            AdapterType::Snowflake,
            AdapterType::Databricks,
            AdapterType::Redshift,
        ] {
            let policy = ConnectionRetryPolicy {
                adapter_type,
                max_retries: 3,
                retry_sleep: Some(Duration::ZERO),
                backoff: BackoffStrategy::Quadratic,
            };
            for cfg in [cfg_default(), cfg_retry_all()] {
                for sqlstate in [
                    b"08R01", b"08R06", b"08R07", b"28R00", b"08R16", b"08R17", b"08R12", b"28R07",
                    b"HYR09", b"XXR05", b"08r06",
                ] {
                    let err = bridge_err(Status::IO, sqlstate);
                    assert!(
                        !policy.is_retryable(&cfg, &err),
                        "{adapter_type:?} {}",
                        String::from_utf8_lossy(sqlstate)
                    );
                }
            }
        }
        // The same message without a Bridge SQLSTATE still retries.
        assert!(test_policy(3, None).is_retryable(&cfg_retry_all(), &adbc_err(Status::IO, "x")));
    }

    #[test]
    fn query_retry_policy_skips_bridge_errors_even_with_retry_all() {
        // Retryable without `retry_all` if it weren't a Bridge error.
        let bridge_query_err = |sqlstate: &[u8; 5]| {
            let mut err = adbc_err(Status::IO, "could not open relation with oid 1");
            err.sqlstate = sqlstate.map(|b| b as _);
            err
        };
        for cfg in [cfg_default(), cfg_retry_all()] {
            let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
            assert!(policy.is_retryable(&bridge_query_err(b"00000")));
            for sqlstate in [
                b"08R06", b"08R07", b"08R16", b"08R17", b"08R12", b"28R07", b"HYR09", b"XXR05",
            ] {
                assert!(
                    !policy.is_retryable(&bridge_query_err(sqlstate)),
                    "{}",
                    String::from_utf8_lossy(sqlstate)
                );
            }
        }

        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg_retry_all());
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Error(bridge_err(
                Status::Unauthenticated,
                b"08R06",
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn quadratic_backoff_matches_python_formula() {
        use BackoffStrategy::Quadratic;
        assert_eq!(
            Quadratic.delay_before_next_attempt(0),
            Duration::from_secs(0)
        );
        assert_eq!(
            Quadratic.delay_before_next_attempt(1),
            Duration::from_secs(1)
        );
        assert_eq!(
            Quadratic.delay_before_next_attempt(2),
            Duration::from_secs(4)
        );
        assert_eq!(
            Quadratic.delay_before_next_attempt(3),
            Duration::from_secs(9)
        );
        assert_eq!(
            Quadratic.delay_before_next_attempt(10),
            Duration::from_secs(100)
        );
    }

    #[test]
    fn connect_retry_policy_default_retries_for_non_snowflake() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Bigquery, &cfg).max_retries,
            1
        );
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Databricks, &cfg).max_retries,
            1
        );
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Redshift, &cfg).max_retries,
            1
        );
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::DuckDB, &cfg).max_retries,
            1
        );
    }

    #[test]
    fn connect_retry_policy_snowflake_defaults() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.max_retries, 0);
        // Quadratic backoff: attempt=1 → 1s, attempt=2 → 4s.
        assert_eq!(
            policy.backoff.delay_before_next_attempt(1),
            Duration::from_secs(1)
        );
        assert_eq!(
            policy.backoff.delay_before_next_attempt(2),
            Duration::from_secs(4)
        );
        // Default predicate: retry on Status::IO, not on auth failures.
        assert!(policy.is_retryable(&cfg, &adbc_err(Status::IO, "dial tcp: i/o timeout")));
        assert!(!policy.is_retryable(&cfg, &adbc_err(Status::Unauthenticated, "bad creds")));
    }

    #[test]
    fn connect_retry_policy_snowflake_reads_profile_fields() {
        let mapping = dbt_yaml::Mapping::from_iter([
            ("connect_retries".into(), 3.into()),
            ("retry_all".into(), true.into()),
        ]);
        let cfg = AdapterConfig::new(mapping);
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.max_retries, 3);
        // retry_all=true matches Python's `retryable_exceptions = [Error]` →
        // catches every error class including auth.
        assert!(policy.is_retryable(&cfg, &adbc_err(Status::Unauthenticated, "bad creds")));
    }

    #[test]
    fn retry_sleep_defaults_to_none() {
        // With `connect_timeout` unset, retry_sleep is None and the loop
        // falls back to quadratic backoff for inter-attempt sleeps.
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.retry_sleep, None);
    }

    #[test]
    fn connect_timeout_parses_seconds_string() {
        let mapping = dbt_yaml::Mapping::from_iter([("connect_timeout".into(), "30s".into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.retry_sleep, Some(Duration::from_secs(30)));
    }

    #[test]
    fn connect_timeout_parses_minutes_string() {
        let mapping = dbt_yaml::Mapping::from_iter([("connect_timeout".into(), "2m".into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.retry_sleep, Some(Duration::from_secs(120)));
    }

    #[test]
    fn connect_timeout_parses_bare_integer_seconds() {
        // Profile schema types `connect_timeout` as `Option<i64>` so YAML may
        // surface it as a number; `get_string` normalizes to a digit string.
        let mapping = dbt_yaml::Mapping::from_iter([(
            "connect_timeout".into(),
            dbt_yaml::Value::number(45i64.into()),
        )]);
        let cfg = AdapterConfig::new(mapping);
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        assert_eq!(policy.retry_sleep, Some(Duration::from_secs(45)));
    }

    #[test]
    fn connect_timeout_invalid_falls_back_to_none() {
        let mapping =
            dbt_yaml::Mapping::from_iter([("connect_timeout".into(), "not_a_duration".into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg);
        // Unparseable → fall back to quadratic backoff (not the internal cap).
        assert_eq!(policy.retry_sleep, None);
    }

    // -- execute() behavior ---------------------------------------------------

    fn test_policy(max_retries: u32, retry_sleep: Option<Duration>) -> ConnectionRetryPolicy {
        ConnectionRetryPolicy {
            adapter_type: AdapterType::Snowflake,
            max_retries,
            retry_sleep,
            backoff: BackoffStrategy::Quadratic,
        }
    }

    #[test]
    fn execute_runs_initial_plus_max_retries_attempts_on_persistent_failure() {
        let policy = test_policy(3, Some(Duration::ZERO));
        let cfg = cfg_default();
        let mut calls: u32 = 0;
        let result = policy.execute(&cfg, || {
            calls += 1;
            Err(adbc_err(Status::IO, "i/o timeout"))
        });
        assert!(result.is_err());
        // 1 initial attempt + 3 retries = 4 total calls.
        assert_eq!(calls, 4);
    }

    #[test]
    fn execute_stops_immediately_on_non_retryable_error() {
        let policy = test_policy(5, Some(Duration::ZERO));
        let cfg = cfg_default();
        let mut calls: u32 = 0;
        let result = policy.execute(&cfg, || {
            calls += 1;
            Err(adbc_err(Status::Unauthenticated, "bad creds"))
        });
        assert!(result.is_err());
        // No retries because the error is permanent.
        assert_eq!(calls, 1);
    }

    #[test]
    fn execute_uses_fixed_retry_sleep_when_connect_timeout_set() {
        // Verifies that retry_sleep takes precedence over quadratic backoff:
        // the loop sleeps exactly `retry_sleep` between attempts.
        let policy = test_policy(3, Some(Duration::from_millis(20)));
        let cfg = cfg_default();
        let mut calls: u32 = 0;
        let start = Instant::now();
        let _ = policy.execute(&cfg, || {
            calls += 1;
            Err(adbc_err(Status::IO, "i/o timeout"))
        });
        let elapsed = start.elapsed();
        assert_eq!(calls, 4);
        // 3 sleeps × 20ms = 60ms minimum; generous upper bound for CI jitter.
        assert!(
            elapsed >= Duration::from_millis(60),
            "expected at least 60ms of fixed sleeps, got {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "fixed 20ms sleeps shouldn't take seconds, got {elapsed:?}"
        );
    }

    #[test]
    fn retry_loop_calls_on_retry_only_before_another_attempt() {
        use std::cell::Cell;

        let attempts = Cell::new(0u32);
        let retries = Cell::new(0u32);
        let exhausted: Result<(), &str> = retry_loop(
            2,
            || {
                attempts.set(attempts.get() + 1);
                Err("boom")
            },
            |_| true,
            |_| Duration::ZERO,
            |_, _, _| retries.set(retries.get() + 1),
        );
        assert!(exhausted.is_err());
        assert_eq!(attempts.get(), 3);
        assert_eq!(retries.get(), 2);

        let attempts = Cell::new(0u32);
        let retries = Cell::new(0u32);
        let permanent: Result<(), &str> = retry_loop(
            2,
            || {
                attempts.set(attempts.get() + 1);
                Err("boom")
            },
            |_| false,
            |_| Duration::ZERO,
            |_, _, _| retries.set(retries.get() + 1),
        );
        assert!(permanent.is_err());
        assert_eq!(attempts.get(), 1);
        assert_eq!(retries.get(), 0);

        let attempts = Cell::new(0u32);
        let reported = Cell::new(0u32);
        let recovered: Result<u8, &str> = retry_loop(
            2,
            || {
                attempts.set(attempts.get() + 1);
                if attempts.get() == 1 {
                    Err("boom")
                } else {
                    Ok(7)
                }
            },
            |_| true,
            |_| Duration::ZERO,
            |attempt, _, _| reported.set(attempt),
        );
        assert_eq!(recovered, Ok(7));
        assert_eq!(attempts.get(), 2);
        assert_eq!(reported.get(), 1);
    }

    // -- `retries` alias for `connect_retries` --------------------------------

    #[test]
    fn retries_alias_maps_to_max_retries() {
        // dbt-redshift / dbt-postgres expose the profile field as `retries`, not
        // `connect_retries`. Accept it as a fallback so dbt 1.x profiles keep working.
        let mapping = dbt_yaml::Mapping::from_iter([("retries".into(), 3.into())]);
        let cfg = AdapterConfig::new(mapping);
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Redshift, &cfg).max_retries,
            3
        );
    }

    #[test]
    fn snowflake_uses_connect_retries_and_ignores_retries() {
        // Redshift is the only adapter that reads `retries`. For every other adapter
        // (here: Snowflake), the field is `connect_retries`; a stray `retries` in the
        // profile is ignored so we don't accidentally alias two semantically-different
        // Python fields together.
        let mapping = dbt_yaml::Mapping::from_iter([
            ("connect_retries".into(), 5.into()),
            ("retries".into(), 2.into()),
        ]);
        let cfg = AdapterConfig::new(mapping);
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Snowflake, &cfg).max_retries,
            5
        );
    }

    #[test]
    fn redshift_uses_retries_and_ignores_connect_retries() {
        // Symmetric: Redshift reads only `retries`. A stray `connect_retries` in a
        // Redshift profile is a schema-level oddity and is ignored so the two adapter
        // families don't share a config surface where they shouldn't.
        let mapping = dbt_yaml::Mapping::from_iter([
            ("connect_retries".into(), 5.into()),
            ("retries".into(), 2.into()),
        ]);
        let cfg = AdapterConfig::new(mapping);
        assert_eq!(
            ConnectionRetryPolicy::new(AdapterType::Redshift, &cfg).max_retries,
            2
        );
    }

    // -- Shared Go-transport transient patterns -------------------------------

    #[test]
    fn shared_go_net_patterns_apply_to_snowflake_and_redshift() {
        // These substrings are surfaced verbatim by every Go-based ADBC driver we
        // ship, so both classifiers must retry on them without repeating the list.
        for msg in [
            "lookup foo: no such host",
            "Post http://x/: context deadline exceeded",
        ] {
            let e = adbc_err(Status::Internal, msg);
            assert!(
                snowflake::is_retryable_snowflake_login_error(&cfg_default(), &e),
                "snowflake should retry {msg:?}"
            );
            assert!(
                redshift::is_retryable_redshift_login_error(&cfg_default(), &e),
                "redshift should retry {msg:?}"
            );
        }
    }

    // -- Exponential backoff --------------------------------------------

    #[test]
    fn exponential_backoff_matches_python_recurrence() {
        use BackoffStrategy::Exponential;
        // Python: backoff starts at 1, then `min(max(backoff * 2, 2), 60)` each retry:
        // 1, 2, 4, 8, 16, 32, 64->60, 60, ...
        assert_eq!(
            Exponential.delay_before_next_attempt(1),
            Duration::from_secs(1)
        );
        assert_eq!(
            Exponential.delay_before_next_attempt(2),
            Duration::from_secs(2)
        );
        assert_eq!(
            Exponential.delay_before_next_attempt(3),
            Duration::from_secs(4)
        );
        assert_eq!(
            Exponential.delay_before_next_attempt(6),
            Duration::from_secs(32)
        );
        assert_eq!(
            Exponential.delay_before_next_attempt(7),
            Duration::from_secs(60)
        );
        assert_eq!(
            Exponential.delay_before_next_attempt(20),
            Duration::from_secs(60)
        );
    }

    // -- QueryRetryPolicy ------------------------------------------------------

    #[test]
    fn query_retry_policy_default_retries_is_one() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        assert_eq!(
            QueryRetryPolicy::new(AdapterType::Redshift, &cfg).max_retries,
            1
        );
    }

    #[test]
    fn query_retry_policy_reads_retries_field() {
        let mapping = dbt_yaml::Mapping::from_iter([("retries".into(), 3.into())]);
        let cfg = AdapterConfig::new(mapping);
        assert_eq!(
            QueryRetryPolicy::new(AdapterType::Redshift, &cfg).max_retries,
            3
        );
    }

    #[test]
    fn query_retry_policy_retries_on_oid_race_then_succeeds() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            if calls < 2 {
                Err(Cancellable::Error(adbc_err(
                    Status::Internal,
                    "ERROR: could not open relation with OID 12345",
                )))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn query_retry_policy_retries_on_concurrent_transaction_conflict() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            if calls < 2 {
                Err(Cancellable::Error(adbc_err(
                    Status::Internal,
                    "ERROR: conflict with concurrent transaction",
                )))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn query_retry_policy_retries_on_missing_schema_race() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            if calls < 2 {
                Err(Cancellable::Error(adbc_err(
                    Status::Internal,
                    "ERROR: schema \"analytics_scratch\" does not exist",
                )))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn query_retry_policy_does_not_retry_unrelated_errors() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Error(adbc_err(
                Status::InvalidArguments,
                "syntax error at or near \"selct\"",
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn query_retry_policy_respects_retry_all_flag() {
        let mapping = dbt_yaml::Mapping::from_iter([("retry_all".into(), true.into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            if calls < 2 {
                Err(Cancellable::Error(adbc_err(
                    Status::InvalidArguments,
                    "some otherwise-unretryable error",
                )))
            } else {
                Ok(calls)
            }
        });
        assert_eq!(result.unwrap(), 2);
    }

    #[test]
    fn query_retry_policy_exhausts_retries_and_returns_last_error() {
        let mapping = dbt_yaml::Mapping::from_iter([("retries".into(), 2.into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Error(adbc_err(
                Status::Internal,
                "ERROR: conflict with concurrent transaction",
            )))
        });
        assert!(result.is_err());
        // 1 initial attempt + 2 retries = 3 total calls.
        assert_eq!(calls, 3);
    }

    #[test]
    fn query_retry_policy_never_retries_cancellation() {
        let cfg = AdapterConfig::new(dbt_yaml::Mapping::new());
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Cancelled)
        });
        assert!(matches!(result, Err(Cancellable::Cancelled)));
        assert_eq!(calls, 1);
    }

    #[test]
    fn query_retry_policy_is_a_no_op_for_non_redshift_adapters() {
        let mapping = dbt_yaml::Mapping::from_iter([("retries".into(), 3.into())]);
        let cfg = AdapterConfig::new(mapping);
        let policy = QueryRetryPolicy::new(AdapterType::Snowflake, &cfg);
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Error(adbc_err(
                Status::Internal,
                "ERROR: could not open relation with OID 12345",
            )))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    /// How a Bridge failure in a result stream reaches the retry policy.
    fn bridge_stream_err() -> AdbcError {
        AdbcError::from(arrow_schema::ArrowError::CDataInterface(
            "[Bridge] the result stream ended early; it may have run".to_string(),
        ))
    }

    #[test]
    fn query_retry_policy_skips_bridge_stream_errors_with_retry_all() {
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg_retry_all());
        let mut calls = 0;
        let result: Result<i32, Cancellable<AdbcError>> = policy.execute(|| {
            calls += 1;
            Err(Cancellable::Error(bridge_stream_err()))
        });
        assert!(result.is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn bridge_errors_without_sqlstate_are_never_retried() {
        let deadline = || {
            adbc_err(
                Status::Timeout,
                "[Bridge] handshake did not finish before the deadline",
            )
        };
        for adapter_type in [
            AdapterType::Snowflake,
            AdapterType::Databricks,
            AdapterType::Redshift,
        ] {
            let policy = ConnectionRetryPolicy {
                adapter_type,
                max_retries: 3,
                retry_sleep: Some(Duration::ZERO),
                backoff: BackoffStrategy::Quadratic,
            };
            for cfg in [cfg_default(), cfg_retry_all()] {
                assert!(!policy.is_retryable(&cfg, &deadline()), "{adapter_type:?}");
            }
        }
        let policy = QueryRetryPolicy::new(AdapterType::Redshift, &cfg_retry_all());
        assert!(!policy.is_retryable(&deadline()));
    }
}
