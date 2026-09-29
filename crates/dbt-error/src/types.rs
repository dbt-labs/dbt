use datafusion_common::error::DataFusionError;
use datafusion_expr::Expr;
use dbt_base::cancel::CancelledError;
use dbt_frontend_common::error::{FrontendError, FrontendResult, NameCandidate, format_candidates};
use itertools::Itertools as _;
use regex::Regex;
use std::{
    backtrace::Backtrace,
    error::Error,
    fmt::{self, Debug, Display, Formatter},
    io, panic,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use super::{ErrorCode, preprocessor_location::MacroSpan};
use crate::code_location::{AbstractLocation, MiniJinjaErrorWrapper};
use crate::utils::{find_enclosed_substring, is_sdf_debug};

pub type FsResult<T, E = Box<FsError>> = Result<T, E>;

// Helper struct to format just the stack trace from a minijinja::Error.
// TODO(jasonlin45): Report stack trace on CodeLocation
struct StackTraceFormatter<'a> {
    err: &'a minijinja::Error,
    file_only_stack_frame: Option<&'a Path>,
}

/// Walk the error source chain of a minijinja error looking for an `AdapterError`.
///
/// This is needed because adapter errors can surface under different Jinja error
/// kinds (`Execution`, `InvalidOperation`, etc.) depending on where they are raised
/// in the macro call chain.
fn find_adapter_error_in_chain(
    err: &minijinja::Error,
) -> Option<&super::adapter_errors::AdapterError> {
    let mut current: Option<&(dyn Error + 'static)> = err.source();
    while let Some(source) = current {
        if let Some(adapter_err) = source.downcast_ref::<super::adapter_errors::AdapterError>() {
            return Some(adapter_err);
        }
        current = source.source();
    }
    None
}

impl<'a> StackTraceFormatter<'a> {
    fn new(err: &'a minijinja::Error) -> Self {
        Self {
            err,
            file_only_stack_frame: None,
        }
    }

    fn with_file_only_stack_frame(err: &'a minijinja::Error, path: &'a Path) -> Self {
        Self {
            err,
            file_only_stack_frame: Some(path),
        }
    }
}

impl Display for StackTraceFormatter<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if !self.err.is_stack_empty() {
            write!(
                f,
                "{}",
                self.err
                    .stack()
                    .iter()
                    .rev()
                    .map(|frame| {
                        if self.file_only_stack_frame.is_some_and(|path| {
                            jinja_stack_frame_matches_path(&frame.filename, path)
                        }) {
                            format!("\n(in {})", frame.filename)
                        } else {
                            format!(
                                "\n(in {}:{}:{})",
                                frame.filename, frame.span.start_line, frame.span.start_col
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("")
            )?;
        }
        Ok(())
    }
}

fn jinja_stack_frame_matches_path(filename: &str, path: &Path) -> bool {
    let filename = filename.replace('\\', "/");
    let path = path.to_string_lossy().replace('\\', "/");

    filename == path || path.ends_with(&filename) || filename.ends_with(&path)
}

fn jinja_file_only_location(
    err: &minijinja::Error,
    path: &Path,
) -> Option<super::CodeLocationWithFile> {
    err.stack()
        .iter()
        .find(|frame| jinja_stack_frame_matches_path(&frame.filename, path))
        .map(|frame| super::CodeLocationWithFile::from(PathBuf::from(&frame.filename)))
}

fn format_jinja_err(err: &minijinja::Error, file_only_stack_frame: Option<&Path>) -> String {
    let mut rendered = if let Some(detail) = err.detail() {
        format!("{}: {}", err.kind(), detail)
    } else {
        err.kind().to_string()
    };

    let stack_trace = if let Some(path) = file_only_stack_frame {
        StackTraceFormatter::with_file_only_stack_frame(err, path).to_string()
    } else {
        StackTraceFormatter::new(err).to_string()
    };
    rendered.push_str(&stack_trace);
    rendered
}

pub struct FsError {
    pub code: ErrorCode,
    pub location: Option<super::CodeLocationWithFile>,
    pub context: String,
    cause: Option<WrappedError>,
    backtrace: Backtrace,

    // Jinja call stack frames (innermost first) captured from a minijinja::Error.
    // This is only used by the LSP layer.
    // TODO: should this become the new basis for `context`?
    // TODO: Should we make these Spans instead to store end locations?
    pub jinja_frames: Vec<super::CodeLocationWithFile>,

    // Chain of errors, to allow returning multiple errors in a single
    // [FsResult]:
    next: Option<Box<FsError>>,
}

pub const MAX_DISPLAY_TOKENS: usize = 7;
impl Debug for FsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        writeln!(f, "[{}]: {}", self.code.name_and_code(), self)?;
        if let Some(loc) = &self.location {
            writeln!(f, " --> {loc}")?;
        }
        if self.backtrace.status() == std::backtrace::BacktraceStatus::Captured {
            write!(f, "\n{}", self.backtrace)?;
        }
        Ok(())
    }
}

static RE_ANTLR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"^.*expecting \{(.*?)\}.*$"#).expect("valid regex"));

impl Display for FsError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self.code {
            ErrorCode::SyntaxInvalid => {
                // Truncate and prettify Antlr syntax error messages
                let message = self.context.as_str();
                let message = if let Some(caps) = RE_ANTLR.captures(message) {
                    if caps.len() == 2 {
                        let original_tokens = caps[1].split(',');
                        let tokens = if original_tokens.clone().count() < MAX_DISPLAY_TOKENS {
                            original_tokens.take(MAX_DISPLAY_TOKENS).join(",")
                        } else {
                            format!("{} ...", original_tokens.take(MAX_DISPLAY_TOKENS).join(","),)
                        };
                        let mat = caps.get(1).unwrap();
                        format!(
                            "{}one of {}{}",
                            &message[..mat.start() - 1],
                            tokens,
                            &message[mat.end() + 1..message.len()]
                        )
                    } else {
                        message.to_string()
                    }
                } else {
                    message.to_string()
                };

                write!(f, "{message}")?
            }
            _ if self.code.is_frontend() => {
                // FrontendErrors have their cause already formatted into the
                // context, so we only need to print the context here
                write!(f, "{}", self.context)?
            }
            _ => {
                write!(f, "{}", self.context)?;
                if let Some(cause) = &self.cause {
                    if !self.context.is_empty() {
                        write!(f, ": ")?;
                    }
                    write!(f, "{cause}")?
                }
            }
        }

        Ok(())
    }
}

