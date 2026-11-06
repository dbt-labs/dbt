pub(crate) mod components;
pub(crate) mod relation_types;

#[cfg(test)]
pub(crate) mod test_helpers;

use dbt_agate::AgateTable;
use indexmap::IndexMap;
use minijinja::{
    arg_utils::ArgParser,
    value::{Value, ValueMap},
};
use minijinja_contrib::dyn_object::DynJinjaObject;

// TODO: delete this and make it use RecordBatches instead
#[derive(Debug, Eq, PartialEq, Hash)]
pub(crate) enum DatabricksRelationMetadataKey {
    InfoSchemaViews,
    InfoSchemaRelationTags,
    InfoSchemaColumnTags,
    DescribeExtended,
    ShowTblProperties,
    ColumnMasks,
    RowFilters,
    PrimaryKeyConstraints,
    ForeignKeyConstraints,
    NonNullConstraints,
}

// string conversions based on string keys from:
// https://github.com/databricks/dbt-databricks/blob/7c282cabb518a5e1173222e7901896d31de8401f/dbt/adapters/databricks/impl.py#L1287-L1470
impl From<DatabricksRelationMetadataKey> for String {
    fn from(key: DatabricksRelationMetadataKey) -> Self {
        match key {
            DatabricksRelationMetadataKey::InfoSchemaViews => {
                "information_schema.views".to_string()
            }
            DatabricksRelationMetadataKey::InfoSchemaRelationTags => {
                "information_schema.tags".to_string()
            }
            DatabricksRelationMetadataKey::InfoSchemaColumnTags => {
                "information_schema.column_tags".to_string()
            }
            DatabricksRelationMetadataKey::DescribeExtended => "describe_extended".to_string(),
            DatabricksRelationMetadataKey::ShowTblProperties => "show_tblproperties".to_string(),
            DatabricksRelationMetadataKey::ColumnMasks => "column_masks".to_string(),
            DatabricksRelationMetadataKey::RowFilters => "row_filters".to_string(),
            DatabricksRelationMetadataKey::PrimaryKeyConstraints => {
                "primary_key_constraints".to_string()
            }
            DatabricksRelationMetadataKey::ForeignKeyConstraints => {
                "foreign_key_constraints".to_string()
            }
            DatabricksRelationMetadataKey::NonNullConstraints => {
                "non_null_constraint_columns".to_string()
            }
        }
    }
}

pub(crate) type DatabricksRelationMetadata = IndexMap<DatabricksRelationMetadataKey, AgateTable>;

/// Mirrors v1's pydantic `DatabricksRelationChangeSet`. Macros call `model_copy(update=...)` on it
/// to override fields such as `requires_full_refresh`.
pub(crate) fn databricks_changeset(fields: ValueMap) -> Value {
    Value::from_object(
        DynJinjaObject::<(), ValueMap>::new("DatabricksRelationChangeSet", (), fields).with_method(
            "model_copy",
            |obj, _state, args, _listeners| {
                let mut fields = obj.repr_ref().clone();
                if let Some(update) = ArgParser::new(args, None)
                    .get_optional::<Value>("update")
                    .filter(|update| !update.is_none())
                {
                    for key in update.try_iter()? {
                        let value = update.get_item(&key)?;
                        fields.insert(key, value);
                    }
                }
                Ok(databricks_changeset(fields))
            },
        ),
    )
}
