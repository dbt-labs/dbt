use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use crate::{
    types::{
        function::{ArgSpec, FunctionType},
        list::ListType,
        tuple::TupleType,
        DynObject, Object, Type,
    },
    TypecheckingEventListener,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructType {
    pub fields: BTreeMap<String, Type>,
}

impl StructType {
    pub fn new(fields: BTreeMap<String, Type>) -> Self {
        Self { fields }
    }
}

impl Object for StructType {
    fn get_attribute(
        &self,
        key: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match key {
            "get" => Ok(Type::Object(DynObject::new(Arc::new(
                StructGetFunctionType {
                    fields: self.fields.clone(),
                },
            )))),
            "update" => Ok(Type::Object(DynObject::new(Arc::new(
                StructUpdateFunctionType {
                    fields: self.fields.clone(),
                },
            )))),
            "copy" => Ok(Type::Object(DynObject::new(Arc::new(
                StructCopyFunctionType {
                    fields: self.fields.clone(),
                },
            )))),
            "items" => Ok(Type::Object(DynObject::new(Arc::new(
                StructItemsFunctionType {
                    fields: self.fields.clone(),
                },
            )))),
            key => {
                if let Some(field_type) = self.fields.get(key) {
                    Ok(field_type.clone())
                } else {
                    listener.warn(&format!("Struct does not have field {key}"));
                    Ok(Type::Any { hard: false })
                }
            }
        }
    }

    fn subscript(
        &self,
        index: &Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match index {
            Type::String(Some(index)) => self.get_attribute(index, listener),
            Type::String(None) => Ok(Type::Any { hard: true }),
            Type::Any { hard: true } => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn(&format!("Failed to subscript {self:?} with {index:?}"));
                Ok(Type::Any { hard: false })
            }
        }
    }
}

#[derive(Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructGetFunctionType {
    pub fields: BTreeMap<String, Type>,
}

impl std::fmt::Debug for StructGetFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructGetFunctionType")
    }
}

impl FunctionType for StructGetFunctionType {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match (&args[0], &args[1]) {
            (Type::String(Some(field_name)), default) => {
                if let Some(field_type) = self.fields.get(field_name) {
                    Ok(field_type.clone().union(default))
                } else {
                    listener.warn(&format!("Struct does not have field {field_name}"));
                    Ok(Type::Any { hard: false })
                }
            }
            (Type::String(None), default) if !matches!(default, Type::None) => Ok(default.clone()),
            (Type::String(None), Type::None) => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn(&format!("Expected string, got {:?}", args[0]));
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("field_name", false),
            ArgSpec::new("default", true),
        ]
    }
}

#[derive(Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructUpdateFunctionType {
    pub fields: BTreeMap<String, Type>,
}

impl std::fmt::Debug for StructUpdateFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructUpdateFunctionType")
    }
}

impl FunctionType for StructUpdateFunctionType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::None)
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("key", false), ArgSpec::new("value", true)]
    }
}

#[derive(Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructCopyFunctionType {
    pub fields: BTreeMap<String, Type>,
}

impl std::fmt::Debug for StructCopyFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructCopyFunctionType")
    }
}

impl FunctionType for StructCopyFunctionType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::Object(DynObject::new(Arc::new(StructType {
            fields: self.fields.clone(),
        }))))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}

#[derive(Default, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StructItemsFunctionType {
    pub fields: BTreeMap<String, Type>,
}

impl std::fmt::Debug for StructItemsFunctionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructItemsFunctionType")
    }
}

impl FunctionType for StructItemsFunctionType {
    fn _resolve_arguments(
        &self,
        _args: &[Type],
        _listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        Ok(Type::List(ListType::new(Type::Tuple(TupleType::new(
            vec![Type::Any { hard: true }, Type::Any { hard: true }],
        )))))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![]
    }
}
