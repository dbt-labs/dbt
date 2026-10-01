use std::collections::BTreeMap;

use dbt_yaml::UntaggedEnumDeserialize;
use serde::Serialize;

/// Variable values in a dbt project.
#[derive(Clone, Debug, Serialize, UntaggedEnumDeserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum DbtVars {
    /// Null YAML values (bare `key:` or `key: null`). Must be listed before
    /// `Vars` so that `UntaggedEnumDeserialize` matches null scalars here
    /// rather than deserializing them as an empty map via `Vars({})`.
    Null,
    Vars(BTreeMap<String, DbtVars>),
    Value(dbt_yaml::Value),
}