impl Error for FsError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.cause.as_ref().map(|e| e as &dyn Error)
    }
}

impl FsError {
    pub fn new(code: ErrorCode, context: impl Into<String>) -> Self {
        FsError {
            code,
            location: None,
            context: context.into(),
            cause: None,
            backtrace: Backtrace::capture(),
            jinja_frames: vec![],
            next: None,
        }
    }

    pub fn new_no_backtrace(code: ErrorCode, context: impl Into<String>) -> Self {
        FsError {
            code,
            location: None,
            context: context.into(),
            cause: None,
            backtrace: Backtrace::disabled(),
            jinja_frames: vec![],
            next: None,
        }
    }

    /// Clones the complete error payload without retaining its backtrace.
    ///
    /// Third-party causes are not generally cloneable, so their rendered form
    /// is retained as a generic cause. Error codes, locations, context, Jinja
    /// frames, and chained errors are cloned directly.
    pub fn clone_without_backtrace(&self) -> Self {
        let cause = self.cause.as_ref().map(|cause| match cause {
            WrappedError::Antlr(error) => WrappedError::Antlr(error.clone()),
            WrappedError::Generic(error) => WrappedError::Generic(error.clone()),
            WrappedError::ExitCode(status) => WrappedError::ExitCode(*status),
            WrappedError::Cli(error) => {
                WrappedError::Cli(Box::new(error.clone_without_backtrace()))
            }
            error => WrappedError::Generic(error.to_string()),
        });

        Self {
            code: self.code,
            location: self.location.clone(),
            context: self.context.clone(),
            cause,
            backtrace: Backtrace::disabled(),
            jinja_frames: self.jinja_frames.clone(),
            next: self
                .next
                .as_deref()
                .map(|error| Box::new(error.clone_without_backtrace())),
        }
    }

    pub fn new_with_forced_backtrace(code: ErrorCode, context: impl Into<String>) -> Self {
        FsError {
            code,
            location: None,
            context: context.into(),
            cause: None,
            backtrace: Backtrace::force_capture(),
            jinja_frames: vec![],
            next: None,
        }
    }

    pub fn new_with_existing_backtrace(
        code: ErrorCode,
        context: impl Into<String>,
        backtrace: Backtrace,
    ) -> Self {
        FsError {
            code,
            location: None,
            context: context.into(),
            cause: None,
            backtrace,
            jinja_frames: vec![],
            next: None,
        }
    }

    /// Create a new [FsError] from a [FrontendError], with the given file and
    /// location offset.
    ///
    /// If the [FrontendError] contains multiple errors, they will all be
    /// converted and chained together.
    pub fn from_frontend_err(
        err: FrontendError,
        file: &Path,
        expanded_file: Option<&Path>,
        location_offset: dbt_frontend_common::error::CodeLocation,
        macro_spans: &[MacroSpan],
    ) -> Self {
        err.flatten()
            .into_iter()
            .map(|err| {
                let location = err
                    .location()
                    .with_offset(&location_offset)
                    .with_file(file)
                    .with_macro_spans(macro_spans, expanded_file.map(|x| Arc::new(x.into())));
                let cause = err.cause.map(|e| (*e).into());
                let context = match &cause {
                    // Delegate full message assembly to NameError so it can
                    // choose the right separator and suffix (e.g. cached-parquet
                    // schemas get a "refresh your cache" hint instead of
                    // "Available are ...").
                    Some(WrappedError::NameError(ne)) => ne.format_with_context(&err.context),
                    _ => err.context,
                };
                FsError {
                    code: err.code.into(),
                    location: Some(location),
                    context,
                    cause,
                    backtrace: err.backtrace,
                    jinja_frames: vec![],
                    next: None,
                }
            })
            .reduce(|acc, err| err.with_chained_errors(Box::new(acc)))
            .expect("at least one error")
    }

    pub fn from_jinja_err(err: minijinja::Error, context: impl Display) -> Self {
        Self::from_jinja_err_inner(err, context, None)
    }

    pub fn from_jinja_err_with_file_only_stack_frame(
        err: minijinja::Error,
        context: impl Display,
        path: &Path,
    ) -> Self {
        Self::from_jinja_err_inner(err, context, Some(path))
    }

