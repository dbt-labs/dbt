use dbt_adbc::str_from_sqlstate;
use dbt_auth::AdapterConfig;

/// Checks if a needle occurs in haystack surrounded by word boundaries (not preceded
/// or followed by ASCII alphanumeric characters or underscore). This avoids false
/// positives on identifiers such as `table_2003_v2` or `col_1040`.
fn contains_isolated_token(haystack: &str, token: &str) -> bool {
    let bytes = haystack.as_bytes();
    let token_bytes = token.as_bytes();
    let mut start = 0;
    while let Some(pos) = haystack[start..].find(token) {
        let abs_pos = start + pos;
        let end_pos = abs_pos + token_bytes.len();
        let before_ok = abs_pos == 0
            || (!bytes[abs_pos - 1].is_ascii_alphanumeric() && bytes[abs_pos - 1] != b'_');
        let after_ok = end_pos == bytes.len()
            || (!bytes[end_pos].is_ascii_alphanumeric() && bytes[end_pos] != b'_');
        if before_ok && after_ok {
            return true;
        }
        start = abs_pos + 1;
    }
    false
}

/// SingleStore retryable error predicate.
///
/// Mirrors Python `dbt-singlestore`:
/// `retryable_exceptions = [singlestoredb.OperationalError, singlestoredb.DatabaseError]`
///
/// SingleStore ADBC driver runs over Go `go-singlestore-driver`. Network, dial, and server
/// connectivity issues surface with Go net errors or SingleStore error codes.
///
/// Priority:
/// 1. ADBC driver vendor_code (direct MySQL/SingleStore error number)
/// 2. SQLSTATE
/// 3. Fallback string matching with word boundaries on error message
pub(super) fn is_retryable_singlestore_login_error(
    config: &AdapterConfig,
    err: &adbc_core::error::Error,
) -> bool {
    use adbc_core::error::Status;

    if config.get_bool("retry_all").unwrap_or(false) {
        return true;
    }

    if matches!(err.status, Status::Unauthenticated | Status::Unauthorized) {
        return false;
    }

    // 1. Check driver vendor code when available.
    // 1045: ER_ACCESS_DENIED_ERROR (permanent auth failure)
    if err.vendor_code == 1045 {
        return false;
    }
    // Retryable vendor codes:
    // 2003: CR_CONN_HOST_ERROR (Can't connect to MySQL server)
    // 2002: CR_CONNECTION_ERROR (Can't connect to local server)
    // 2006: CR_SERVER_GONE_ERROR (MySQL server has gone away)
    // 2013: CR_SERVER_LOST (Lost connection to MySQL server)
    // 1040: ER_CON_COUNT_ERROR (Too many connections)
    // 1205: ER_LOCK_WAIT_TIMEOUT (Lock wait timeout exceeded)
    // 1213: ER_LOCK_DEADLOCK (Deadlock found when trying to get lock)
    const RETRYABLE_VENDOR_CODES: &[i32] = &[1040, 1205, 1213, 2002, 2003, 2006, 2013];
    if RETRYABLE_VENDOR_CODES.contains(&err.vendor_code) {
        return true;
    }

    // 2. Check SQLSTATE when available.
    let sqlstate = str_from_sqlstate(&err.sqlstate);
    if sqlstate == "28000" {
        return false;
    }
    if sqlstate.starts_with("08") || sqlstate == "40001" {
        return true;
    }

    // 3. Fallback: string matching on error message.
    let msg = err.message.to_lowercase();
    if msg.contains("access denied") || contains_isolated_token(&msg, "1045") {
        return false;
    }

    if super::matches_go_net_transient(&msg) {
        return true;
    }

    if config.get_bool("retry_on_database_errors").unwrap_or(false) {
        return true;
    }

    const SINGLESTORE_TRANSIENT_PHRASES: &[&str] = &[
        "server has gone away",
        "lost connection",
        "too many connections",
        "lock wait timeout",
        "deadlock",
    ];
    if SINGLESTORE_TRANSIENT_PHRASES
        .iter()
        .any(|p| msg.contains(p))
    {
        return true;
    }

    const FALLBACK_CODE_TOKENS: &[&str] = &["1040", "1205", "1213", "2002", "2003", "2006", "2013"];
    FALLBACK_CODE_TOKENS
        .iter()
        .any(|code| contains_isolated_token(&msg, code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry::tests::adbc_err;
    use crate::retry::tests::cfg_default;
    use crate::retry::tests::cfg_retry_all;

    use adbc_core::error::{Error as AdbcError, Status};

    fn adbc_err_with_vendor(status: Status, msg: &str, vendor_code: i32) -> AdbcError {
        let mut err = AdbcError::with_message_and_status(msg, status);
        err.vendor_code = vendor_code;
        err
    }

    fn adbc_err_with_sqlstate(status: Status, msg: &str, sqlstate_bytes: &[u8; 5]) -> AdbcError {
        let mut err = AdbcError::with_message_and_status(msg, status);
        err.sqlstate = sqlstate_bytes.map(|b| b as std::os::raw::c_char);
        err
    }

    #[test]
    fn singlestore_error_classification() {
        let cfg = cfg_default();

        // 1. ADBC driver vendor_code matching
        let e = adbc_err_with_vendor(Status::Internal, "generic error", 2003);
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err_with_vendor(Status::Internal, "generic error", 1205);
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err_with_vendor(Status::Internal, "generic error", 1045);
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        // 2. SQLSTATE matching
        let e = adbc_err_with_sqlstate(Status::Internal, "generic connection error", b"08001");
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err_with_sqlstate(Status::Internal, "serialization failure", b"40001");
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err_with_sqlstate(Status::Internal, "bad auth", b"28000");
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        // 3. Transient Go net errors
        let e = adbc_err(
            Status::IO,
            "dial tcp 127.0.0.1:3306: connect: connection refused",
        );
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        // 4. Fallback: string matching with word boundaries
        let e = adbc_err(
            Status::Internal,
            "Error 2003 (HY000): Can't connect to MySQL server on 'localhost:3306'",
        );
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err(
            Status::Internal,
            "Error 2006 (HY000): MySQL server has gone away",
        );
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err(
            Status::Internal,
            "Error 1205 (HY000): Lock wait timeout exceeded",
        );
        assert!(is_retryable_singlestore_login_error(&cfg, &e));

        // Negative test: table or column identifiers containing digits must NOT trigger retry
        let e = adbc_err(
            Status::Internal,
            "Table 'dbt_ss.table_2003_v2' does not exist",
        );
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err(
            Status::Internal,
            "Unknown column 'col_1040' in 'field list'",
        );
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err(
            Status::Internal,
            "Unknown column 'id_1205_ref' in 'where clause'",
        );
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        // 5. Permanent auth failures: Error 1045 or Unauthenticated status
        let e = adbc_err(
            Status::Unauthenticated,
            "Error 1045 (28000): Access denied for user 'foo'",
        );
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        let e = adbc_err(Status::Internal, "Access denied for user 'foo'@'localhost'");
        assert!(!is_retryable_singlestore_login_error(&cfg, &e));

        // 6. retry_all overrides permanent errors
        assert!(is_retryable_singlestore_login_error(&cfg_retry_all(), &e));
    }
}
