//! # Error handling
//!
//! This module defines the error handling infrastructure for the frontend.
//!
//! We use a two-tiered error structure: [InternalError] and [FrontendError].
//! ```text
//!       ┏ ━ ━ ━ ━ ━ ━ ━ ┓    ┏ ━ ━ ━ ━ ━ ━ ━ ━ ━ ━ ━┓           
//!        DatafusionError      (other library errors)            
//!       ┗ ━ ━ ━ ━ ━ ━ ━ ┛    ┗ ━ ━ ━ ━ ━ ━ ━ ━ ━ ━ ━┛           
//!               │                        │                      
//!               │                        │                      
//!               └───────────┬────────────┘                      
//!                           │                                   
//!                           │  (Into)                           
//!                           ▼                                   
//!                   ┌───────────────┐         ╔════════════════╗
//!                   │ InternalError │ ═══════ ║ InternalResult ║
//!                   └───────────────┘         ╚════════════════╝
//!            ──┐            │                          │        
//! ErrorCode    │            ▼                          ▼        
//!              │   ┌ ─ ─ ─ ─ ─ ─ ─ ┐          ┌ ─ ─ ─ ─ ─ ─ ─ ┐
//! CodeLocation ├───| lift()        |          | lift()        |
//!              │   └ ─ ─ ─ ─ ─ ─ ─ ┘          └ ─ ─ ─ ─ ─ ─ ─ ┘
//! Description  │            │                          │        
//!            ──┘            ▼                          ▼        
//!                  ┌────────────────┐         ╔════════════════╗
//!                  │ FrontendError  │════════ ║ FrontendResult ║
//!                  └────────────────┘         ╚════════════════╝
//!                           │                          │        
//!                                                               
//!                           │                          │        
//!                           ▼                          ▼        
//!                      (To caller)                (To caller)   
//! ```

mod code_location;
mod codes;
mod name_candidate;

use std::{backtrace::Backtrace, sync::Arc};

pub use code_location::CodeLocation;
pub use codes::ErrorCode;
pub use name_candidate::NameCandidate;
pub use name_candidate::format_candidates;

/// Arrow schema metadata key used to record the parquet file path a schema was
/// loaded from.  Written by `dbt-schema-store` when reading a cached parquet
/// file; read by the binder to produce a more actionable error message.
pub const DBT_CACHED_PARQUET_PATH_KEY: &str = "DBT:cached_parquet_path";

pub type FrontendResult<T, E = Box<FrontendError>> = Result<T, E>;

pub type InternalResult<T, E = Box<InternalError>> = Result<T, E>;

#[macro_export]
macro_rules! internal_err {
    ($($arg:tt)*) => {
        Err($crate::make_internal_err!($($arg)*))
    }
}

#[macro_export]
macro_rules! make_internal_err {
    (loc => $location:expr, $($arg:tt)*) => {
        Box::new($crate::error::InternalError::new_with_location(
            format!($($arg)*),
            $location,
        ))
    };
    ($($arg:tt)*) => {
        Box::new($crate::error::InternalError::new(format!($($arg)*)))
    }
}

/// Convenience macro to construct a user-facing [FrontendError].
#[macro_export]
macro_rules! frontend_err {
    ($code:expr, $location:expr, $($arg:tt)*) => {
        Err(Box::new($crate::error::FrontendError::new(
            $code,
            $location,
            format!($($arg)*),
        )))
    }
}

#[macro_export]
macro_rules! unexpected_err {
    ($($arg:tt)*) => {
        Err(Box::new($crate::error::FrontendError::new_with_forced_backtrace(
            $crate::error::ErrorCode::Unexpected,
            $crate::error::CodeLocation::default(),
            format!($($arg)*),
        )))
    }
}

#[macro_export]
macro_rules! notimplemented_err {
    ($($arg:tt)*) => {
        Err(Box::new($crate::error::FrontendError::new(
            $crate::error::ErrorCode::NotImplemented,
            $crate::error::CodeLocation::default(),
            format!($($arg)*),
        )))
    }
}