    fn from_jinja_err_inner(
        err: minijinja::Error,
        context: impl Display,
        file_only_stack_frame: Option<&Path>,
    ) -> Self {
        if err.kind() == minijinja::ErrorKind::ExitWithStatus {
            if let Some(detail) = err.detail().filter(|d| !d.is_empty()) {
                return FsError::new_no_backtrace(
                    ErrorCode::JinjaWarnUpgradedToError,
                    detail.to_string(),
                );
            }
            return *FsError::exit_with_status(1);
        }

        let jinja_frames: Vec<super::CodeLocationWithFile> = err
            .stack()
            .iter()
            .map(|frame| {
                super::CodeLocationWithFile::new_with_arc(
                    frame.span.start_line,
                    frame.span.start_col,
                    frame.span.start_offset,
                    Arc::new(PathBuf::from(&frame.filename)),
                )
            })
            .collect();
        let file_only_location =
            file_only_stack_frame.and_then(|path| jinja_file_only_location(&err, path));

        // Check for AdapterError as source regardless of Jinja error kind.
        // Previously this was limited to ErrorKind::Execution, but adapter errors
        // can also surface as ErrorKind::InvalidOperation (e.g. from run_query calls),
        // so we check unconditionally by walking the source chain.
        if let Some(adapter_err) = find_adapter_error_in_chain(&err) {
            let mut fs_err = Box::<FsError>::from(adapter_err.clone());
            if !err.is_stack_empty() {
                let stack_trace = if let Some(path) = file_only_stack_frame {
                    StackTraceFormatter::with_file_only_stack_frame(&err, path).to_string()
                } else {
                    StackTraceFormatter::new(&err).to_string()
                };
                let mut frames = stack_trace.lines().filter(|s| !s.is_empty());
                // Always show the first frame (points to user code / compiled SQL)
                if let Some(first_frame) = frames.next() {
                    fs_err.context.push('\n');
                    fs_err.context.push_str(first_frame);
                }
                // Only include remaining internal macro frames in debug mode
                if is_sdf_debug() {
                    for frame in frames {
                        fs_err.context.push('\n');
                        fs_err.context.push_str(frame);
                    }
                }
            }
            let mut result = if let Some(location) = file_only_location {
                fs_err.with_file_location(location)
            } else {
                fs_err.with_location(MiniJinjaErrorWrapper(err))
            };
            result.jinja_frames = jinja_frames;
            return result;
        }
        let err_code = match err.kind() {
            minijinja::ErrorKind::SyntaxError => ErrorCode::MacroSyntaxInvalid,
            minijinja::ErrorKind::DisabledModel => ErrorCode::DisabledModel,
            minijinja::ErrorKind::Execution => ErrorCode::ExecutionError,
            _ => ErrorCode::JinjaError,
        };
        let mut result = FsError::new(
            err_code,
            format!(
                "{context} {}",
                format_jinja_err(&err, file_only_stack_frame)
            ),
        );
        result = if let Some(location) = file_only_location {
            result.with_file_location(location)
        } else {
            result.with_location(MiniJinjaErrorWrapper(err))
        };
        result.jinja_frames = jinja_frames;
        result
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

    /// Returns a pretty-printed version of this error, including the error code
    /// and file location as a suffix.
    pub fn pretty(&self) -> String {
        let mut s = format!("[{}]: {}", self.code.name_and_code(), self);
        if let Some(location) = &self.location {
            s.push_str(&format!("\n  --> {location}"));
        }
        if is_sdf_debug()
            && let Some(cause) = &self.cause
        {
            s.push_str(&format!("\n{:#?}", cause));
        }
        if let Some(backtrace) = self.get_backtrace() {
            s.push_str(&format!("\n{backtrace}"));
        }
        s
    }

    /// Returns the error message without the error code prefix.
    /// Includes file location and backtrace if present.
    /// This is used by tracing layers where the code prefix is added by formatters.
    pub fn message(&self) -> String {
        let mut s = self.to_string();
        if let Some(location) = &self.location {
            s.push_str(&format!("\n  --> {location}"));
        }
        if is_sdf_debug()
            && let Some(cause) = &self.cause
        {
            s.push_str(&format!("\n{:#?}", cause));
        }
        if let Some(backtrace) = self.get_backtrace() {
            s.push_str(&format!("\n{backtrace}"));
        }
        s
    }

    /// True if this error contains multiple errors.
    pub fn is_multiple_errors(&self) -> bool {
        self.next.is_some()
    }

    /// Returns the number of errors in this error chain.
    pub fn count(&self) -> usize {
        let mut count = 1;
        let mut cur = self;
        while let Some(e) = &cur.next {
            count += 1;
            cur = e;
        }
        count
    }

    /// Maps the location of this error to a pre-macro-expansion location, using
    /// the given macro spans and an optional path to the expanded file.
    ///
    /// If this error contains multiple errors, all of their locations will be
    /// mapped.
    pub fn with_macro_spans(
        mut self,
        macro_spans: &[MacroSpan],
        expanded_file: Option<impl Into<PathBuf>>,
    ) -> Self {
        if macro_spans.is_empty() {
            return self;
        }

        let expanded_file = expanded_file.map(|f| f.into());
        self.for_each_mut(|e| {
            let location = e.location.take().map(|loc| {
                loc.with_macro_spans(
                    macro_spans,
                    expanded_file.as_ref().map(|x| Arc::new(x.into())),
                )
            });
            e.location = location
        });
        self
    }

    /// Adds a cause to this error, replacing the existing cause if any
    ///
    /// Note: if you attach a cause to an error, make sure you don't format the
    /// cause into the [Self::context] for this error, as then the cause would
    /// be double printed when formatting this error.
    pub fn with_cause(self, cause: impl Into<WrappedError>) -> Self {
        FsError {
            cause: Some(cause.into()),
            ..self
        }
    }

    /// Adds a location to this error, replacing an existing location if it's more specific
    pub fn with_location(self, location: impl Into<super::CodeLocationWithFile>) -> FsError {
        let location = location.into();
        let location = if location.has_position() {
            location
        } else if self.location.is_some() && self.location.as_ref().unwrap().has_position() {
            // The existing location is more specific, so keep it
            self.location.unwrap()
        } else if let Some(wrapped_err) = &self.cause {
            // We can extract the line/column info from certain types of inner
            // error:
            match wrapped_err {
                WrappedError::Frontend(e) => e.location().with_arc_file(location.file),
                WrappedError::FrontendInternal(e) => {
                    if let Some(loc) = &e.location {
                        loc.with_arc_file(location.file)
                    } else {
                        location
                    }
                }
                // WrappedError::Jinja(e) => {
                //     if let Some(lineno) = e.line() {
                //         CodeLocation::new(lineno, 0, location.file)
                //     } else {
                //         location
                //     }
                // }
                _ => location,
            }
        } else {
            location
        };

        FsError {
            location: Some(location),
            ..self
        }
    }

    /// Adds a file-only location to this error, replacing any existing location.
    pub fn with_file_location(self, location: impl Into<super::CodeLocationWithFile>) -> FsError {
        let location = location.into();
        FsError {
            location: Some(super::CodeLocationWithFile::from(
                location.file.as_ref().clone(),
            )),
            ..self
        }
    }

    /// Hackity-hack location for YAML: find stuff by regex
    ///
    /// TODO: implement Span support in serde-yaml
    pub fn with_hacky_yml_location(
        self,
        location: Option<impl Into<super::CodeLocationWithFile>>,
    ) -> FsError {
        if location.is_none() {
            return self;
        }

        let location = location.unwrap().into();
        if location.has_position() {
            return self.with_location(location);
        }

        let Ok(in_dir) = std::env::var("SDF_IN_DIR").map(PathBuf::from) else {
            return self.with_location(location);
        };

        static RE_QUOTE: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"'([^']*)'").expect("valid regex"));
        static RE_BACKTICK: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r#"`([^`]*)`"#).expect("valid regex"));

        let inferred_loc = {
            let file = &location.file;
            let msg = self.to_string();
            let token = if msg.contains('\'') {
                find_enclosed_substring(&msg, &RE_QUOTE)
            } else if msg.contains('`') {
                find_enclosed_substring(&msg, &RE_BACKTICK)
            } else {
                None
            };

            if let Some(token) = token
                && in_dir.join(file.as_path()).exists()
            {
                // patch up trying to find the line/column of the token
                match crate::utils::find_locations(&token, Path::new(&in_dir.join(file.as_path())))
                {
                    Ok(Some((line, col, index))) => {
                        super::CodeLocationWithFile::new_with_arc(line, col, index, file.clone())
                    }
                    _ => location,
                }
            } else {
                location
            }
        };

