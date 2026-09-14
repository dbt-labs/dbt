use std::time::Duration;

use dbt_adapter_core::AdapterType;
use dbt_adbc::Connection;
use dbt_adbc::duration::parse_duration;
use dbt_auth::AdapterConfig;

#[derive(Debug)]
pub(crate) enum BackoffStrategy {
    /// Quadratic backoff: `attempt * attempt seconds`.
    ///
    /// Matches dbt-snowflake (Python)'s `exponential_backoff(attempt) = attempt * attempt`
    /// (the Python name is misleading; the formula is quadratic).
    Quadratic,
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
        }
    }
}

/// Policy for retrying a connection-establishment failure.
#[derive(Debug)]
pub(crate) struct ConnectionRetryPolicy {
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
        mut connect_fn: impl FnMut() -> adbc_core::error::Result<Box<dyn Connection>>,
    ) -> adbc_core::error::Result<Box<dyn Connection>> {
        let mut attempt: u32 = 0;
        loop {
            match connect_fn() {
                Ok(conn) => return Ok(conn),
                Err(err) => {
                    if attempt >= self.max_retries || !self.is_retryable(config, &err) {
                        return Err(err);
                    }
                    // XXX: consider printing the error as a warning before hanging on the user
                    let delay = match self.retry_sleep {
                        Some(t) => t,
                        None => self.backoff.delay_before_next_attempt(attempt + 1),
                    };
                    if !delay.is_zero() {
                        std::thread::sleep(delay);
                    }
                    attempt += 1;
                }
            }
        }
    }

    /// Per-adapter default for the `connect_retries` profile field.
    ///
    /// Snowflake intentionally diverges from `dbt-snowflake`'s upstream
    /// default of `1`: with `LOGIN_TIMEOUT=60s` in `dbt-auth`, each outer
    /// attempt has up to 60s for gosnowflake to do its own internal HTTP
    /// retries (capped at `MaxRetryCount=7` with 1s/128s exponential backoff
    /// — see `snowflakedb/gosnowflake::retry.go`). 7 outer retries gives 8
    /// total outer attempts × ≤60s ≈ Python's 8-HTTP-attempt total budget
    /// (`DEFAULT_AUTH_CLASS_TIMEOUT × (MAX_CON_RETRY_ATTEMPTS+1) ×
    /// dbt-adapter connect_retries+1 = 120 × 2 × 2 = 480s`).
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
        match self.adapter_type {
            Snowflake => is_retryable_snowflake_login_error(config, err),
            Databricks => is_retryable_databricks_error(config, err),
            Redshift => is_retryable_redshift_login_error(config, err),
            // Other adapters don't have a retryable-error criteria implemented here yet.
            _ => false,
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

fn matches_go_net_transient(msg_lowercase: &str) -> bool {
    GO_NET_TRANSIENT_PATTERNS
        .iter()
        .any(|p| msg_lowercase.contains(p))
}

/// Mirrors the Python dbt-snowflake retryable exception list:
///
/// ```text
/// InternalError, InternalServerError, ServiceUnavailableError,
/// GatewayTimeoutError, RequestTimeoutError, BadGatewayError,
/// OtherHTTPRetryableError, BindUploadError
/// ```
///
/// We don't have those exception types directly in ADBC; the same failures
/// surface as `Status::IO` (network-level) or `Status::Internal` with the
/// gosnowflake error string preserved. We match on both the status and
/// well-known substrings of the underlying Go error.
///
/// - `retry_all`: catch every error class (Python's `retry_all=True` →
///   `retryable_exceptions = [Error]`).
/// - `retry_on_database_errors`: broaden to internal/unknown statuses
///   (Python's `retry_on_database_errors=True` → adds `DatabaseError`).
fn is_retryable_snowflake_login_error(
    config: &AdapterConfig,
    err: &adbc_core::error::Error,
) -> bool {
    use adbc_core::error::Status;

    let retry_all = config.get_bool("retry_all").unwrap_or(false);
    if retry_all {
        return true;
    }

    // Known-permanent: do not retry real auth failures.
    if matches!(err.status, Status::Unauthenticated | Status::Unauthorized) {
        return false;
    }

    let msg = err.message.to_lowercase();
    if matches_go_net_transient(&msg) {
        return true;
    }
    // Snowflake-specific patterns on top of the shared Go transport set.
    const SNOWFLAKE_TRANSIENT_PATTERNS: &[&str] = &[
        // gosnowflake HTTP client (net/http Client.Timeout fired)
        "client.timeout exceeded while awaiting headers",
        // Snowflake / HTTP gateway responses
        "internal server error",
        "bad gateway",
        "service unavailable",
        "gateway timeout",
        "request timeout",
        // gosnowflake stage-file upload transient
        "bindupload",
    ];
    if SNOWFLAKE_TRANSIENT_PATTERNS.iter().any(|p| msg.contains(p)) {
        return true;
    }

    // Network-level failures.
    if matches!(err.status, Status::IO) {
        return true;
    }

    // `retry_on_database_errors=true` broadens to internal/unknown statuses,
    // mirroring Python's inclusion of `DatabaseError` in the retryable list.
    let retry_on_database_errors = config.get_bool("retry_on_database_errors").unwrap_or(false);
    if retry_on_database_errors && matches!(err.status, Status::Internal | Status::Unknown) {
        return true;
    }

    false
}

/// Mirrors dbt-databricks (Python) `retry_all` and the databricks-sql-go driver's
/// classification of transient HTTP failures.
///
/// Transient errors are wrapped as `databricks: retryableError: ...
/// https://github.com/databricks/databricks-sql-go/blob/47829480ca775b0e16c43969414fdf908c0a7894/internal/errors/err.go#L225-L226
///
/// - `retry_all`: retry any error (Python's `retryable_exceptions = [Error]`).
fn is_retryable_databricks_error(config: &AdapterConfig, err: &adbc_core::error::Error) -> bool {
    if config.get_bool("retry_all").unwrap_or(false) {
        return true;
    }

    err.message.to_lowercase().contains("retryableerror")
}

/// Mirrors the Python dbt-redshift retryable exception list:
///
/// Uses **message-substring matching** rather than ADBC status codes. Statuses are
/// unreliable on this path because the amazon ADBC driver
/// deliberately collapses categories:
/// - `errToAdbcErr(adbc.StatusInternal, err, "connect to Redshift")`
///   (`shared_connection.go:439,494`) is the single sink for every pgx failure —
///   dial timeouts, DNS `no such host`, `tls error`, AND server-side
///   `FATAL: password authentication failed` all surface as `Status::Internal`.
/// - `Status::Unauthenticated` is used for both permanent OAuth misconfig (`Failed to
///   get access token`) AND transient file-lock contention on the token cache
///   (`failed to acquire lease`, `shared_connection.go:417`). A status-based predicate
///   can't tell them apart.
///
/// Message matching mirrors how Python's `_handle_execute_exception`
/// https://github.com/dbt-labs/dbt-adapters/blob/802d40f0f0eb663c17e5be08d1e3fb2e55b9ee34/dbt-redshift/src/dbt/adapters/redshift/connections.py#L670-L671
/// classifies retryable errors — check substrings of the driver's error text against a
/// known-transient allowlist.
fn is_retryable_redshift_login_error(
    config: &AdapterConfig,
    err: &adbc_core::error::Error,
) -> bool {
    if config.get_bool("retry_all").unwrap_or(false) {
        return true;
    }

    let msg = err.message.to_lowercase();
    if matches_go_net_transient(&msg) {
        return true;
    }
    // Redshift-specific patterns on top of the shared Go transport set. Cross-reference
    // against `amazon-redshift-python-driver` (`redshift_connector/core.py`)
    // https://github.com/dbt-labs/dbt-adapters/blob/802d40f0f0eb663c17e5be08d1e3fb2e55b9ee34/dbt-redshift/src/dbt/adapters/redshift/connections.py#L668-L669
    const REDSHIFT_TRANSIENT_PATTERNS: &[&str] = &[
        // SQL:2003 Class 08 (Connection Exception).
        // When Redshift returns one of these, pgx renders it as `... (SQLSTATE 08XXX)`.
        // v1 catches all of them via `ProgrammingError` → `DatabaseError` (part of the default retryable set).
        // Search for "Class 08" in the reference: https://www.postgresql.org/docs/8.0/errcodes-appendix.html
        "sqlstate 0800",
        // Amazon driver: transient file-lock contention on the token cache
        // https://github.com/adbc-drivers/amazon/blob/bc8fd6d014cc0ffe7436ec8930ddde081a0029e9/go/shared_connection.go#L418-L419
        "failed to acquire lease",
        // AWS specifics (DescribeClusters, GetCredentials, GetWorkgroup)
        "throttlingexception",
        "serviceunavailable",
        "internalservererror",
    ];
    REDSHIFT_TRANSIENT_PATTERNS.iter().any(|p| msg.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;
    use adbc_core::error::{Error as AdbcError, Status};
    use std::time::Instant;

    fn adbc_err(status: Status, msg: &str) -> AdbcError {
        AdbcError::with_message_and_status(msg, status)
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
        // Snowflake intentionally diverges from upstream Python's 1: with
        // LOGIN_TIMEOUT=60s in dbt-auth + gosnowflake's internal retries,
        // 8 outer attempts × 60s ≈ Python's 8-HTTP-attempt total budget.
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

    // -- Snowflake retryable-error classifier ---------------------------------

    fn cfg_default() -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::Mapping::new())
    }

    fn cfg_retry_all() -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::Mapping::from_iter([(
            "retry_all".into(),
            true.into(),
        )]))
    }

    fn cfg_retry_on_database_errors() -> AdapterConfig {
        AdapterConfig::new(dbt_yaml::Mapping::from_iter([(
            "retry_on_database_errors".into(),
            true.into(),
        )]))
    }

    #[test]
    fn snowflake_matches_gosnowflake_client_timeout() {
        let e = adbc_err(
            Status::Internal,
            "Post \"https://acct.snowflakecomputing.com:443/session/v1/login-request\": \
             context deadline exceeded (Client.Timeout exceeded while awaiting headers)",
        );
        assert!(is_retryable_snowflake_login_error(&cfg_default(), &e));
    }

    #[test]
    fn snowflake_matches_io_status() {
        let e = adbc_err(Status::IO, "dial tcp 1.2.3.4:443: i/o timeout");
        assert!(is_retryable_snowflake_login_error(&cfg_default(), &e));
    }

    #[test]
    fn snowflake_matches_http_5xx_substring() {
        let cfg = cfg_default();
        for substr in [
            "Internal Server Error",
            "Bad Gateway",
            "Service Unavailable",
            "Gateway Timeout",
            "Request Timeout",
        ] {
            let e = adbc_err(Status::Internal, &format!("response: 5xx {substr}"));
            assert!(
                is_retryable_snowflake_login_error(&cfg, &e),
                "expected retry for substring {substr:?}"
            );
        }
    }

    #[test]
    fn snowflake_skips_real_auth_failures_by_default() {
        let e = adbc_err(
            Status::Unauthenticated,
            "Snowflake authentication failed: bad password",
        );
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        // retry_all matches Python: catches every Error class, including auth.
        assert!(is_retryable_snowflake_login_error(&cfg_retry_all(), &e));
    }

    #[test]
    fn snowflake_respects_retry_on_database_errors_flag() {
        let e = adbc_err(Status::Unknown, "some snowflake DB-level failure");
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        assert!(is_retryable_snowflake_login_error(
            &cfg_retry_on_database_errors(),
            &e
        ));
    }

    #[test]
    fn snowflake_skips_truly_unrelated_failure() {
        let e = adbc_err(Status::InvalidArguments, "bad config option");
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        assert!(!is_retryable_snowflake_login_error(
            &cfg_retry_on_database_errors(),
            &e
        ));
    }

    // -- Databricks retryable-error classifier ---------------------------------

    #[test]
    fn databricks_matches_retryable_error_marker() {
        let e = adbc_err(
            Status::Internal,
            "databricks: request error: get operation status request error: \
             request error after 1 attempt(s): databricks: retryableError: \
             unexpected HTTP status 503 Service Unavailable",
        );
        assert!(is_retryable_databricks_error(&cfg_default(), &e));
    }

    #[test]
    fn databricks_skips_non_retryable_errors() {
        let e = adbc_err(
            Status::Internal,
            "databricks: execution error: Table or view not found",
        );
        assert!(!is_retryable_databricks_error(&cfg_default(), &e));
    }

    #[test]
    fn databricks_respects_retry_all_flag() {
        let e = adbc_err(Status::InvalidArguments, "bad config option");
        assert!(!is_retryable_databricks_error(&cfg_default(), &e));
        assert!(is_retryable_databricks_error(&cfg_retry_all(), &e));
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
                is_retryable_snowflake_login_error(&cfg_default(), &e),
                "snowflake should retry {msg:?}"
            );
            assert!(
                is_retryable_redshift_login_error(&cfg_default(), &e),
                "redshift should retry {msg:?}"
            );
        }
    }

    // -- Redshift retryable-error classifier ----------------------------------

    #[test]
    fn redshift_matches_pgx_dial_via_shared_go_net_set() {
        // pgx dial failures land as `Status::Internal` via the amazon driver's
        // `errToAdbcErr(adbc.StatusInternal, ..., "connect to Redshift")` sink
        // (`shared_connection.go:439`). The Redshift predicate has no pgx-wrapper
        // patterns of its own — it relies entirely on the shared Go-transport set
        // catching the inner substring (`i/o timeout`, `connection refused`,
        // `no such host`, …).
        //
        // Known gap: on macOS the OS-level TCP `ETIMEDOUT` stringifies as
        // `dial tcp X: connect: operation timed out`, which contains none of the
        // shared substrings and therefore does NOT retry. Linux emits
        // `dial tcp X: i/o timeout` for the same failure and does retry.
        // `retry_all: true` is the escape hatch for macOS users on the bad path.
        for msg in [
            "lookup my-cluster.redshift.amazonaws.com: no such host",
            "Post http://x/: context deadline exceeded",
        ] {
            let e = adbc_err(Status::Internal, msg);
            assert!(
                is_retryable_redshift_login_error(&cfg_default(), &e),
                "expected retry for msg {msg:?}"
            );
        }
    }

    #[test]
    fn redshift_matches_sqlstate_class_08_connection_exception() {
        for msg in [
            // 08000 connection_exception
            "server terminated (SQLSTATE 08000)",
            // 08003 connection_does_not_exist
            "connection does not exist (SQLSTATE 08003)",
            // 08006 connection_failure
            "FATAL: connection reset by administrator (SQLSTATE 08006)",
            // 08007 transaction_resolution_unknown
            "transaction resolution unknown (SQLSTATE 08007)",
            // Case-insensitive: pgx renders `SQLSTATE` but nothing prevents an
            // intermediate wrapper from re-casing along the way.
            "server terminated (SqlState 08006)",
        ] {
            let e = adbc_err(Status::Internal, msg);
            assert!(
                is_retryable_redshift_login_error(&cfg_default(), &e),
                "expected retry for msg {msg:?}"
            );
        }
    }

    #[test]
    fn redshift_skips_sqlstate_08p01_protocol_violation() {
        let e = adbc_err(Status::Internal, "protocol violation (SQLSTATE 08P01)");
        assert!(!is_retryable_redshift_login_error(&cfg_default(), &e));
    }

    #[test]
    fn redshift_matches_transient_lease_contention() {
        let e = adbc_err(
            Status::Unauthenticated,
            "[redshift] failed to acquire lease: timeout waiting for lock on /home/x/.dbt/leases/redshift-token",
        );
        assert!(is_retryable_redshift_login_error(&cfg_default(), &e));
    }

    #[test]
    fn redshift_matches_aws_control_plane_transients() {
        for msg in [
            "ThrottlingException: Your request rate is too high",
            "ServiceUnavailable: The service is temporarily unavailable",
        ] {
            let e = adbc_err(Status::Internal, msg);
            assert!(
                is_retryable_redshift_login_error(&cfg_default(), &e),
                "expected retry for msg {msg:?}"
            );
        }
    }

    #[test]
    fn redshift_skips_class_28_auth_failure_by_default() {
        // SQL Class 28 (invalid_authorization_specification / invalid_password) is
        // permanent. Auth failures surface as `Status::Internal` via the amazon
        // driver's "connect to Redshift" sink, but no transient substring matches
        // — so this correctly does NOT retry.
        //
        // Stricter than Python (which classifies this as `OperationalError` and
        // retries it via the exception-class set). `retry_all: true` is the escape
        // hatch for users who want the Python-style behavior.
        let e = adbc_err(
            Status::Internal,
            "[redshift] Could not connect to Redshift: FATAL: password authentication \
             failed for user \"root\" (SQLSTATE 28P01)",
        );
        assert!(!is_retryable_redshift_login_error(&cfg_default(), &e));
        assert!(is_retryable_redshift_login_error(&cfg_retry_all(), &e));
    }

    #[test]
    fn redshift_skips_permanent_oauth_config_errors() {
        // OAuth misconfig via IdC (`Failed to get access token`,
        // `shared_connection.go:469`) is permanent — doesn't match any transient
        // pattern.
        let e = adbc_err(
            Status::Unauthenticated,
            "[redshift] Failed to get access token: invalid_client",
        );
        assert!(!is_retryable_redshift_login_error(&cfg_default(), &e));
    }

    #[test]
    fn redshift_skips_config_errors() {
        // Config / URI-parse errors from `database.go` (many `Status::InvalidArgument`
        // sites) are permanent — no transient substring matches.
        for msg in [
            "[redshift] Unknown URI parameter: 'foo'",
            "[redshift] Invalid redshift.connect_timeout: time: missing unit",
            "[redshift] No cluster 'my-cluster' found",
            "[redshift] URI is missing host",
        ] {
            let e = adbc_err(Status::InvalidArguments, msg);
            assert!(
                !is_retryable_redshift_login_error(&cfg_default(), &e),
                "expected NO retry for msg {msg:?}"
            );
        }
    }
}
