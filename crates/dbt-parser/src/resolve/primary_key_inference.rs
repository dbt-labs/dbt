use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use dbt_schemas::schemas::common::{ConstraintType, DbtUniqueKey};
use dbt_schemas::schemas::nodes::TestMetadata;
use dbt_schemas::schemas::project::ResolvableConfig;
use dbt_schemas::schemas::{DbtModel, DbtTest, Nodes};
use dbt_yaml::Value as YmlValue;

// Test metadata keys
const KEY_COLUMN_NAME: &str = "column_name";
const KEY_COMBINATION_OF_COLUMNS: &str = "combination_of_columns";

// Canonical generic test names
const TEST_UNIQUE: &str = "unique";
const TEST_UNIQUE_COMBO: &str = "unique_combination_of_columns";
const TEST_NOT_NULL: &str = "not_null";

fn unversioned_model_unique_id(unique_id: &str) -> &str {
    if let Some(pos) = unique_id.rfind('.') {
        let last = &unique_id[pos + 1..];
        if last.starts_with('v') {
            return &unique_id[..pos];
        }
    }
    unique_id
}

fn infer_from_unique_key(model: &DbtModel) -> Vec<String> {
    match &model.deprecated_config.unique_key {
        Some(DbtUniqueKey::Single(s)) => vec![s.clone()],
        Some(DbtUniqueKey::Multiple(v)) if !v.is_empty() => v.clone(),
        _ => vec![],
    }
}

fn infer_from_constraints(model: &DbtModel) -> Option<Vec<String>> {
    if let Some(cols) = model
        .__model_attr__
        .constraints
        .iter()
        .find(|c| c.type_ == ConstraintType::PrimaryKey && c.columns.is_some())
        .and_then(|c| c.columns.clone())
    {
        let mut inferred = cols;
        inferred.sort();
        inferred.dedup();
        if !inferred.is_empty() {
            return Some(inferred);
        }
    }

    if let Some(col) = model.__base_attr__.columns.iter().find(|col| {
        col.constraints
            .iter()
            .any(|cc| cc.type_ == ConstraintType::PrimaryKey)
    }) {
        return Some(vec![col.name.clone()]);
    }

    None
}

fn extract_columns_from_metadata(meta: &TestMetadata) -> Vec<String> {
    if let Some(v) = meta.kwargs.get(KEY_COLUMN_NAME)
        && let YmlValue::String(s, _) = v
    {
        return vec![s.clone()];
    }
    if let Some(YmlValue::Sequence(arr, _)) = meta.kwargs.get(KEY_COMBINATION_OF_COLUMNS) {
        let mut cols = Vec::with_capacity(arr.len());
        for item in arr {
            if let YmlValue::String(s, _) = item {
                cols.push(s.clone());
            }
        }
        if !cols.is_empty() {
            return cols;
        }
    }
    Vec::new()
}

fn infer_from_tests(tests_for_model: &[(&DbtTest, bool)]) -> Vec<String> {
    let mut columns_with_enabled_unique: BTreeSet<String> = BTreeSet::new();
    let mut columns_with_disabled_unique: BTreeSet<String> = BTreeSet::new();
    let mut columns_with_not_null: BTreeSet<String> = BTreeSet::new();

    for (test, is_disabled) in tests_for_model {
        if let Some(meta) = &test.__test_attr__.test_metadata {
            let columns = extract_columns_from_metadata(meta);
            let name = meta.name.as_str();
            let is_enabled = test.deprecated_config.get_enabled_with_default() && !*is_disabled;
            for column in columns {
                match name {
                    TEST_UNIQUE | TEST_UNIQUE_COMBO => {
                        if is_enabled {
                            columns_with_enabled_unique.insert(column);
                        } else {
                            columns_with_disabled_unique.insert(column);
                        }
                    }
                    TEST_NOT_NULL => {
                        columns_with_not_null.insert(column);
                    }
                    _ => {}
                }
            }
        }
    }

    let mut unique_and_not_null: Vec<String> = columns_with_not_null
        .iter()
        .filter(|c| {
            columns_with_enabled_unique.contains(*c) || columns_with_disabled_unique.contains(*c)
        })
        .cloned()
        .collect();
    unique_and_not_null.sort();
    unique_and_not_null.dedup();
    if !unique_and_not_null.is_empty() {
        return unique_and_not_null;
    }

    if !columns_with_enabled_unique.is_empty() {
        return columns_with_enabled_unique.into_iter().collect();
    }
    if !columns_with_disabled_unique.is_empty() {
        return columns_with_disabled_unique.into_iter().collect();
    }
    Vec::new()
}

