use serde::{Deserialize, Serialize};

use crate::machinery::Span;
use crate::types::iterable::IterableType;
use crate::types::list::ListType;
use crate::types::utils::CodeLocation;
use crate::types::{Object, Type};
use crate::TypecheckingEventListener;
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// The argument specification of a function.
#[derive(Clone, Eq, PartialEq, Debug, Serialize, Deserialize)]
pub struct ArgSpec {
    /// The name of the argument.
    pub name: String,
    /// Whether the argument is optional.
    pub is_optional: bool,
}

impl From<Argument> for ArgSpec {
    fn from(arg: Argument) -> Self {
        Self {
            name: arg.name,
            is_optional: arg.is_optional,
        }
    }
}
impl ArgSpec {
    /// Create a new argument specification.
    pub fn new(name: &str, is_optional: bool) -> Self {
        Self {
            name: name.to_string(),
            is_optional,
        }
    }
}

pub trait FunctionType: Object + Send + Sync + std::fmt::Debug {
    fn resolve_arguments(
        &self,
        positional_args: &[Type],
        kwargs: &BTreeMap<String, Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        let mut args = vec![];
        let mut kwargs = kwargs.clone();

        for (i, spec) in self.arg_specs().iter().enumerate() {
            if i < positional_args.len() {
                let name = spec.name.clone();
                if kwargs.contains_key(&name) {
                    listener.warn(&format!("Duplicate argument: {name}"));
                    return Ok(Type::Any { hard: false });
                }
                args.push(positional_args[i].clone());
            } else if let Some(value) = kwargs.get(&spec.name) {
                args.push(value.clone());
                kwargs.remove(&spec.name);
            } else if spec.is_optional {
                args.push(Type::None);
            } else {
                listener.warn(&format!("Missing required argument: {}", spec.name));
                return Ok(Type::Any { hard: false });
            }
        }
        // caller is a special argument, it is not in the arg_specs
        kwargs.remove("caller");
        if !kwargs.is_empty() {
            listener.warn(&format!("Unknown arguments: {:?}", kwargs.keys()));
            return Ok(Type::Any { hard: false });
        }
        self._resolve_arguments(&args, listener)
    }

    fn arg_specs(&self) -> Vec<ArgSpec>;

    fn _resolve_arguments(
        &self,
        actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error>;

    fn function_get_span(&self) -> Option<Span> {
        None
    }

    fn function_get_path(&self) -> Option<PathBuf> {
        None
    }

    fn function_get_unique_id(&self) -> Option<String> {
        None
    }
}

impl<T: FunctionType> Object for T {
    fn get_attribute(
        &self,
        name: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        listener.warn(&format!("Attribute {name} not found"));
        Ok(Type::Any { hard: false })
    }

    fn call(
        &self,
        positional_args: &[Type],
        kwargs: &BTreeMap<String, Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        self.resolve_arguments(positional_args, kwargs, listener)
    }

    fn subscript(
        &self,
        _index: &Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        listener.warn("Subscript not supported for function type");
        Ok(Type::Any { hard: false })
    }

    fn get_span(&self) -> Option<Span> {
        self.function_get_span()
    }

    fn get_path(&self) -> Option<PathBuf> {
        self.function_get_path()
    }

    fn get_unique_id(&self) -> Option<String> {
        self.function_get_unique_id()
    }
}

/// The argument of a function.
#[derive(Clone)]
pub struct Argument {
    /// The name of the argument.
    pub name: String,
    /// The type of the argument.
    pub type_: Type,
    /// Whether the argument is optional.
    pub is_optional: bool,
}

#[derive(Clone)]
pub struct LambdaType {
    pub args: Vec<Type>,
    pub ret_type: Type,
}

impl LambdaType {
    pub fn new(args: Vec<Type>, ret_type: Type) -> Self {
        Self { args, ret_type }
    }
}

impl std::fmt::Debug for LambdaType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LambdaType({:?}, {:?})", self.args, self.ret_type)
    }
}

impl FunctionType for LambdaType {
    fn _resolve_arguments(
        &self,
        actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if self.args.len() != actual_arguments.len() {
            listener.warn(&format!(
                "Expected {} arguments, got {}",
                self.args.len(),
                actual_arguments.len()
            ));
        }
        for (arg, actual_arg) in self.args.iter().zip(actual_arguments.iter()) {
            if !actual_arg.is_compatible_with(arg) {
                listener.warn(&format!("Expected {arg:?}, got {actual_arg:?}"));
            }
        }
        Ok(self.ret_type.clone())
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        self.args
            .iter()
            .enumerate()
            .map(|(i, _)| ArgSpec::new(&format!("arg{i}"), false))
            .collect()
    }
}

