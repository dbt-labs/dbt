use std::collections::BTreeMap;

use dbt_yaml::{Spanned, Verbatim};
use serde::Deserialize;

use crate::schemas::project::PluginNodeConfig;

type YmlValue = dbt_yaml::Value;

/// An entry under a plugin's key in a properties `.yml`: the fields every plugin node has.
///
/// Everything else in the entry belongs to the plugin and is kept as written, unrendered, in
/// [`PluginNodeProperties::__additional_properties__`].
#[derive(Debug, Clone, Deserialize)]
pub struct PluginNodeProperties {
    pub name: String,
    pub description: Option<String>,
    pub config: Option<PluginNodeConfig>,
    /// `ref()`, `source()` and `metric()` calls the node depends on, as exposures write them.
    pub depends_on: Option<Vec<Spanned<String>>>,
    pub __additional_properties__: Verbatim<BTreeMap<String, YmlValue>>,
}
