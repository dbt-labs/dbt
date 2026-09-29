pub(crate) mod components;
pub(crate) mod relation_types;

#[cfg(test)]
pub(crate) mod test_helpers;

use dbt_agate::AgateTable;
use indexmap::IndexMap;

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
