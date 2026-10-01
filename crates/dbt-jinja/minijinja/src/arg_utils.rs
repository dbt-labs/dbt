use std::{borrow::Cow, cell::Cell, collections::BTreeMap};

use crate::{
    value::{
        argtypes::type_name_suffix, mutable_map::MutableMap, value_map_with_capacity, ArgType,
        Kwargs, ValueKind, ValueMap,
    },
    Value,
};

/// Report invalid argument error (returns a minijinja::Error)
#[macro_export]
macro_rules! invalid_argument_inner {
    ($msg:expr) => {
        $crate::Error::new($crate::ErrorKind::InvalidArgument, $msg)
    };

    ($($arg:tt)*) => {
        $crate::Error::new($crate::ErrorKind::InvalidArgument, format!($($arg)*))
    };
}

/// Report invalid argument error (returns an Err wrapped minijinja::Error)
#[macro_export]
macro_rules! invalid_argument {
    ($msg:expr) => {
        Err(invalid_argument_inner!($msg))
    };

    ($($arg:tt)*) => {
        Err(invalid_argument_inner!($($arg)*))
    };
}

/// Report missing argument error
#[macro_export]
macro_rules! missing_argument {
    ($msg:expr) => {
        Err($crate::Error::new($crate::ErrorKind::MissingArgument, $msg))
    };

    ($($arg:tt)*) => {
        Err($crate::Error::new($crate::ErrorKind::MissingArgument, format!($($arg)*)))
    };
}

/// Report too many arguments error
#[macro_export]
macro_rules! too_many_arguments {
    ($msg:expr) => {
        Err($crate::Error::new($crate::ErrorKind::TooManyArguments, $msg))
    };

    ($($arg:tt)*) => {
        Err($crate::Error::new($crate::ErrorKind::TooManyArguments, format!($($arg)*)))
    };
}

/// Util function to check that the number of arguments known to the
/// parser is within the given bounds.
pub fn check_num_args(
    func_name: impl Into<String>,
    parser: &ArgParser,
    min: usize,
    max: usize,
) -> Result<(), crate::Error> {
    let num_args = parser.positional_len() + parser.kwargs_len();

    if num_args < min {
        Err(crate::Error::new(
            crate::ErrorKind::MissingArgument,
            format!("{} requires {}..{} argument(s)", func_name.into(), min, max),
        ))
    } else if num_args > max {
        Err(crate::Error::new(
            crate::ErrorKind::TooManyArguments,
            format!("{} requires {}..{} argument(s)", func_name.into(), min, max),
        ))
    } else {
        Ok(())
    }
}

/// Struct for parsing arguments for Jinja macros
#[derive(Debug)]
pub struct ArgParser {
    positional: Vec<Value>,
    kwargs: BTreeMap<String, Value>,
}

/// Implementation of the ArgParser struct
impl ArgParser {
    /// Create a new ArgParser instance that handles both mixed args and separate kwargs
    pub fn new(args: &[Value], kwargs: Option<Kwargs>) -> Self {
        let mut parser = ArgParser {
            positional: Vec::new(),
            kwargs: BTreeMap::new(),
        };

        // First process any kwargs provided separately
        if let Some(kw) = kwargs {
            for key in kw.args() {
                parser
                    .kwargs
                    .insert(key.to_string(), kw.get::<Value>(key).unwrap().clone());
            }
        }

        // Then process args, which might contain more kwargs
        for arg in args.iter() {
            if arg.is_kwargs() {
                if let Some(map) = arg.as_object() {
                    if let Some(iter) = map.try_iter() {
                        for key in iter {
                            if let Some(value) = map.get_value(&key) {
                                // Later kwargs override earlier ones
                                parser.kwargs.insert(
                                    key.as_str().unwrap_or_default().to_string(),
                                    value.clone(),
                                );
                            }
                        }
                    }
                }
            } else {
                parser.positional.push(arg.clone());
            }
        }

        parser
    }