fn build_model_to_tests_map<'a>(
    nodes: &'a Nodes,
    disabled_nodes: &'a Nodes,
) -> BTreeMap<String, Vec<(&'a DbtTest, bool)>> {
    let mut map: BTreeMap<String, Vec<(&DbtTest, bool)>> = BTreeMap::new();
    for test in nodes.tests.values() {
        if let Some(attached) = &test.__test_attr__.attached_node {
            map.entry(attached.clone())
                .or_default()
                .push((test.as_ref(), false));
        }
    }
    for test in disabled_nodes.tests.values() {
        if let Some(attached) = &test.__test_attr__.attached_node {
            map.entry(attached.clone())
                .or_default()
                .push((test.as_ref(), true));
        }
    }
    map
}

/// Infer and set primary keys on model nodes based on constraints and attached data tests.
///
/// This must run after data tests are resolved (so `attached_node` and `test_metadata` are present),
/// but does not depend on unit tests.
/// Infers and applies primary keys for each model in `nodes`.
///
/// The inference runs after generic data tests have been resolved (so tests carry
/// `attached_node` and structured `test_metadata`) and proceeds in this order:
///
/// 1) Model-level constraints
///    - If the model declares a `PrimaryKey` constraint with explicit `columns`,
///      those columns are used (sorted and de-duplicated).
///
/// 2) Column-level constraints
///    - If any column has a `PrimaryKey` constraint, that single column is used.
///
/// 3) Generic data tests (from `test_metadata`)
///    - Collect column names from tests attached to the model (considering both
///      the exact model unique_id and its unversioned variant). Only generic
///      tests contribute because they carry `test_metadata`:
///        - `unique` and `unique_combination_of_columns` mark columns as unique
///          (tracked separately for enabled and disabled tests).
///        - `not_null` marks columns as non-nullable.
///    - Selection precedence:
///      a. Columns that are both unique (enabled or disabled) and not_null.
///      b. If none, columns marked unique by enabled tests.
///      c. If none, columns marked unique by disabled tests.
///      d. Otherwise, no primary key is inferred.
///
/// 4) config.unique_key
///    - Incremental models commonly set `unique_key` for merge/upsert deduplication.
///      Used as a lowest-priority fallback when no constraint or test signal exists.
///
/// Implementation details:
/// - Disabled tests are included to mirror dbt semantics where uniqueness intent
///   may be present even if currently disabled; not_null must still hold.
/// - Uses a read-only pass to compute updates and a write pass that applies
///   them via `Arc::make_mut`, avoiding unnecessary cloning.
/// - Only generic tests (with `test_metadata`) contribute; singular tests do not.
pub fn infer_and_apply_primary_keys(nodes: &mut Nodes, disabled_nodes: &Nodes) {
    // Build model -> tests map from enabled and disabled tests
    let model_to_tests = build_model_to_tests_map(nodes, disabled_nodes);

    // First pass: compute updates without mutating the map
    let mut pk_updates: Vec<(String, Vec<String>)> = Vec::new();

    for (model_id, model_arc) in nodes.models.iter() {
        // Collect tests for versioned and unversioned keys
        let mut tests_for_model: Vec<(&DbtTest, bool)> = Vec::new();
        if let Some(v) = model_to_tests.get(model_id) {
            tests_for_model.extend(v.iter().cloned());
        }
        let unversioned = unversioned_model_unique_id(model_id);
        if unversioned != model_id
            && let Some(v) = model_to_tests.get(unversioned)
        {
            tests_for_model.extend(v.iter().cloned());
        }

        let model = Arc::as_ref(model_arc);

        if let Some(inferred) = infer_from_constraints(model) {
            pk_updates.push((model_id.clone(), inferred));
            continue;
        }

        let inferred = infer_from_tests(&tests_for_model);
        if !inferred.is_empty() {
            pk_updates.push((model_id.clone(), inferred));
            continue;
        }

        let from_unique_key = infer_from_unique_key(model);
        if !from_unique_key.is_empty() {
            pk_updates.push((model_id.clone(), from_unique_key));
        }
    }

    // Second pass: apply updates
    for (model_id, pk) in pk_updates.into_iter() {
        if let Some(model_arc) = nodes.models.get_mut(&model_id) {
            let model_mut: &mut DbtModel = Arc::make_mut(model_arc);
            model_mut.__model_attr__.primary_key = pk;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::nodes::{DbtTestAttr, TestMetadata};
    use dbt_schemas::schemas::project::ModelConfig;
    use dbt_schemas::schemas::{CommonAttributes, IntrospectionKind, NodeBaseAttributes};

    fn empty_nodes() -> Nodes {
        Nodes::default()
    }

    #[test]
    fn pk_from_model_constraints() {
        let mut nodes = empty_nodes();
        let disabled = empty_nodes();

        let mut model: DbtModel = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.pkg.m".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        model.__model_attr__.constraints =
            vec![dbt_schemas::schemas::properties::ModelConstraint {
                type_: ConstraintType::PrimaryKey,
                expression: None,
                name: None,
                to: None,
                to_columns: None,
                columns: Some(vec!["id".to_string()]),
                warn_unsupported: None,
                warn_unenforced: None,
            }];
        nodes
            .models
            .insert("model.pkg.m".to_string(), Arc::new(model));

        infer_and_apply_primary_keys(&mut nodes, &disabled);

        let updated = nodes.models.get("model.pkg.m").unwrap();
        assert_eq!(
            Arc::as_ref(updated).__model_attr__.primary_key,
            vec!["id".to_string()]
        );
    }

    #[test]
    fn pk_from_unique_and_not_null_tests() {
        let mut nodes = empty_nodes();
        let disabled = empty_nodes();

        let model: DbtModel = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.pkg.cust".to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                ..Default::default()
            },
            ..Default::default()
        };
        nodes
            .models
            .insert("model.pkg.cust".to_string(), Arc::new(model));

        let mut t_unique: DbtTest = DbtTest {
            __test_attr__: DbtTestAttr {
                column_name: None,
                attached_node: Some("model.pkg.cust".to_string()),
                test_metadata: Some(TestMetadata {
                    name: "unique".to_string(),
                    kwargs: {
                        let mut m = BTreeMap::new();
                        m.insert(
                            "column_name".to_string(),
                            YmlValue::string("id".to_string()),
                        );
                        m
                    },
                    namespace: None,
                }),
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            ..Default::default()
        };
        t_unique.deprecated_config.enabled = Some(true);

        let mut t_not_null: DbtTest = DbtTest {
            __test_attr__: DbtTestAttr {
                column_name: None,
                attached_node: Some("model.pkg.cust".to_string()),
                test_metadata: Some(TestMetadata {
                    name: "not_null".to_string(),
                    kwargs: {
                        let mut m = BTreeMap::new();
                        m.insert(
                            "column_name".to_string(),
                            YmlValue::string("id".to_string()),
                        );
                        m
                    },
                    namespace: None,
                }),
                file_key_name: None,
                introspection: IntrospectionKind::None,
                original_name: None,
                group: None,
                state: None,
            },
            ..Default::default()
        };
        t_not_null.deprecated_config.enabled = Some(true);

        nodes.tests.insert("test.a".to_string(), Arc::new(t_unique));
        nodes
            .tests
            .insert("test.b".to_string(), Arc::new(t_not_null));

        infer_and_apply_primary_keys(&mut nodes, &disabled);

        let updated = nodes.models.get("model.pkg.cust").unwrap();
        assert_eq!(
            Arc::as_ref(updated).__model_attr__.primary_key,
            vec!["id".to_string()]
        );
    }

    #[test]
    fn pk_from_unique_key_single() {
        let mut nodes = empty_nodes();
        let disabled = empty_nodes();

        let mut model: DbtModel = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.pkg.orders".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        model.deprecated_config = ModelConfig {
            unique_key: Some(DbtUniqueKey::Single("order_id".to_string())),
            ..Default::default()
        };
        nodes
            .models
            .insert("model.pkg.orders".to_string(), Arc::new(model));

        infer_and_apply_primary_keys(&mut nodes, &disabled);

        let updated = nodes.models.get("model.pkg.orders").unwrap();
        assert_eq!(
            Arc::as_ref(updated).__model_attr__.primary_key,
            vec!["order_id".to_string()]
        );
    }

    #[test]
    fn pk_from_unique_key_multiple() {
        let mut nodes = empty_nodes();
        let disabled = empty_nodes();

        let mut model: DbtModel = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.pkg.order_items".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        model.deprecated_config = ModelConfig {
            unique_key: Some(DbtUniqueKey::Multiple(vec![
                "order_id".to_string(),
                "item_id".to_string(),
            ])),
            ..Default::default()
        };
        nodes
            .models
            .insert("model.pkg.order_items".to_string(), Arc::new(model));

        infer_and_apply_primary_keys(&mut nodes, &disabled);

        let updated = nodes.models.get("model.pkg.order_items").unwrap();
        assert_eq!(
            Arc::as_ref(updated).__model_attr__.primary_key,
            vec!["order_id".to_string(), "item_id".to_string()]
        );
    }

    #[test]
    fn constraint_takes_precedence_over_unique_key() {
        let mut nodes = empty_nodes();
        let disabled = empty_nodes();

        let mut model: DbtModel = DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: "model.pkg.m".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        model.__model_attr__.constraints =
            vec![dbt_schemas::schemas::properties::ModelConstraint {
                type_: ConstraintType::PrimaryKey,
                columns: Some(vec!["id".to_string()]),
                expression: None,
                name: None,
                to: None,
                to_columns: None,
                warn_unsupported: None,
                warn_unenforced: None,
            }];
        model.deprecated_config = ModelConfig {
            unique_key: Some(DbtUniqueKey::Single("other_col".to_string())),
            ..Default::default()
        };
        nodes
            .models
            .insert("model.pkg.m".to_string(), Arc::new(model));

        infer_and_apply_primary_keys(&mut nodes, &disabled);

        let updated = nodes.models.get("model.pkg.m").unwrap();
        assert_eq!(
            Arc::as_ref(updated).__model_attr__.primary_key,
            vec!["id".to_string()],
            "constraint must win over unique_key"
        );
    }
}
