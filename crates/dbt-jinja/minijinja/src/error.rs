use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::{fmt, iter};

use crate::compiler::tokens::Span;
use crate::constants::{DBT_INTERNAL_PACKAGES_DIR_NAME, DBT_PACKAGES_DIR_NAME};

/// Represents template errors.
///
/// If debug mode is enabled a template error contains additional debug
/// information that can be displayed by formatting an error with the
/// alternative formatting (``format!("{:#}", err)``).  That information
/// is also shown for the [`Debug`] display where the extended information
/// is hidden when the alternative formatting is used.
///
/// Since MiniJinja takes advantage of chained errors it's recommended
/// to render the entire chain to better understand the causes.
///
/// # Example
///
/// Here is an example of how you might want to render errors:
///
/// ```rust
/// # use minijinja::listener::DefaultRenderingEventListener;
/// # use std::rc::Rc;
/// # let mut env = minijinja::Environment::new();
/// # env.add_template("", "");
/// # let template = env.get_template("").unwrap(); let ctx = ();
/// match template.render(ctx, &[Rc::new(DefaultRenderingEventListener::default())]) {
///     Ok(result) => println!("{}", result),
///     Err(err) => {
///         eprintln!("Could not render template: {:#}", err);
///         // render causes as well
///         let mut err = &err as &dyn std::error::Error;
///         while let Some(next_err) = err.source() {
///             eprintln!();
///             eprintln!("caused by: {:#}", next_err);
///             err = next_err;
///         }
///     }
/// }
/// ```
pub struct Error {
    repr: Box<ErrorRepr>,
}

#[derive(Debug, Clone)]
pub struct ErrorStackItem {
    pub filename: String,
    pub span: Span,
}

/// The internal error data
#[derive(Clone)]
struct ErrorRepr {
    kind: ErrorKind,
    detail: Option<Cow<'static, str>>,
    stack: Vec<ErrorStackItem>,
    source: Option<Arc<dyn std::error::Error + Send + Sync>>,
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut err = f.debug_struct("Error");
        err.field("kind", &self.kind());
        if let Some(ref detail) = self.repr.detail {
            err.field("detail", detail);
        }
        if let Some(ref name) = self.name() {
            err.field("name", name);
        }
        if let Some(ref source) = std::error::Error::source(self) {
            err.field("source", source);
        }
        ok!(err.finish());

        Ok(())
    }
}

/// An enum describing the error kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ErrorKind {
    /// A non primitive value was encountered where one was expected.
    NonPrimitive,
    /// A value is not valid for a key in a map.
    NonKey,
    /// An invalid operation was attempted.
    InvalidOperation,
    /// The template has a syntax error
    SyntaxError,
    /// A template was not found.
    TemplateNotFound,
    /// Too many arguments were passed to a function.
    TooManyArguments,
    /// A expected argument was missing
    MissingArgument,
    /// Invalid Argument
    InvalidArgument,
    /// A filter is unknown
    UnknownFilter,
    /// A test is unknown
    UnknownTest,
    /// A function is unknown
    UnknownFunction,
    /// Un unknown method was called
    UnknownMethod,
    /// A bad escape sequence in a string was encountered.
    BadEscape,
    /// An operation on an undefined value was attempted.
    UndefinedError,
    /// Not able to serialize this value.
    BadSerialization,
    /// Not able to deserialize this value.
    #[cfg(feature = "deserialization")]
    CannotDeserialize,
    /// An error happened in an include.
    BadInclude,
    /// An error happened in a super block.
    EvalBlock,
    /// Unable to unpack a value.
    CannotUnpack,
    /// Failed writing output.
    WriteFailure,
    /// Engine ran out of fuel
    #[cfg(feature = "fuel")]
    OutOfFuel,
    #[cfg(feature = "custom_syntax")]
    /// Error creating aho-corasick delimiters
    InvalidDelimiter,
    /// An unknown block was called
    #[cfg(feature = "multi_template")]
    UnknownBlock,
    /// Return value
    ReturnValue,
    /// Suspend vm
    SuspendVm,
    /// EnvVarMissingError
    EnvVarMissingError,
    /// SecretEnvVarLocationError
    SecretEnvVarLocationError,
    /// MacroResultAlreadyLoadedError
    MacroResultAlreadyLoadedError,
    /// DeserializeError
    SerdeDeserializeError,
    /// RegexError
    RegexError,
    /// Error for when a disabled model is encountered
    DisabledModel,
    /// Error for typecheck
    TypeError,
    /// Execution error
    Execution,
    /// Exit with status without producing an extra dbt error log.
    ExitWithStatus,
}

