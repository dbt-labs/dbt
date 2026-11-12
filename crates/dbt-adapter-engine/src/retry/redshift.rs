use std::borrow::Cow;

use dbt_auth::AdapterConfig;

use crate::cow::lowercase;

/// Substring heuristics for known-transient Redshift errors, not proof that
/// every match is actually transient. `retry_all` widens to any error.
pub(super) fn is_retryable_redshift_query_error(retry_all: bool, message: &str) -> bool {
    if retry_all {
        return true;
    }

    let msg = lowercase(Cow::Borrowed(message));
    // Catalog race after a concurrent DROP/CREATE.
    msg.contains("could not open relation with oid")
        // MVCC serialization conflict.
        || msg.contains("conflict with concurrent transaction")
        // Schema catalog race (can also be a genuine permanent error).
        || (msg.contains("schema") && msg.contains("does not exist"))
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
pub(super) fn is_retryable_redshift_login_error(
    config: &AdapterConfig,
    err: &adbc_core::error::Error,
) -> bool {
    if config.get_bool("retry_all").unwrap_or(false) {
        return true;
    }

    let msg = err.message.to_lowercase();
    if super::matches_go_net_transient(&msg) {
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
    use crate::retry::tests::adbc_err;
    use crate::retry::tests::cfg_default;
    use crate::retry::tests::cfg_retry_all;

    use adbc_core::error::Status;

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