    /// Get and consume a value by name or next positional argument
    pub fn get<T>(&mut self, name: &str) -> Result<T, crate::Error>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        // First check kwargs
        if let Some(value) = self.kwargs.remove(name) {
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert argument '{name}': {e}"),
                )
            });
        }

        // Then take the next positional argument
        // TODO: this try_from conversion is troublesome; we need to verify value.is_none() and return an Option accordingly
        // For example if None is used in the template,
        // this method returns "none" string literal if it's invoked as get<String>(...)
        if let Some(value) = self.positional.first().cloned() {
            self.positional.remove(0);
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert next positional argument: {e}"),
                )
            });
        }

        Err(crate::Error::new(
            crate::ErrorKind::InvalidOperation,
            format!("Required argument '{name}' not provided"),
        ))
    }

    /// Get and consume the next positional argument
    pub fn next_positional<T>(&mut self) -> Result<T, crate::Error>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        if let Some(value) = self.positional.first().cloned() {
            self.positional.remove(0);
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert next positional argument: {e}"),
                )
            });
        }
        Err(crate::Error::new(
            crate::ErrorKind::InvalidOperation,
            "No positional arguments left".to_string(),
        ))
    }

    /// Get and consume an optional value by name or next positional argument
    pub fn get_optional<T>(&mut self, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        self.get(name).ok()
    }

    /// Get and consume a value by either of two names or next positional argument
    pub fn get_either<T>(&mut self, name1: &str, name2: &str) -> Result<T, crate::Error>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        // First check kwargs for name1
        if let Some(value) = self.kwargs.remove(name1) {
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert argument '{name1}': {e}"),
                )
            });
        }

        // Then check kwargs for name2
        if let Some(value) = self.kwargs.remove(name2) {
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert argument '{name2}': {e}"),
                )
            });
        }

        // Finally check positional args
        if let Some(value) = self.positional.first().cloned() {
            self.positional.remove(0);
            return T::try_from(value).map_err(|e| {
                crate::Error::new(
                    crate::ErrorKind::InvalidOperation,
                    format!("Failed to convert next positional argument: {e}"),
                )
            });
        }

        Err(crate::Error::new(
            crate::ErrorKind::InvalidOperation,
            format!("Required argument '{name1}' or '{name2}' not provided"),
        ))
    }

    /// Get and consume an optional value by either of two names or next positional argument
    pub fn get_optional_either<T>(&mut self, name1: &str, name2: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        self.get_either(name1, name2).ok()
    }

    /// Check if a named argument exists
    pub fn has_kwarg(&self, name: &str) -> bool {
        self.kwargs.contains_key(name)
    }

    /// Get the number of positional arguments
    pub fn positional_len(&self) -> usize {
        self.positional.len()
    }

    /// Get all kwargs
    pub fn drain_kwargs(&mut self) -> BTreeMap<String, Value> {
        // Drain the kwargs and return them from BTreeMap
        let mut drained = BTreeMap::new();
        std::mem::swap(&mut self.kwargs, &mut drained);
        drained
    }

    /// iterate over all of the kwargs
    pub fn kwargs_iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.kwargs.iter()
    }

    /// number of kwargs
    pub fn kwargs_len(&self) -> usize {
        self.kwargs.len()
    }

    /// Return the remaining positional arguments
    pub fn get_args_as_vec_of_values(&self) -> Vec<Value> {
        self.positional.clone()
    }

    /// Return the remaining kwargs as a ValueMap
    pub fn get_kwargs_as_value_map(&self) -> ValueMap {
        let mut value_map = value_map_with_capacity(self.kwargs.len());
        for (key, value) in self.kwargs.clone() {
            value_map.insert(Value::from(key), value);
        }
        value_map
    }

    /// Return the remaining kwargs as a MutableMap (supports .pop() and other mutating methods)
    pub fn get_kwargs_as_mutable_map(&mut self) -> MutableMap {
        let mutable_map = MutableMap::new();
        // Use std::mem::take to move the kwargs out, avoiding clones
        for (key, value) in std::mem::take(&mut self.kwargs) {
            mutable_map.insert(Value::from(key), value);
        }
        mutable_map
    }

    /// Get and consume an optional value by name from kwargs
    pub fn consume_optional_only_from_kwargs<T>(&mut self, name: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        // Then check kwargs for name
        if let Some(value) = self.kwargs.remove(name) {
            return if !value.is_none() {
                T::try_from(value).ok()
            } else {
                None
            };
        }
        None
    }

    /// Get and consume an optional value by either of two names from kwargs
    pub fn consume_optional_either_from_kwargs<T>(&mut self, name1: &str, name2: &str) -> Option<T>
    where
        T: TryFrom<Value>,
        T::Error: std::fmt::Display,
    {
        self.consume_optional_only_from_kwargs(name1)
            .or_else(|| self.consume_optional_only_from_kwargs(name2))
    }

    /// Validate the number of arguments for a method call
    pub fn check_num_args(
        &self,
        func_name: impl Into<String>,
        min: usize,
        max: usize,
    ) -> Result<(), crate::Error> {
        let num_args = self.positional_len() + self.kwargs_len();
        let err_msg = format!("{} requires {}..{} argument(s)", func_name.into(), min, max);

        // TODO: migrate Jinja Error related macros from dbt-adapter/src/macros.rs here
        if num_args < min {
            Err(crate::Error::new(
                crate::ErrorKind::InvalidArgument,
                err_msg,
            ))
        } else if num_args > max {
            Err(crate::Error::new(
                crate::ErrorKind::TooManyArguments,
                err_msg,
            ))
        } else {
            Ok(())
        }
    }

    /// Assert that all arguments have been consumed
    pub fn assert_all_used(&self) -> Result<(), crate::Error> {
        if self.positional_len() > 0 || !self.kwargs.is_empty() {
            Err(crate::Error::new(
                crate::ErrorKind::TooManyArguments,
                "Too many positional arguments".to_string(),
            ))
        } else {
            Ok(())
        }
    }
}

enum PosParams<'a> {
    Named(&'a [&'a str]),
    Unnamed(usize),
}

impl PosParams<'_> {
    pub fn get(&self, idx: usize) -> Option<&str> {
        match self {
            PosParams::Named(params) => Some(params[idx]),
            PosParams::Unnamed(num_pos_params) => {
                debug_assert!(idx < *num_pos_params);
                None
            }
        }
    }

    /// Get the number of positional parameters.
    pub fn len(&self) -> usize {
        match self {
            PosParams::Named(params) => params.len(),
            PosParams::Unnamed(len) => *len,
        }
    }

    pub fn are_named(&self) -> bool {
        matches!(self, PosParams::Named(_))
    }
}

/// A zero-copy, streaming parser of arguments from a `&[Value]` slice.
///
/// NOTE(felipecrv): this is experimental and not part of the original minijinja
/// codebase. It may change in the future.
pub struct ArgsIter<'a> {
    fn_name: &'a str,
    /// The required positional parameters of the function.
    pos_params: PosParams<'a>,
    args: &'a [Value],
    /// The number of provided positional arguments.
    num_pos_args: usize,
    index: Cell<usize>,
    kwargs: Kwargs,
}

impl<'a> ArgsIter<'a> {
    /// Create a new `ArgsIter` from a slice of `Value`s.
    ///
    /// PRECONDITION: if one of the args `.is_kwargs()`, it must be the last argument in the slice.
    pub fn new(fn_name: &'a str, pos_params: &'a [&'a str], args: &'a [Value]) -> ArgsIter<'a> {
        Self::_new(fn_name, PosParams::Named(pos_params), args)
    }

