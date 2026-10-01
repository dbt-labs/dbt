use crate::compiler::typecheck::FunctionRegistry;
use crate::types::function::{ArgSpec, FunctionType};
use crate::types::Type;
use crate::TypecheckingEventListener;
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone)]
/// AdapterDispatchFunction is a singleton that type check adapter.dispatch
pub struct AdapterDispatchFunction {
    adapter_type: Arc<Mutex<Option<String>>>,
    function_registry: Arc<Mutex<Option<Arc<FunctionRegistry>>>>,
}

// Singleton instance storage
static ADAPTER_DISPATCH_INSTANCE: OnceLock<AdapterDispatchFunction> = OnceLock::new();

impl AdapterDispatchFunction {
    fn new(
        adapter_type: Arc<Mutex<Option<String>>>,
        function_registry: Arc<Mutex<Option<Arc<FunctionRegistry>>>>,
    ) -> Self {
        Self {
            adapter_type,
            function_registry,
        }
    }

    /// Get the singleton instance of AdapterDispatchFunction
    pub fn instance() -> Self {
        ADAPTER_DISPATCH_INSTANCE
            .get_or_init(|| {
                AdapterDispatchFunction::new(Arc::new(Mutex::new(None)), Arc::new(Mutex::new(None)))
            })
            .clone()
    }

    /// Get a reference to the function registry
    pub fn function_registry(&self) -> &Arc<Mutex<Option<Arc<FunctionRegistry>>>> {
        &self.function_registry
    }

    /// Set the function registry
    pub fn set_function_registry(&self, new_registry: Arc<FunctionRegistry>) {
        let mut registry = self.function_registry.lock().unwrap();
        *registry = Some(new_registry);
    }

    /// Set the adapter type
    pub fn set_adapter_type(&self, new_adapter_type: &Option<String>) {
        *self.adapter_type.lock().unwrap() = new_adapter_type.clone();
    }
}

impl std::fmt::Debug for AdapterDispatchFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("adapter.dispatch")
    }
}

impl FunctionType for AdapterDispatchFunction {
    fn _resolve_arguments(
        &self,
        args: &[Type],
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<Type, crate::Error> {
        if let Some(Type::String(Some(name))) = args.get(0) {
            if let Ok(registry_opt) = self.function_registry.lock() {
                if let Some(ref registry) = *registry_opt {
                    if let Some(adapter_type) = self.adapter_type.lock().unwrap().as_ref() {
                        let key = format!("{adapter_type}__{name}");
                        if let Some(func) = registry.get(&key) {
                            return Ok(Type::Object(func.clone()));
                        }
                    }
                    // try default adapter
                    let key = format!("default__{name}");
                    if let Some(func) = registry.get(&key) {
                        return Ok(Type::Object(func.clone()));
                    }

                    listener.warn(&format!(
                        "Function {name} not found in any supported adapter"
                    ));
                    Ok(Type::Any { hard: false })
                } else {
                    Err(crate::Error::new(
                        crate::error::ErrorKind::InvalidOperation,
                        "Function registry not initialized",
                    ))
                }
            } else {
                Err(crate::Error::new(
                    crate::error::ErrorKind::InvalidOperation,
                    "Failed to lock function registry",
                ))
            }
        } else {
            listener.warn("Expected literal string for first argument of adapter.dispatch");
            Ok(Type::Any { hard: false })
        }
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![ArgSpec::new("name", false)]
    }
}