        self.with_location(inferred_loc)
    }

    pub fn with_context(self, context: impl Into<String>) -> Self {
        FsError {
            context: context.into(),
            ..self
        }
    }

    pub fn with_code(self, code: ErrorCode) -> Self {
        FsError { code, ..self }
    }

    pub fn with_chained_errors(self, next: Box<FsError>) -> Self {
        let mut head = Box::new(self);
        let mut last = &mut head;
        while last.next.is_some() {
            last = last.next.as_mut().expect("last.next.is_some()");
        }
        last.next = Some(next);
        *head
    }

    /// Removes and returns the next error in a chain built with [`Self::with_chained_errors`].
    pub fn pop_next(&mut self) -> Option<Box<FsError>> {
        self.next.take()
    }

    /// Flattens multiple errors into a single vector.
    ///
    /// If this error is a single error, the result will be a vector with a
    /// single element, self. If this error contains multiple errors, the result
    /// will be a vector containing all errors in the chain, where each error is
    /// a single error.
    pub fn flatten(self) -> Vec<FsError> {
        let mut errors = vec![];
        let mut cur = self;
        loop {
            let mut next = cur.next.take();
            errors.push(cur);
            if let Some(e) = next.take() {
                cur = *e;
            } else {
                break;
            }
        }
        errors
    }

    /// Applies the given mutation to this error and all chained errors.
    pub fn for_each_mut<F>(&mut self, f: F)
    where
        F: Fn(&mut Self),
    {
        let mut cur = self;
        loop {
            f(cur);
            if let Some(e) = cur.next.as_mut() {
                cur = e;
            } else {
                break;
            }
        }
    }

    /// Applies the given function to this error and all chained errors.
    pub fn for_each<F>(&self, f: F)
    where
        F: Fn(&Self),
    {
        let mut cur = self;
        loop {
            f(cur);
            if let Some(e) = &cur.next {
                cur = e.as_ref();
            } else {
                break;
            }
        }
    }

    #[allow(dead_code)]
    /// Transforms this error into an [ErrContext].
    ///
    /// Panics if this error contains multiple errors.
    pub(crate) fn lower_to_context(self) -> ErrContext {
        assert!(
            !self.is_multiple_errors(),
            "cannot lower multiple errors to a single context"
        );

        ErrContext {
            code: Some(self.code),
            location: self.location,
            context: Some(self.context),
        }
    }

    /// Create an [FsError] that signals main() should exit with the given
    /// status code. The status code is stored in the cause as
    /// [WrappedError::ExitCode].
    #[cold]
    pub fn exit_with_status(status: i32) -> Box<Self> {
        let err = FsError {
            code: ErrorCode::ExitWithStatus,
            location: None,
            context: String::new(),
            cause: Some(WrappedError::ExitCode(status)),
            backtrace: Backtrace::capture(),
            jinja_frames: vec![],
            next: None,
        };
        Box::new(err)
    }

    /// If this error represents an exit-with-status request, returns the
    /// status code. Returns [None] for all other errors.
    pub fn exit_status(&self) -> Option<i32> {
        match (self.code, &self.cause) {
            (ErrorCode::ExitWithStatus, Some(WrappedError::ExitCode(status))) => Some(*status),
            (ErrorCode::ExitWithStatus, Some(_) | None) => {
                // Anything else if invalid...
                debug_assert!(
                    false,
                    "ExitWithStatus error should have an WrappedError::ExitCode cause; \
use FsError::exit_with_status() to properly construct these errors"
                );
                Some(1) // ...but we signal it as an error so the problem is more likely addressed.
            }
            (ErrorCode::ExitRepl, _) => {
                // ExitRepl is always a successful exit.
                Some(0)
            }
            (_, _) => None,
        }
    }

    pub fn with_relative_path(mut self, path: &str) -> Self {
        if let Some(ref mut location) = self.location {
            location.file = Arc::new(PathBuf::from(path));
        } else {
            self.location = Some(super::CodeLocationWithFile::new(
                1,
                1,
                0,
                PathBuf::from(path),
            ));
        }
        self
    }
}

#[derive(Debug)]
pub struct GenericNameError {
    target: String,
    available: Vec<String>,
}

impl GenericNameError {
    pub fn new(target: impl Into<String>, available: Vec<String>) -> Self {
        Self {
            target: target.into(),
            available,
        }
    }
}

#[derive(Debug)]
pub enum NameError {
    /// Error when looking up an alias name from a set of aliased expressions
    Aliases(dbt_frontend_common::error::AliasedExprsError),
    /// Error when looking up a column name from a schema
    Schema(dbt_frontend_common::error::SchemaError),
    /// Generic error when looking up a name from a set of names
    Generic(GenericNameError),
    /// Generic schema error originating from Datafusion
    Datafusion(datafusion_common::SchemaError),
}

impl Display for NameError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        let msg = match self {
            NameError::Aliases(e) => format_candidates(
                e.items()
                    .iter()
                    .filter_map(|e| match e {
                        Expr::Alias(alias) => Some(NameCandidate {
                            name: alias.name.to_owned().into(),
                            qualifier: None,
                        }),
                        _ => None,
                    })
                    .collect(),
                e.target(),
                Some(MAX_DISPLAY_TOKENS),
            ),
            NameError::Schema(e) => format_candidates(
                e.schemas()
                    .iter()
                    .flat_map(|s| s.iter().map(|f| f.into()))
                    .collect::<Vec<_>>(),
                e.target(),
                Some(MAX_DISPLAY_TOKENS),
            ),
            NameError::Datafusion(e) => e.to_string(),
            NameError::Generic(e) => format_candidates(
                e.available
                    .iter()
                    .map(|s| s.to_owned().into())
                    .collect::<Vec<_>>(),
                e.target.as_str(),
                Some(MAX_DISPLAY_TOKENS),
            ),
        };
        write!(f, "{msg}")
    }
}

