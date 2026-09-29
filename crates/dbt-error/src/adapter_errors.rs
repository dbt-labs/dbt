use dbt_base::Cancellable;
use minijinja::{Error as MinijinjaError, ErrorKind as MinijinjaErrorKind};
use std::future::Future;
use std::io::{self};
use std::pin::Pin;
use std::sync::Arc;
use std::{fmt, panic};

use super::{ErrorCode, types::FsError};

pub type AdapterResult<T> = Result<T, AdapterError>;

/// A pinned Future that produces a `Result<T, Cancellable<AdapterError>>`.
pub type AsyncAdapterResult<'a, T, E = AdapterError> =
    Pin<Box<dyn Future<Output = Result<T, Cancellable<E>>> + Send + 'a>>;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub enum AdapterErrorKind {
    /// Internal error
    Internal,
    /// SQL execution error
    SqlExecution,
    /// Configuration-related error
    Configuration,
    /// Authenrication error
    Authentication,
    /// Driver error (mostly ADBC)
    Driver,
    /// Data not found on remote
    NotFound,
    /// Arrow error
    Arrow,
    /// Unexpected result
    UnexpectedResult,
    /// Recorded replay data was missing or invalid
    ReplayDataInvalid,
    /// Recorded replay data missing for requested operation
    ReplayDataMissing,
    /// SQL output did not match expected (conformance/build diff)
    SqlMismatch,
    /// Unexpected Database Ref
    UnexpectedDbReference,
    /// Cancelled operation
    Cancelled,
    /// Unsupported type
    UnsupportedType,
    /// Input/Output error
    Io,
    /// JSON ser/deserialization error
    SerdeJSON,
    /// YAML ser/deserialization error
    SerdeYAML,
    /// Replay of an error
    Replay,
    /// Not supported
    NotSupported,
    /// Python models are not supported in replay mode
    PythonModelsNotSupportedInReplay,
}

impl fmt::Display for AdapterErrorKind {
    /// For a string representation of the error kind
    /// For a description/explanation, use [AdapterErrorKind::description]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                AdapterErrorKind::Internal => "Internal Error",
                AdapterErrorKind::SqlExecution => "Sql Execution Error",
                AdapterErrorKind::Configuration => "Configuration Error",
                AdapterErrorKind::Driver => "Driver Error",
                AdapterErrorKind::NotFound => "Not Found",
                AdapterErrorKind::Authentication => "Authentication Error",
                AdapterErrorKind::Arrow => "Arrow Error",
                AdapterErrorKind::UnexpectedResult => "Unexpected Result",
                AdapterErrorKind::ReplayDataInvalid => "Replay Data Invalid",
                AdapterErrorKind::ReplayDataMissing => "Replay Data Missing",
                AdapterErrorKind::SqlMismatch => "SQL Mismatch",
                AdapterErrorKind::UnexpectedDbReference => "Unexpected Database Reference",
                AdapterErrorKind::Cancelled => "Cancelled",
                AdapterErrorKind::UnsupportedType => "Unsupported Type",
                AdapterErrorKind::Io => "IoError",
                AdapterErrorKind::SerdeJSON => "Serde JSON Error",
                AdapterErrorKind::SerdeYAML => "Serde YAML Error",
                AdapterErrorKind::Replay => "Replay Error",
                AdapterErrorKind::NotSupported => "Not Supported",
                AdapterErrorKind::PythonModelsNotSupportedInReplay =>
                    "Python Models Not Supported in Replay",
            }
        )
    }
}

