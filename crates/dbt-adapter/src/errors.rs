use arrow_schema::ArrowError;
use dbt_auth::AuthError;

pub use dbt_common::error::{
    AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult, Cancellable, into_fs_error,
};

/// Convert [AuthError] to [AdapterError].
///
/// Use with `.map_err(auth_error_to_adapter_error)` where needed.
#[inline]
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
#[inline]
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

    let sqlstate: [u8; 5] = {
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
    };
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
        adbc_core::error::Status::Unauthenticated => AdapterErrorKind::Authentication,
        adbc_core::error::Status::Unauthorized => AdapterErrorKind::Authentication,
        adbc_core::error::Status::NotFound => AdapterErrorKind::NotFound,
        _ => AdapterErrorKind::Driver,
    };

    AdapterError::new_with_sqlstate_and_vendor_code(kind, err.message, sqlstate, vendor_code)
}