impl NameError {
    /// Produces the full display string by combining the `FrontendError`
    /// context (e.g. "No column X found") with the appropriate suffix.
    ///
    /// For a cached-parquet schema error the suffix is joined directly (no
    /// period), yielding:
    ///   "No column X found in the locally cached schema for the source in
    ///     <path>
    ///    It is likely that this cache needs to be refreshe by running: dbt clean"
    ///
    /// For all other cases the existing ". Available are ..." suffix is used.
    pub fn format_with_context(&self, context: &str) -> String {
        match self {
            NameError::Schema(e) if e.parquet_path.is_some() => {
                let path = e.parquet_path.as_deref().unwrap();
                format!(
                    "{context} in the locally cached schema for the source in\n  {path}\n   It is likely that this cache needs to be refreshed by running: dbt clean"
                )
            }
            _ => {
                let suffix = self.to_string();
                if suffix.is_empty() {
                    context.to_string()
                } else {
                    format!("{context}. {suffix}")
                }
            }
        }
    }
}

/// Dynamically typed wrapper to allow propagating structured error info
///
/// A wrapped error can be any type that may provide potentially useful
/// debugging information. These are generally error types from third-party
/// libraries, such as Arrow, DataFusion, or Parquet, but can also be custom
/// error types from the sdf-cli or sdf-frontend crates, such as NameError.
///
/// Note: you don't have to add all third-party library error types here, only
/// those that may be useful for debugging or error handling. If not, then just
/// use the [Generic(String)] variant.
#[derive(Debug)]
#[non_exhaustive]
pub enum WrappedError {
    Antlr(String),
    Arrow(Box<arrow::error::ArrowError>),
    Parquet(Box<parquet::errors::ParquetError>),
    Datafusion(DataFusionError),
    Frontend(FrontendError),
    FrontendInternal(dbt_frontend_common::error::InternalError),
    // ObjectStore(object_store::Error),
    SerdeYml(dbt_yaml::Error),
    SerdeJson(serde_json::Error),
    NameError(NameError),
    Jinja(minijinja::Error),
    Adapter(super::adapter_errors::AdapterError),
    // Preprocessor(sdf_preprocessor::error::PreprocError),
    Io(io::Error),
    Fmt(fmt::Error),
    Generic(String),
    Cli(Box<FsError>),
    Reqwest(reqwest::Error),
    ExitCode(i32),
}

impl Display for WrappedError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            WrappedError::Antlr(e) => write!(f, "{e}"),
            WrappedError::Datafusion(e) => write!(f, "{e}"),
            WrappedError::Generic(e) => write!(f, "{e}"),
            WrappedError::Arrow(e) => write!(f, "{e}"),
            WrappedError::Frontend(e) => write!(f, "{e}"),
            WrappedError::Io(e) => write!(f, "{e}"),
            WrappedError::FrontendInternal(e) => write!(f, "{e}"),
            WrappedError::Cli(e) => write!(f, "{e}"),
            WrappedError::SerdeYml(e) => write!(f, "{e}"),
            WrappedError::Jinja(e) => write!(f, "{e}"),
            WrappedError::Adapter(e) => write!(f, "{e}"),
            // WrappedError::Preprocessor(e) => write!(f, "{}", e),
            WrappedError::SerdeJson(e) => write!(f, "{e}"),
            WrappedError::Parquet(e) => write!(f, "{e}"),
            // WrappedError::ObjectStore(e) => write!(f, "{}", e),
            WrappedError::NameError(e) => write!(f, "{e}"),
            WrappedError::Reqwest(e) => {
                write!(f, "{e}")?;
                let mut source = Error::source(e);
                while let Some(err) = source {
                    write!(f, ": {err}")?;
                    source = err.source();
                }
                Ok(())
            }
            WrappedError::Fmt(e) => write!(f, "{e}"),
            WrappedError::ExitCode(code) => write!(f, "exit code {code}"),
        }
    }
}

impl Error for WrappedError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            WrappedError::Datafusion(e) => Some(e),
            WrappedError::Arrow(e) => Some(e),
            WrappedError::Reqwest(e) => Some(e),
            _ => None,
        }
    }
}

impl From<dbt_frontend_common::error::WrappedError> for WrappedError {
    fn from(e: dbt_frontend_common::error::WrappedError) -> Self {
        match e {
            dbt_frontend_common::error::WrappedError::Frontend(err) => Self::Frontend(err),
            dbt_frontend_common::error::WrappedError::Antlr(err) => Self::Antlr(err.to_string()),
            dbt_frontend_common::error::WrappedError::Arrow(err) => Self::Arrow(err),
            dbt_frontend_common::error::WrappedError::Datafusion(err) => Self::Datafusion(err),
            dbt_frontend_common::error::WrappedError::SerdeJson(err) => Self::SerdeJson(err),
            dbt_frontend_common::error::WrappedError::ParseFloat(_)
            | dbt_frontend_common::error::WrappedError::ParseInt(_)
            | dbt_frontend_common::error::WrappedError::Generic(_) => Self::Generic(e.to_string()),
            dbt_frontend_common::error::WrappedError::Schema(err) => {
                Self::NameError(NameError::Schema(err))
            }
            dbt_frontend_common::error::WrappedError::AliasedExprs(err) => {
                Self::NameError(NameError::Aliases(err))
            }
            _ => Self::Generic(e.to_string()),
        }
    }
}

// --- Implicit conversions ---
// impl From<ANTLRError> for FsError {
//     fn from(e: ANTLRError) -> Self {
//         FsError::new(ErrorCode::default(), "Parsing failed")
//             .with_cause(WrappedError::Antlr(e.to_string()))
//     }
// }

// impl From<ANTLRError> for Box<FsError> {
//     fn from(e: ANTLRError) -> Self {
//         Box::new(e.into())
//     }
// }

// impl From<ANTLRError> for WrappedError {
//     fn from(e: ANTLRError) -> Self {
//         WrappedError::Antlr(e.to_string())
//     }
// }

impl From<CancelledError> for FsError {
    fn from(_: CancelledError) -> Self {
        FsError::new(ErrorCode::OperationCanceled, "Operation cancelled")
    }
}

impl From<CancelledError> for Box<FsError> {
    fn from(value: CancelledError) -> Self {
        Box::new(value.into())
    }
}

impl From<tokio::task::JoinError> for FsError {
    fn from(e: tokio::task::JoinError) -> Self {
        if e.is_cancelled() {
            FsError::new(ErrorCode::OperationCanceled, "Operation cancelled")
        } else if e.is_panic() {
            panic::resume_unwind(e.into_panic());
        } else {
            // as of today, this is unreachable, but we keep it for future-proofing
            FsError::new(ErrorCode::Unknown, format!("Join error: {e}"))
        }
    }
}

