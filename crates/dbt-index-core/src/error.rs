#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("ADBC error: {0}")]
    Adbc(#[from] adbc_core::error::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("{0}")]
    Other(String),
}
