use crate::{
    types::{
        function::{ArgSpec, FunctionType},
        list::ListType,
        Type,
    },
    TypecheckingEventListener,
};
use std::rc::Rc;

#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StringStripFunction {}

impl FunctionType for StringStripFunction {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StringLowerFunction {}

impl FunctionType for StringLowerFunction {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StringUpperFunction {}

impl FunctionType for StringUpperFunction {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}

#[derive(Debug, Default, Eq, PartialEq, Clone)]
pub struct StringReplaceFunction {}

impl FunctionType for StringReplaceFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        let old = args[0].clone();
        let new = args[1].clone();
        if matches!(old, Type::String(_)) {
            listener.warn(&format!(
                "Expected a string argument for replace function, got {old:?}",
            ));
        }
        if matches!(new, Type::String(_)) {
            listener.warn(&format!(
                "Expected a string argument for replace function, got {new:?}",
            ));
        }
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("old", false), ArgSpec::new("new", false)]
    }
}

#[derive(Default, Eq, PartialEq, Clone)]
pub struct StringSplitFunction {}

impl std::fmt::Debug for StringSplitFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("string.split")
    }
}

impl FunctionType for StringSplitFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::String(None)) {
            listener.warn("Expected a string argument for split function");
        }
        Ok(Type::List(ListType::new(Type::String(None))))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("sep", false)]
    }
}

#[derive(Default, Eq, PartialEq, Clone)]
pub struct StringFormatFunction {}

impl std::fmt::Debug for StringFormatFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("string.format")
    }
}

impl FunctionType for StringFormatFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::String(None)) {
            listener.warn("Expected a string argument for format function");
        }
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("format_string", false)]
    }
}
