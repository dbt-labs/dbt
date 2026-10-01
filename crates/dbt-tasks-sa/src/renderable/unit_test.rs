use std::collections::{BTreeMap, BTreeSet};

use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::properties::UnitTestOverrides;

/// Build a map of model unique_id -> unit test overrides for models that are dependencies of unit tests
pub fn build_unit_test_overrides_map(
    nodes: &Nodes,
    deps: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeMap<String, UnitTestOverrides> {
    let mut overrides_map = BTreeMap::new();

    for (unit_test_id, unit_test) in &nodes.unit_tests {
        if let Some(overrides) = &unit_test.__unit_test_attr__.overrides
            && let Some(unit_test_deps) = deps.get(unit_test_id)
        {
            for dep_id in unit_test_deps {
                // Only apply overrides to model dependencies, not other types
                if dep_id.starts_with("model.") {
                    overrides_map.insert(dep_id.clone(), overrides.clone());
                }
            }
        }
    }

    overrides_map
}
