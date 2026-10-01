use std::{path::PathBuf, rc::Rc, sync::Arc};

use dashmap::DashMap;
use indexmap::IndexMap;

use crate::{
    machinery::Span,
    types::{
        funcsign_parser::parse_type,
        function::{
            Argument, BatchFunctionType, FirstFunctionType, FunctionType, ListFunctionType,
            MapFunctionType, PrintFunctionType, RejectAttrFunctionType, SelectAttrFunctionType,
            TryOrCompilerErrorFunctionType, UserDefinedFunctionType,
        },
        DynObject, Object, Type,
    },
    TypecheckingEventListener, Value,
};

// Singleton for builtins registry
static BUILTINS_REGISTRY: std::sync::OnceLock<Arc<DashMap<String, Type>>> =
    std::sync::OnceLock::new();
static JINJA_BUILTINS_REGISTRY: std::sync::OnceLock<Arc<DashMap<String, Type>>> =
    std::sync::OnceLock::new();

/// Initialize built-in types registry (internal function)
fn init_jinja_builtins() -> Arc<DashMap<String, Type>> {
    let definitions = minijinja_typecheck_builtins::get_definitions();
    let registry = Arc::new(DashMap::new()); // key is object id, value is Type

    for definition in definitions.iter() {
        let definition: Definition = definition.clone().into();
        let id = definition.get_id();
        let dyn_object = DynObject::new(Arc::new(BuiltinDefinition::new(
            &definition,
            registry.clone(),
        )));
        registry.insert(id, Type::Object(dyn_object));
    }
    registry.insert(
        "adapter.dispatch".to_string(),
        Type::Object(DynObject::new(Arc::new(
            crate::types::adapter::AdapterDispatchFunction::instance(),
        ))),
    );
    registry.insert(
        "map".to_string(),
        Type::Object(DynObject::new(Arc::new(MapFunctionType::default()))),
    );
    registry.insert(
        "list".to_string(),
        Type::Object(DynObject::new(Arc::new(ListFunctionType::default()))),
    );
    registry.insert(
        "try_or_compiler_error".to_string(),
        Type::Object(DynObject::new(Arc::new(
            TryOrCompilerErrorFunctionType::default(),
        ))),
    );
    registry.insert(
        "selectattr".to_string(),
        Type::Object(DynObject::new(Arc::new(SelectAttrFunctionType::default()))),
    );
    registry.insert(
        "rejectattr".to_string(),
        Type::Object(DynObject::new(Arc::new(RejectAttrFunctionType::default()))),
    );
    registry.insert(
        "print".to_string(),
        Type::Object(DynObject::new(Arc::new(PrintFunctionType::default()))),
    );
    registry.insert(
        "first".to_string(),
        Type::Object(DynObject::new(Arc::new(FirstFunctionType::default()))),
    );
    registry.insert(
        "batch".to_string(),
        Type::Object(DynObject::new(Arc::new(BatchFunctionType::default()))),
    );

    registry
}

/// Load built-in types with namespace registry
///
/// This function loads the Jinja built-in types and optionally extends them with
/// namespace types from the provided namespace registry. The namespace registry
/// contains package names that should be available as namespace types for macro
/// resolution and dispatch.
///
/// The function uses caching to ensure the registry is initialized only once
/// per unique namespace configuration and reused for subsequent calls.
///
/// # Arguments
/// * `namespace_registry` - Optional registry containing package names as keys.
///   If provided, each package name will be added as a `Type::Namespace`.
///   If `None`, only the base Jinja built-ins are returned.
///
/// # Returns
/// A shared reference to the cached map of type names to `Type` objects,
/// including both Jinja built-ins and namespace types.
///
/// # Errors
/// Returns a `crate::Error` if there are issues during type initialization.
pub fn load_builtins_with_namespace(
    namespace_registry: Option<Arc<IndexMap<Value, Value>>>,
) -> Result<Arc<DashMap<String, Type>>, crate::Error> {
    if let Some(namespace_registry) = namespace_registry {
        Ok(BUILTINS_REGISTRY
            .get_or_init(|| {
                let namespace_registry = namespace_registry
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .iter()
                    .map(|k| k.as_str().unwrap().to_string())
                    .collect::<Vec<_>>();
                let builtins = load_jinja_builtins();
                for name in namespace_registry {
                    builtins.insert(name.clone(), Type::Namespace(name));
                }
                builtins
            })
            .clone())
    } else {
        Ok(load_jinja_builtins())
    }
}