    /// Create a parser for functions with unnamed positional parameters.
    ///
    /// This is the case for built-in functions like `len`, `sum`, etc. The error messages
    /// for these will not include the parameter names, but will still indicate the number of
    /// positional parameters expected. For example, when you call `len()` with no arguments:
    ///
    /// ```text
    ///     len() takes exactly one argument (0 given)
    /// ```
    pub fn for_unnamed_pos_args(
        fn_name: &'a str,
        num_pos_params: usize,
        args: &'a [Value],
    ) -> ArgsIter<'a> {
        Self::_new(fn_name, PosParams::Unnamed(num_pos_params), args)
    }

    /// Create a parser for functions with no positional parameters.
    ///
    /// Calling `finish()?` will return an error if any arguments are provided.
    pub fn nullary(fn_name: &'a str, args: &'a [Value]) -> ArgsIter<'a> {
        Self::_new(fn_name, PosParams::Unnamed(0), args)
    }

    fn _new(fn_name: &'a str, pos_params: PosParams<'a>, args: &'a [Value]) -> ArgsIter<'a> {
        let (num_pos_args, kwargs) = Self::_extract_kwargs(args);
        ArgsIter {
            fn_name,
            pos_params,
            args,
            num_pos_args,
            index: Cell::new(0),
            kwargs,
        }
    }

    #[inline(never)]
    fn _extract_kwargs(args: &'a [Value]) -> (usize, Kwargs) {
        let kwargs = args.last().and_then(Kwargs::extract);
        let num_pos_args = args.len()
            - match kwargs {
                Some(_) => 1,
                None => 0,
            };
        (num_pos_args, kwargs.unwrap_or_default())
    }

    /// Get the next (required) positional argument.
    pub fn next_arg<T>(&'a self) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        self._next_arg_impl::<T>(&[])
    }

    /// Get the next (required) positional argument allowing for different names
    /// if it's passed as a kwarg.
    ///
    /// This is useful when a single Rust function is implementing multiple Python
    /// functions (e.g. multiple overloads of a method) that have different names
    /// for the same positional parameter.
    ///
    /// ```python
    ///     def information_schema(self, identifier)
    ///     // and
    ///     def information_schema(self, view_name)
    /// ```
    ///
    /// A single Rust function replacing both can be written as:
    ///
    /// ```rust
    /// # use minijinja::{Error, Value};
    /// # use minijinja::arg_utils::ArgsIter;
    /// fn information_schema(args: &[Value]) -> Result<String, Error> {
    ///    let iter = ArgsIter::new("information_schema", &["identifier"], args);
    ///    let identifier = iter.next_pos_arg_aliased::<&str>(&["view_name"])?;
    ///    iter.finish()?;
    ///    // ...use identifier here...
    ///    # Ok(String::new())
    /// }
    /// ```
    pub fn next_pos_arg_aliased<T>(&'a self, aliases: &[&'a str]) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        self._next_arg_impl::<T>(aliases)
    }

    fn _next_arg_impl<T>(&'a self, aliases: &[&'a str]) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        if self.index.get() >= self.pos_params.len() {
            unreachable!(
                "next_arg() called more than the number of positional parameters: {}",
                self.pos_params.len()
            )
        }

        let arg = self._next();
        let idx = self.index.get() - 1;
        let name = self.pos_params.get(idx);
        if arg.is_some() {
            let rv = T::from_value(arg)
                .map_err(|err| self._detail_from_value_err(Some(idx), name, arg, err))?;
            self._ensure_pos_arg_not_in_kwargs(idx)?;
            Ok(rv)
        } else {
            // positional arguments have been consumed,
            // so we check if it was passed as a kwarg
            self._get_pos_arg_from_kwargs::<T>(idx, aliases)
        }
    }

    fn _get_pos_arg_from_kwargs<T>(
        &'a self,
        idx: usize,
        aliases: &[&'a str],
    ) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        let name = match self.pos_params.get(idx) {
            Some(name) => name,
            None => return Err(self._missing_pos_arg(idx, aliases)),
        };
        self.kwargs
            .get::<'a, T>(name)
            .map_err(|err| {
                if err.kind() == crate::ErrorKind::MissingArgument {
                    // Missing kwarg would be the wrong diagnostic here,
                    // so we produce a better one.
                    self._missing_pos_arg(idx, aliases)
                } else {
                    // peek() the existing value (because it's not a missing argument error) as a
                    // [Value] to render the name of the type we got where T was expected instead.
                    let value = self.kwargs.peek::<Option<&'a Value>>(name).ok().flatten();
                    self._detail_from_value_err(None, Some(name), value, err)
                }
            })
            .or_else(|err| {
                for name in aliases {
                    match self.kwargs.peek::<Option<&'a Value>>(name) {
                        Ok(value @ Some(_)) => {
                            return self.kwargs.get::<'a, T>(name).map_err(|err| {
                                // Render the name of the type we got in the peeked [Value]
                                // to render the type we got instead of T.
                                self._detail_from_value_err(None, Some(name), value, err)
                            });
                        }
                        Ok(None) | Err(_) => continue,
                    }
                }
                Err(err)
            })
    }

    /// Get the next kwarg at the current iterator position.
    ///
    /// ```python
    ///     def f(x, y, alpha=None, beta=None)
    /// ```
    ///
    /// ```rust
    /// # use minijinja::{Error, Value};
    /// # use minijinja::arg_utils::ArgsIter;
    /// fn f(args: &[Value]) -> Result<(), Error> {
    ///     let iter = ArgsIter::new("f", &["x", "y"], args);
    ///     let x = iter.next_arg::<&Value>()?;
    ///     let y = iter.next_arg::<&Value>()?;
    ///     let alpha = iter.next_kwarg::<Option<&Value>>("alpha")?;
    ///     let beta = iter.next_kwarg::<Option<&Value>>("beta")?;
    ///     iter.finish()?;
    ///     // ...use x, y, alpha, beta here...
    ///     Ok(())
    /// }
    /// ```
    pub fn next_kwarg<T>(&'a self, name: &'a str) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        self._next_kwarg_impl::<T>(name, &[])
    }

    /// Get the next kwarg allowing for different names.
    ///
    /// This is useful when a single Rust function is implementing multiple Python
    /// functions (e.g. multiple overloads of a method) that have different names
    /// for the same keyword parameter.
    ///
    /// ```python
    ///     def information_schema(self, identifier=None)
    ///     // and
    ///     def information_schema(self, view_name=None)
    /// ```
    ///
    /// A single Rust function replacing both can be written as:
    ///
    /// ```rust
    /// # use minijinja::{Error, Value};
    /// # use minijinja::arg_utils::ArgsIter;
    /// fn information_schema(args: &[Value]) -> Result<String, Error> {
    ///    let iter = ArgsIter::new("information_schema", &[], args);
    ///    let identifier = iter.next_kwarg_aliased::<Option<&str>>("identifier", &["view_name"])?;
    ///    iter.finish()?;
    ///    // ...use identifier here...
    ///    # Ok(String::new())
    /// }
    /// ```
    pub fn next_kwarg_aliased<T>(
        &'a self,
        name: &'a str,
        aliases: &[&'a str],
    ) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        self._next_kwarg_impl::<T>(name, aliases)
    }

    fn _next_kwarg_impl<T>(&'a self, name: &'a str, aliases: &[&'a str]) -> Result<T, crate::Error>
    where
        T: ArgType<'a, Output = T>,
    {
        if let Some(arg) = self._next() {
            let rv = T::from_value(Some(arg))
                .map_err(|err| self._detail_from_value_err(None, Some(name), Some(arg), err))?;
            self._ensure_not_in_kwargs(Some(name))?;
            return Ok(rv);
        }

        let prefix = [name];
        let iter = prefix.iter().chain(aliases.iter());
        for name in iter {
            match self.kwargs.peek::<Option<&'a Value>>(name) {
                Ok(Some(_)) => return self.kwargs.get::<T>(name),
                Ok(None) | Err(_) => continue,
            }
        }
        // produce an error with the main name
        self.kwargs.get::<T>(name)
    }

    /// Ensure all positional arguments have been consumed when no kwargs are expected.
    #[inline(never)]
    pub fn finish(&self) -> Result<(), crate::Error> {
        if self.index.get() < self.num_pos_args {
            // The user called finish() now, so the current iterator position
            // is the expected maximum number of positional arguments.
            let max_pos_args = self.index.get();
            if self.pos_params.are_named() || self.kwargs.values.is_empty() {
                Err(self._unexpected_positional_arg(max_pos_args))
            } else {
                Err(self._didnt_expect_any_kwarg())
            }
        } else {
            #[allow(clippy::collapsible_else_if)]
            if self.pos_params.are_named() {
                self.kwargs.assert_all_used()
            } else if self.kwargs.values.is_empty() {
                Ok(())
            } else {
                Err(self._didnt_expect_any_kwarg())
            }
        }
    }

    /// Ensure all positional arguments have been consumed and return any trailing kwargs.
    ///
    /// Call this instead of `finish()` if you expect trailing kwargs. Example:
    ///
    /// ```python
    ///     def f(x, y=20, **kwargs)
    /// ```
    pub fn trailing_kwargs(&'a self) -> Result<&'a Kwargs, crate::Error> {
        if self.index.get() < self.num_pos_args {
            // The user called trailing_kwargs() now, so the current iterator
            // position is the expected maximum number of positional arguments.
            let max_pos_args = self.index.get();
            Err(self._unexpected_positional_arg(max_pos_args))
        } else {
            Ok(&self.kwargs)
        }
    }

    fn _didnt_expect_any_kwarg(&self) -> crate::Error {
        let err = crate::Error::new(
            crate::ErrorKind::TooManyArguments,
            format!("{}() takes no keyword arguments", self.fn_name),
        );
        err
    }

    #[inline(never)]
    fn _unexpected_positional_arg(&self, max_pos_args: usize) -> crate::Error {
        // handle the unexpected positional argument case
        debug_assert!(
            max_pos_args >= self.pos_params.len(),
            "trailing_kwargs() or finish() called before {} arguments \
were consumed from the iterator. You are misusing the ArgsIter API.",
            self.pos_params.len()
        );

        let msg = if self.pos_params.len() == max_pos_args {
            let num_pos_params = self.pos_params.len();
            format!(
                "{}() takes exactly {} positional argument{} ({} given)",
                self.fn_name,
                if num_pos_params == 0 {
                    "zero".to_string()
                } else {
                    format!("{num_pos_params}")
                },
                if num_pos_params == 1 { "" } else { "s" },
                self.num_pos_args
            )
        } else {
            format!(
                "{}() takes from {} to {} positional arguments but {} were given",
                self.fn_name,
                self.pos_params.len(),
                max_pos_args,
                self.num_pos_args
            )
        };
        crate::Error::new(crate::ErrorKind::TooManyArguments, msg)
    }

    #[inline(never)]
    fn _missing_pos_arg(&self, idx: usize, aliases: &[&'a str]) -> crate::Error {
        let msg = match self.pos_params {
            PosParams::Named(param_names) => {
                use std::fmt::Write as _;
                let missing = {
                    let mut missing: Vec<&'a str> = vec![];
                    for name in &param_names[idx..] {
                        let peek = match self.kwargs.peek::<Option<&Value>>(name) {
                            Ok(Some(_)) => true,
                            Ok(None) => false,
                            Err(e) => return e,
                        };
                        if !peek {
                            missing.push(name);
                        }
                    }
                    missing
                };
                debug_assert!(
                    !missing.is_empty(),
                    "_missing_pos_arg() must be called after checking kwargs for the next pos argument");

                let mut msg = String::new();
                (if missing.len() == 1 {
                    let _ = write!(
                        &mut msg,
                        "{}() missing 1 required positional argument: '{}'",
                        self.fn_name, missing[0]
                    );
                    match aliases {
                        [] => Ok(()),
                        [alias, others @ ..] => {
                            let _ = write!(&mut msg, " (or '{alias}'");
                            for a in others {
                                let _ = write!(&mut msg, ", '{a}'");
                            }
                            write!(&mut msg, ")")
                        }
                    }
                } else {
                    write!(
                        &mut msg,
                        "{}() missing {} required positional arguments: ",
                        self.fn_name,
                        missing.len()
                    )
                    .and_then(|_| {
                        for a in missing.iter().take(missing.len().saturating_sub(1)) {
                            write!(&mut msg, "'{a}', ")?;
                        }
                        let last = missing.last().unwrap_or(&"");
                        write!(
                            &mut msg,
                            "{}'{last}'",
                            if missing.len() > 1 { "and " } else { "" },
                        )
                    })
                })
                .unwrap_or_default();
                msg
            }
            PosParams::Unnamed(arity) => {
                if self.kwargs.values.is_empty() {
                    format!(
                        "{}() takes exactly {} positional argument{} ({} given)",
                        self.fn_name,
                        arity,
                        if arity == 1 { "" } else { "s" },
                        self.num_pos_args
                    )
                } else {
                    return self._didnt_expect_any_kwarg();
                }
            }
        };
        crate::Error::new(crate::ErrorKind::MissingArgument, msg)
    }

    /// Try to determine the type name that gets as closes as possible to
    /// the type of the value if this were Jinja in Python.
    pub fn type_name_of_value(value: Option<&Value>) -> Cow<'static, str> {
        value.map_or_else(
            || Cow::Borrowed("None"),
            |v| {
                if let Some(obj) = v.as_object() {
                    let full_name = obj.type_name();
                    return Cow::Borrowed(type_name_suffix(full_name));
                }
                let kind = v.kind();
                match kind {
                    ValueKind::None => Cow::Borrowed("None"),
                    _ => Cow::Owned(kind.to_string()),
                }
            },
        )
    }

    /// Add more detail to the errors returned by `T::from_value()` calls.
    #[inline(never)]
    fn _detail_from_value_err(
        &self,
        idx: Option<usize>,
        name: Option<&str>,
        value: Option<&'a Value>,
        err: crate::Error,
    ) -> crate::Error {
        let kind = err.kind();
        if kind == crate::ErrorKind::InvalidOperation {
            // `got` is the name of the "type" of the value we've got.
            let got = Self::type_name_of_value(value);
            // `expected` contains the text produced by `T::from_value()` and it's
            // usually something like:
            // - "cannot convert string to i64"
            // - "value is not a string"
            // - "expected MyStruct"
            let expected = err.detail().unwrap_or("invalid operation");
            let detail = match (idx, name) {
                (None, None) => format!(
                    "argument to {}() has incompatible type {}; {}",
                    self.fn_name,
                    got.as_ref(),
                    expected
                ),
                (Some(idx), None) => format!(
                    "argument {} to {}() has incompatible type {}; {}",
                    idx + 1,
                    self.fn_name,
                    got.as_ref(),
                    expected
                ),
                (_, Some(name)) => format!(
                    "argument '{}' to {}() has incompatible type {}; {}",
                    name,
                    self.fn_name,
                    got.as_ref(),
                    expected
                ),
            };
            crate::Error::new(kind, detail)
        } else {
            err
        }
    }

    fn _ensure_pos_arg_not_in_kwargs(&'a self, idx: usize) -> Result<(), crate::Error> {
        let name = self.pos_params.get(idx);
        self._ensure_not_in_kwargs(name)
    }

    #[inline(never)]
    fn _ensure_not_in_kwargs(&'a self, name: Option<&'a str>) -> Result<(), crate::Error> {
        if self.kwargs.values.is_empty() {
            return Ok(());
        }
        let name = match name {
            Some(name) => name,
            None => return Err(self._didnt_expect_any_kwarg()),
        };
        // Jinja has no way of knowing how a function implemented in Rust
        // is defined, so if we got a param from the positional arguments,
        // we should ensure it is not present in the kwargs as well.
        let kwarg = self.kwargs.peek::<Option<&'a Value>>(name)?;
        if kwarg.is_some() {
            let err = crate::Error::new(
                crate::ErrorKind::TooManyArguments,
                format!(
                    "{}() got multiple values for argument '{}'",
                    self.fn_name, name
                ),
            );
            Err(err)
        } else {
            Ok(())
        }
    }

    fn _next(&self) -> Option<&'a Value> {
        let rv = if self.index.get() < self.num_pos_args {
            let arg = &self.args[self.index.get()];
            Some(arg)
        } else {
            None
        };
        // Always increment the index, even if we return None.
        // This allows us to track how many positional parameters
        // have been matched even if they were passed as kwargs.
        self.index.set(self.index.get() + 1);
        rv
    }
}

