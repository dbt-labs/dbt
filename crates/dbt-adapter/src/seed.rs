use std::sync::Arc;

use arrow_schema::{FieldRef, Schema};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::types::metadata_sql_type_key;
use indexmap::IndexMap;

use crate::AdapterResult;

pub(crate) fn ingest_schema_with_column_overrides(
    arrow_schema: &Schema,
    column_overrides: &IndexMap<String, String>,
    adapter_type: AdapterType,
) -> AdapterResult<Schema> {
    let sql_type_key = metadata_sql_type_key(adapter_type);

    let new_fields = arrow_schema
        .fields()
        .iter()
        .map(|field_ref| {
            let Some(data_type) = column_overrides.get(field_ref.name()) else {
                return Ok(Arc::clone(field_ref));
            };

            let mut metadata = field_ref.metadata().clone();
            metadata.insert(sql_type_key.to_string(), data_type.to_string());

            let field = field_ref.as_ref().clone().with_metadata(metadata);

            Ok(Arc::new(field))
        })
        .collect::<AdapterResult<Vec<FieldRef>>>()?;

    Ok(Schema::new(new_fields))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use arrow_schema::{DataType, Field};

    use super::*;

    #[test]
    fn test_bigquery_ingest_schema_with_column_overrides() {
        let original_metadata = HashMap::from([("existing".to_string(), "value".to_string())]);
        let arrow_schema = Schema::new(vec![
            Field::new("id", DataType::Utf8, true).with_metadata(original_metadata),
            Field::new("name", DataType::Utf8, false),
        ]);
        let column_overrides = IndexMap::from([("id".to_string(), "int64".to_string())]);

        let ingest_schema = ingest_schema_with_column_overrides(
            &arrow_schema,
            &column_overrides,
            AdapterType::Bigquery,
        )
        .expect("ingest schema should apply BigQuery column overrides");

        let id = ingest_schema.field(0);
        assert_eq!(id.name(), "id");
        assert_eq!(id.data_type(), &DataType::Utf8);
        assert!(id.is_nullable());
        assert_eq!(id.metadata().get("existing"), Some(&"value".to_string()));
        assert_eq!(
            id.metadata()
                .get(metadata_sql_type_key(AdapterType::Bigquery)),
            Some(&"int64".to_string())
        );

        let name = ingest_schema.field(1);
        assert_eq!(name.name(), "name");
        assert_eq!(name.data_type(), &DataType::Utf8);
        assert!(!name.is_nullable());
        assert!(name.metadata().is_empty());
    }
}