/// load dbt jinja built-in types (singleton version)
///
/// Should be called before loading any pakcages.
/// This function returns a cached version of the dbt jinja built-in types registry.
/// The registry is initialized only once and reused for subsequent calls.
///
/// # Returns
/// A shared reference to the cached map of object id to Type.
pub fn load_jinja_builtins() -> Arc<DashMap<String, Type>> {
    JINJA_BUILTINS_REGISTRY
        .get_or_init(init_jinja_builtins)
        .clone()
}

#[derive(Clone)]
pub(crate) enum Definition {
    Object(minijinja_typecheck_builtins::Object),
    Alias(minijinja_typecheck_builtins::Alias),
}

impl From<minijinja_typecheck_builtins::Definition> for Definition {
    fn from(definition: minijinja_typecheck_builtins::Definition) -> Self {
        if let Some(object) = definition.object {
            Definition::Object(object)
        } else if let Some(alias) = definition.alias {
            Definition::Alias(alias)
        } else {
            panic!("Definition has no object or alias");
        }
    }
}

impl Definition {
    pub(crate) fn get_id(&self) -> String {
        match self {
            Definition::Object(object) => object.id.clone(),
            Definition::Alias(alias) => alias.id.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct BuiltinDefinition {
    pub(crate) definition: Definition,
    registry: Arc<DashMap<String, Type>>,
}

impl std::fmt::Debug for BuiltinDefinition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.definition.get_id())
    }
}

impl BuiltinDefinition {
    pub(crate) fn new(definition: &Definition, registry: Arc<DashMap<String, Type>>) -> Self {
        Self {
            definition: definition.clone(),
            registry,
        }
    }

    pub(crate) fn get_alias_type(&self) -> Option<Type> {
        match &self.definition {
            Definition::Alias(alias) => {
                Some(parse_type(alias.type_.as_str(), self.registry.clone()).unwrap())
            }
            Definition::Object(_) => None,
        }
    }
}

