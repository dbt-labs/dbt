use arrow_schema::SchemaRef;

use crate::Dialect;

/// Encoding applied to a result column before collection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultEncoding {
    None,
    BigQueryGeographyWkt,
    SnowflakeGeographyGeoJson,
    SnowflakeGeometryGeoJson,
    SnowflakeVariantArrayJson,
}

/// SQL and metadata needed to restore the source types of a query result.
#[derive(Clone, Debug)]
pub struct LocalResultQuery {
    pub execution_sql: String,
    pub diagnostic_sql: String,
    pub result_schema: SchemaRef,
    pub result_encodings: Vec<ResultEncoding>,
    pub source_dialect: Dialect,
}

impl LocalResultQuery {
    pub fn result_schema(&self) -> SchemaRef {
        self.result_schema.clone()
    }
}