/// Constructs a lazily evaluated error context. Used as argument to the
/// `lift()` and `with_context()` methods.
#[macro_export]
macro_rules! ectx {

    // ---
    // This section contains variants that can be used in both `lift()` and `with_context()`
    // ---

    // If the context message is omitted, then we simply inherit the message in
    // the upstream error:
    ($code:expr, $location:expr) => {
        |err| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: err.message()
        }
    };
    // The "more" variant allows for additional context to be added to the
    // error. In this case, the context format string must contain the variable
    // `_cause`:
    ($code:expr, $location:expr, more => ($($arg:tt)*))  => {
        |err| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: format!($($arg)*, _cause = err.message()),
        }
    };
    ($code:expr, $location:expr, more => $arg:tt) => {
        |err| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: format!($arg, _cause = err.message()),
        }
    };
    // The "replace" variant allows for the context to be replaced entirely:
    ($code:expr, $location:expr, replace => ($($arg:tt)*))  => {
        |_| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: format!($($arg)*),
        }
    };
    ($code:expr, $location:expr, replace => $arg:tt)  => {
        |_| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: format!($arg),
        }
    };
    // Default is to "replace"
    ($code:expr, $location:expr, $($arg:tt)*) => {
        |_| $crate::error::ErrContext {
            code: $code,
            location: $location,
            context: format!($($arg)*),
        }
    };

    // ---
    // The following variants can *only* be used in [FrontendResult::with_context()].
    // They provide a shorthand to specify only a new context message, while inheriting
    // the error code and location from the upstream error.
    // ---

    // The "more" variant allows for additional context to be added to the
    // existing error message. Requires the context format string to contain
    // "{_cause}".
    (more => $arg:tt) => {
        |err| $crate::error::ErrContext {
            code: err.code,
            location: err.location().clone(),
            context: format!($arg, _cause = err.message()),
        }
    };
    (more => ($($arg:tt)*))  => {
        |err| $crate::error::ErrContext {
            code: err.code,
            location: err.location().clone(),
            context: format!($($arg)*, _cause = err.message()),
        }
    };

    // The "replace" variant allows for the context to be replaced entirely.
    (replace => $arg:tt) => {
        |err| $crate::error::ErrContext {
            code: err.code,
            location: err.location().clone(),
            context: format!($arg),
        }
    };
    (replace => ($($arg:tt)*))  => {
        |err| $crate::error::ErrContext {
            code: err.code,
            location: err.location().clone(),
            context: format!($($arg)*),
        }
    };
}

pub use frontend_err;
pub use internal_err;
pub use make_internal_err;
pub use notimplemented_err;
pub use unexpected_err;

/// Main trait for all errors returned from the frontend.
#[derive(Debug)]
pub struct FrontendError {
    pub code: ErrorCode,
    pub location: CodeLocation,
    pub context: String,
    pub cause: Option<Box<WrappedError>>,
    pub backtrace: Backtrace,

    // Chain of errors, to allow returning multiple errors in a single
    // [FrontendResult]:
    next: Option<Box<FrontendError>>,
}

impl std::fmt::Display for FrontendError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.context)?;
        if self.code == ErrorCode::Unknown
            && let Some(cause) = self.cause.as_ref()
        {
            write!(f, ": {cause:?}")?;
        }
        Ok(())
    }
}

impl std::error::Error for FrontendError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause.as_ref().map(|e| &**e as &dyn std::error::Error)
    }
}

impl FrontendError {
    pub fn new(code: ErrorCode, location: CodeLocation, context: impl Into<String>) -> Self {
        FrontendError {
            code,
            location,
            context: context.into(),
            cause: None,
            backtrace: Backtrace::capture(),
            next: None,
        }
    }