impl Object for BuiltinDefinition {
    fn get_attribute(
        &self,
        name: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        match &self.definition {
            Definition::Alias(alias) => {
                let object =
                    parse_type(alias.type_.as_str(), self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {} failed: {}", alias.type_, e),
                        )
                    })?;
                object.get_attribute(name, listener)
            }
            Definition::Object(object) => {
                if let Some(attributes) = &object.attributes {
                    for attribute in attributes {
                        if attribute.name == name {
                            return parse_type(attribute.type_.as_str(), self.registry.clone())
                                .map_err(|e| {
                                    crate::Error::new(
                                        crate::error::ErrorKind::InvalidOperation,
                                        std::format!(
                                            "parse type {} failed: {}",
                                            attribute.type_,
                                            e
                                        ),
                                    )
                                });
                        }
                    }
                }
                if let Some(parent) = &object.inherit_from {
                    let parent_object = parse_type(parent, self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {parent} failed: {e}"),
                        )
                    })?;
                    return parent_object.get_attribute(name, listener).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("Failed to get {self:?}.{name} from {parent}: {e}"),
                        )
                    });
                }
                listener.warn(&format!("{self:?}.{name} does not exist"));
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn call(
        &self,
        positional_args: &[super::Type],
        kwargs: &std::collections::BTreeMap<String, super::Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        match &self.definition {
            Definition::Alias(alias) => {
                let object =
                    parse_type(alias.type_.as_str(), self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {} failed: {}", alias.type_, e),
                        )
                    })?;
                object.call(positional_args, kwargs, listener)
            }
            Definition::Object(object) => {
                if let Some(call) = &object.call {
                    let args = call
                        .arguments
                        .iter()
                        .map(|arg| {
                            Ok(Argument {
                                name: arg.name.clone(),
                                type_: parse_type(arg.type_.as_str(), self.registry.clone())
                                    .map_err(|e| {
                                        crate::Error::new(
                                            crate::error::ErrorKind::InvalidOperation,
                                            std::format!("parse type {} failed: {}", arg.type_, e),
                                        )
                                    })?,
                                is_optional: arg.is_optional,
                            })
                        })
                        .collect::<Result<Vec<Argument>, crate::Error>>()?;
                    let ret_type = parse_type(call.return_type.as_str(), self.registry.clone())
                        .map_err(|e| {
                            crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                std::format!("parse type {} failed: {}", call.return_type, e),
                            )
                        })?;
                    let udf = UserDefinedFunctionType::new(
                        "udf",
                        args,
                        ret_type,
                        &PathBuf::from(""),
                        &Span::default(),
                        "",
                    );
                    return udf.resolve_arguments(positional_args, kwargs, listener);
                }
                if let Some(parent) = &object.inherit_from {
                    let parent_object = parse_type(parent, self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {parent} failed: {e}"),
                        )
                    })?;
                    return parent_object
                        .call(positional_args, kwargs, listener)
                        .map_err(|e| {
                            crate::Error::new(
                                crate::error::ErrorKind::InvalidOperation,
                                std::format!("Failed to call {self:?} from {parent}: {e}"),
                            )
                        });
                }
                listener.warn(&format!("{self:?} does not support call"));
                Ok(Type::Any { hard: false })
            }
        }
    }

    fn subscript(
        &self,
        index: &super::Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        match &self.definition {
            Definition::Alias(alias) => {
                let object =
                    parse_type(alias.type_.as_str(), self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {} failed: {}", alias.type_, e),
                        )
                    })?;
                object.subscript(index, listener)
            }
            Definition::Object(object) => {
                if let Some(parent) = &object.inherit_from {
                    let parent_object = parse_type(parent, self.registry.clone()).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("parse type {parent} failed: {e}"),
                        )
                    })?;
                    return parent_object.subscript(index, listener).map_err(|e| {
                        crate::Error::new(
                            crate::error::ErrorKind::InvalidOperation,
                            std::format!("Failed to subscript {self:?} from {parent}: {e}"),
                        )
                    });
                }
                listener.warn(&format!("{self:?} does not support subscript"));
                Ok(Type::Any { hard: false })
            }
        }
    }
}

pub(crate) struct Reference {
    id: String,
    registry: Arc<DashMap<String, Type>>,
}

impl std::fmt::Debug for Reference {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.id)
    }
}

impl Reference {
    pub(crate) fn new(id: String, registry: Arc<DashMap<String, Type>>) -> Self {
        Self { id, registry }
    }

    pub(crate) fn get_type(&self) -> Result<Type, crate::Error> {
        let type_ = self.registry.get(&self.id);
        if let Some(type_) = type_ {
            Ok(type_.value().clone())
        } else {
            Err(crate::Error::new(
                crate::error::ErrorKind::InvalidOperation,
                format!("Unknown type: {}", self.id),
            ))
        }
    }
}

impl Object for Reference {
    fn get_attribute(
        &self,
        name: &str,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        let type_ = self.registry.get(&self.id);
        if let Some(type_) = type_ {
            type_.value().get_attribute(name, listener)
        } else {
            Err(crate::Error::new(
                crate::error::ErrorKind::InvalidOperation,
                format!("Unknown type: {}", self.id),
            ))
        }
    }

    fn call(
        &self,
        positional_args: &[super::Type],
        kwargs: &std::collections::BTreeMap<String, super::Type>,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        let type_ = self.registry.get(&self.id);
        if let Some(type_) = type_ {
            type_.value().call(positional_args, kwargs, listener)
        } else {
            Err(crate::Error::new(
                crate::error::ErrorKind::InvalidOperation,
                format!("Unknown type: {}", self.id),
            ))
        }
    }

    fn subscript(
        &self,
        index: &super::Type,
        listener: Rc<dyn TypecheckingEventListener>,
    ) -> Result<super::Type, crate::Error> {
        let type_ = self.registry.get(&self.id);
        if let Some(type_) = type_ {
            type_.value().subscript(index, listener)
        } else {
            Err(crate::Error::new(
                crate::error::ErrorKind::InvalidOperation,
                format!("Unknown type: {}", self.id),
            ))
        }
    }
}