impl<'a> Iterator for ArgsIter<'a> {
    type Item = &'a Value;

    fn next(&mut self) -> Option<Self::Item> {
        self._next()
    }
}

/// Assert that not arguments are provided for a nullary function.
#[macro_export]
macro_rules! assert_nullary_args {
    ($fn_name:expr, $args:expr) => {
        ArgsIter::nullary($fn_name, $args).finish()
    };
}

#[cfg(test)]
mod tests {
    use crate::value::Object;

    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn test_empty_args() {
        let parser = ArgParser::new(&[], None);
        assert!(parser.positional.is_empty());
        assert!(parser.kwargs.is_empty());
    }

    #[test]
    fn test_positional_only() {
        let args = vec![Value::from("first"), Value::from("second")];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("arg0").unwrap(), "first");
        assert_eq!(parser.get::<String>("arg1").unwrap(), "second");
        assert!(parser.kwargs.is_empty());
    }

    #[test]
    fn test_kwargs_only() {
        let args = vec![Value::from(Kwargs::from_iter([
            ("name", Value::from("test")),
            ("namespace", Value::from("test_ns")),
        ]))];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("name").unwrap(), "test");
        assert_eq!(parser.get::<String>("namespace").unwrap(), "test_ns");
        assert!(parser.positional.is_empty());
    }

    #[test]
    fn test_kwargs_only_reversed() {
        let args = vec![Value::from(Kwargs::from_iter([
            ("namespace", Value::from("test_ns")),
            ("name", Value::from("test")),
        ]))];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("name").unwrap(), "test");
        assert_eq!(parser.get::<String>("namespace").unwrap(), "test_ns");
        assert!(parser.positional.is_empty());
    }

    #[test]
    fn test_kwargs_btreemap() {
        let mut map = BTreeMap::new();
        map.insert("name".to_string(), Value::from("test"));
        map.insert("namespace".to_string(), Value::from("test_ns"));

        let args = vec![Value::from(Kwargs::from_iter(map))];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("name").unwrap(), "test");
        assert_eq!(parser.get::<String>("namespace").unwrap(), "test_ns");
    }

    #[test]
    fn test_mixed_args() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("namespace".to_string(), Value::from("test_ns"));

        let args = vec![
            Value::from("positional"),
            Value::from(Kwargs::from_iter(kwargs)),
        ];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("arg0").unwrap(), "positional");
        assert_eq!(parser.get::<String>("namespace").unwrap(), "test_ns");
    }

    #[test]
    fn test_optional_args() {
        let mut parser = ArgParser::new(&[], None);
        assert!(parser.get_optional::<String>("missing").is_none());

        let args = vec![Value::from("test")];
        let mut parser = ArgParser::new(&args, None);
        assert_eq!(parser.get_optional::<String>("arg0").unwrap(), "test");
        assert!(parser.get_optional::<String>("missing").is_none());
    }

    #[test]
    fn test_type_conversion() {
        let args = vec![Value::from(42), Value::from(true), Value::from("string")];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<i64>("num").unwrap(), 42);
        assert!(parser.get::<bool>("bool").unwrap());
        assert_eq!(parser.get::<String>("str").unwrap(), "string");
    }

    #[test]
    fn test_has_kwarg() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("test".to_string(), Value::from(true));

        let args = vec![Value::from(Kwargs::from_iter(kwargs))];
        let parser = ArgParser::new(&args, None);

        assert!(parser.has_kwarg("test"));
        assert!(!parser.has_kwarg("missing"));
    }

    #[test]
    fn test_positional_len() {
        let args = vec![Value::from(1), Value::from(2), Value::from(3)];
        let parser = ArgParser::new(&args, None);

        assert_eq!(parser.positional_len(), 3);
    }

    #[test]
    fn test_error_cases() {
        let mut parser = ArgParser::new(&[], None);

        // Missing required argument
        let err = parser.get::<String>("required").unwrap_err();
        assert!(err
            .to_string()
            .contains("Required argument 'required' not provided"));

        // Wrong type conversion
        let args = vec![Value::from("not a number")];
        let mut parser = ArgParser::new(&args, None);
        assert!(parser.get::<i64>("num").is_err());
    }

    #[test]
    fn test_kwargs_precedence() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("value".to_string(), Value::from("kwarg_value"));

        let args = vec![
            Value::from("positional_value"),
            Value::from(Kwargs::from_iter(kwargs)),
        ];
        let mut parser = ArgParser::new(&args, None);

        // Kwargs should take precedence over positional args
        assert_eq!(parser.get::<String>("value").unwrap(), "kwarg_value");
    }

    #[test]
    fn test_multiple_kwargs() {
        let mut kwargs1 = BTreeMap::new();
        kwargs1.insert("first".to_string(), Value::from("value1"));

        let mut kwargs2 = BTreeMap::new();
        kwargs2.insert("second".to_string(), Value::from("value2"));

        let args = vec![
            Value::from(Kwargs::from_iter(kwargs1)),
            Value::from(Kwargs::from_iter(kwargs2)),
        ];
        let mut parser = ArgParser::new(&args, None);

        assert_eq!(parser.get::<String>("first").unwrap(), "value1");
        assert_eq!(parser.get::<String>("second").unwrap(), "value2");
    }

    #[test]
    fn test_get_either() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("alt_name".to_string(), Value::from("test_value"));

        let args = vec![Value::from(Kwargs::from_iter(kwargs))];
        let mut parser = ArgParser::new(&args, None);

        // Should find value under alternative name
        assert_eq!(
            parser.get_either::<String>("name", "alt_name").unwrap(),
            "test_value"
        );
    }

    #[test]
    fn test_get_optional_either() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("v".to_string(), Value::from("1.0.0"));

        let args = vec![Value::from(Kwargs::from_iter(kwargs))];
        let mut parser = ArgParser::new(&args, None);

        // Should find value under alternative name
        assert_eq!(
            parser
                .get_optional_either::<String>("version", "v")
                .unwrap(),
            "1.0.0"
        );

        // Should return None when neither name exists
        assert!(parser
            .get_optional_either::<String>("missing1", "missing2")
            .is_none());
    }

    // ArgsIter tests ---------------------------------------------------------

    // def f(x, y, alpha=None, beta=None)
    fn f(args: &[Value]) -> Result<(i64, i64, Option<Value>, Option<Value>), crate::Error> {
        let iter = ArgsIter::new("f", &["x", "y"], args);
        let x = iter.next_arg::<i64>()?;
        let y = iter.next_arg::<i64>()?;
        let alpha = iter.next_kwarg::<Option<&Value>>("alpha")?;
        let beta = iter.next_kwarg::<Option<&Value>>("beta")?;
        iter.finish()?;
        // ...use x, y, alpha, beta here...
        Ok((x, y, alpha.cloned(), beta.cloned()))
    }

    #[test]
    fn test_args_iter() {
        use crate::ErrorKind;

        let x = Value::from(42);
        let y = Value::from(1337);
        let alpha = Value::from("Alpha");
        let beta = Value::from("Beta");
        let all_vars = (42, 1337, Some(alpha.clone()), Some(beta.clone()));

        // 1) Passing all arguments as positional arguments:
        //
        // f()
        // f(x)
        // f(x, y)
        // f(x, y, alpha)
        // f(x, y, alpha, beta)

        // f()
        let args: [Value; 0] = [];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::MissingArgument);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() missing 2 required positional arguments: 'x', and 'y'"
        );

        // f(x)
        let args = [x.clone()];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::MissingArgument);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() missing 1 required positional argument: 'y'"
        );

        // f(x, y)
        let args = [x.clone(), y.clone()];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, None, None));

        // f(x, y, alpha)
        let args = [x.clone(), y.clone(), alpha.clone()];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, Some(Value::from("Alpha")), None));

        // f(x, y, alpha, beta)
        let args = [x.clone(), y.clone(), alpha.clone(), beta.clone()];
        let vars = f(&args).unwrap();
        assert_eq!(vars, all_vars);

        // 2) Passing all arguments as kwargs:
        //
        // f(x=x)
        // f(x=x, y=y)
        // f(x=x, y=y, alpha=alpha)
        // f(x=x, y=y, alpha=alpha, beta=beta)

        // f(x=x)
        let args = [Value::from(Kwargs::from_iter([(
            "x".to_string(),
            x.clone(),
        )]))];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::MissingArgument);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() missing 1 required positional argument: 'y'"
        );

        // f(x=x, y=y)
        let args = [Value::from(Kwargs::from_iter([
            ("x".to_string(), x.clone()),
            ("y".to_string(), y.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, None, None));

        // f(x=x, y=y, alpha=alpha)
        let args = [Value::from(Kwargs::from_iter([
            ("x".to_string(), x.clone()),
            ("y".to_string(), y.clone()),
            ("alpha".to_string(), alpha.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, Some(alpha.clone()), None));

        // f(x=x, y=y, alpha=alpha, beta=beta)
        let args = [Value::from(Kwargs::from_iter([
            ("x".to_string(), x.clone()),
            ("y".to_string(), y.clone()),
            ("alpha".to_string(), alpha.clone()),
            ("beta".to_string(), beta.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, Some(alpha.clone()), Some(beta.clone())));

        // 3) Passing all arguments as kwargs but in an order that doesn't match the function
        //    signature:
        //
        // f(y=y)
        // f(y=y, x=x)
        // f(y=y, x=x, beta=beta)
        // f(y=y, x=x, beta=beta, alpha=alpha)

        // f(y=y)
        let args = [Value::from(Kwargs::from_iter([(
            "y".to_string(),
            y.clone(),
        )]))];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::MissingArgument);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() missing 1 required positional argument: 'x'"
        );

        // f(y=y, x=x)
        let args = [Value::from(Kwargs::from_iter([
            ("y".to_string(), y.clone()),
            ("x".to_string(), x.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, None, None));

        // f(y=y, x=x, beta=beta)
        let args = [Value::from(Kwargs::from_iter([
            ("y".to_string(), y.clone()),
            ("x".to_string(), x.clone()),
            ("beta".to_string(), beta.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, None, Some(beta.clone())));

        // f(y=y, x=x, beta=beta, alpha=alpha)
        let args = [Value::from(Kwargs::from_iter([
            ("y".to_string(), y.clone()),
            ("x".to_string(), x.clone()),
            ("beta".to_string(), beta.clone()),
            ("alpha".to_string(), alpha.clone()),
        ]))];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, Some(alpha.clone()), Some(beta.clone())));

        // 4) Passing some arguments as positional and some as kwargs:
        // f(x, y=y, beta=beta)
        // f(x, x=x)
        // f(x, y, alpha, alpha="Alpha 2")

        // f(x, y=y, beta="Beta")
        let args = [
            x.clone(),
            Value::from(Kwargs::from_iter([
                ("y".to_string(), y.clone()),
                ("beta".to_string(), beta.clone()),
            ])),
        ];
        let vars = f(&args).unwrap();
        assert_eq!(vars, (42, 1337, None, Some(beta.clone())));

        // f(x, x=x)  [can't pass x twice]
        let args = [
            x.clone(),
            Value::from(Kwargs::from_iter([("x".to_string(), x.clone())])),
        ];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() got multiple values for argument 'x'"
        );

        // f(x, y, alpha, alpha="Alpha 2")  [can't pass alpha twice]
        let args = [
            x.clone(),
            y.clone(),
            alpha.clone(),
            Value::from(Kwargs::from_iter([(
                "alpha".to_string(),
                Value::from("Alpha 2"),
            )])),
        ];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() got multiple values for argument 'alpha'"
        );

        // 5) More arguments than expected:
        //
        // f(x, y, alpha, beta, "Gamma")
        // f(x, y, alpha, beta, gamma="Gamma")

        // f(x, y, alpha, beta, "Gamma")  [unexpected positional arg]
        let args = [
            x.clone(),
            y.clone(),
            alpha.clone(),
            beta.clone(),
            Value::from("Gamma"),
        ];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "f() takes from 2 to 4 positional arguments but 5 were given"
        );

        // f(x, y, alpha, beta, gamma="Gamma")  [unexpected kwarg]
        let args = [
            x,
            y,
            alpha,
            beta,
            Value::from(Kwargs::from_iter([(
                "gamma".to_string(),
                Value::from("Gamma"),
            )])),
        ];
        let e = f(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "unknown keyword argument 'gamma'"
        );
    }

    // def now()
    fn now(args: &[Value]) -> Result<(), crate::Error> {
        let iter = ArgsIter::for_unnamed_pos_args("now", 0, args);
        iter.finish()
    }

    // def tuple.count()
    fn count(args: &[Value]) -> Result<String, crate::Error> {
        let iter = ArgsIter::for_unnamed_pos_args("tuple.count", 1, args);
        let arg = iter.next_arg::<&str>()?;
        iter.finish()?;
        Ok(arg.to_string())
    }

    #[test]
    fn test_args_iter_for_unnamed_args() {
        use crate::ErrorKind;

        // now()
        // now("test")
        // now("test", "another")
        // now(x="test")
        // now("test", x="another")

        let args = [];
        now(&args).unwrap();
        let args = [Value::from("test")];
        let e = now(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "now() takes exactly zero positional arguments (1 given)"
        );

        let args = [Value::from("test"), Value::from("another")];
        let e = now(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "now() takes exactly zero positional arguments (2 given)"
        );

        let args = [Value::from(Kwargs::from_iter([(
            "x".to_string(),
            Value::from("test"),
        )]))];
        let e = now(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "now() takes no keyword arguments"
        );

        let args = [
            Value::from("test"),
            Value::from(Kwargs::from_iter([("x".to_string(), Value::from("test"))])),
        ];
        let e = now(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "now() takes no keyword arguments"
        );

        // count()
        // count("test")
        // count("test", "another")
        // count(x="test")
        // count("test", x="another")

        let args = [];
        let e = count(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::MissingArgument);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "tuple.count() takes exactly 1 positional argument (0 given)"
        );

        let args = [Value::from("test")];
        let result = count(&args).unwrap();
        assert_eq!(result, "test");

        let args = [Value::from("test"), Value::from("another")];
        let e = count(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "tuple.count() takes exactly 1 positional argument (2 given)"
        );
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);

        let args = [Value::from(Kwargs::from_iter([(
            "x".to_string(),
            Value::from("test"),
        )]))];
        let e = count(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "tuple.count() takes no keyword arguments"
        );
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);

        let args = [
            Value::from("test"),
            Value::from(Kwargs::from_iter([(
                "x".to_string(),
                Value::from("another"),
            )])),
        ];
        let e = count(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "tuple.count() takes no keyword arguments"
        );
    }

    // def print_table(max_columns=20, max_rows=6, **kwargs)
    fn print_table(args: &[Value]) -> Result<(i64, i64, Kwargs), crate::Error> {
        let iter = ArgsIter::new("print_table", &[], args);
        let max_columns = iter.next_kwarg::<Option<i64>>("max_columns")?.unwrap_or(20);
        let max_rows = iter.next_kwarg::<Option<i64>>("max_rows")?.unwrap_or(6);
        let kwargs = iter.trailing_kwargs()?;
        Ok((max_columns, max_rows, kwargs.clone()))
    }

    #[test]
    fn test_args_iter_with_trailing_kwargs() {
        use crate::ErrorKind;
        // print_table()
        // print_table(10)
        // print_table(10, 3)
        // print_table(10, 3, 0)

        let args = [];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 20);
        assert_eq!(max_rows, 6);
        assert!(kwargs.values.is_empty());

        let args = [Value::from(10)];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 10);
        assert_eq!(max_rows, 6);
        assert!(kwargs.values.is_empty());

        let args = [Value::from(10), Value::from(3)];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 10);
        assert_eq!(max_rows, 3);
        assert!(kwargs.values.is_empty());

        let args = [Value::from(10), Value::from(3), Value::from(0)];
        let e = print_table(&args).unwrap_err();
        assert_eq!(e.kind(), ErrorKind::TooManyArguments);
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "print_table() takes from 0 to 2 positional arguments but 3 were given"
        );

        // print_table(max_rows=3)
        // print_table(max_rows=3, max_columns=10)
        // print_table(max_rows=3, max_columns=10, x=1337)

        let args = [Value::from(Kwargs::from_iter([(
            "max_rows".to_string(),
            Value::from(3),
        )]))];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 20);
        assert_eq!(max_rows, 3);
        assert!(kwargs.assert_all_used().is_ok());

        let args = [Value::from(Kwargs::from_iter([
            ("max_rows".to_string(), Value::from(3)),
            ("max_columns".to_string(), Value::from(10)),
        ]))];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 10);
        assert_eq!(max_rows, 3);
        assert!(kwargs.assert_all_used().is_ok());

        let args = [Value::from(Kwargs::from_iter([
            ("max_rows".to_string(), Value::from(3)),
            ("max_columns".to_string(), Value::from(10)),
            ("x".to_string(), Value::from(1337)),
        ]))];
        let (max_columns, max_rows, kwargs) = print_table(&args).unwrap();
        assert_eq!(max_columns, 10);
        assert_eq!(max_rows, 3);
        assert_eq!(kwargs.get::<i64>("x").unwrap(), 1337);
        assert!(kwargs.assert_all_used().is_ok());
    }

    #[test]
    fn test_args_iter_type_matching_error() {
        let args = [Value::from(42), Value::from("not a number")];
        let e = f(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'y' to f() has incompatible type string; cannot convert string to i64"
        );

        let args = [Value::from(1), Value::from_object(MyStruct { value: 2 })];
        let e = f(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'y' to f() has incompatible type MyStruct; cannot convert map to i64"
        );
    }

    #[derive(Copy, Clone, Debug)]
    struct MyStruct {
        value: i32,
    }

    impl Object for MyStruct {}

    #[derive(Copy, Clone, Debug)]
    struct NotMyStruct {}

    impl Object for NotMyStruct {}

    fn unwrap_my_struct(args: &[Value]) -> Result<i32, crate::Error> {
        let iter = ArgsIter::new("unwrap_my_struct", &["x"], args);
        let my_struct = iter.next_arg::<&MyStruct>()?;
        iter.finish()?;
        Ok(my_struct.value)
    }

    fn take_unamed_args(args: &[Value]) -> Result<i32, crate::Error> {
        let iter = ArgsIter::for_unnamed_pos_args("take_unamed_args", 2, args);
        let my_struct = iter.next_arg::<&MyStruct>()?;
        let _not_my_struct = iter.next_arg::<&NotMyStruct>()?;
        iter.finish()?;
        Ok(my_struct.value)
    }

    #[test]
    fn test_args_iter_object_type_matching() {
        let my = MyStruct { value: 42 };

        let args = [Value::from_object(my)];
        let x = unwrap_my_struct(&args).unwrap();
        assert_eq!(x, 42);

        let args = [Value::from(())];
        let e = unwrap_my_struct(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'x' to unwrap_my_struct() has incompatible type None; expected MyStruct"
        );

        let args = [Value::from(true)];
        let e = unwrap_my_struct(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'x' to unwrap_my_struct() has incompatible type bool; expected MyStruct"
        );

        let args = [Value::from("hello")];
        let e = unwrap_my_struct(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'x' to unwrap_my_struct() has incompatible type string; expected MyStruct"
        );

        let not_my = NotMyStruct {};
        let args = [Value::from_object(not_my)];
        let e = unwrap_my_struct(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'x' to unwrap_my_struct() has incompatible type NotMyStruct; expected MyStruct"
        );

        let args = [Value::from(Kwargs::from_iter([(
            "x".to_string(),
            Value::from_object(my),
        )]))];
        let x = unwrap_my_struct(&args).unwrap();
        assert_eq!(x, 42);

        let args = [Value::from(Kwargs::from_iter([(
            "x".to_string(),
            Value::from_object(not_my),
        )]))];
        let e = unwrap_my_struct(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 'x' to unwrap_my_struct() has incompatible type NotMyStruct; expected MyStruct"
        );

        let args = [Value::from_object(my), Value::from_object(not_my)];
        let x = take_unamed_args(&args).unwrap();
        assert_eq!(x, 42);

        let args = [Value::from_object(not_my), Value::from_object(my)];
        let e = take_unamed_args(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 1 to take_unamed_args() has incompatible type NotMyStruct; expected MyStruct"
        );

        let args = [Value::from_object(my), Value::from_object(my)];
        let e = take_unamed_args(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "argument 2 to take_unamed_args() has incompatible type MyStruct; expected NotMyStruct"
        )
    }

    /// Subtle example: handling optional positional argument.
    ///
    /// ```python
    /// def string_type(size: int) -> str
    /// ```
    fn string_type(args: &[Value]) -> Result<String, crate::Error> {
        let iter = ArgsIter::new("string_type", &["size"], args);
        let size = iter.next_arg::<Option<i64>>()?;
        iter.finish()?;
        let s = match size {
            None => "STRING".to_string(),
            Some(size) => format!("STRING({size})"),
        };
        Ok(s)
    }

    #[test]
    fn test_optional_positional_argument() {
        // string_type()
        let args = [];
        let result = string_type(&args).unwrap();
        assert_eq!(result, "STRING");

        // string_type(255)
        let args = [Value::from(255)];
        let result = string_type(&args).unwrap();
        assert_eq!(result, "STRING(255)");

        // string_type(size=None)
        let args = [Value::from(Kwargs::from_iter([(
            "size".to_string(),
            Value::from(()),
        )]))];
        let result = string_type(&args).unwrap();
        assert_eq!(result, "STRING");
    }

    /// Example: Handling aliased positional argument names.
    ///
    /// ```python
    ///     def information_schema(self, identifier)
    ///     // and
    ///     def information_schema(self, view_name)
    /// ```
    fn information_schema(args: &[Value]) -> Result<String, crate::Error> {
        let iter = ArgsIter::new("information_schema", &["identifier"], args);
        let identifier = iter.next_pos_arg_aliased::<&str>(&["view_name"])?;
        iter.finish()?;
        Ok(identifier.to_string())
    }

    /// Example: Handling aliased kwarg argument names.
    ///
    /// ```python
    ///     def information_schema(self, identifier=None)
    ///     // and
    ///     def information_schema(self, view_name=None)
    /// ```
    fn information_schema2(args: &[Value]) -> Result<String, crate::Error> {
        let iter = ArgsIter::new("information_schema", &[], args);
        let identifier = iter.next_kwarg_aliased::<Option<&str>>("identifier", &["view_name"])?;
        iter.finish()?;
        Ok(format!("{:?}", identifier))
    }

    #[test]
    fn test_args_iter_aliased_pos_arg() {
        // information_schema()
        let args = [];
        let e = information_schema(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "information_schema() missing 1 required positional argument: 'identifier' (or 'view_name')"
        );

        // information_schema("test")
        let args = [Value::from("test")];
        let result = information_schema(&args).unwrap();
        assert_eq!(result, "test");

        // information_schema(identifier="test")
        let args = [Value::from(Kwargs::from_iter([(
            "identifier".to_string(),
            Value::from("test"),
        )]))];
        let result = information_schema(&args).unwrap();
        assert_eq!(result, "test");

        // information_schema(view_name="test")
        let args = [Value::from(Kwargs::from_iter([(
            "view_name".to_string(),
            Value::from("test"),
        )]))];
        let result = information_schema(&args).unwrap();
        assert_eq!(result, "test");

        // information_schema(blah="test")
        let args = [Value::from(Kwargs::from_iter([(
            "blah".to_string(),
            Value::from("test"),
        )]))];
        let e = information_schema(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "information_schema() missing 1 required positional argument: 'identifier' (or 'view_name')"
        );

        // If both are passed (theoretically possible, not expected in practice),
        // we get an error
        //
        // information_schema(identifier="the identifier", view_name="the view_name")
        let args = [Value::from(Kwargs::from_iter([
            ("view_name".to_string(), Value::from("the view_name")),
            ("identifier".to_string(), Value::from("the identifier")),
        ]))];
        let e = information_schema(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "unknown keyword argument 'view_name'"
        );
    }

    #[test]
    fn test_args_iter_aliased_kwarg() {
        // information_schema()
        let args = [];
        let result = information_schema2(&args).unwrap();
        assert_eq!(result, "None");

        // information_schema("test")
        let args = [Value::from("test")];
        let result = information_schema2(&args).unwrap();
        assert_eq!(result, "Some(\"test\")");

        // information_schema(identifier="test")
        let args = [Value::from(Kwargs::from_iter([(
            "identifier".to_string(),
            Value::from("test"),
        )]))];
        let result = information_schema2(&args).unwrap();
        assert_eq!(result, "Some(\"test\")");

        // information_schema(view_name="test")
        let args = [Value::from(Kwargs::from_iter([(
            "view_name".to_string(),
            Value::from("test"),
        )]))];
        let result = information_schema2(&args).unwrap();
        assert_eq!(result, "Some(\"test\")");

        // information_schema(blah="test")
        let args = [Value::from(Kwargs::from_iter([(
            "blah".to_string(),
            Value::from("test"),
        )]))];
        let e = information_schema2(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "unknown keyword argument 'blah'"
        );

        // If both are passed (theoretically possible, not expected in practice),
        // we get an error
        //
        // information_schema(identifier="the identifier", view_name="the view_name")
        let args = [Value::from(Kwargs::from_iter([
            ("view_name".to_string(), Value::from("the view_name")),
            ("identifier".to_string(), Value::from("the identifier")),
        ]))];
        let e = information_schema2(&args).unwrap_err();
        assert_eq!(
            e.to_string().split_once(": ").unwrap().1,
            "unknown keyword argument 'view_name'"
        );
    }
}