    pub fn new_with_forced_backtrace(
        code: ErrorCode,
        location: CodeLocation,
        context: impl Into<String>,
    ) -> Self {
        FrontendError {
            code,
            location,
            context: context.into(),
            cause: None,
            backtrace: Backtrace::force_capture(),
            next: None,
        }
    }

    pub fn new_unexpected(context: impl Into<String>) -> Self {
        FrontendError::new_with_forced_backtrace(
            ErrorCode::Unexpected,
            CodeLocation::default(),
            format!("An unexpected error occurred: {}", context.into()),
        )
    }

    /// Description of the error.
    pub fn message(&self) -> &str {
        self.context.as_str()
    }

    /// True if this error contains a backtrace.
    pub fn has_backtrace(&self) -> bool {
        self.backtrace.status() == std::backtrace::BacktraceStatus::Captured
    }

    /// Returns the backtrace as a string, if available.
    pub fn get_backtrace(&self) -> Option<String> {
        if self.has_backtrace() {
            Some(self.backtrace.to_string())
        } else {
            None
        }
    }

    /// True if this error contains multiple errors.
    pub fn is_multiple_errors(&self) -> bool {
        self.next.is_some()
    }

    pub fn with_new_location(self, location: CodeLocation) -> Self {
        FrontendError { location, ..self }
    }

    pub fn with_location_delta(self, start: &CodeLocation) -> Self {
        FrontendError {
            location: self.location.with_offset(start),
            ..self
        }
    }

    pub fn with_context(self, context: impl std::fmt::Display) -> Self {
        FrontendError {
            context: format!("{context}"),
            ..self
        }
    }

    pub fn with_new_code(self, code: ErrorCode) -> Self {
        FrontendError { code, ..self }
    }

    pub fn chain(self, next: Box<FrontendError>) -> Self {
        FrontendError {
            next: Some(next),
            ..self
        }
    }

    /// Flattens multiple errors into a single vector.
    ///
    /// If this error is a single error, the result will be a vector with a
    /// single element, self. If this error contains multiple errors, the result
    /// will be a vector containing all errors in the chain, where each error is
    /// a single error.
    pub fn flatten(self) -> Vec<Box<FrontendError>> {
        let mut errors = vec![];
        let mut cur = Box::new(self);
        loop {
            let mut next = cur.next.take();
            errors.push(cur);
            if let Some(e) = next.take() {
                cur = e;
            } else {
                break;
            }
        }
        errors
    }

    /// Returns a reference to the [CodeLocation] of this error.
    pub fn location(&self) -> &CodeLocation {
        &self.location
    }

    fn from_internal_err(
        code: ErrorCode,
        location: CodeLocation,
        context: Option<impl Into<String>>,
        err: InternalError,
    ) -> Self {
        let msg = err.message();
        let cause = if let WrappedError::Frontend(e) = err.error {
            if e.code.is_bug() {
                // For unexpected errors, we always propagate the original
                // cause:
                return e;
            } else {
                Some(Box::new(WrappedError::Generic(e.context)))
            }
        } else {
            Some(Box::new(err.error))
        };

        FrontendError {
            code,
            // Prefer the location from the internal error, if any -- the lower
            // level location should be more specific:
            location: if let Some(err_location) = err.location {
                if err_location.has_position() {
                    err_location
                } else {
                    location
                }
            } else {
                location
            },

            context: if let Some(context) = context {
                context.into()
            } else {
                msg
            },
            cause,
            backtrace: err.backtrace,
            next: None,
        }
    }
}

use datafusion_common::DFSchema;
use datafusion_common::error::DataFusionError;
use datafusion_expr::expr::Expr;
use dbt_antlr4::errors::ANTLRError;

#[derive(Debug)]
pub struct SchemaError {
    context: String,
    target: String,
    schemas: Vec<Arc<DFSchema>>,
    /// Path to the parquet file this schema was loaded from, if any.
    pub parquet_path: Option<String>,
}

