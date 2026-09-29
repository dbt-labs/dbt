pub(crate) mod components;
pub(crate) mod relation_types;
pub(crate) mod test_helpers;

use arrow::record_batch::RecordBatch;
use arrow_array::{Array, BooleanArray, StringArray};
use std::convert::TryFrom;
use std::sync::Arc;

use dbt_agate::AgateTable;
use minijinja::Value;

use crate::record_batch::RecordBatchExt;

/// Deserialization target for macros snowflake__describe_dynamic_table and
/// snowflake__describe_interactive_table — both return the same shape, just under different
/// `Value` map keys (`DYNAMIC_TABLE_KEY` / `INTERACTIVE_TABLE_KEY`).
/// https://github.com/dbt-labs/dbt-adapters/blob/61221f455f5960daf80024febfae6d6fb4b46251/dbt-snowflake/src/dbt/include/snowflake/macros/relations/dynamic_table/describe.sql#L3
#[derive(Debug)]
pub struct SnowflakeDescribeResults {
    pub record_batch: Arc<RecordBatch>,
}

/// The `Value` map key under which `describe_dynamic_table` returns its result.
pub(crate) const DYNAMIC_TABLE_KEY: &str = "dynamic_table";

/// The `Value` map key under which `describe_interactive_table` returns its result.
pub(crate) const INTERACTIVE_TABLE_KEY: &str = "interactive_table";

/// The exact columns `describe_interactive_table` selects from `SHOW INTERACTIVE TABLES`, in
/// order. This is the single source of truth for both the production `.select()` list in
/// `adapter_impl.rs` and the `make_remote_interactive_state` test fixture: the components in
/// this module read these names verbatim, so a mismatch between producer and fixture would
/// otherwise go undetected (a missing column is read back as `None`, not an error).
pub(crate) const INTERACTIVE_TABLE_COLUMNS: [&str; 8] = [
    "name",
    "schema_name",
    "database_name",
    "text",
    "target_lag",
    "refresh_warehouse",
    "initialization_warehouse",
    "cluster_by",
];

impl SnowflakeDescribeResults {
    pub fn from_value_with_key(value: &Value, key: &str) -> Result<Self, String> {
        let record_batch = value
            .get_item(&Value::from_safe_string(key.into()))
            .map_err(|e| format!("Expected key `{key}`: {e}"))?
            .downcast_object::<AgateTable>()
            .ok_or_else(|| format!("Failed to convert {key} to AgateTable"))?
            .original_record_batch();

        Ok(Self { record_batch })
    }
}

impl TryFrom<&Value> for SnowflakeDescribeResults {
    type Error = String;

    fn try_from(value: &Value) -> Result<Self, Self::Error> {
        Self::from_value_with_key(value, DYNAMIC_TABLE_KEY)
    }
}

// Helper function to get a bool value from a RecordBatch by column name.
// Returns None if the column is absent.
fn get_bool_by_name_from_record_batch(batch: &Arc<RecordBatch>, col_name: &str) -> Option<bool> {
    let col = batch.column_values::<BooleanArray>(col_name).ok()?;
    if col.len() != 1 {
        return None;
    }
    col.is_valid(0).then(|| col.value(0))
}

fn get_string_by_name_from_record_batch(
    batch: &Arc<RecordBatch>,
    col_name: &str,
) -> Result<String, String> {
    // Shared by dynamic and interactive readback, so the message names neither.
    if let Ok(column_values) = batch.column_values::<StringArray>(col_name) {
        if column_values.len() != 1 {
            return Err(format!(
                "Describe returned an unexpected number of values for {col_name}."
            ));
        }

        Ok(column_values.value(0).to_string())
    } else {
        Err(format!("Describe is missing {col_name}."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::config_v2::SimpleComponentConfigImpl;
    use crate::relation::snowflake::config::relation_types::interactive_table;
    use crate::relation::snowflake::config::test_helpers::{
        TestRemoteState, make_remote_interactive_state,
    };
    use minijinja::value::ValueMap;

    /// A missing-column read is tolerated as "unset" rather than erroring, so a wrong key or
    /// column name phantom-diffs on every run instead of failing the build.
    #[test]
    fn describe_interactive_table_value_round_trips_under_its_own_key() {
        let batch = make_remote_interactive_state(TestRemoteState {
            target_lag: Some("1 minute".to_string()),
            refresh_warehouse: Some("WH".to_string()),
            initialization_warehouse: Some("INIT_WH".to_string()),
            cluster_by: Some("(id, val)".to_string()),
            ..Default::default()
        })
        .record_batch;

        let value = Value::from(ValueMap::from([(
            Value::from(INTERACTIVE_TABLE_KEY),
            Value::from_object(AgateTable::from_record_batch(batch)),
        )]));

        let results =
            SnowflakeDescribeResults::from_value_with_key(&value, INTERACTIVE_TABLE_KEY).unwrap();
        let loader = interactive_table::new_loader();
        let existing = loader.from_remote_state(&results).unwrap();

        for (type_name, expected) in [
            (components::target_lag::TYPE_NAME, "1 minute"),
            (components::interactive_table_warehouse::TYPE_NAME, "WH"),
            (
                components::snowflake_initialization_warehouse::TYPE_NAME,
                "INIT_WH",
            ),
            (components::cluster_by::TYPE_NAME, "(id, val)"),
        ] {
            let component = existing
                .get(type_name)
                .unwrap_or_else(|| panic!("{type_name} missing from loaded config"))
                .as_any()
                .downcast_ref::<SimpleComponentConfigImpl<Option<String>>>()
                .unwrap_or_else(|| panic!("{type_name} is not a string component"));
            assert_eq!(
                component.value.as_deref(),
                Some(expected),
                "{type_name} did not survive the round trip"
            );
        }

        // The dynamic key must NOT resolve this value, and vice versa.
        assert!(SnowflakeDescribeResults::try_from(&value).is_err());
    }

    /// Neither the fixture nor `adapter_impl.rs`'s `.select()` list is derived from the other;
    /// this pins both to `INTERACTIVE_TABLE_COLUMNS`.
    #[test]
    fn interactive_table_fixture_columns_match_production_select_list() {
        let batch = make_remote_interactive_state(TestRemoteState::default()).record_batch;
        let schema = batch.schema();
        let field_names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        assert_eq!(field_names, INTERACTIVE_TABLE_COLUMNS);
    }
}
