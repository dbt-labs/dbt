use dbt_common::time::current_time_micros;

/// Generate random (and unique) schema to use in tests.
pub fn random_schema(prefix: &str) -> String {
    format!("{}___{}___", prefix, current_time_micros())
}
