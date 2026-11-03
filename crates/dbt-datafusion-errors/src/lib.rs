//! Conversions between DataFusion's errors and dbt's.
//!
//! `dbt-error` and `dbt-frontend-common` keep a DataFusion error only as an
//! opaque cause. Code that gets errors from DataFusion converts them here:
//! `.into_fs()` on a DataFusion result, and
//! `.into_datafusion()` on an `FsResult` that has to be returned to
//! DataFusion. (The frontend converts DataFusion errors to its own
//! `InternalError` itself.)

use datafusion_common::DataFusionError;
use dbt_error::{ErrorCode, FsError, FsResult, NameError, WrappedError};

/// The [FsError] for a DataFusion error.
pub fn fs_error(err: DataFusionError) -> FsError {
    match err {
        // --- !!FIXME!! --- For migration only! This is to allow
        // "tunneling" FsErrors through DataFusionError. Remove once
        // we get rid of all DataFusionResult usage.
        DataFusionError::External(e) if e.is::<FsError>() => {
            *e.downcast::<FsError>().expect("e.is::<FsError>()")
        }
        // --- End of !!FIXME!! ---
        DataFusionError::Execution(s) => {
            // TODO https://github.com/sdf-labs/sdf/issues/3515 We cannot use .with_cause, that
            // would produce a bad message for the user for cases where
            // DataFusionError::Execution originates from SDF (so-called "legacy errors").
            FsError::new(ErrorCode::ExecutionError, s)
        }
        DataFusionError::ExecutionJoin(_) => {
            FsError::new(ErrorCode::ExecutionError, "Execution join error")
                .with_cause(WrappedError::Datafusion(Box::new(err)))
        }
        DataFusionError::ArrowError(ae, _) => {
            FsError::new(ErrorCode::ArrowError, "Arrow error").with_cause(WrappedError::Arrow(ae))
        }
        DataFusionError::ParquetError(pe) => FsError::new(ErrorCode::ParquetError, "Parquet error")
            .with_cause(WrappedError::Parquet(pe)),
        DataFusionError::IoError(ie) => {
            FsError::new(ErrorCode::IoError, "IO error").with_cause(WrappedError::Io(ie))
        }
        DataFusionError::Plan(s) => FsError::new(ErrorCode::LogicalPlanError, "Semantic error")
            .with_cause(WrappedError::Generic(s)),
        DataFusionError::SchemaError(se, _) => {
            FsError::new(ErrorCode::LogicalPlanError, "Schema error").with_cause(
                WrappedError::NameError(NameError::Datafusion(se.to_string())),
            )
        }
        DataFusionError::ResourcesExhausted(s) => {
            FsError::new(ErrorCode::ResourceError, "Resource error")
                .with_cause(WrappedError::Generic(s))
        }
        DataFusionError::SQL(_, _)
        | DataFusionError::NotImplemented(_)
        | DataFusionError::Internal(_)
        | DataFusionError::Configuration(_)
        | DataFusionError::Context(_, _)
        | DataFusionError::Substrait(_)
        | DataFusionError::External(_)
        | DataFusionError::ObjectStore(_)
        | DataFusionError::Diagnostic(_, _)
        | DataFusionError::Collection(_)
        | DataFusionError::Shared(_)
        | DataFusionError::Ffi(_) => {
            FsError::new(ErrorCode::GenericDatafusionError, "Datafusion error")
                .with_cause(WrappedError::Datafusion(Box::new(err)))
        }
    }
}

/// The DataFusion error for an [FsError]: the DataFusion error it wraps, if
/// any, and the [FsError] itself as an external error otherwise.
pub fn datafusion_error(err: FsError) -> DataFusionError {
    match err.into_datafusion_cause() {
        Ok(cause) => match cause.downcast::<DataFusionError>() {
            Ok(err) => *err,
            Err(cause) => DataFusionError::External(cause),
        },
        Err(err) => DataFusionError::External(err),
    }
}

pub trait DatafusionResultExt<T> {
    fn into_fs(self) -> FsResult<T>;
}

impl<T> DatafusionResultExt<T> for Result<T, DataFusionError> {
    fn into_fs(self) -> FsResult<T> {
        self.map_err(|err| Box::new(fs_error(err)))
    }
}

pub trait FsResultExt<T> {
    fn into_datafusion(self) -> Result<T, DataFusionError>;
}

impl<T> FsResultExt<T> for FsResult<T> {
    fn into_datafusion(self) -> Result<T, DataFusionError> {
        self.map_err(|err| datafusion_error(*err))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_fs_error_tunnels_through_datafusion_unchanged() {
        let err = FsError::new(ErrorCode::IoError, "disk on fire");
        let back = fs_error(datafusion_error(err));
        assert_eq!(back.code, ErrorCode::IoError);
        assert_eq!(back.context, "disk on fire");
    }

    #[test]
    fn a_wrapped_datafusion_error_converts_back_to_itself() {
        let err = fs_error(DataFusionError::Internal("boom".to_string()));
        assert_eq!(err.code, ErrorCode::GenericDatafusionError);
        assert!(matches!(
            datafusion_error(err),
            DataFusionError::Internal(message) if message == "boom"
        ));
    }
}
