//! Module for the parse config object to be used during parsing

use std::{
    collections::{BTreeMap, HashSet},
    rc::Rc,
    slice,
    sync::Arc,
};

use dbt_common::dashmap::DashMap;
use dbt_common::{
    CodeLocationWithFile, ErrorCode, fs_err, tracing::dbt_emit::emit_warn_log_from_fs_error,
};
use minijinja::{
    Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State, Value,
    arg_utils::ArgParser,
    listener::RenderingEventListener,
    value::{Enumerator, Object},
};

/// A struct that represents a compile time config object to be used during compile time
#[derive(Debug, Clone)]
pub struct CompileConfig {
    /// A map of config values to be used during compile time
    pub config: Arc<DashMap<String, Value>>,
    /// Set of valid config field names for this config type
    pub valid_keys: HashSet<String>,
}

impl Object for CompileConfig {
    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        _args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        Ok(Value::from(""))
    }

    /// Get the value of a key from the config
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.config
            .get(key.as_str().unwrap())
            .map(|v| v.value().clone())
    }

    /// Flatten the wrapper struct so that this Object can be treated like a Map of the inner `config`
    /// This is critical when a Value of this is properly deserialized to the underlying type
    /// Without this, the deserialized instance has its fields all set to default value
    /// See an example of the ManifestModelConfig::deserialize usage in adapters crate
    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let keys = self
            .config
            .iter()
            .map(|v| Value::from(v.key().clone()))
            .collect::<Vec<_>>();
        Enumerator::Iter(Box::new(keys.into_iter()))
    }

    #[allow(clippy::cognitive_complexity)]
    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        match name {
            // At compile time, this will return the value of the config variable if it exists
            // If not found in config, checks config.meta for the key
            "get" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;
                let default = args
                    .get_optional::<Value>("default")
                    .unwrap_or_else(|| Value::from(None::<Option<String>>));

                // First, try to get the value from config.<key>
                let result = match self.config.get(&name) {
                    Some(val) if !val.is_none() => val.clone(),
                    _ => {
                        // Check if key exists in meta and warn if it's a custom config
                        if let Some(meta_value) = self.config.get("meta") {
                            let meta = meta_value.value();
                            if let Some(meta_obj) = meta.as_object()
                                && let Some(value) = meta_obj.get_value(&Value::from(name.clone()))
                                && !value.is_none()
                                && !self.valid_keys.contains(&name)
                            {
                                // Emit warning with location information
                                let span = state.current_span();
                                let path = state.current_path();
                                let location = CodeLocationWithFile::new(
                                    span.start_line,
                                    span.start_col,
                                    span.start_offset,
                                    path.clone(),
                                );
                                let error = fs_err!(
                                    ErrorCode::Generic,
                                    "The key '{}' was not found using config.get('{}'), but was detected as a custom config under 'meta'. \
                                    Please use config.meta_get('{}') or config.meta_require('{}') instead of config.get('{}') \
                                    to access the custom config value if intended.",
                                    name, name, name, name, name
                                ).with_location(location);
                                emit_warn_log_from_fs_error(error);
                            }
                        }
                        // Always return default, don't return meta values
                        default
                    }
                };

                // Then handle validator if provided
                let validator = args.get_optional::<Value>("validator");
                if let Some(validator) = validator {
                    // Pass the actual value to the validator
                    let result = validator.call(state, slice::from_ref(&result), listeners);
                    result?;
                }

                Ok(result)
            }
            // At compile time, this just returns an empty string
            "set" => {
                let mut args = ArgParser::new(args, None);
                let key: String = args.get("name")?;
                let value: String = args.get("value")?;
                self.config.insert(key, Value::from(value));
                Ok(Value::from(""))
            }
            // At compile time, this will throw an error if the config required does not exist
            // If not found in config, checks config.meta for the key
            "require" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;

                // First, try to get the value from config.<key>
                match self.config.get(&name) {
                    Some(val) if !val.is_none() => Ok(val.clone()),
                    _ => {
                        // Check if key exists in meta and warn if it's a custom config
                        if let Some(meta_value) = self.config.get("meta") {
                            let meta = meta_value.value();
                            if let Some(meta_obj) = meta.as_object()
                                && let Some(value) = meta_obj.get_value(&Value::from(name.clone()))
                                && !value.is_none()
                                && !self.valid_keys.contains(&name)
                            {
                                // Emit warning with location information
                                let span = state.current_span();
                                let path = state.current_path();
                                let location = CodeLocationWithFile::new(
                                    span.start_line,
                                    span.start_col,
                                    span.start_offset,
                                    path.clone(),
                                );
                                let error = fs_err!(
                                    ErrorCode::Generic,
                                    "The key '{}' was not found using config.require('{}'), but was detected as a custom config under 'meta'. \
                                    Please use config.meta_get('{}') or config.meta_require('{}') instead of config.require('{}') \
                                    to access the custom config value if intended.",
                                    name, name, name, name, name
                                ).with_location(location);
                                emit_warn_log_from_fs_error(error);
                            }
                        }
                        // Always throw an error, don't return meta values
                        Err(MinijinjaError::new(
                            MinijinjaErrorKind::InvalidOperation,
                            format!("Required config key '{}' not found in config", name),
                        ))
                    }
                }
            }
            "persist_relation_docs" => {
                let default_value = Value::from(BTreeMap::<String, Value>::new());
                let persist_docs = match self.config.get("persist_docs") {
                    Some(val) if !val.is_none() => val.value().clone(),
                    _ => default_value,
                };
                let persist_docs_map = match persist_docs.as_object() {
                    Some(obj) => obj,
                    None => {
                        return Err(MinijinjaError::new(
                            MinijinjaErrorKind::InvalidOperation,
                            "persist_docs must be a dictionary".to_string(),
                        ));
                    }
                };

                Ok(persist_docs_map
                    .get_value(&Value::from("relation"))
                    .unwrap_or_else(|| Value::from(false)))
            }
            "persist_column_docs" => {
                let default_value = Value::from(BTreeMap::<String, Value>::new());
                let persist_docs = match self.config.get("persist_docs") {
                    Some(val) if !val.is_none() => val.value().clone(),
                    _ => default_value,
                };
                let persist_docs_map = match persist_docs.as_object() {
                    Some(obj) => obj,
                    None => {
                        return Err(MinijinjaError::new(
                            MinijinjaErrorKind::InvalidOperation,
                            "persist_docs must be a dictionary".to_string(),
                        ));
                    }
                };

                Ok(persist_docs_map
                    .get_value(&Value::from("columns"))
                    .unwrap_or_else(|| Value::from(false)))
            }
            // New method that only checks config.meta
            "meta_get" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;
                let default = args
                    .get_optional::<Value>("default")
                    .unwrap_or_else(|| Value::from(None::<Option<String>>));

                // Get the value from meta or use default
                let result = if let Some(meta_value) = self.config.get("meta") {
                    let meta = meta_value.value();
                    if let Some(meta_obj) = meta.as_object() {
                        if let Some(value) = meta_obj.get_value(&Value::from(name)) {
                            if !value.is_none() { value } else { default }
                        } else {
                            default
                        }
                    } else {
                        default
                    }
                } else {
                    default
                };

                // Handle validator if provided (same as in get method)
                let validator = args.get_optional::<Value>("validator");
                if let Some(validator) = validator {
                    // Pass the actual value to the validator
                    let validated = validator.call(state, slice::from_ref(&result), listeners);
                    validated?;
                }

                Ok(result)
            }
            // New method that only checks config.meta and requires the key
            "meta_require" => {
                let mut args = ArgParser::new(args, None);
                let name: String = args.get("name")?;

                // Only check config.meta.<key>
                if let Some(meta_value) = self.config.get("meta") {
                    let meta = meta_value.value();
                    if let Some(meta_obj) = meta.as_object() {
                        if let Some(value) = meta_obj.get_value(&Value::from(name.clone())) {
                            if !value.is_none() {
                                return Ok(value);
                            }
                        }
                    }
                }
                // If not found in meta, throw an error
                Err(MinijinjaError::new(
                    MinijinjaErrorKind::InvalidOperation,
                    format!("Required config key '{}' not found in config.meta", name),
                ))
            }
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("Unknown method on compile config: {name}"),
            )),
        }
    }
}
