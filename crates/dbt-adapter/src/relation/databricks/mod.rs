pub mod config;

mod defaults;
pub use defaults::{DEFAULT_DATABRICKS_DATABASE, INFORMATION_SCHEMA_SCHEMA, SYSTEM_DATABASE};

pub mod typed_constraint;

/// Whether a raw Databricks/Unity Catalog table-type string denotes a shallow clone.
/// Case-insensitive: callers may pass either the `information_schema.tables.table_type`
/// value or the `DESCRIBE TABLE EXTENDED ... AS JSON` `type` field.
pub fn is_shallow_clone_type(type_str: &str) -> bool {
    matches!(
        type_str.to_uppercase().as_str(),
        "MANAGED_SHALLOW_CLONE" | "EXTERNAL_SHALLOW_CLONE"
    )
}