impl From<AdapterErrorKind> for ErrorCode {
    fn from(val: AdapterErrorKind) -> Self {
        match val {
            AdapterErrorKind::Internal => ErrorCode::Unexpected,
            AdapterErrorKind::SqlExecution => ErrorCode::ExecutorFailed,
            AdapterErrorKind::Configuration => ErrorCode::InvalidConfig,
            AdapterErrorKind::Authentication => ErrorCode::DbAuthFailed,
            AdapterErrorKind::NotFound => ErrorCode::DbNotFound,
            AdapterErrorKind::Driver => ErrorCode::DbDriverFailed,
            AdapterErrorKind::Arrow => ErrorCode::ArrowError,
            AdapterErrorKind::UnexpectedResult => ErrorCode::ExecutorFailed,
            AdapterErrorKind::ReplayDataInvalid => ErrorCode::ReplayDataInvalid,
            AdapterErrorKind::ReplayDataMissing => ErrorCode::ReplayDataMissing,
            AdapterErrorKind::SqlMismatch => ErrorCode::SqlMismatch,
            // When DB is configured incorrectly, e.g. RA3
            AdapterErrorKind::UnexpectedDbReference => ErrorCode::InvalidConfig,
            AdapterErrorKind::Cancelled => ErrorCode::TaskCancelled,
            AdapterErrorKind::UnsupportedType => ErrorCode::InvalidType,
            AdapterErrorKind::Io => ErrorCode::IoError,
            AdapterErrorKind::SerdeJSON => ErrorCode::JsonInvalid,
            AdapterErrorKind::SerdeYAML => ErrorCode::YamlInvalid,
            AdapterErrorKind::NotSupported => ErrorCode::DbUnsupportedFeature,
            // Test framework related
            AdapterErrorKind::Replay => ErrorCode::Generic,
            AdapterErrorKind::PythonModelsNotSupportedInReplay => ErrorCode::NotSupported,
        }
    }
}

fn map_sqlstate_to_code(sqlstate: &str) -> Option<ErrorCode> {
    if sqlstate.len() < 2 {
        return None;
    }

    let class = sqlstate[0..2].to_ascii_uppercase();
    match class.as_str() {
        "08" => Some(ErrorCode::DbConnectionFailed),
        "28" => Some(ErrorCode::DbAuthFailed),
        "42" => Some(ErrorCode::DbSyntaxInvalid),
        "53" => Some(ErrorCode::DbResourceExceeded),
        "57" | "58" => Some(ErrorCode::DbUnavailable),
        "40" => Some(ErrorCode::DbTxnConflict),
        "0A" => Some(ErrorCode::DbUnsupportedFeature),
        "HY" => Some(ErrorCode::DbDriverFailed),
        _ => None,
    }
}

/// Adapter error.
#[derive(Debug, Clone)]
pub struct AdapterError {
    kind: AdapterErrorKind,
    message: String,
    /// SQLSTATE code from database operations.
    ///
    /// Use [AdapterError::sqlstate()] to get the string representation.
    sqlstate: [u8; 5],
    /// Vendor-specific error code, if applicable.
    vendor_code: Option<i32>,
    /// Source of this error
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl AdapterError {
    /// Create new error.
    pub fn new(kind: AdapterErrorKind, msg: impl Into<String>) -> Self {
        Self {
            kind,
            message: msg.into(),
            sqlstate: [b'0'; 5],
            vendor_code: None,
            source: None,
        }
    }

    /// Attaches another error as source to this error.
    pub fn with_source<E: std::error::Error + Send + Sync + 'static>(mut self, source: E) -> Self {
        self.source = Some(Arc::new(source));
        self
    }

    pub fn new_with_sqlstate_and_vendor_code(
        kind: AdapterErrorKind,
        message: String,
        sqlstate: [u8; 5],
        vendor_code: Option<i32>,
    ) -> Self {
        AdapterError {
            kind,
            message,
            sqlstate,
            vendor_code,
            source: None,
        }
    }

    /// Create an AdapterError from a configuration message
    pub fn from_config(msg: impl Into<String>) -> Self {
        AdapterError::new(AdapterErrorKind::Configuration, msg)
    }

    pub fn kind(&self) -> AdapterErrorKind {
        self.kind
    }

    pub fn message(&self) -> &str {
        let stripped_message = if matches!(self.kind, AdapterErrorKind::Driver) {
            // Remove prefixes like "Unknown: " or "Internal: " which don't
            // add any informational value to the error message.
            self.message
                .strip_prefix("Unknown: ")
                .or_else(|| self.message.strip_prefix("Internal: "))
        } else {
            None
        };
        stripped_message.unwrap_or(&self.message)
    }

    /// Get SQLSTATE as an ASCII string.
    ///
    /// Error codes defined by the SQL standard and vendor implementations [1][2].
    ///
    /// [1] https://en.wikipedia.org/wiki/SQLSTATE
    /// [2] https://learn.microsoft.com/en-us/sql/odbc/reference/appendixes/appendix-a-odbc-error-codes
    pub fn sqlstate(&self) -> &str {
        // SQLSTATE is an ASCII string, so we can convert
        // it to a str without allocating a new string.
        let res = std::str::from_utf8(&self.sqlstate);
        debug_assert!(
            res.is_ok(),
            "SQLSTATE is not valid ASCII: {:?}",
            &self.sqlstate
        );
        res.unwrap_or("")
    }

