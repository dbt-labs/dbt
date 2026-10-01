use std::rc::Rc;

use crate::{
    types::{Object, Type},
    TypecheckingEventListener,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TupleType {
    pub fields: Vec<Type>,
}

impl TupleType {
    pub fn new(fields: Vec<Type>) -> Self {
        Self { fields }
    }
}

impl Object for TupleType {
    fn get_attribute(
        &self,
        _key: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        listener.warn("Tuple does not support attribute access");
        Ok(Type::Any { hard: false })
    }

    fn subscript(
        &self,
        index: &Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        match index {
            Type::Integer(Some(index)) => {
                if *index < 0 || *index >= self.fields.len() as i64 {
                    listener.warn(&format!("Index out of range: {index}"));
                    return Ok(Type::Any { hard: false });
                }
                Ok(self.fields[*index as usize].clone())
            }
            Type::Integer(None) => Ok(Type::Any { hard: true }),
            Type::Any { hard: true } => Ok(Type::Any { hard: true }),
            _ => {
                listener.warn(&format!("Failed to subscript {self:?} with {index:?}"));
                Ok(Type::Any { hard: false })
            }
        }
    }
}