impl ErrorKind {
    /// Returns a plain text description of the error kind.
    ///
    /// **NOTE**: Do *not* make this function return a `String`!
    /// Context-specific error messages should be provided in the `details`
    /// field of [Error] objects (by using [Error::new], or
    /// [Error::set_detail]), not here.
    fn description(&self) -> &'static str {
        match self {
            ErrorKind::NonPrimitive => "not a primitive",
            ErrorKind::NonKey => "not a key type",
            ErrorKind::InvalidOperation => "invalid operation",
            ErrorKind::SyntaxError => "syntax error",
            ErrorKind::TemplateNotFound => "template not found",
            ErrorKind::TooManyArguments => "too many arguments",
            ErrorKind::MissingArgument => "missing argument",
            ErrorKind::UnknownFilter => "unknown filter",
            ErrorKind::UnknownFunction => "unknown function",
            ErrorKind::UnknownTest => "unknown test",
            ErrorKind::UnknownMethod => "unknown method",
            ErrorKind::BadEscape => "bad string escape",
            ErrorKind::UndefinedError => "undefined value",
            ErrorKind::BadSerialization => "could not serialize to value",
            ErrorKind::BadInclude => "could not render include",
            ErrorKind::EvalBlock => "could not render block",
            ErrorKind::CannotUnpack => "cannot unpack",
            ErrorKind::WriteFailure => "failed to write output",
            #[cfg(feature = "deserialization")]
            ErrorKind::CannotDeserialize => "cannot deserialize",
            #[cfg(feature = "fuel")]
            ErrorKind::OutOfFuel => "engine ran out of fuel",
            #[cfg(feature = "custom_syntax")]
            ErrorKind::InvalidDelimiter => "invalid custom delimiters",
            #[cfg(feature = "multi_template")]
            ErrorKind::UnknownBlock => "unknown block",
            ErrorKind::ReturnValue => "return value",
            ErrorKind::SuspendVm => "suspend vm",
            ErrorKind::EnvVarMissingError => "env var required but not provided",
            ErrorKind::SecretEnvVarLocationError => {
                "secret env vars are allowed only in profiles.yml or packages.yml."
            }
            ErrorKind::InvalidArgument => "invalid argument",
            ErrorKind::MacroResultAlreadyLoadedError => "macro result already loaded",
            ErrorKind::SerdeDeserializeError => "could not deserialize",
            ErrorKind::RegexError => "regex error",
            ErrorKind::DisabledModel => "model is disabled",
            ErrorKind::TypeError => "type error",
            ErrorKind::Execution => "execution error",
            ErrorKind::ExitWithStatus => "exit with status",
        }
    }
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ref detail) = self.repr.detail {
            ok!(write!(f, "{}: {}", self.kind(), detail));
        } else {
            ok!(write!(f, "{}", self.kind()));
        }
        self.stack_trace(f)
    }
}

