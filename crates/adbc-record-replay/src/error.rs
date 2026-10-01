use std::fmt;
use std::path::Path;

#[derive(Debug)]
pub(crate) struct RecordReplayError(pub String);

impl fmt::Display for RecordReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for RecordReplayError {}

impl From<std::io::Error> for RecordReplayError {
    fn from(e: std::io::Error) -> Self {
        RecordReplayError(format!("IO error: {e}"))
    }
}

impl From<arrow_schema::ArrowError> for RecordReplayError {
    fn from(e: arrow_schema::ArrowError) -> Self {
        RecordReplayError(format!("Arrow error: {e}"))
    }
}

impl From<parquet::errors::ParquetError> for RecordReplayError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        RecordReplayError(format!("Parquet error: {e}"))
    }
}

impl From<serde_json::Error> for RecordReplayError {
    fn from(e: serde_json::Error) -> Self {
        RecordReplayError(format!("JSON error: {e}"))
    }
}

impl From<rusqlite::Error> for RecordReplayError {
    fn from(e: rusqlite::Error) -> Self {
        RecordReplayError(format!("SQLite error: {e}"))
    }
}

impl From<base64::DecodeError> for RecordReplayError {
    fn from(e: base64::DecodeError) -> Self {
        RecordReplayError(format!("Base64 decode error: {e}"))
    }
}

pub(crate) fn to_adbc_error(e: RecordReplayError, path: Option<&Path>) -> adbc_core::error::Error {
    let message = if let Some(path) = path {
        format!("{} (path: {})", e, path.display())
    } else {
        e.to_string()
    };
    adbc_core::error::Error::with_message_and_status(message, adbc_core::error::Status::IO)
}
