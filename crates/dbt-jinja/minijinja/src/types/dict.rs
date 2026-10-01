use std::{rc::Rc, sync::Arc};

use crate::{
    types::{
        function::{ArgSpec, FunctionType},
        iterable::IterableType,
        tuple::TupleType,
        DynObject, Object, Type,
    },
    TypecheckingEventListener,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DictType {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

impl DictType {
    pub fn new(key: Type, value: Type) -> Self {
        Self {
            key: Box::new(key),
            value: Box::new(value),
        }
    }
}

impl Object for DictType {
    fn get_attribute(
        &self,
        key: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match key {
            "items" => Ok(Type::Object(DynObject::new(Arc::new(
                DictItemsFunction::new(*self.key.clone(), *self.value.clone()),
            )))),
            "get" => Ok(Type::Object(DynObject::new(Arc::new(
                DictGetFunction::new(*self.key.clone(), *self.value.clone()),
            )))),
            "keys" => Ok(Type::Object(DynObject::new(Arc::new(
                DictKeysFunction::new(*self.key.clone(), *self.value.clone()),
            )))),
            _ => {
                listener.warn(&format!("dict.{key} is not supported"));
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
            Type::String(_) | Type::Integer(_) => Ok(*self.value.clone()),
            Type::Any { hard: true } => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn(&format!("Failed to subscript {self:?} with {index:?}"));
                Ok(Type::Any { hard: false })
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct DictItemsFunction {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

impl DictItemsFunction {
    pub fn new(key: Type, value: Type) -> Self {
        Self {
            key: Box::new(key),
            value: Box::new(value),
        }
    }
}

impl FunctionType for DictItemsFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args.is_empty() {
            listener.warn("dict.items() takes 0 arguments");
        }
        let element_type =
            Type::Tuple(TupleType::new(vec![*self.key.clone(), *self.value.clone()]));
        Ok(Type::Iterable(IterableType::new(element_type)))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}

#[derive(Clone)]
pub struct DictGetFunction {
    pub key: Box<Type>,
    pub value: Box<Type>,
}

impl DictGetFunction {
    pub fn new(key: Type, value: Type) -> Self {
        Self {
            key: Box::new(key),
            value: Box::new(value),
        }
    }
}

impl std::fmt::Debug for DictGetFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dict.get")
    }
}

impl FunctionType for DictGetFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(self.key.as_ref()) {
            listener.warn(&format!(
                "dict.get() expected key type {self:?}, got {:?}",
                args[0]
            ));
        }
        Ok(*self.value.clone())
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("key", false), ArgSpec::new("default", true)]
    }
}

#[derive(Debug, Clone)]
pub struct DictKeysFunction {
    pub key: Box<Type>,
    pub _value: Box<Type>,
}

impl DictKeysFunction {
    pub fn new(key: Type, value: Type) -> Self {
        Self {
            key: Box::new(key),
            _value: Box::new(value),
        }
    }
}

impl FunctionType for DictKeysFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args.is_empty() {
            listener.warn("dict.keys() takes 0 arguments");
        }
        Ok(Type::Iterable(IterableType::new(*self.key.clone())))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}