impl From<tokio::task::JoinError> for Box<FsError> {
    fn from(e: tokio::task::JoinError) -> Self {
        Box::new(e.into())
    }
}

impl From<dbt_runtime::JoinError> for FsError {
    fn from(e: dbt_runtime::JoinError) -> Self {
        if e.is_cancelled() {
            FsError::new(ErrorCode::OperationCanceled, "Operation cancelled")
        } else if e.is_panic() {
            panic::resume_unwind(e.into_panic());
        } else {
            // as of today, this is unreachable, but we keep it for future-proofing
            FsError::new(ErrorCode::Unknown, format!("Join error: {e}"))
        }
    }
}

impl From<dbt_runtime::JoinError> for Box<FsError> {
    fn from(e: dbt_runtime::JoinError) -> Self {
        Box::new(e.into())
    }
}

impl From<arrow::error::ArrowError> for FsError {
    fn from(e: arrow::error::ArrowError) -> Self {
        FsError::new(ErrorCode::ArrowError, "Arrow error")
            .with_cause(WrappedError::Arrow(Box::new(e)))
    }
}

impl From<arrow::error::ArrowError> for Box<FsError> {
    fn from(e: arrow::error::ArrowError) -> Self {
        Box::new(e.into())
    }
}

impl From<arrow::error::ArrowError> for WrappedError {
    fn from(e: arrow::error::ArrowError) -> Self {
        WrappedError::Arrow(Box::new(e))
    }
}

impl From<parquet::errors::ParquetError> for FsError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        FsError::new(ErrorCode::ParquetError, "Parquet error")
            .with_cause(WrappedError::Parquet(Box::new(e)))
    }
}

impl From<parquet::errors::ParquetError> for Box<FsError> {
    fn from(e: parquet::errors::ParquetError) -> Self {
        Box::new(e.into())
    }
}

impl From<parquet::errors::ParquetError> for WrappedError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        WrappedError::Parquet(Box::new(e))
    }
}

impl From<Box<dyn Error>> for Box<FsError> {
    fn from(value: Box<dyn Error>) -> Self {
        Box::new(FsError::new(ErrorCode::Generic, format!("{value}")))
    }
}
impl From<io::Error> for Box<FsError> {
    fn from(e: io::Error) -> Self {
        Box::new(FsError::new(ErrorCode::IoError, format!("{e}")).with_cause(WrappedError::Io(e)))
    }
}

// We cannot implement From<std::io::Error> for FsError because IO Error usually carries
// to little information.
impl<T> LiftableResult<T> for Result<T, io::Error> {
    fn expect_ok(self) -> FsResult<T> {
        self.map_err(|e| {
            FsError::new_with_forced_backtrace(
                ErrorCode::Unexpected,
                format!("Unexpected IO error: {e}"),
            )
            .with_cause(WrappedError::Io(e))
            .into()
        })
    }

    fn lift(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.map_err(|e| {
            let e =
                FsError::new(ErrorCode::IoError, format!("{e}")).with_cause(WrappedError::Io(e));
            let ctx = f();
            let e = if let Some(code) = ctx.code {
                e.with_code(code)
            } else {
                e
            };
            let e = if let Some(location) = ctx.location {
                e.with_location(location)
            } else {
                e
            };
            let e = if let Some(context) = ctx.context {
                let msg = e.context.clone();
                e.with_context(format!("{context}: {msg}"))
            } else {
                e
            };
            e.into()
        })
    }
}

impl From<io::Error> for WrappedError {
    fn from(e: io::Error) -> Self {
        WrappedError::Io(e)
    }
}

impl From<std::string::FromUtf8Error> for FsError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        FsError::new(ErrorCode::EncodingError, format!("Encoding error: {e}"))
    }
}

impl From<std::string::FromUtf8Error> for Box<FsError> {
    fn from(e: std::string::FromUtf8Error) -> Self {
        Box::new(e.into())
    }
}

impl From<std::string::FromUtf8Error> for WrappedError {
    fn from(e: std::string::FromUtf8Error) -> Self {
        WrappedError::Generic(e.to_string())
    }
}

impl From<dbt_yaml::Error> for WrappedError {
    fn from(e: dbt_yaml::Error) -> Self {
        WrappedError::SerdeYml(e)
    }
}

impl From<dbt_yaml::Error> for FsError {
    fn from(e: dbt_yaml::Error) -> Self {
        FsError::new(ErrorCode::YamlInvalid, "YAML error").with_cause(WrappedError::SerdeYml(e))
    }
}

impl From<dbt_yaml::Error> for Box<FsError> {
    fn from(e: dbt_yaml::Error) -> Self {
        Box::new(e.into())
    }
}

impl From<serde_json::Error> for FsError {
    fn from(e: serde_json::Error) -> Self {
        FsError::new(ErrorCode::JsonInvalid, "JSON error").with_cause(WrappedError::SerdeJson(e))
    }
}

impl From<serde_json::Error> for Box<FsError> {
    fn from(e: serde_json::Error) -> Self {
        Box::new(e.into())
    }
}

impl From<serde_json::Error> for WrappedError {
    fn from(e: serde_json::Error) -> Self {
        WrappedError::SerdeJson(e)
    }
}

// impl From<sdf_preprocessor::error::PreprocError> for FsError {
//     fn from(e: sdf_preprocessor::error::PreprocError) -> Self {
//         match e {
//             sdf_preprocessor::error::PreprocError::MacroSyntaxError {
//                 line,
//                 col,
//                 file,
//                 message,
//             } => FsError::new(ErrorCode::MacroSyntaxInvalid, message)
//                 .with_location(CodeLocation::new(line, col, file)),
//             sdf_preprocessor::error::PreprocError::Minijinja(e) => {
//                 FsError::new(ErrorCode::JinjaError, "Macro error")
//                     .with_cause(WrappedError::Jinja(e))
//             }
//             e => FsError::new(ErrorCode::JinjaError, "Preprocessor error")
//                 .with_cause(WrappedError::Preprocessor(e)),
//         }
//     }
// }

// impl From<sdf_preprocessor::error::PreprocError> for Box<FsError> {
//     fn from(e: sdf_preprocessor::error::PreprocError) -> Self {
//         Box::new(e.into())
//     }
// }

impl From<minijinja::Error> for WrappedError {
    fn from(e: minijinja::Error) -> Self {
        WrappedError::Jinja(e)
    }
}

