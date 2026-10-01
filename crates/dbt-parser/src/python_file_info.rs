//! Python file information collection
//!
//! This module contains structures for collecting information from Python model files,
//! analogous to SqlFileInfo for SQL models.

use dbt_frontend_common::error::CodeLocation;
use dbt_schemas::schemas::{common::DbtChecksum, project::ResolvableConfig, serde::NodeVersion};

/// Collected details about processed Python files
#[derive(Debug, Clone)]
pub struct PythonFileInfo<T: ResolvableConfig<T>> {
    /// e.g. dbt.source('a', 'b')
    pub sources: Vec<(String, String, CodeLocation)>,

    /// e.g. dbt.ref('a', 'b', 'c')
    pub refs: Vec<(String, Option<String>, Option<NodeVersion>, CodeLocation)>,

    /// e.g. dbt.config(materialized='table')
    pub config: Box<T>,

    /// Python packages imported in the file (for telemetry)
    pub packages: Vec<String>,

    /// Config keys accessed via dbt.config.get('key')
    pub config_keys_used: Vec<String>,

    /// Default values provided to dbt.config.get('key', default)
    /// Stored in the same order as config_keys_used for Jinja zip()
    /// Stored as minijinja Values which render as Python literals
    pub config_keys_defaults: Vec<minijinja::value::Value>,

    /// Meta keys accessed via dbt.config.meta_get('key')
    pub meta_keys_used: Vec<String>,

    /// Default values provided to dbt.config.meta_get('key', default)
    /// Stored in the same order as meta_keys_used for Jinja zip()
    /// Stored as minijinja Values which render as Python literals
    pub meta_keys_defaults: Vec<minijinja::value::Value>,

    /// File checksum
    pub checksum: DbtChecksum,

    /// Whether the model function is defined correctly
    pub has_valid_model_function: bool,
}

impl<T: ResolvableConfig<T>> Default for PythonFileInfo<T> {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            refs: Vec::new(),
            config: Box::new(T::default()),
            packages: Vec::new(),
            config_keys_used: Vec::new(),
            config_keys_defaults: Vec::new(),
            meta_keys_used: Vec::new(),
            meta_keys_defaults: Vec::new(),
            checksum: DbtChecksum::default(),
            has_valid_model_function: false,
        }
    }
}

impl<T: ResolvableConfig<T>> PythonFileInfo<T> {
    /// Create a new PythonFileInfo with a checksum
    pub fn new(checksum: DbtChecksum) -> Self {
        Self {
            checksum,
            ..Default::default()
        }
    }

    /// Update config with new values
    pub fn update_config(&mut self, new_config: T) {
        let mut updated = Box::new(new_config);
        updated.default_to(&*self.config);
        self.config = updated;
    }
}