impl Error {
    /// Creates a new error with kind and detail.
    pub fn new<D: Into<Cow<'static, str>>>(kind: ErrorKind, detail: D) -> Error {
        Error {
            repr: Box::new(ErrorRepr {
                kind,
                detail: Some(detail.into()),
                stack: Vec::new(),
                source: None,
            }),
        }
    }

    /// Writes out the stack trace for an error if the stack is not empty
    pub fn stack_trace(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.is_stack_empty() {
            ok!(write!(
                f,
                "{}",
                self.repr
                    .stack
                    .iter()
                    .rev()
                    .map(|x| format!(
                        "\n(in {}:{}:{})",
                        x.filename, x.span.start_line, x.span.start_col
                    ))
                    .collect::<Vec<_>>()
                    .join("")
            ));
        }
        Ok(())
    }

    pub(crate) fn internal_clone(&self) -> Error {
        Error {
            repr: self.repr.clone(),
        }
    }

    pub(crate) fn new_not_found(name: &str) -> Error {
        Error::new(
            ErrorKind::TemplateNotFound,
            format!("template {name:?} does not exist"),
        )
    }

    /// Attaches another error as source to this error.
    pub fn with_source<E: std::error::Error + Send + Sync + 'static>(mut self, source: E) -> Self {
        self.repr.source = Some(Arc::new(source));
        self
    }

    /// Returns the error kind
    pub fn kind(&self) -> ErrorKind {
        self.repr.kind
    }

    /// Returns the error detail
    ///
    /// The detail is an error message that provides further details about
    /// the error kind.
    pub fn detail(&self) -> Option<&str> {
        self.repr.detail.as_deref()
    }

    /// Overrides the detail.
    pub(crate) fn set_detail<D: Into<Cow<'static, str>>>(&mut self, d: D) {
        self.repr.detail = Some(d.into());
    }

    /// Returns the filename of the template that caused the error.
    pub fn name(&self) -> Option<&str> {
        if self.repr.stack.is_empty() {
            None
        } else {
            Some(&self.repr.stack.last().unwrap().filename)
        }
    }

    /// Return the significant name of the error.
    pub fn significant_name(&self) -> Option<&str> {
        self.repr
            .stack
            .iter()
            .find_map(|err| {
                // check if any component of the filename contains dbt_internal_packages
                let filename = PathBuf::from(err.filename.as_str());
                if !filename.components().any(|component| {
                    let component = component.as_os_str().to_string_lossy();

                    component.contains(DBT_INTERNAL_PACKAGES_DIR_NAME)
                        || component.contains(DBT_PACKAGES_DIR_NAME)
                }) {
                    Some(err.filename.as_ref())
                } else {
                    None
                }
            })
            .or_else(|| self.repr.stack.last().map(|x| x.filename.as_str()))
    }

    /// Returns if the stack is empty.
    pub fn is_stack_empty(&self) -> bool {
        self.repr.stack.is_empty()
    }

    /// Returns all stack frames, innermost first.
    pub fn stack(&self) -> &[ErrorStackItem] {
        self.repr.stack.as_slice()
    }

    /// Returns the line number where the error occurred.
    pub fn span(&self) -> Option<Span> {
        self.repr.stack.last().map(|x| x.span)
    }

    /// Returns the significant span of the error.
    pub fn significant_span(&self) -> Option<Span> {
        self.repr
            .stack
            .iter()
            .find_map(|err| {
                let filename = PathBuf::from(err.filename.as_str());
                if !filename.components().any(|component| {
                    let component = component.as_os_str().to_string_lossy();
                    component.contains(DBT_INTERNAL_PACKAGES_DIR_NAME)
                        || component.contains(DBT_PACKAGES_DIR_NAME)
                }) {
                    Some(err.span)
                } else {
                    None
                }
            })
            .or_else(|| self.repr.stack.last().map(|x| x.span))
    }

    /// Returns the byte range of where the error occurred if available.
    ///
    /// In combination with [`template_source`](Self::template_source) this can be
    /// used to better visualize where the error is coming from.  By indexing into
    /// the template source one ends up with the source of the failing expression.
    ///
    /// Note that debug mode ([`Environment::set_debug`](crate::Environment::set_debug))
    /// needs to be enabled, and the `debug` feature must be turned on.  The engine
    /// usually keeps track of spans in all cases, but there is no absolute guarantee
    /// that it is able to provide a range in all error cases.
    ///
    /// ```
    /// # use minijinja::{Error, Environment, context, listener::DefaultRenderingEventListener};
    /// # use std::rc::Rc;
    /// # let mut env = Environment::new();
    /// # env.set_debug(true);
    /// let tmpl = env.template_from_str("Hello {{ foo + bar }}!").unwrap();
    /// let err = tmpl.render(context!(foo => "a string", bar => 0), &[Rc::new(DefaultRenderingEventListener::default())]).unwrap_err();
    /// let src = err.template_source().unwrap();
    /// assert_eq!(&src[err.range().unwrap()], "{{ foo + bar }}");
    /// ```
    #[cfg(feature = "debug")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debug")))]
    pub fn range(&self) -> Option<std::ops::Range<usize>> {
        self.span()
            .map(|x| x.start_offset as usize..x.end_offset as usize)
    }

    pub(crate) fn with_span(&self, file_path: &Path, span: &Span) -> Error {
        Error {
            repr: Box::new(ErrorRepr {
                stack: self
                    .repr
                    .stack
                    .iter()
                    .cloned()
                    .chain(iter::once(ErrorStackItem {
                        filename: file_path.to_string_lossy().to_string(),
                        span: *span,
                    }))
                    .collect(),
                ..*self.repr.clone()
            }),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.repr.source.as_ref().map(|err| err.as_ref() as _)
    }
}

impl From<ErrorKind> for Error {
    /// Constructs a default Error instance from an `ErrorKind`.
    ///
    /// Note: This does not take a detail message, so the error will not have
    /// any context-specific information. If you need to provide a detail
    /// message, use `Error::new` instead.
    fn from(kind: ErrorKind) -> Self {
        Error {
            repr: Box::new(ErrorRepr {
                kind,
                detail: None,
                stack: Vec::new(),
                source: None,
            }),
        }
    }
}

impl From<fmt::Error> for Error {
    fn from(_: fmt::Error) -> Self {
        Error::new(ErrorKind::WriteFailure, "formatting failed")
    }
}
