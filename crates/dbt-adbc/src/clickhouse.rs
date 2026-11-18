// Names of ClickHouse driver options -------------------------------------

/// Key prefix for passing a ClickHouse server setting through the driver:
/// `clickhouse.setting.<name>`, string-valued, accepted at Database,
/// Connection and Statement level; sent as a URL query parameter on every
/// HTTP request.
pub const SETTING_PREFIX: &str = "clickhouse.setting.";

/// Build the driver option key for a ClickHouse server setting.
pub fn setting_key(setting_name: &str) -> String {
    format!("{SETTING_PREFIX}{setting_name}")
}

/// Space-separated `<name>/<version>` pairs the driver prepends to the HTTP
/// `User-Agent` (visible as `http_user_agent` in `system.query_log`), ahead
/// of its own `adbc_clickhouse/<ver> clickhouse-rs/<ver>` identification.
pub const PRODUCT_INFO: &str = "clickhouse.client.product_info";

/// Statement option. The id the server runs the statement under (`query_id`
/// in `system.query_log`). The driver generates one per statement.
pub const QUERY_ID: &str = "clickhouse.client.query_id";

// Names of schema metadata keys -----------------------------------------

pub mod schema_metadata {
    /// The [`super::QUERY_ID`] the statement ran under. The driver does not
    /// put it in the result, so `adbc_execute_with_options` copies it here.
    pub const QUERY_ID: &str = "CLICKHOUSE_QUERY_ID";
}
