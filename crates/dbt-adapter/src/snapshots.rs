use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub struct SnapshotStrategy {
    pub unique_key: Option<StringOrArrayOfStrings>,
    pub updated_at: Option<String>,
    pub row_changed: Option<String>,
    pub scd_id: Option<String>,
    pub hard_deletes: Option<String>,
}
