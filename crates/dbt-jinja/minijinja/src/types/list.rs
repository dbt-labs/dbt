use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use crate::{
    types::{
        function::{ArgSpec, FunctionType},
        DynObject, Object, Type,
    },
    TypecheckingEventListener,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ListType {
    pub element: Box<Type>,
}

impl ListType {
    pub fn new(element: Type) -> Self {
        Self {
            element: Box::new(element),
        }
    }
}

impl Object for ListType {
    fn get_attribute(
        &self,
        key: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match key {
            "append" => Ok(Type::Object(DynObject::new(Arc::new(
                ListAppendFunctionType::new(*self.element.clone()),
            )))),
            "extend" => Ok(Type::Object(DynObject::new(Arc::new(
                ListExtendFunctionType::new(*self.element.clone()),
            )))),
            _ => {
                listener.warn(&format!("{self:?}.{key} is not supported"));
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn subscript(
        &self,
        index: &Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match index {
            Type::Integer(_) => Ok(*self.element.clone()),
            Type::Any { hard: true } => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn(&format!("Failed to subscript {self:?} with {index:?}"));
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn call(
        &self,
        _positional_args: &[Type],
        _kwargs: &BTreeMap<String, Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        listener.warn("List does not support call");
        Ok(Type::Any { hard: false })
    }
}

pub struct ListAppendFunctionType {
    pub element: Box<Type>,
}

impl ListAppendFunctionType {
    pub fn new(element: Type) -> Self {
        Self {
            element: Box::new(element),
        }
    }
}

impl std::fmt::Debug for ListAppendFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "list.append")
    }
}

impl FunctionType for ListAppendFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(self.element.as_ref()) {
            listener.warn(&format!(
                "list.append expected same type with list element {}, got {}",
                self.element, args[0]
            ));
        }
        Ok(Type::None)
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}

pub struct ListExtendFunctionType {
    pub element: Box<Type>,
}

impl std::fmt::Debug for ListExtendFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "list.extend")
    }
}

impl ListExtendFunctionType {
    pub fn new(element: Type) -> Self {
        Self {
            element: Box::new(element),
        }
    }
}
impl FunctionType for ListExtendFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::List(ListType::new(*self.element.clone()))) {
            listener.warn(&format!(
                "list.extend expected same type with list {}, got {}",
                self.element, args[0]
            ));
        }
        Ok(Type::None)
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("value", false)]
    }
}
