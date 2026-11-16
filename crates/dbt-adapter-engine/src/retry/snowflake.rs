use std::borrow::Cow;

use dbt_auth::AdapterConfig;

use crate::cow::lowercase;

/// The Snowflake error number, from the ADBC vendor code when the driver sets
/// it, otherwise from the leading `NNNNNN (SQLSTATE):`
fn snowflake_error_number(err: &adbc_core::error::Error) -> Option<i32> {
    if err.vendor_code > 0 {
        return Some(err.vendor_code);
    }
    let msg = err.message.trim_start().trim_start_matches("[Snowflake] ");
    let (number, rest) = msg.split_once(' ')?;
    if number.len() == 6 && rest.starts_with('(') {
        number.parse().ok()
    } else {
        None
    }
}

/// A login or SSO endpoint answered with a non-200 status, which the message
/// carries as `HTTP: <status>`. These four gosnowflake codes are contiguous:
/// `ErrFailedToAuth`, `ErrFailedToAuthSAML`, `ErrFailedToAuthOKTA` and
/// `ErrFailedToGetSSO`.
///
/// https://github.com/dbt-labs/gosnowflake/blob/v1.17.11/errors.go#L161-L168
const SNOWFLAKE_ERR_FAILED_TO_AUTH: i32 = 261004;
const SNOWFLAKE_ERR_FAILED_TO_GET_SSO: i32 = 261007;

/// Snowflake answered the login request and refused it: bad credentials, SSO
/// not configured on the account, and the rest of the `390xxx` family.
const SNOWFLAKE_LOGIN_REFUSED_FIRST: i32 = 390000;
const SNOWFLAKE_LOGIN_REFUSED_LAST: i32 = 390999;

fn is_retryable_http_status(status: u16) -> bool {
    const REQUEST_TIMEOUT: u16 = 408;
    const TOO_MANY_REQUESTS: u16 = 429;
    const SERVER_ERROR_FIRST: u16 = 500;
    const SERVER_ERROR_LAST: u16 = 599;
    matches!(
        status,
        REQUEST_TIMEOUT | TOO_MANY_REQUESTS | SERVER_ERROR_FIRST..=SERVER_ERROR_LAST
    )
}

/// The status in gosnowflake's `... HTTP: <status>, URL: ...` login errors.
fn http_status_in_message(msg: &str) -> Option<u16> {
    let tail = msg.split_once("HTTP: ")?.1;
    let end = tail
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(tail.len());
    tail[..end].parse().ok()
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
pub(super) fn is_retryable_snowflake_login_error(
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

    let msg = lowercase(Cow::Borrowed(&err.message));
    if super::matches_go_net_transient(&msg) {
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

    // The driver reports every connect failure as `Status::IO`, so the Snowflake
    // error number is the only thing separating permanent from transient below.
    match snowflake_error_number(err) {
        Some(SNOWFLAKE_ERR_FAILED_TO_AUTH..=SNOWFLAKE_ERR_FAILED_TO_GET_SSO) => {
            return http_status_in_message(&err.message).is_some_and(is_retryable_http_status);
        }
        Some(SNOWFLAKE_LOGIN_REFUSED_FIRST..=SNOWFLAKE_LOGIN_REFUSED_LAST) => {
            return config.get_bool("retry_on_database_errors").unwrap_or(false);
        }
        _ => {}
    }

    // gosnowflake blocks external-browser sign-in for 60s after a failure, so a retry
    // inside that window always fails, and one past it opens another browser tab.
    // This error carries no Snowflake number, so the message is the only handle.
    //
    // https://github.com/dbt-labs/gosnowflake/blob/v1.17.11/auth.go#L760-L995
    if msg.contains("external browser sign-in failed recently") {
        return false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry::tests::adbc_err;
    use crate::retry::tests::cfg_default;
    use crate::retry::tests::cfg_retry_all;

    use adbc_core::error::Status;

    // -- Snowflake retryable-error classifier ---------------------------------

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

    #[test]
    fn snowflake_skips_saml_404() {
        let e = adbc_err(
            Status::IO,
            "[Snowflake] 261005 (08004): failed to auth via SAML for unknown reason. \
             HTTP: 404, URL: https://acct.snowflakecomputing.com:443/session/authenticator-request?requestId=x",
        );
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        assert!(is_retryable_snowflake_login_error(&cfg_retry_all(), &e));
    }

    #[test]
    fn snowflake_skips_login_404() {
        let e = adbc_err(
            Status::IO,
            "261004 (08004): failed to auth for unknown reason. \
             HTTP: 404, URL: https://acct.snowflakecomputing.com:443/session/v1/login-request",
        );
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
    }

    #[test]
    fn snowflake_skips_get_sso_4xx() {
        let forbidden = adbc_err(
            Status::IO,
            "261007 (08004): failed to auth via OKTA for unknown reason. HTTP: 403, URL: https://x",
        );
        assert!(!is_retryable_snowflake_login_error(
            &cfg_default(),
            &forbidden
        ));

        let unavailable = adbc_err(
            Status::IO,
            "261007 (08004): failed to auth via OKTA for unknown reason. HTTP: 503, URL: https://x",
        );
        assert!(is_retryable_snowflake_login_error(
            &cfg_default(),
            &unavailable
        ));
    }

    #[test]
    fn snowflake_retries_auth_5xx_and_throttling() {
        for status in [408, 429, 500, 503] {
            let e = adbc_err(
                Status::IO,
                &format!(
                    "261004 (08004): failed to auth for unknown reason. HTTP: {status}, URL: https://x"
                ),
            );
            assert!(
                is_retryable_snowflake_login_error(&cfg_default(), &e),
                "expected retry for HTTP {status}"
            );
        }
    }

    #[test]
    fn snowflake_login_rejection_follows_retry_on_database_errors() {
        let e = adbc_err(
            Status::IO,
            "390190 (08004): There was an error related to the SAML Identity Provider account parameter.",
        );
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        assert!(is_retryable_snowflake_login_error(
            &cfg_retry_on_database_errors(),
            &e
        ));
    }

    #[test]
    fn snowflake_skips_external_browser_backoff() {
        let e = adbc_err(
            Status::IO,
            "External browser sign-in failed recently due to an unrecoverable \
             authentication failure (e.g., IP restriction, IDP error)",
        );
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
        assert!(is_retryable_snowflake_login_error(&cfg_retry_all(), &e));
    }

    #[test]
    fn snowflake_error_number_prefers_vendor_code() {
        let mut e = adbc_err(Status::IO, "connection failed with no numeric prefix");
        e.vendor_code = 390100;
        assert_eq!(snowflake_error_number(&e), Some(390100));
        assert!(!is_retryable_snowflake_login_error(&cfg_default(), &e));
    }

    #[test]
    fn snowflake_still_retries_uncoded_failures() {
        for msg in [
            "timeout after 5s and 3 attempts. HTTP Status: 503. Hanging?",
            "dial tcp 1.2.3.4:443: connect: connection refused",
        ] {
            let e = adbc_err(Status::IO, msg);
            assert!(
                is_retryable_snowflake_login_error(&cfg_default(), &e),
                "expected retry for {msg:?}"
            );
        }
    }
}