impl From<UserDefinedFunctionType> for LambdaType {
    fn from(value: UserDefinedFunctionType) -> Self {
        Self {
            args: value.args.iter().map(|arg| arg.type_.clone()).collect(),
            ret_type: value.ret_type.clone(),
        }
    }
}

/// The user defined function type.
#[derive(Clone)]
pub struct UserDefinedFunctionType {
    /// The name of the function.
    pub name: String,
    /// The arguments of the function.
    pub args: Vec<Argument>,
    /// The return type of the function.
    pub ret_type: Type,
    /// The relative path of the macro.
    pub path: PathBuf,
    /// The start span of the macro.
    pub span: Span,
    /// The unique id of the function.
    pub unique_id: String,
}

impl fmt::Debug for UserDefinedFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)
    }
}

impl UserDefinedFunctionType {
    /// Create a new user defined function type.
    pub fn new(
        name: &str,
        args: Vec<Argument>,
        ret_type: Type,
        path: &Path,
        span: &Span,
        unique_id: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            args,
            ret_type,
            path: path.to_path_buf(),
            span: *span,
            unique_id: unique_id.to_string(),
        }
    }
}

impl FunctionType for UserDefinedFunctionType {
    fn _resolve_arguments(
        &self,
        actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        // match the actual arguments with the expected arguments, if matches return Ok else Err
        if self.args.len() != actual_arguments.len() {
            listener.warn(&format!(
                "Argument number mismatch: expected {}, got {}",
                self.args.len(),
                actual_arguments.len()
            ));
        } else {
            for (i, (expected, actual)) in self.args.iter().zip(actual_arguments).enumerate() {
                if !actual.is_compatible_with(&expected.type_) {
                    listener.warn(&format!(
                        "Argument type mismatch: expected {:?}, got {actual:?}, at index {i}",
                        expected.type_,
                    ));
                }
            }
        }
        Ok(self.ret_type.clone())
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        self.args.iter().map(|arg| arg.clone().into()).collect()
    }

    fn function_get_span(&self) -> Option<Span> {
        Some(self.span)
    }

    fn function_get_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn function_get_unique_id(&self) -> Option<String> {
        Some(self.unique_id.clone())
    }
}

/// The undefined function type.
#[derive(Clone)]
pub struct UndefinedFunctionType {
    /// The name of the function.
    pub name: String,
    /// The location of the function.
    pub location: CodeLocation,
    /// The relative path of the function.
    pub path: PathBuf,
    /// The start span of the function.
    pub span: Span,
    /// The unique id of the function.
    pub unique_id: String,
}

impl fmt::Debug for UndefinedFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name.to_string())
    }
}

impl UndefinedFunctionType {
    /// Create a new undefined function type.
    pub fn new(
        name: &str,
        location: CodeLocation,
        path: &Path,
        span: &Span,
        unique_id: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            location,
            path: path.to_path_buf(),
            span: *span,
            unique_id: unique_id.to_string(),
        }
    }
}

impl FunctionType for UndefinedFunctionType {
    fn _resolve_arguments(
        &self,
        _actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        listener.warn(&format!(
            "Function {} @ {} is not defined",
            self.name, self.location
        ));
        Ok(Type::Any { hard: false })
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }

    fn function_get_span(&self) -> Option<Span> {
        Some(self.span)
    }

    fn function_get_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn function_get_unique_id(&self) -> Option<String> {
        Some(self.unique_id.clone())
    }
}

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct MapFunctionType {}

impl FunctionType for MapFunctionType {
    fn _resolve_arguments(
        &self,
        actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if let Type::String(Some(key)) = &actual_arguments[1] {
            let element = match &actual_arguments[0] {
                Type::List(ListType { element }) | Type::Iterable(IterableType { element }) => {
                    element.get_attribute(key.as_str(), listener)
                }
                Type::Any { hard: true } => Ok(Type::Any { hard: true }),
                _ => {
                    listener.warn(&format!(
                        "map requires a list or iterable argument as the first argument, got {:?}",
                        actual_arguments[0]
                    ));
                    return Ok(Type::Any { hard: false });
                }
            }?;
            Ok(Type::Iterable(IterableType::new(element)))
        } else if matches!(actual_arguments[1], Type::Any { hard: true }) {
            Ok(Type::Any { hard: true })
        } else {
            listener.warn(&format!(
                "map requires a literal string argument as the second argument, got {:?}",
                actual_arguments[1]
            ));
            Ok(Type::Any { hard: false })
        }
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("iterable", false),
            ArgSpec::new("attribute", false),
        ]
    }
}

