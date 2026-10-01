use crate::AdapterResult;
use crate::metadata::databricks::schemas::DatabricksDescribeTableExtended;
use crate::metadata::*;
use crate::record_batch::RecordBatchExt;
use crate::sql_types::{TypeOps, make_arrow_field_v2};
use arrow::array::{RecordBatch, StringArray};
use arrow_schema::{Field, Schema};

use std::collections::BTreeMap;
use std::sync::Arc;

pub struct DatabricksTableMetadata(pub DatabricksDescribeTableExtended);

impl std::ops::Deref for DatabricksTableMetadata {
    type Target = DatabricksDescribeTableExtended;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for DatabricksTableMetadata {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl MetadataProcessor for DatabricksTableMetadata {
    type Key = String;
    type Value = String;

    /// Convert serialized table metadata into a String -> String map
    fn into_metadata(self) -> BTreeMap<Self::Key, Self::Value> {
        // Direct conversion without JSON string intermediate
        let value = serde_json::to_value(&self.0).unwrap();
        let obj = value.as_object().unwrap();

        // Serde will flatten values into JSON strings - if we need internal fields
        // in the future, we should either use the serialized struct directly
        // or customize the flattening logic here
        obj.iter()
            .map(|(k, v)| (k.clone(), v.to_string()))
            .collect()
    }

    /// Create [DatabricksTableMetadata] from serialized [RecordBatch] query result
    fn from_record_batch(batch: Arc<RecordBatch>) -> AdapterResult<Self> {
        let json_metadata = batch
            .column_values::<StringArray>("json_metadata")?
            .value(0)
            .to_string();
        let json_metadata =
            serde_json::from_str::<DatabricksDescribeTableExtended>(&json_metadata)?;
        Ok(DatabricksTableMetadata(json_metadata))
    }

    /// Obtain Arrow Schema of the table being described
    fn to_arrow_schema(&self, type_ops: &'_ dyn TypeOps) -> AdapterResult<Arc<Schema>> {
        let mut fields: Vec<Field> = Vec::with_capacity(self.columns.len());
        for col_info in &self.columns {
            let field = make_arrow_field_v2(
                type_ops,
                col_info.name.clone(),
                &col_info.type_.sql_type(),
                Some(col_info.nullable),
                col_info.comment.clone(),
            )?;
            fields.push(field);
        }
        Ok(Arc::new(Schema::new(fields)))
    }
}
