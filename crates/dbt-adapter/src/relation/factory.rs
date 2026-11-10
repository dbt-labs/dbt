use std::sync::Arc;

use dbt_adapter_core::AdapterType;

use crate::relation::RelationStatic;
use crate::relation::StaticBaseRelationObject;

use dbt_schemas::schemas::common::ResolvedQuoting;
use minijinja::Value;

/// Create a static relation value from an adapter type
/// To be used as api.Relation in the Jinja environment
pub fn create_static_relation(
    adapter_type: AdapterType,
    quoting: ResolvedQuoting,
) -> Option<Value> {
    use AdapterType::*;
    let result = match adapter_type {
        Snowflake | Databricks | Spark | Fabric | DuckDB | LakeCompute | Exasol | Postgres
        | Redshift | Salesforce | Bigquery | ClickHouse => {
            let relation_type = RelationStatic {
                adapter_type,
                quoting,
            };
            StaticBaseRelationObject::new(Arc::new(relation_type))
        }
        Starburst => todo!("Starburst"),
        Athena => todo!("Athena"),
        Trino => todo!("Trino"),
        Dremio => todo!("Dremio"),
        Oracle => todo!("Oracle"),
        Datafusion => todo!("Datafusion"),
    };
    Some(Value::from_object(result))
}