    pub fn vendor_code(&self) -> Option<i32> {
        self.vendor_code
    }
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = self.message();
        if message.is_empty() {
            write!(f, "{}", self.kind)?;
        } else {
            // Some error kinds do not have to be part of the message because the error
            // message is already descriptive enough and prefixed with context like
            // "[Snowflake] ...".
            match self.kind {
                AdapterErrorKind::Driver | AdapterErrorKind::NotFound => write!(f, "{message}")?,
                _ => write!(f, "{}: {message}", self.kind)?,
            }
        }
        // Driver errors should already contain SQL State and Vendor codes
        if !matches!(self.kind, AdapterErrorKind::Driver) {
            let sqlstate: &str = self.sqlstate();
            if sqlstate != "00000" || self.vendor_code.is_some() {
                write!(f, " (SQLSTATE: {sqlstate}")?;
                if let Some(vendor_code) = self.vendor_code {
                    write!(f, ", Vendor code: {vendor_code}")?;
                }
                write!(f, ")")?;
            }
        }
        // If we contain a Jinja source, we should try to grab the stack trace
        if let Some(jinja_err) = self
            .source
            .as_ref()
            .and_then(|e| e.downcast_ref::<MinijinjaError>())
        {
            let _ = jinja_err.stack_trace(f);
        }
        Ok(())
    }
}

impl std::error::Error for AdapterError {}

// Convert AdapterError to MinijinjaError to enable bridge to report
// errors easily
impl From<AdapterError> for MinijinjaError {
    fn from(err: AdapterError) -> Self {
        MinijinjaError::new(MinijinjaErrorKind::Execution, format!("{err}")).with_source(err)
    }
}

impl From<MinijinjaError> for AdapterError {
    fn from(err: MinijinjaError) -> Self {
        AdapterError::new(AdapterErrorKind::Configuration, err.to_string()).with_source(err)
    }
}

impl From<io::Error> for AdapterError {
    fn from(err: io::Error) -> Self {
        AdapterError::new(AdapterErrorKind::Io, err.to_string())
    }
}

impl From<parquet::errors::ParquetError> for AdapterError {
    fn from(err: parquet::errors::ParquetError) -> Self {
        AdapterError::new(AdapterErrorKind::Io, err.to_string())
    }
}

impl From<serde_json::Error> for AdapterError {
    fn from(err: serde_json::Error) -> Self {
        AdapterError::new(AdapterErrorKind::SerdeJSON, err.to_string())
    }
}

impl From<dbt_yaml::Error> for AdapterError {
    fn from(err: dbt_yaml::Error) -> Self {
        AdapterError::new(AdapterErrorKind::SerdeYAML, err.to_string())
    }
}

impl From<tokio::task::JoinError> for AdapterError {
    fn from(err: tokio::task::JoinError) -> Self {
        if err.is_cancelled() {
            AdapterError::new(AdapterErrorKind::Cancelled, "")
        } else if err.is_panic() {
            panic::resume_unwind(err.into_panic());
        } else {
            // as of today, this is unreachable, but we keep it for future-proofing
            AdapterError::new(AdapterErrorKind::Internal, err.to_string())
        }
    }
}

impl From<dbt_runtime::JoinError> for AdapterError {
    fn from(err: dbt_runtime::JoinError) -> Self {
        if err.is_cancelled() {
            AdapterError::new(AdapterErrorKind::Cancelled, "")
        } else if err.is_panic() {
            panic::resume_unwind(err.into_panic());
        } else {
            // as of today, this is unreachable, but we keep it for future-proofing
            AdapterError::new(AdapterErrorKind::Internal, err.to_string())
        }
    }
}

impl From<AdapterError> for Box<FsError> {
    fn from(err: AdapterError) -> Self {
        let err_code = map_sqlstate_to_code(err.sqlstate()).unwrap_or_else(|| err.kind().into());
        Box::new(FsError::new(err_code, format!("{err}")))
    }
}

pub fn into_fs_error(err: Cancellable<AdapterError>) -> Box<FsError> {
    match err {
        Cancellable::Cancelled => {
            let e = FsError::new(
                ErrorCode::OperationCanceled,
                "Adapter operation was cancelled",
            );
            Box::new(e)
        }
        Cancellable::Error(err) => err.into(),
    }
}
