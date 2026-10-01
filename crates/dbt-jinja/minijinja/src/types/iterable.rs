use std::rc::Rc;

use crate::{
    types::{Object, Type},
    TypecheckingEventListener,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IterableType {
    pub element: Box<Type>,
}

impl IterableType {
    pub fn new(element: Type) -> Self {
        Self {
            element: Box::new(element),
        }
    }
}

impl Object for IterableType {
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
}
