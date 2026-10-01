use std::{error::Error, fmt};

pub type TracingResult<T> = Result<T, TracingError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TracingError {
    Io(String),
    AlreadyInitialized,
    SetGlobalSubscriber,
    InvalidFilterDirective(String),
    ThreadJoin(String),
    ChannelClosed(String),
    Shutdown(String),
}

impl TracingError {
    pub fn io(message: impl Into<String>) -> Self {
        Self::Io(message.into())
    }

    pub fn thread_join(message: impl Into<String>) -> Self {
        Self::ThreadJoin(message.into())
    }

    pub fn channel_closed(message: impl Into<String>) -> Self {
        Self::ChannelClosed(message.into())
    }

    pub fn shutdown(message: impl Into<String>) -> Self {
        Self::Shutdown(message.into())
    }

    pub fn invalid_filter_directive(message: impl Into<String>) -> Self {
        Self::InvalidFilterDirective(message.into())
    }
}

impl fmt::Display for TracingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TracingError::Io(message)
            | TracingError::InvalidFilterDirective(message)
            | TracingError::ThreadJoin(message)
            | TracingError::ChannelClosed(message)
            | TracingError::Shutdown(message) => write!(f, "{message}"),
            TracingError::AlreadyInitialized => write!(f, "Tracing is already initialized"),
            TracingError::SetGlobalSubscriber => write!(f, "Failed to set-up tracing"),
        }
    }
}

impl Error for TracingError {}

impl From<std::io::Error> for TracingError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error.to_string())
    }
}
