use arrow_schema::ArrowError;
use dbt_auth::AuthError;

pub use dbt_common::error::{
    AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult, Cancellable, into_fs_error,
    is_bridge_error, is_bridge_sqlstate,
};

/// Convert [AuthError] to [AdapterError].
///
/// Use with `.map_err(auth_error_to_adapter_error)` where needed.
pub fn auth_error_to_adapter_error(err: AuthError) -> AdapterError {
    match err {
        AuthError::Adbc(adbc_err) => adbc_error_to_adapter_error(adbc_err),
        AuthError::Config(msg) => AdapterError::from_config(msg),
        AuthError::JSON(json_err) => json_err.into(),
        AuthError::YAML(yaml_err) => yaml_err.into(),
        AuthError::Io(io_err) => io_err.into(),
    }
}

/// Convert [ArrowError] to [AdapterError].
///
/// Use with `.map_err(arrow_error_to_adapter_error)` where needed.
pub fn arrow_error_to_adapter_error(err: ArrowError) -> AdapterError {
    AdapterError::new(AdapterErrorKind::Arrow, err.to_string())
}

const BIGQUERY_STORAGE_API_ERROR_MESSAGE: &str = "There was a problem connecting to the BigQuery Storage API.\n\ndbt v2 requires the BigQuery Read Session User role to connect to BigQuery. See https://docs.getdbt.com/docs/platform/connect-data-platform/connect-bigquery?version=2.0&name=Fusion#required-permissions to make sure you have all the required permissions to use BigQuery with v2.";

/// Convert [adbc_core::error::Error] to [AdapterError]
/// This will also chain the given error to the [AdapterError]
pub fn adbc_error_to_adapter_error(err: adbc_core::error::Error) -> AdapterError {
    if err
        .message
        .contains("Storage API is not available for query")
    {
        return AdapterError::new(AdapterErrorKind::Driver, BIGQUERY_STORAGE_API_ERROR_MESSAGE);
    }

    let sqlstate = adbc_sqlstate(&err);
    // This special vendor code is used to indicate that the error information
    // lives in the `private_data` field and not in the vendor_code.
    const ADBC_ERROR_VENDOR_CODE_PRIVATE_DATA: i32 = -2147483648;
    // XXX: should 0 become Some(0) instead of None?
    let vendor_code = if [0, -1, ADBC_ERROR_VENDOR_CODE_PRIVATE_DATA].contains(&err.vendor_code) {
        None
    } else {
        Some(err.vendor_code)
    };

    let kind = match err.status {
        adbc_core::error::Status::Cancelled => AdapterErrorKind::Cancelled,
        // The Bridge driver derives the status from the gRPC code, so a lost
        // session arrives as Unauthenticated. The SQLSTATE says what happened.
        _ if is_bridge_sqlstate(&sqlstate) => AdapterErrorKind::Driver,
        adbc_core::error::Status::Unauthenticated => AdapterErrorKind::Authentication,
        adbc_core::error::Status::Unauthorized => AdapterErrorKind::Authentication,
        adbc_core::error::Status::NotFound => AdapterErrorKind::NotFound,
        _ => AdapterErrorKind::Driver,
    };

    AdapterError::new_with_sqlstate_and_vendor_code(kind, err.message, sqlstate, vendor_code)
}

/// SQLSTATE of `err` as ASCII bytes, `00000` when unset.
fn adbc_sqlstate(err: &adbc_core::error::Error) -> [u8; 5] {
    // Transmute SQLSTATE to unsigned bytes. It was mistake to make this i8
    // in ADBC core [1].
    //
    // `err.sqlstate` is `[i8; 5]` with adbc_core's default features but
    // `[u8; 5]` without them, so under `--no-default-features` this transmute
    // is a no-op; allow the lint rather than diverge the two feature configs.
    //
    // [1] https://github.com/apache/arrow-adbc/pull/1725#discussion_r1567531539
    #[allow(clippy::useless_transmute)]
    let unsigned: [u8; 5] = unsafe { std::mem::transmute(err.sqlstate) };
    if unsigned[0] == 0 {
        // If the string is full of '\0' bytes, we set it to "00000" (b'0' is 48).
        [b'0'; 5]
    } else {
        unsigned
    }
}

/// Whether Bridge raised `err`. See [is_bridge_error].
pub fn is_bridge_adbc_error(err: &adbc_core::error::Error) -> bool {
    is_bridge_error(&adbc_sqlstate(err), &err.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use adbc_core::error::{Error as AdbcError, Status};

    fn with_sqlstate(status: Status, sqlstate: &[u8; 5]) -> AdbcError {
        let mut err = AdbcError::with_message_and_status("[Bridge] session not found", status);
        err.sqlstate = sqlstate.map(|b| b as _);
        err
    }

    #[test]
    fn bridge_sqlstate_ignores_adbc_status_for_kind() {
        for status in [Status::Unauthenticated, Status::NotFound, Status::IO] {
            let err = adbc_error_to_adapter_error(with_sqlstate(status, b"08R06"));
            assert_eq!(err.kind(), AdapterErrorKind::Driver);
            assert_eq!(err.sqlstate(), "08R06");
            for sqlstate in [b"08R16", b"28R07", b"XXR05"] {
                let err = adbc_error_to_adapter_error(with_sqlstate(status, sqlstate));
                assert_eq!(err.kind(), AdapterErrorKind::Driver);
            }
        }
    }

    #[test]
    fn bridge_prefix_without_sqlstate_is_bridge() {
        let cancel = AdbcError::with_message_and_status(
            "[Bridge] handshake was cancelled",
            Status::Cancelled,
        );
        assert!(is_bridge_adbc_error(&cancel));
        assert_eq!(
            adbc_error_to_adapter_error(cancel).kind(),
            AdapterErrorKind::Cancelled
        );
        let deadline = AdbcError::with_message_and_status(
            "[Bridge] the query did not finish before the deadline; it may have run",
            Status::Timeout,
        );
        assert!(is_bridge_adbc_error(&deadline));
        assert_eq!(
            adbc_error_to_adapter_error(deadline).kind(),
            AdapterErrorKind::Driver
        );
        let stream = AdbcError::from(ArrowError::CDataInterface(
            "[Bridge] the result stream ended early; it may have run".to_string(),
        ));
        assert!(is_bridge_adbc_error(&stream), "{}", stream.message);
        assert!(!is_bridge_adbc_error(&AdbcError::with_message_and_status(
            "[Snowflake] 390114 (08001): Authentication token has expired.",
            Status::IO,
        )));
    }

    #[test]
    fn other_sqlstates_keep_status_derived_kind() {
        let err = adbc_error_to_adapter_error(with_sqlstate(Status::Unauthenticated, b"28000"));
        assert_eq!(err.kind(), AdapterErrorKind::Authentication);
        let err = adbc_error_to_adapter_error(with_sqlstate(Status::NotFound, b"08P01"));
        assert_eq!(err.kind(), AdapterErrorKind::NotFound);
    }
}