impl SchemaError {
    pub fn new(
        context: impl Into<String>,
        target: impl Into<String>,
        schemas: impl Into<Vec<Arc<DFSchema>>>,
    ) -> Self {
        SchemaError {
            context: context.into(),
            target: target.into(),
            schemas: schemas.into(),
            parquet_path: None,
        }
    }

    pub fn with_parquet_path(mut self, path: String) -> Self {
        self.parquet_path = Some(path);
        self
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn schemas(&self) -> &[Arc<DFSchema>] {
        &self.schemas
    }

    pub fn fields(&self) -> Vec<String> {
        self.schemas.iter().flat_map(|s| s.field_names()).collect()
    }

    /// If the Arrow schema backing `schema` carries a
    /// [`DBT_CACHED_PARQUET_PATH_KEY`] metadata entry, records that path on
    /// this error so the display layer can emit a more actionable message.
    pub fn with_parquet_path_from_schema(mut self, schema: &Arc<DFSchema>) -> Self {
        if let Some(path) = schema.as_ref().metadata().get(DBT_CACHED_PARQUET_PATH_KEY) {
            self.parquet_path = Some(path.clone());
        }
        self
    }
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.context)
    }
}

#[derive(Debug)]
pub struct AliasedExprsError {
    context: String,
    target: String,
    items: Vec<Expr>,
}

impl AliasedExprsError {
    pub fn new(context: impl Into<String>, target: impl Into<String>, items: Vec<Expr>) -> Self {
        AliasedExprsError {
            context: context.into(),
            target: target.into(),
            items,
        }
    }

    pub fn context(&self) -> &str {
        &self.context
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn items(&self) -> &[Expr] {
        &self.items
    }

    pub fn fields(&self) -> Vec<String> {
        self.items.iter().map(|e| e.to_string()).collect()
    }
}

impl std::fmt::Display for AliasedExprsError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.context)
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum WrappedError {
    Frontend(FrontendError),
    Antlr(ANTLRError),
    Arrow(Box<arrow::error::ArrowError>),
    Datafusion(DataFusionError),
    ParseFloat(std::num::ParseFloatError),
    ParseInt(std::num::ParseIntError),
    SerdeJson(serde_json::Error),
    Schema(SchemaError),
    AliasedExprs(AliasedExprsError),
    Generic(String),
}

impl std::fmt::Display for WrappedError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            WrappedError::Antlr(e) => write!(f, "{e}"),
            WrappedError::Datafusion(e) => match e {
                DataFusionError::Plan(e) => write!(f, "{e}"),
                DataFusionError::SchemaError(e, _) => write!(f, "{e}"),
                DataFusionError::ArrowError(e, _) => write!(f, "{e}"),
                _ => write!(f, "{e}"),
            },
            WrappedError::Generic(e) => write!(f, "{e}"),
            WrappedError::Arrow(e) => write!(f, "{e}"),
            WrappedError::ParseFloat(e) => write!(f, "{e}"),
            WrappedError::ParseInt(e) => write!(f, "{e}"),
            WrappedError::Frontend(e) => write!(f, "{e}"),
            WrappedError::SerdeJson(e) => write!(f, "{e}"),
            WrappedError::Schema(e) => write!(f, "{e}"),
            WrappedError::AliasedExprs(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for WrappedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WrappedError::Frontend(e) => Some(e),
            WrappedError::Datafusion(e) => Some(e),
            WrappedError::Arrow(e) => Some(e),
            WrappedError::ParseFloat(e) => Some(e),
            WrappedError::ParseInt(e) => Some(e),
            WrappedError::SerdeJson(e) => Some(e),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct InternalError {
    error: WrappedError,
    pub location: Option<CodeLocation>,
    pub backtrace: Backtrace,
}

impl std::fmt::Display for InternalError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        #[allow(unreachable_patterns)] // Note: this shouldn't be necessary because of
        // the #[non_exhaustive] attribute
        match &self.error {
            WrappedError::Frontend(e) => write!(f, "{e}"),
            WrappedError::Antlr(e) => write!(f, "ANTLR error: {e}"),
            WrappedError::Datafusion(e) => write!(f, "DataFusion error: {e}"),
            WrappedError::Generic(e) => write!(f, "{e}"),
            WrappedError::Arrow(e) => write!(f, "Arrow error: {e}"),
            WrappedError::ParseFloat(e) => write!(f, "ParseFloat error: {e}"),
            WrappedError::ParseInt(e) => write!(f, "ParseInt error: {e}"),
            WrappedError::SerdeJson(e) => write!(f, "SerdeJson error: {e}"),
            WrappedError::Schema(e) => write!(f, "Schema error: {e}"),
            _ => write!(f, "Unknown error"),
        }
    }
}

