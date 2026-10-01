//! https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/relation_configs/column_mask.py

use dbt_schemas::schemas::{DbtModel, InternalDbtNodeAttributes, dbt_column::ColumnMask};
use indexmap::IndexMap;
use minijinja::Value;
use serde::Serialize;

use crate::errors::AdapterResult;
use crate::relation::{
    config_v2::{ComponentConfig, ComponentConfigLoader, impl_loader},
    databricks::config::{DatabricksRelationMetadata, DatabricksRelationMetadataKey},
};

pub(crate) const TYPE_NAME: &str = "column_masks";

/// Component for Databricks column masks (liquid clustering)
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ColumnMasks {
    pub set_column_masks: IndexMap<String, ColumnMask>,
    pub unset_column_masks: Vec<String>,
}

impl ColumnMasks {
    pub fn new(
        set_column_masks: IndexMap<String, ColumnMask>,
        unset_column_masks: Vec<String>,
    ) -> Self {
        Self {
            set_column_masks,
            unset_column_masks,
        }
    }

    pub fn from_local_config(relation_config: &dyn InternalDbtNodeAttributes) -> Self {
        let mut set_column_masks: IndexMap<String, ColumnMask> = IndexMap::new();

        if let Some(model) = relation_config.as_any().downcast_ref::<DbtModel>() {
            for column in &model.__base_attr__.columns {
                if let Some(column_mask) = &column.column_mask {
                    set_column_masks.insert(
                        column.name.clone(),
                        ColumnMask {
                            function: column_mask.function.clone(),
                            using_columns: column_mask.using_columns.clone(),
                        },
                    );
                }
            }
        }

        Self::new(set_column_masks, Vec::new())
    }

    pub fn from_remote_state(state: &DatabricksRelationMetadata) -> Self {
        let mut set_column_masks: IndexMap<String, ColumnMask> = IndexMap::new();

        let (column_name_idx, mask_name_idx, using_columns_idx) =
            (Value::from(0), Value::from(1), Value::from(2));
        if let Some(column_masks_table) = state.get(&DatabricksRelationMetadataKey::ColumnMasks) {
            for row in column_masks_table.rows() {
                if let (Ok(column_name_val), Ok(mask_name_val), Ok(using_columns_val)) = (
                    row.get_item(&column_name_idx),
                    row.get_item(&mask_name_idx),
                    row.get_item(&using_columns_idx),
                ) && let (Some(column_name), Some(mask_name), Some(using_columns)) = (
                    column_name_val.as_str(),
                    mask_name_val.as_str(),
                    using_columns_val.as_str(),
                ) {
                    let using_columns_opt =
                        (!using_columns.is_empty()).then(|| using_columns.to_string());
                    set_column_masks.insert(
                        column_name.to_string(),
                        ColumnMask {
                            function: mask_name.to_string(),
                            using_columns: using_columns_opt,
                        },
                    );
                }
            }
        }

        Self::new(set_column_masks, Vec::new())
    }
}

impl ComponentConfig for ColumnMasks {
    fn diff_from(
        &self,
        current_state: Option<&dyn ComponentConfig>,
    ) -> Option<Box<dyn ComponentConfig>> {
        // If the config was just introduced, we want to apply the entirety of `self`
        let Some(current_state) = current_state else {
            return Some(Box::new(self.clone()));
        };

        // The config is not of type `ColumnMasks`, so we can't diff
        let current_state = current_state.as_any().downcast_ref::<Self>()?;

        // Reference: https://github.com/databricks/dbt-databricks/blob/b6479991ceee369351cdbeacb4468e2599dcd0e3/dbt/adapters/databricks/relation_configs/column_mask.py#L18
        let normalize = |s: &str| s.to_lowercase();
        let desired_column_masks_lower: IndexMap<String, (&String, &ColumnMask)> = self
            .set_column_masks
            .iter()
            .map(|(col, mask)| (normalize(col), (col, mask)))
            .collect();

        // Build column masks that need to be unset and a map of normalized column names to their masks
        let mut current_column_masks_lower: IndexMap<String, &ColumnMask> =
            IndexMap::with_capacity(current_state.set_column_masks.len());
        let unset_column_masks = Vec::from_iter(current_state.set_column_masks.iter().filter_map(
            |(col, mask)| {
                let normalized_col = normalize(col);
                let present_in_desired = desired_column_masks_lower.contains_key(&normalized_col);

                current_column_masks_lower.insert(normalized_col, mask);

                // If not present in the desired state, return the original column name as one to be unset
                (!present_in_desired).then(|| col.clone())
            },
        ));

        // Build column masks that need to be set or updated
        let set_column_masks: IndexMap<String, ColumnMask> = desired_column_masks_lower
            .into_iter()
            .filter_map(|(normalized_col, (col, desired_mask))| {
                if let Some(current_mask) = current_column_masks_lower.get(&normalized_col)
                    && *current_mask == desired_mask
                {
                    None
                } else {
                    Some((col.clone(), desired_mask.clone()))
                }
            })
            .collect();

        if set_column_masks.is_empty() && unset_column_masks.is_empty() {
            None
        } else {
            Some(Box::new(ColumnMasks::new(
                set_column_masks,
                unset_column_masks,
            )))
        }
    }

    fn type_name(&self) -> &'static str {
        TYPE_NAME
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_jinja(&self) -> Value {
        Value::from_serialize(self)
    }
}

