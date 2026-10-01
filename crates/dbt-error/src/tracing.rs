use dbt_tracing::error::TracingError;

use crate::{ErrorCode, FsError};

/// Fusion error bridge for tracing-owned errors.
impl From<TracingError> for FsError {
    fn from(error: TracingError) -> Self {
        let code = match error {
            TracingError::AlreadyInitialized
            | TracingError::SetGlobalSubscriber
            | TracingError::InvalidFilterDirective(_) => ErrorCode::Unexpected,
            TracingError::Io(_)
            | TracingError::ThreadJoin(_)
            | TracingError::ChannelClosed(_)
            | TracingError::Shutdown(_) => ErrorCode::IoError,
        };

        FsError::new(code, error.to_string())
    }
}