impl From<super::adapter_errors::AdapterError> for WrappedError {
    fn from(e: super::adapter_errors::AdapterError) -> Self {
        WrappedError::Adapter(e)
    }
}

// impl From<sdf_preprocessor::error::PreprocError> for WrappedError {
//     fn from(e: sdf_preprocessor::error::PreprocError) -> Self {
//         WrappedError::Preprocessor(e)
//     }
// }

impl From<DataFusionError> for FsError {
    fn from(err: DataFusionError) -> Self {
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
                    .with_cause(WrappedError::Datafusion(err))
            }
            DataFusionError::ArrowError(ae, _) => {
                FsError::new(ErrorCode::ArrowError, "Arrow error")
                    .with_cause(WrappedError::Arrow(ae))
            }
            DataFusionError::ParquetError(pe) => {
                FsError::new(ErrorCode::ParquetError, "Parquet error")
                    .with_cause(WrappedError::Parquet(pe))
            }
            // DataFusionError::ObjectStore(oe) => {
            //     FsError::new(ErrorCode::ObjectStoreError, "Object store error")
            //         .with_cause(WrappedError::ObjectStore(oe))
            // }
            DataFusionError::IoError(ie) => {
                FsError::new(ErrorCode::IoError, "IO error").with_cause(WrappedError::Io(ie))
            }
            DataFusionError::Plan(s) => FsError::new(ErrorCode::LogicalPlanError, "Semantic error")
                .with_cause(WrappedError::Generic(s)),
            DataFusionError::SchemaError(se, _) => {
                FsError::new(ErrorCode::LogicalPlanError, "Schema error")
                    .with_cause(WrappedError::NameError(NameError::Datafusion(*se)))
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
            | DataFusionError::Shared(_) => {
                FsError::new(ErrorCode::GenericDatafusionError, "Datafusion error")
                    .with_cause(WrappedError::Datafusion(err))
            }
        }
    }
}

impl From<DataFusionError> for Box<FsError> {
    fn from(e: DataFusionError) -> Self {
        Box::new(e.into())
    }
}

// impl From<reqwest::Error> for Box<FsError> {
//     fn from(e: reqwest::Error) -> Self {
//         Box::new(e.into())
//     }
// }

impl From<DataFusionError> for WrappedError {
    fn from(e: DataFusionError) -> Self {
        WrappedError::Datafusion(e)
    }
}

impl From<GenericNameError> for WrappedError {
    fn from(e: GenericNameError) -> Self {
        WrappedError::NameError(NameError::Generic(e))
    }
}

impl From<FrontendError> for WrappedError {
    fn from(e: FrontendError) -> Self {
        WrappedError::Frontend(e)
    }
}

impl From<dbt_frontend_common::error::InternalError> for WrappedError {
    fn from(e: dbt_frontend_common::error::InternalError) -> Self {
        WrappedError::FrontendInternal(e)
    }
}

impl From<fmt::Error> for FsError {
    fn from(e: fmt::Error) -> Self {
        FsError::new(ErrorCode::FmtError, "Fmt error").with_cause(WrappedError::Fmt(e))
    }
}

impl From<fmt::Error> for Box<FsError> {
    fn from(e: fmt::Error) -> Self {
        Box::new(e.into())
    }
}

// impl From<sdf_connectors::error::ConnectorError> for FsError {
//     fn from(e: sdf_connectors::error::ConnectorError) -> Self {
//         FsError::new(ErrorCode::RemoteError, "Connector error")
//             .with_cause(WrappedError::Generic(e.to_string()))
//     }
// }

// impl From<sdf_connectors::error::ConnectorError> for Box<FsError> {
//     fn from(e: sdf_connectors::error::ConnectorError) -> Self {
//         Box::new(e.into())
//     }
// }

// --- Explicit conversions ---

pub trait LiftableResult<T>: private::Sealed {
    fn expect_ok(self) -> FsResult<T>;

    fn lift(self, f: impl FnOnce() -> ErrContext) -> FsResult<T>;
}

impl<T, E> LiftableResult<T> for FsResult<T, E>
where
    E: Into<FsError>,
{
    fn expect_ok(self) -> FsResult<T> {
        self.map_err(|e| {
            let e = e.into();
            FsError::new_with_forced_backtrace(
                ErrorCode::Unexpected,
                format!("Unexpected error: {e}"),
            )
            .with_cause(WrappedError::Cli(Box::new(e)))
            .into()
        })
    }

    fn lift(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.with_context(f)
    }
}

impl<T> LiftableResult<T> for dbt_frontend_common::error::InternalResult<T> {
    fn expect_ok(self) -> FsResult<T> {
        self.map_err(|e| {
            FsError::new_with_forced_backtrace(
                ErrorCode::Unexpected,
                format!("Unexpected internal error: {}", e.message()),
            )
            .with_cause(WrappedError::FrontendInternal(*e))
            .into()
        })
    }

    fn lift(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.map_err(|e| {
            let cause = WrappedError::Generic(e.message());
            let e = FsError::new_with_existing_backtrace(ErrorCode::Generic, "", e.backtrace)
                .with_cause(cause);
            let ctx = f();
            let e = if let Some(code) = ctx.code {
                e.with_code(code)
            } else {
                e
            };
            let e = if let Some(location) = ctx.location {
                e.with_location(location)
            } else {
                e
            };
            let e = if let Some(context) = ctx.context {
                let msg = e.context.clone();
                e.with_context(format!("{context}: {msg}"))
            } else {
                e
            };
            e.into()
        })
    }
}

impl<T> LiftableResult<T> for FrontendResult<T> {
    fn expect_ok(self) -> FsResult<T> {
        self.map_err(|e| {
            FsError::new_with_forced_backtrace(
                ErrorCode::Unexpected,
                format!("Unexpected frontend error: {}", e.message()),
            )
            .with_cause(WrappedError::Frontend(*e))
            .into()
        })
    }

    fn lift(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.map_err(|e| {
            let cause = WrappedError::Generic(e.to_string());
            let e = FsError::new_with_existing_backtrace(e.code.into(), "", e.backtrace)
                .with_cause(cause);
            let ctx = f();
            let e = if let Some(code) = ctx.code {
                e.with_code(code)
            } else {
                e
            };
            let e = if let Some(location) = ctx.location {
                e.with_location(location)
            } else {
                e
            };
            let e = if let Some(context) = ctx.context {
                let msg = e.context.clone();
                e.with_context(format!("{context}: {msg}"))
            } else {
                e
            };
            e.into()
        })
    }
}