fn from_remote_state(state: &DatabricksRelationMetadata) -> AdapterResult<ColumnMasks> {
    Ok(ColumnMasks::from_remote_state(state))
}

fn from_local_config(
    relation_config: &dyn InternalDbtNodeAttributes,
) -> AdapterResult<ColumnMasks> {
    Ok(ColumnMasks::from_local_config(relation_config))
}

impl_loader!(ColumnMasks, DatabricksRelationMetadata);

impl ColumnMasksLoader {
    pub fn new_component_type_erased(
        set_column_masks: IndexMap<String, ColumnMask>,
        unset_column_masks: Vec<String>,
    ) -> Box<dyn ComponentConfig> {
        Box::new(ColumnMasks::new(set_column_masks, unset_column_masks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::databricks::describe_json::*;
    use crate::metadata::databricks::*;
    use arrow::array::*;
    use arrow_schema::*;
    use dbt_agate::AgateTable;
    use std::sync::Arc;

    #[test]
    fn test_diff_update_column_mask() {
        let old_masks = ColumnMasks::new(
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn1".to_string(),
                    using_columns: None,
                },
            )]),
            Vec::new(),
        );
        let next_masks = ColumnMasks::new(
            IndexMap::from([
                (
                    "col1".to_string(),
                    ColumnMask {
                        function: "fn1".to_string(),
                        using_columns: None,
                    },
                ),
                (
                    "col2".to_string(),
                    ColumnMask {
                        function: "fn2".to_string(),
                        using_columns: None,
                    },
                ),
            ]),
            Vec::new(),
        );

        let diff = next_masks.diff_from(Some(&old_masks)).unwrap();
        let diff = diff.as_any().downcast_ref::<ColumnMasks>().unwrap();
        assert!(diff.unset_column_masks.is_empty());
        assert_eq!(
            diff.set_column_masks,
            IndexMap::from([(
                "col2".to_string(),
                ColumnMask {
                    function: "fn2".to_string(),
                    using_columns: None,
                }
            )])
        );
    }

    #[test]
    fn test_diff_change_column_mask() {
        let prev_masks = ColumnMasks::new(
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn1".to_string(),
                    using_columns: None,
                },
            )]),
            Vec::new(),
        );
        let next_masks = ColumnMasks::new(
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn2".to_string(),
                    using_columns: None,
                },
            )]),
            Vec::new(),
        );

        let diff = next_masks.diff_from(Some(&prev_masks)).unwrap();
        let diff = diff.as_any().downcast_ref::<ColumnMasks>().unwrap();
        assert!(diff.unset_column_masks.is_empty());
        assert_eq!(
            diff.set_column_masks,
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn2".to_string(),
                    using_columns: None,
                }
            )])
        );

        let next_masks = ColumnMasks::new(
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn1".to_string(),
                    using_columns: Some("id, 'literal'".to_string()),
                },
            )]),
            Vec::new(),
        );

        let diff = next_masks.diff_from(Some(&prev_masks)).unwrap();
        let diff = diff.as_any().downcast_ref::<ColumnMasks>().unwrap();
        assert!(diff.unset_column_masks.is_empty());
        assert_eq!(
            diff.set_column_masks,
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn1".to_string(),
                    using_columns: Some("id, 'literal'".to_string()),
                }
            )])
        );
    }

    #[test]
    fn test_diff_remove_column_mask() {
        let prev_masks = ColumnMasks::new(
            IndexMap::from([(
                "col1".to_string(),
                ColumnMask {
                    function: "fn1".to_string(),
                    using_columns: None,
                },
            )]),
            Vec::new(),
        );
        let next_masks = ColumnMasks::new(IndexMap::from([]), Vec::new());

        let diff = next_masks.diff_from(Some(&prev_masks)).unwrap();
        let diff = diff.as_any().downcast_ref::<ColumnMasks>().unwrap();
        assert!(diff.set_column_masks.is_empty());
        assert_eq!(diff.unset_column_masks, vec!["col1"]);
    }

    #[test]
    fn test_as_json_path_produces_no_false_diff_vs_info_schema_path() {
        let schema = Arc::new(Schema::new(vec![
            Field::new("column_name", DataType::Utf8, false),
            Field::new("mask_name", DataType::Utf8, false),
            Field::new("using_columns", DataType::Utf8, true),
        ]));
        let info_schema_batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["phone_number"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["main.db.mask_phone"])) as ArrayRef,
                Arc::new(StringArray::from(vec!["city"])) as ArrayRef,
            ],
        )
        .unwrap();
        let info_schema_results = IndexMap::from([(
            DatabricksRelationMetadataKey::ColumnMasks,
            AgateTable::from_record_batch(Arc::new(info_schema_batch)),
        )]);
        let info_schema_config = ColumnMasks::from_remote_state(&info_schema_results);

        let rows = vec![ColumnMaskRow {
            column_name: "phone_number".to_string(),
            mask_name: "main.db.mask_phone".to_string(),
            using_columns: Some("city".to_string()),
        }];
        let as_json_results = IndexMap::from([(
            DatabricksRelationMetadataKey::ColumnMasks,
            column_masks_to_agate(&rows).unwrap(),
        )]);
        let as_json_config = ColumnMasks::from_remote_state(&as_json_results);

        assert!(
            as_json_config
                .diff_from(Some(&info_schema_config))
                .is_none(),
            "AS-JSON path should not produce a diff against the info_schema path for identical column masks"
        );
    }
}
