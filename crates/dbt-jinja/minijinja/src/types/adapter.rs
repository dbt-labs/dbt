use crate::compiler::typecheck::FunctionRegistry;
use crate::constants::{
    DBT_AND_ADAPTERS_NAMESPACE, DIALECT, MACRO_DISPATCH_ORDER, MACRO_NAMESPACE_REGISTRY,
    ROOT_PACKAGE_NAME,
};
use crate::dispatch_object::{dispatch_search_packages, get_adapter_prefixes};
use crate::types::function::{ArgSpec, FunctionType};
use crate::types::Type;
use crate::vm::typemeta::macro_namespace_template_resolver;
use crate::{TypecheckingEventListener, Value};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;

/// Typechecks dispatch against one template's dialect and macro registry.
#[derive(Clone, Default)]
pub struct AdapterDispatchFunction {
    context: BTreeMap<String, Value>,
    function_registry: Arc<FunctionRegistry>,
}

impl AdapterDispatchFunction {
    pub(crate) fn new(
        context: BTreeMap<String, Value>,
        function_registry: Arc<FunctionRegistry>,
    ) -> Self {
        Self {
            context,
            function_registry,
        }
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
        if let Some(Type::String(Some(name))) = args.first() {
            let dialect = self
                .context
                .get(DIALECT)
                .and_then(Value::as_str)
                .unwrap_or("postgres");
            let namespace = match args.get(1) {
                Some(Type::String(Some(namespace))) => Some(namespace.as_str()),
                Some(Type::None) | None => None,
                _ => return Ok(Type::Any { hard: false }),
            };
            let root = self
                .context
                .get(ROOT_PACKAGE_NAME)
                .and_then(Value::as_str)
                .unwrap_or("dbt");
            let packages = dispatch_search_packages(
                namespace,
                root,
                self.context
                    .get(MACRO_DISPATCH_ORDER)
                    .unwrap_or(&Value::UNDEFINED),
                |namespace| {
                    self.context
                        .get(MACRO_NAMESPACE_REGISTRY)
                        .and_then(|registry| registry.get_item(&Value::from(namespace)).ok())
                        .is_some_and(|value| !value.is_undefined())
                },
            );
            for package in packages {
                for prefix in get_adapter_prefixes(dialect) {
                    let search_name = format!("{prefix}__{name}");
                    let qualified = match package.as_deref() {
                        Some("dbt") => self
                            .context
                            .get(DBT_AND_ADAPTERS_NAMESPACE)
                            .and_then(|map| map.get_item(&Value::from(dialect)).ok())
                            .and_then(|map| map.get_item(&Value::from(search_name.as_str())).ok())
                            .and_then(|package| {
                                package
                                    .as_str()
                                    .map(|package| format!("{package}.{search_name}"))
                            }),
                        Some(package) => Some(format!("{package}.{search_name}")),
                        None => macro_namespace_template_resolver(
                            &self.context,
                            self.function_registry.clone(),
                            &search_name,
                            &mut Vec::new(),
                        ),
                    };
                    if let Some(function) = qualified
                        .as_ref()
                        .and_then(|name| self.function_registry.get(name))
                    {
                        return Ok(Type::Object(function.clone()));
                    }
                }
                if package.is_none() {
                    if let Some(function) = macro_namespace_template_resolver(
                        &self.context,
                        self.function_registry.clone(),
                        name,
                        &mut Vec::new(),
                    )
                    .as_ref()
                    .and_then(|name| self.function_registry.get(name))
                    {
                        return Ok(Type::Object(function.clone()));
                    }
                }
            }

            listener.warn(&format!(
                "Function {name} not found in any supported adapter"
            ));
            Ok(Type::Any { hard: false })
        } else {
            listener.warn("Expected literal string for first argument of adapter.dispatch");
            Ok(Type::Any { hard: false })
        }
    }

    fn arg_specs(&self) -> Vec<ArgSpec> {
        vec![
            ArgSpec::new("macro_name", false),
            ArgSpec::new("macro_namespace", true),
        ]
    }
}