pub trait ContextableResult<T>: private::Sealed {
    fn with_context(self, f: impl FnOnce() -> ErrContext) -> FsResult<T>;

    fn with_cause(self, cause: impl Into<WrappedError>) -> FsResult<T>;
}

#[derive(Debug, Clone)]
pub struct ErrContext {
    pub code: Option<ErrorCode>,
    pub location: Option<super::CodeLocationWithFile>,
    pub context: Option<String>,
}

impl<T, E> ContextableResult<T> for FsResult<T, E>
where
    E: Into<FsError>,
{
    fn with_context(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.map_err(|e| {
            let e = e.into();
            let ctx = f();
            let e = if let Some(code) = ctx.code {
                e.with_code(code)
            } else {
                e
            };
            let e = if let Some(location) = ctx.location {
                e.with_location(location)
            } else {
                e
            };
            let e = if let Some(context) = ctx.context {
                e.with_context(context)
            } else {
                e
            };
            e.into()
        })
    }

    fn with_cause(self, cause: impl Into<WrappedError>) -> FsResult<T> {
        self.map_err(|e| {
            let e = e.into();
            e.with_cause(cause).into()
        })
    }
}

impl<T> ContextableResult<T> for FsResult<T> {
    fn with_context(self, f: impl FnOnce() -> ErrContext) -> FsResult<T> {
        self.map_err(|e| {
            let e = *e;
            let ctx = f();
            let e = if let Some(code) = ctx.code {
                e.with_code(code)
            } else {
                e
            };
            let e = if let Some(location) = ctx.location {
                e.with_location(location)
            } else {
                e
            };
            let e = if let Some(context) = ctx.context {
                let mut e = e;
                // When adding context to an error, make sure to record the
                // original error as cause
                let cause = e
                    .cause
                    .take()
                    .unwrap_or(WrappedError::Generic(e.context.clone()));
                e.with_context(context).with_cause(cause)
            } else {
                e
            };
            e.into()
        })
    }

    fn with_cause(self, cause: impl Into<WrappedError>) -> FsResult<T> {
        self.map_err(|e| {
            let e = *e;
            e.with_cause(cause).into()
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

// Step 1. Delete this part and remove all uses of DataFusionResult
//
// This part allows "tunneling" InternalErrors through DataFusionError, thus
// allowing DataFusionResult to be used interchangeably with InternalResult

impl From<FsError> for DataFusionError {
    fn from(e: FsError) -> Self {
        match e.cause {
            Some(WrappedError::Datafusion(e)) => e,
            _ => DataFusionError::External(Box::new(e)),
        }
    }
}

impl From<Box<FsError>> for DataFusionError {
    fn from(e: Box<FsError>) -> Self {
        (*e).into()
    }
}

// impl From<sdf_auth::validate_credentials::CredentialError> for Box<FsError> {
//     fn from(e: sdf_auth::validate_credentials::CredentialError) -> Self {
//         Box::new(FsError::new(ErrorCode::RemoteError, e.to_string()))
//     }
// }

// --- End of !!FIXME!! ---

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        path::{Path, PathBuf},
    };

    use crate::{CodeLocationWithFile, ErrorCode};

    use super::*;

    #[test]
    fn with_file_location_discards_existing_position() {
        let err = FsError::new(ErrorCode::Generic, "test")
            .with_location(CodeLocationWithFile::new(1, 15, 14, "synthetic"))
            .with_file_location(PathBuf::from("run/test/models/test_fail.sql"));

        let location = err.location.expect("location");
        assert!(!location.has_position());
        assert_eq!(
            location.file.as_ref().as_path(),
            Path::new("run/test/models/test_fail.sql")
        );
    }

    #[test]
    fn from_jinja_err_with_file_only_stack_frame_demotes_matching_frame() {
        use minijinja::{
            Value,
            compiler::tokens::Span,
            constants::{CURRENT_PATH, CURRENT_SPAN},
        };

        let mut env = minijinja::Environment::new();
        env.add_template("run/test/models/test_fail.sql", "{{ 'a' + 1 }}")
            .expect("template should compile");
        let ctx = BTreeMap::from([
            (
                CURRENT_PATH.to_string(),
                Value::from("run/test/models/test_fail.sql"),
            ),
            (
                CURRENT_SPAN.to_string(),
                Value::from_serialize(Span::default()),
            ),
        ]);
        let err = env
            .get_template("run/test/models/test_fail.sql")
            .expect("template")
            .render(ctx, &[])
            .expect_err("render should fail");

        let err = FsError::from_jinja_err_with_file_only_stack_frame(
            err,
            "Failed to eval the compiled Jinja expression",
            Path::new("../scratch/target/run/test/models/test_fail.sql"),
        );

        assert!(
            err.context.contains("(in run/test/models/test_fail.sql)"),
            "{}",
            err.context
        );
        assert!(!err.context.contains("(in run/test/models/test_fail.sql:"));

        let location = err.location.expect("location");
        assert!(!location.has_position());
        assert_eq!(
            location.file.as_ref().as_path(),
            Path::new("run/test/models/test_fail.sql")
        );
    }

    #[tokio::test]
    async fn reqwest_wrapped_error_display_includes_source_chain() {
        // Bind then immediately drop a listener to obtain a local port that is
        // guaranteed to refuse connections, so the request fails fast and
        // deterministically without depending on external network conditions.
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind should succeed");
            listener
                .local_addr()
                .expect("local_addr should succeed")
                .port()
        };

        let client = reqwest::Client::new();

        let reqwest_err = client
            .get(format!("http://127.0.0.1:{port}"))
            .send()
            .await
            .expect_err("request to closed local port should fail");

        let wrapped = WrappedError::Reqwest(reqwest_err);
        let rendered = wrapped.to_string();

        assert!(
            rendered.contains(": "),
            "expected rendered error to include at least one appended source, got: {rendered}"
        );
    }
}

mod private {
    use super::*;

    pub trait Sealed {}

    impl Sealed for FsError {}

    impl<T, E> Sealed for FsResult<T, E> where E: Into<FsError> {}

    impl<T> Sealed for FsResult<T> {}

    impl<T> Sealed for FrontendResult<T> {}

    impl<T> Sealed for dbt_frontend_common::error::InternalResult<T> {}

    impl<T> Sealed for Result<T, io::Error> {}
}
