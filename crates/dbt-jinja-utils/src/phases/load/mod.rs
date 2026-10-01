//! This module contains the rendering functionality for the load phase.

use std::collections::BTreeMap;

use minijinja::Value;
use serde::Serialize;

use crate::{functions::Var, phases::load::secret_renderer::secret_context_env_var};

pub mod init;
pub mod secret_renderer;

/// A struct that contains the context for the deps phase.
#[derive(Serialize)]
pub struct LoadContext {
    env_var: Value,
    var: Value,
    target: Value,
}

impl LoadContext {
    /// Create a new DepsContext.
    pub fn new(vars: BTreeMap<String, dbt_yaml::Value>) -> Self {
        Self {
            env_var: Value::from_func_func("env_var", secret_context_env_var),
            var: Value::from_object(Var::new(vars)),
            target: Value::from_serialize(BTreeMap::<String, Value>::new()),
        }
    }
}