#[derive(Default, Clone, Debug, Eq, PartialEq)]
pub struct ListFunctionType;

impl FunctionType for ListFunctionType {
    fn _resolve_arguments(
        &self,
        actual_arguments: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if actual_arguments.len() != 1 {
            listener.warn("list requires exactly 1 argument");
            return Ok(Type::Any { hard: false });
        }
        let element = match &actual_arguments[0] {
            Type::List(ListType { element }) | Type::Iterable(IterableType { element }) => element,
            Type::Any { hard: true } => {
                return Ok(Type::List(ListType::new(Type::Any { hard: true })))
            }
            _ => {
                listener.warn(&format!(
                    "list requires a list or iterable argument, got {:?}",
                    actual_arguments[0]
                ));
                return Ok(Type::Any { hard: false });
            }
        };
        Ok(Type::List(ListType::new(*element.clone())))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("iterable", false)]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct TryOrCompilerErrorFunctionType;

impl fmt::Debug for TryOrCompilerErrorFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "try_or_compiler_error")
    }
}

impl FunctionType for TryOrCompilerErrorFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if args.len() <= 3 {
            listener.warn("Expected at least 3 arguments for try_or_compiler_error function");
            return Ok(Type::Any { hard: false });
        }
        if !args[0].is_compatible_with(&Type::String(None)) {
            listener.warn("Expected a string argument for try_or_compiler_error function");
            return Ok(Type::Any { hard: false });
        }
        if let Type::Object(_callable) = &args[1] {
            // It is not possible to resolve the module here.
        } else if !&args[1].is_none() {
            listener.warn(&format!(
                "Expected an optional argument for try_or_compiler_error function, got {:?}",
                args[1]
            ));
            return Ok(Type::Any { hard: false });
        }
        if !args[2].is_compatible_with(&Type::String(None)) {
            // It is not possible to resolve the arguments of the function,
            // because the function args are not known.
            // let rest_args = args[2..].to_vec();
            // func.resolve_arguments(&rest_args)
            listener.warn(&format!(
                "Expected a string argument for try_or_compiler_error function, got {:?}",
                args[2]
            ));
            return Ok(Type::Any { hard: false });
        }
        Ok(Type::Any { hard: true })
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("message_if_exception", false),
            ArgSpec::new("func", false),
            ArgSpec::new("args", false), // TODO: arg number depends on the function
        ]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct SelectAttrFunctionType;

impl fmt::Debug for SelectAttrFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("selectattr")
    }
}

impl FunctionType for SelectAttrFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::List(ListType::new(Type::Any { hard: true }))) {
            listener.warn(&format!(
                "Expected a list argument for selectattr function, got {:?}",
                args[0]
            ));
            return Ok(Type::Any { hard: false });
        }
        if !args[1].is_compatible_with(&Type::String(None)) {
            listener.warn(&format!(
                "Expected a string argument for selectattr function, got {:?}",
                args[1]
            ));
            return Ok(Type::Any { hard: false });
        }
        if !args[2].is_compatible_with(&Type::String(None)) {
            listener.warn(&format!(
                "Expected a string argument for selectattr function, got {:?}",
                args[2]
            ));
            return Ok(Type::Any { hard: false });
        }
        // TODO check the args[3] based on the op

        Ok(args[0].clone())
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("list", false),
            ArgSpec::new("name", false),
            ArgSpec::new("op", false),
            ArgSpec::new("inside_transaction", true),
        ]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct RejectAttrFunctionType;

impl fmt::Debug for RejectAttrFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("rejectattr")
    }
}

