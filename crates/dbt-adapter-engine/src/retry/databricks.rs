use std::borrow::Cow;

use dbt_auth::AdapterConfig;

use crate::cow::lowercase;

/// Mirrors dbt-databricks (Python) `retry_all` and the databricks-sql-go driver's
/// classification of transient HTTP failures.
///
/// Transient errors are wrapped as `databricks: retryableError: ...
/// https://github.com/databricks/databricks-sql-go/blob/47829480ca775b0e16c43969414fdf908c0a7894/internal/errors/err.go#L225-L226
///
/// - `retry_all`: retry any error (Python's `retryable_exceptions = [Error]`).
pub(super) fn is_retryable_databricks_error(
    config: &AdapterConfig,
    err: &adbc_core::error::Error,
) -> bool {
    if config.get_bool("retry_all").unwrap_or(false) {
        return true;
    }

    lowercase(Cow::Borrowed(&err.message)).contains("retryableerror")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retry::tests::adbc_err;
    use crate::retry::tests::cfg_default;
    use crate::retry::tests::cfg_retry_all;

    use adbc_core::error::Status;

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
}
