use std::rc::Rc;

use crate::{
    types::{
        function::{ArgSpec, FunctionType},
        Type,
    },
    TypecheckingEventListener,
};

#[derive(Default, Clone, Eq, PartialEq)]
pub struct PyDateTimeStrftimeFunction {}

impl std::fmt::Debug for PyDateTimeStrftimeFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("timestamp.strftime")
    }
}

impl FunctionType for PyDateTimeStrftimeFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if !args[0].is_compatible_with(&Type::String(None)) {
            listener.warn(&format!("Expected string, got {}", args[0]));
        }
        Ok(Type::String(None))
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("format", false)]
    }
}