impl std::error::Error for InternalError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.error.source()
    }
}

impl From<String> for InternalError {
    fn from(e: String) -> Self {
        InternalError {
            error: WrappedError::Generic(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<String> for Box<InternalError> {
    fn from(e: String) -> Self {
        Box::new(e.into())
    }
}

impl From<Box<FrontendError>> for InternalError {
    fn from(e: Box<FrontendError>) -> Self {
        let location = Some(e.location);
        InternalError {
            error: WrappedError::Frontend(*e),
            location,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<Box<FrontendError>> for Box<InternalError> {
    fn from(e: Box<FrontendError>) -> Self {
        Box::new(e.into())
    }
}

impl From<SchemaError> for InternalError {
    fn from(e: SchemaError) -> Self {
        InternalError {
            error: WrappedError::Schema(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<SchemaError> for Box<InternalError> {
    fn from(e: SchemaError) -> Self {
        Box::new(e.into())
    }
}

impl From<AliasedExprsError> for InternalError {
    fn from(e: AliasedExprsError) -> Self {
        InternalError {
            error: WrappedError::AliasedExprs(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<AliasedExprsError> for Box<InternalError> {
    fn from(e: AliasedExprsError) -> Self {
        Box::new(e.into())
    }
}

impl From<std::num::ParseFloatError> for InternalError {
    fn from(e: std::num::ParseFloatError) -> Self {
        InternalError {
            error: WrappedError::ParseFloat(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::num::ParseFloatError> for Box<InternalError> {
    fn from(e: std::num::ParseFloatError) -> Self {
        Box::new(e.into())
    }
}

impl From<std::num::ParseIntError> for InternalError {
    fn from(e: std::num::ParseIntError) -> Self {
        InternalError {
            error: WrappedError::ParseInt(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::num::ParseIntError> for Box<InternalError> {
    fn from(e: std::num::ParseIntError) -> Self {
        Box::new(e.into())
    }
}

impl From<ANTLRError> for InternalError {
    fn from(e: ANTLRError) -> Self {
        InternalError {
            error: WrappedError::Antlr(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<ANTLRError> for Box<InternalError> {
    fn from(e: ANTLRError) -> Self {
        Box::new(e.into())
    }
}

impl From<arrow::error::ArrowError> for InternalError {
    fn from(e: arrow::error::ArrowError) -> Self {
        InternalError {
            error: WrappedError::Arrow(Box::new(e)),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<arrow::error::ArrowError> for Box<InternalError> {
    fn from(e: arrow::error::ArrowError) -> Self {
        Box::new(e.into())
    }
}

impl From<serde_json::Error> for InternalError {
    fn from(e: serde_json::Error) -> Self {
        InternalError {
            error: WrappedError::SerdeJson(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<serde_json::Error> for Box<InternalError> {
    fn from(e: serde_json::Error) -> Self {
        Box::new(e.into())
    }
}

impl From<DataFusionError> for InternalError {
    fn from(e: DataFusionError) -> Self {
        InternalError {
            error: WrappedError::Datafusion(e),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<DataFusionError> for Box<InternalError> {
    fn from(e: DataFusionError) -> Self {
        Box::new(e.into())
    }
}

impl From<std::fmt::Error> for InternalError {
    fn from(e: std::fmt::Error) -> Self {
        InternalError {
            error: WrappedError::Generic(e.to_string()),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }
}

impl From<std::fmt::Error> for Box<InternalError> {
    fn from(e: std::fmt::Error) -> Self {
        Box::new(e.into())
    }
}

impl InternalError {
    pub fn new(context: impl Into<String>) -> Self {
        InternalError {
            error: WrappedError::Generic(context.into()),
            location: None,
            backtrace: Backtrace::capture(),
        }
    }

    pub fn message(&self) -> String {
        self.error.to_string()
    }

    pub fn new_with_location(context: impl Into<String>, location: CodeLocation) -> Self {
        InternalError {
            error: WrappedError::Generic(context.into()),
            location: Some(location),
            backtrace: Backtrace::capture(),
        }
    }

    /// Adds a location to this error, replacing the existing location if any
    pub fn with_location(self, location: CodeLocation) -> InternalError {
        InternalError {
            location: Some(location),
            ..self
        }
    }

    /// Adds a context to this error, replacing the existing [WrappedError]
    pub fn with_context(self, context: impl Into<String>) -> InternalError {
        InternalError {
            error: WrappedError::Generic(context.into()),
            ..self
        }
    }

    /// Transforms this error into a [FrontendError] with the specified contexts
    pub fn lift(
        self,
        code: ErrorCode,
        location: CodeLocation,
        context: Option<impl Into<String>>,
    ) -> Box<FrontendError> {
        FrontendError::from_internal_err(code, location, context, self).into()
    }

    /// Like [lift], but prioritizes any wrapped [FrontendError] over the
    /// provided context. This is mainly intended for use by the `default_ctx!`
    /// macro.
    pub fn default_lift(
        self,
        code: ErrorCode,
        location: CodeLocation,
        context: Option<impl Into<String>>,
    ) -> Box<FrontendError> {
        match self.error {
            WrappedError::Frontend(e) => e.into(),
            _ => FrontendError::from_internal_err(code, location, context, self).into(),
        }
    }

    /// Propagates this error as a bug
    pub fn lift_as_bug(self) -> Box<FrontendError> {
        let location = self.location.unwrap_or_default();
        self.lift(
            ErrorCode::Unexpected,
            location,
            Some("An unexpected error occurred"),
        )
    }
}

#[derive(Debug, Clone)]
pub struct ErrContext {
    pub code: ErrorCode,
    pub location: CodeLocation,
    pub context: String,
}

pub trait WithMoreContext<F, T, E> {
    fn with_context(self, f: F) -> Self;
}

impl<F, T> WithMoreContext<F, T, FrontendError> for FrontendResult<T>
where
    F: FnOnce(&FrontendError) -> ErrContext,
{
    fn with_context(self, f: F) -> Self {
        self.map_err(|e| {
            let ErrContext {
                code,
                location,
                context,
            } = f(e.as_ref());
            FrontendError {
                code,
                location,
                context,
                ..*e
            }
            .into()
        })
    }
}

impl<F, T> WithMoreContext<F, T, InternalError> for InternalResult<T>
where
    F: FnOnce(&InternalError) -> String,
{
    fn with_context(self, f: F) -> Self {
        self.map_err(|e| {
            let context = f(e.as_ref());
            e.with_context(context).into()
        })
    }
}

pub trait LiftableResult<T>: private::Sealed {
    fn lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext;

    fn default_lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext;

    /// Convenience method to declare a Result is expected to be Ok, and thus
    /// any error will be surfaced as a bug.
    fn lift_as_bug(self) -> FrontendResult<T>
    where
        Self: Sized,
    {
        self.lift(|err| ErrContext {
            code: ErrorCode::Unexpected,
            location: CodeLocation::default(),
            context: format!("An unexpected error occurred: {err}"),
        })
    }
}

impl<T> LiftableResult<T> for InternalResult<T> {
    fn lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext,
    {
        self.map_err(|err| {
            let ErrContext {
                code,
                location,
                context,
            } = f(err.as_ref());
            (*err).lift(code, location, Some(context))
        })
    }

    fn default_lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext,
    {
        self.map_err(|err| {
            let ErrContext {
                code,
                location,
                context,
            } = f(err.as_ref());
            (*err).default_lift(code, location, Some(context))
        })
    }
}

impl<T, E> LiftableResult<T> for Result<T, E>
where
    E: Into<InternalError>,
{
    fn lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext,
    {
        self.map_err(|e| {
            let internal = e.into();
            let ErrContext {
                code,
                location,
                context,
            } = f(&internal);
            internal.lift(code, location, Some(context))
        })
    }

    fn default_lift<F>(self, f: F) -> FrontendResult<T>
    where
        F: FnOnce(&InternalError) -> ErrContext,
    {
        self.map_err(|e| {
            let internal = e.into();
            let ErrContext {
                code,
                location,
                context,
            } = f(&internal);
            internal.default_lift(code, location, Some(context))
        })
    }
}

// --- !!FIXME!! --- Start of migration support code
//
// This section exists purely for the purpose of incrementally transitioning to
// the new error infra, will be removed once all errors are migrated. In the
// meantime, delete parts of this section to have the type system surface
// remaining gaps in our error handling, then fix them by moving to the proper
// error type or by attaching proper context to the errors

// Step 1. Delete this part and remove all uses of DataFusionResult [DONE!]
//
// This part allows "tunneling" InternalErrors through DataFusionError, thus
// allowing DataFusionResult to be used interchangeably with InternalResult

// impl From<InternalError> for DataFusionError {
//     fn from(e: InternalError) -> Self {
//         match e.error {
//             WrappedError::Datafusion(e) => e,
//             _ => DataFusionError::External(Box::new(e)),
//         }
//     }
// }

// impl From<Box<InternalError>> for DataFusionError {
//     fn from(e: Box<InternalError>) -> Self {
//         (*e).into()
//     }
// }

// Step 2. Delete this part and attach proper context to lift [InternalError] to
// [FrontendError]. [DONE!]

// impl From<InternalError> for FrontendError {
//     fn from(e: InternalError) -> Self {
//         *e.lift_as_bug()
//     }
// }

// impl From<Box<InternalError>> for Box<FrontendError> {
//     fn from(e: Box<InternalError>) -> Self {
//         (*e).lift_as_bug()
//     }
// }

// impl From<DataFusionError> for Box<FrontendError> {
//     fn from(e: DataFusionError) -> Self {
//         InternalError::from(e).lift(
//             ErrorCode::LegacyDatafusion,
//             CodeLocation::default(),
//             Option::<String>::None,
//         )
//     }
// }

// Step 3. remove this legacy adapter and use [LiftableResult::lift] to attach
// proper error codes and context messages to each callsite: [DONE!]
// #[derive(Debug, Clone)]
// pub(crate) struct BinderError {
//     pub location: CodeLocation,
//     pub message: String,
// }

// impl BinderError {
//     pub fn lift(self) -> Box<FrontendError> {
//         FrontendError::new(ErrorCode::LegacyBinder, self.location, self.message).into()
//     }
// }

// Step 4. Delete this part and remove all uses of DataFusionError from the
// sdf-cli crate. [DONE!]

// impl From<Box<FrontendError>> for DataFusionError {
//     fn from(e: Box<FrontendError>) -> Self {
//         DataFusionError::External(e)
//     }
// }

// impl From<FrontendError> for DataFusionError {
//     fn from(e: FrontendError) -> Self {
//         Box::new(e).into()
//     }
// }

// --- End of !!FIXME!! ---

mod private {
    use super::*;

    pub trait Sealed {}

    impl<T> Sealed for InternalResult<T> {}

    impl<T, E> Sealed for Result<T, E> where E: Into<InternalError> {}
}