impl FunctionType for RejectAttrFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::List(ListType::new(Type::Any { hard: true }))) {
            listener.warn(&format!(
                "Expected a list argument for rejectattr function, got {:?}",
                args[0]
            ));
            return Ok(Type::Any { hard: false });
        }
        if !args[1].is_compatible_with(&Type::String(None)) {
            listener.warn(&format!(
                "Expected a string argument for rejectattr function, got {:?}",
                args[1]
            ));
            return Ok(Type::Any { hard: false });
        }
        if !args[2].is_compatible_with(&Type::String(None)) {
            listener.warn(&format!(
                "Expected a string argument for rejectattr function, got {:?}",
                args[2]
            ));
            return Ok(Type::Any { hard: false });
        }
        // TODO check the args[3] based on the op

        Ok(args[0].clone())
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("list", false),
            ArgSpec::new("name", false),
            ArgSpec::new("op", false),
            ArgSpec::new("inside_transaction", true),
        ]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct PrintFunctionType;

impl fmt::Debug for PrintFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("print")
    }
}

impl FunctionType for PrintFunctionType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::None)
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct FirstFunctionType;

impl fmt::Debug for FirstFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("first")
    }
}

impl FunctionType for FirstFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match &args[0] {
            Type::List(ListType { element, .. }) => Ok(element.as_ref().clone()),
            _ => {
                listener.warn("Expected a list argument for first function");
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("iterable", false)]
    }
}

#[derive(Default, Clone, Eq, PartialEq)]
pub struct BatchFunctionType;

impl fmt::Debug for BatchFunctionType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("batch")
    }
}

impl FunctionType for BatchFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[1].is_compatible_with(&Type::Integer(None)) {
            listener.warn("Expected an integer argument for batch function");
        }

        match &args[0] {
            Type::List(ListType { element }) => Ok(Type::List(ListType::new(Type::List(
                ListType::new(*element.clone()),
            )))),
            Type::Iterable(IterableType { element }) => Ok(Type::List(ListType::new(Type::List(
                ListType::new(*element.clone()),
            )))),
            Type::Any { hard: true } => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn("Expected a list or iterable argument for batch function");
                Ok(Type::Any { hard: false })
            }
        }
    }
    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("value", false),
            ArgSpec::new("count", false),
            ArgSpec::new("fill_with", true),
        ]
    }
}

/// Helper function to check if a type is a collection type that filters don't support
fn is_unsupported_filter_input(input_type: &Type) -> Option<&'static str> {
    match input_type {
        Type::List(_) => Some("sequence"),
        Type::Dict(_) => Some("map"),
        Type::Iterable(_) => Some("iterable"),
        Type::Tuple(_) => Some("sequence"),
        Type::Plain => Some("plain"),
        _ => None,
    }
}

/// Filter type for `as_bool` - passes through the value as-is.
///
/// This filter accepts any value and returns `Any { hard: true }`.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct AsBoolFilterType;

impl fmt::Debug for AsBoolFilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("as_bool")
    }
}

impl FunctionType for AsBoolFilterType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        // as_bool only converts while rendering native values, so for
        // typechecking purposes it passes the value through as-is
        Ok(Type::Any { hard: true })
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}

/// Filter type for `as_number` - converts a value to number (integer).
///
/// This filter does not support collection types (sequences, dicts, iterables, tuples, plain).
#[derive(Default, Clone, Eq, PartialEq)]
pub struct AsNumberFilterType;

impl fmt::Debug for AsNumberFilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("as_number")
    }
}

impl FunctionType for AsNumberFilterType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if let Some(input_type) = args.first() {
            if let Some(type_name) = is_unsupported_filter_input(input_type) {
                listener.warn_filter(&format!("as_number on {type_name} not supported"));
            }
        }
        Ok(Type::Integer(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}

/// Filter type for `as_text` - converts a value to string.
///
/// This filter does not support collection types (sequences, dicts, iterables, tuples, plain).
#[derive(Default, Clone, Eq, PartialEq)]
pub struct AsTextFilterType;

impl fmt::Debug for AsTextFilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("as_text")
    }
}

impl FunctionType for AsTextFilterType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if let Some(input_type) = args.first() {
            if let Some(type_name) = is_unsupported_filter_input(input_type) {
                listener.warn_filter(&format!("as_text on {type_name} not supported"));
            }
        }
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}

/// Filter type for `as_native` - passes through the value as-is.
///
/// This filter accepts any value and returns `Any { hard: true }`.
#[derive(Default, Clone, Eq, PartialEq)]
pub struct AsNativeFilterType;

impl fmt::Debug for AsNativeFilterType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("as_native")
    }
}

impl FunctionType for AsNativeFilterType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        // as_native just passes through the value as-is
        Ok(Type::Any { hard: true })
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}
