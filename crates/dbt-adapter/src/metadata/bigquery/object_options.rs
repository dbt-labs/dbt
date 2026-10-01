use dbt_common::serde_utils::convert_yml_to_value_map;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_schemas::schemas::project::ModelConfig;
use dbt_schemas::schemas::{CommonAttributes, InternalDbtNodeWrapper};
use indexmap::IndexMap;
use minijinja::Value;
use minijinja::value::mutable_vec::MutableVec;

use crate::catalog_relation::CatalogRelation;
use crate::{AdapterType, load_catalogs};

/// Escape a string like Python's json.dumps() does with ensure_ascii=True.
/// This converts non-ASCII characters (Unicode > 127) to \uXXXX escape sequences,
/// matching dbt-core's sql_escape behavior.
fn sql_escape_like_python_json(s: &str) -> String {
    let mut result = String::with_capacity(s.len() * 2);
    for c in s.chars() {
        match c {
            '\\' => result.push_str("\\\\"),
            '"' => result.push_str("\\\""),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c as u32 > 127 => {
                // Non-ASCII: encode as \uXXXX
                if c as u32 <= 0xFFFF {
                    result.push_str(&format!("\\u{:04x}", c as u32));
                } else {
                    // Surrogate pair for characters outside BMP
                    let code = c as u32 - 0x10000;
                    let high = 0xD800 + (code >> 10);
                    let low = 0xDC00 + (code & 0x3FF);
                    result.push_str(&format!("\\u{:04x}\\u{:04x}", high, low));
                }
            }
            c if c < ' ' => {
                // Control characters
                result.push_str(&format!("\\u{:04x}", c as u32));
            }
            _ => result.push(c),
        }
    }
    result
}

/// Shared, pure helper to compute common table options for BigQuery without DB access.
pub(crate) fn get_common_table_options_value(
    state: &minijinja::State,
    config: ModelConfig,
    common_attr: &CommonAttributes,
    temporary: bool,
) -> IndexMap<String, Value> {
    let _ = state;
    // Insertion-ordered: BigQuery `OPTIONS(...)` are rendered in this order, and
    // dbt-core emits them in insertion order (e.g. `expiration_timestamp` before
    // `description`). A sorted map would reorder them and break parity.
    let mut result = IndexMap::new();

    if let Some(hours) = config
        .__warehouse_specific_config__
        .hours_to_expiration
        .as_ref()
        .and_then(|o| o.clone())
        && !temporary
    {
        let expiration = format!("TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL {hours} hour)");
        result.insert("expiration_timestamp".to_string(), Value::from(expiration));
    }

    // Handle description if persist_docs is enabled
    if let Some(persist_docs) = &config.persist_docs
        && persist_docs.relation.unwrap_or(false)
        && let Some(description) = &common_attr.description
    {
        let escaped_description = sql_escape_like_python_json(description);
        result.insert(
            "description".to_string(),
            Value::from(format!("\"\"\"{escaped_description}\"\"\"")),
        );
    }

    let mut labels = config
        .__warehouse_specific_config__
        .labels
        .clone()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| Value::from_iter(vec![Value::from(key), Value::from(value)]))
        .collect::<Vec<_>>();

    // https://github.com/dbt-labs/dbt-adapters/pull/890
    // Merge with priority to labels
    if config
        .__warehouse_specific_config__
        .labels_from_meta
        .unwrap_or_default()
        && let Some(meta) = &config.meta
    {
        // Convert meta values to strings
        for (key, value) in meta {
            labels.push(Value::from_iter(vec![
                Value::from(key),
                value.as_str().map(Value::from).unwrap_or_default(),
            ]));
        }
    }

    // Add labels to opts if any exist
    if !labels.is_empty() {
        result.insert(
            "labels".to_string(),
            Value::from_object(MutableVec::from_iter(labels)),
        );
    }

    let resource_tags = config
        .__warehouse_specific_config__
        .resource_tags
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| Value::from_iter(vec![Value::from(key), Value::from(value)]))
        .collect::<Vec<_>>();

    // Add resource_tags to opts if any exist
    if !resource_tags.is_empty() {
        result.insert(
            "tags".to_string(),
            Value::from_object(MutableVec::from_iter(resource_tags)),
        );
    }

    result
}

/// Shared, pure helper to compute full table options for BigQuery without DB access.
pub(crate) fn get_table_options_value(
    state: &minijinja::State,
    config: ModelConfig,
    node: &InternalDbtNodeWrapper,
    temporary: bool,
    adapter_type: AdapterType,
) -> AdapterResult<IndexMap<String, Value>> {
    // Common options
    let common_attr = node.as_internal_node().common();
    let mut opts = get_common_table_options_value(state, config.clone(), common_attr, temporary);

    // Node serialization and catalogs lookup are in-memory/pure
    // TODO(anna): Ideally from_model_config_and_catalogs would just take in an InternalDbtNodeWrapper instead of a Value. This is blocked by a Snowflake hack in `snowflake__drop_table`.
    let node_yml = node.as_internal_node().serialize();
    let catalog_relation = CatalogRelation::from_model_config_and_catalogs(
        adapter_type,
        &Value::from_object(convert_yml_to_value_map(node_yml)),
        load_catalogs::fetch_catalogs(),
    )?;

    // KMS key name if present
    if let Some(kms_key_name) = config.__warehouse_specific_config__.kms_key_name {
        opts.insert(
            "kms_key_name".to_string(),
            Value::from(format!("'{kms_key_name}'")),
        );
    }

    if temporary {
        // For temporary tables, set 12-hour expiration
        opts.insert(
            "expiration_timestamp".to_string(),
            Value::from("TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL 12 hour)"),
        );
    } else {
        // Partition filter requirements for non-temporary tables
        if config
            .__warehouse_specific_config__
            .require_partition_filter
            .unwrap_or(false)
            && config.__warehouse_specific_config__.partition_by.is_some()
        {
            opts.insert(
                "require_partition_filter".to_string(),
                Value::from(
                    config
                        .__warehouse_specific_config__
                        .require_partition_filter,
                ),
            );
        }

        if catalog_relation.table_format.is_iceberg() {
            opts.extend(iceberg_table_options(&catalog_relation)?);
        }
    }

    // Partition expiration applies only to the final table.
    if !temporary
        && let Some(days) = config
            .__warehouse_specific_config__
            .partition_expiration_days
    {
        opts.insert("partition_expiration_days".to_string(), Value::from(days));
    }

    if !temporary && let Some(enabled) = config.__warehouse_specific_config__.enable_change_history
    {
        opts.insert("enable_change_history".to_string(), Value::from(enabled));
    }

    Ok(opts)
}

/// Precondition: table format is iceberg. `lakehouse_catalog` is None-overloaded
/// and can misrepresent mismatched adapter type as real semantics otherwise.
fn iceberg_table_options(
    catalog_relation: &CatalogRelation,
) -> AdapterResult<IndexMap<String, Value>> {
    let mut opts = IndexMap::new();

    match catalog_relation.lakehouse_catalog() {
        Some(_) => {}
        None => {
            opts.insert(
                "table_format".to_string(),
                Value::from(format!("'{}'", catalog_relation.table_format.as_str())),
            );

            let file_format = catalog_relation.file_format.as_deref().ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Internal,
                    "file_format is not set in catalog",
                )
            })?;
            opts.insert(
                "file_format".to_string(),
                Value::from(format!("'{file_format}'")),
            );

            let storage_uri = catalog_relation
                .adapter_properties
                .get("storage_uri")
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Internal,
                        "storage_uri is not set in catalog",
                    )
                })?;
            opts.insert(
                "storage_uri".to_string(),
                Value::from(format!("'{storage_uri}'")),
            );
        }
    }

    Ok(opts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::dbt_catalogs::CatalogType;
    use dbt_schemas::schemas::relations::base::TableFormat;
    use dbt_schemas::schemas::serde::StringOrInteger;
    use std::collections::BTreeMap;

    fn expiration_for(hours: Option<StringOrInteger>, temporary: bool) -> Option<String> {
        let env = minijinja::Environment::new();
        let state = env.empty_state();
        let mut config = ModelConfig::default();
        config.__warehouse_specific_config__.hours_to_expiration =
            dbt_common::serde_utils::Omissible::Present(hours);
        let common = CommonAttributes::default();
        let opts = get_common_table_options_value(&state, config, &common, temporary);
        opts.get("expiration_timestamp").map(|v| v.to_string())
    }

    // dbt-labs/fs#11681: dbt-core emits `expiration_timestamp` whenever
    // `hours_to_expiration` is present and not null (interpolating str(value)),
    // even when it renders to the string "null". Fusion must match.
    #[test]
    fn expiration_timestamp_present_null_string_is_emitted() {
        assert_eq!(
            expiration_for(Some(StringOrInteger::String("null".to_string())), false),
            Some("TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL null hour)".to_string())
        );
    }

    #[test]
    fn expiration_timestamp_integer_is_emitted() {
        assert_eq!(
            expiration_for(Some(StringOrInteger::Integer(12)), false),
            Some("TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL 12 hour)".to_string())
        );
    }

    #[test]
    fn expiration_timestamp_absent_is_omitted() {
        assert_eq!(expiration_for(None, false), None);
    }

    #[test]
    fn expiration_timestamp_omitted_for_temporary() {
        assert_eq!(
            expiration_for(Some(StringOrInteger::Integer(12)), true),
            None
        );
    }

    fn biglake_catalog_relation() -> CatalogRelation {
        CatalogRelation {
            adapter_type: AdapterType::Bigquery,
            catalog_name: Some("BQ".to_string()),
            integration_name: None,
            catalog_type: CatalogType::BiglakeMetastore,
            table_format: TableFormat::Iceberg,
            adapter_properties: BTreeMap::from([(
                "storage_uri".to_string(),
                "gs://my-bucket/_dbt/analytics/events".to_string(),
            )]),
            is_transient: None,
            external_volume: Some("gs://my-bucket".to_string()),
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            file_format: Some("parquet".to_string()),
        }
    }

    fn lrc_catalog_relation() -> CatalogRelation {
        CatalogRelation {
            lakehouse_catalog: Some("sales_catalog".to_string()),
            ..biglake_catalog_relation()
        }
    }

    #[test]
    fn iceberg_options_for_plain_biglake_metastore_catalog() {
        let opts = iceberg_table_options(&biglake_catalog_relation()).unwrap();
        insta::assert_debug_snapshot!(opts);
    }

    #[test]
    fn iceberg_options_empty_for_lrc_catalog() {
        assert!(
            iceberg_table_options(&lrc_catalog_relation())
                .unwrap()
                .is_empty()
        );
    }

    fn table_options_with_partition_expiration(temporary: bool) -> IndexMap<String, Value> {
        let env = minijinja::Environment::new();
        let state = env.empty_state();
        let mut config = ModelConfig::default();
        config
            .__warehouse_specific_config__
            .partition_expiration_days = Some(90);
        let node = InternalDbtNodeWrapper::Model(Box::default());
        get_table_options_value(&state, config, &node, temporary, AdapterType::Bigquery).unwrap()
    }

    #[test]
    fn partition_expiration_is_retained_for_final_tables() {
        let options = table_options_with_partition_expiration(false);
        assert_eq!(
            options.get("partition_expiration_days"),
            Some(&Value::from(90))
        );
    }

    #[test]
    fn partition_expiration_is_omitted_for_temporary_tables() {
        let options = table_options_with_partition_expiration(true);
        assert_eq!(options.get("partition_expiration_days"), None);
        assert_eq!(
            options.get("expiration_timestamp"),
            Some(&Value::from(
                "TIMESTAMP_ADD(CURRENT_TIMESTAMP(), INTERVAL 12 hour)"
            ))
        );
    }

    #[test]
    fn enable_change_history_is_emitted_only_for_final_tables() {
        let env = minijinja::Environment::new();
        let state = env.empty_state();
        let node = InternalDbtNodeWrapper::Model(Box::default());
        let options = |enabled: bool, temporary: bool| {
            let mut config = ModelConfig::default();
            config.__warehouse_specific_config__.enable_change_history = Some(enabled);
            get_table_options_value(&state, config, &node, temporary, AdapterType::Bigquery)
                .unwrap()
        };

        assert_eq!(
            options(true, false).get("enable_change_history"),
            Some(&Value::from(true))
        );
        assert_eq!(
            options(false, false).get("enable_change_history"),
            Some(&Value::from(false))
        );
        assert_eq!(options(true, true).get("enable_change_history"), None);
    }

    #[test]
    fn test_sql_escape_ascii() {
        // ASCII characters should pass through unchanged (except special chars)
        assert_eq!(sql_escape_like_python_json("hello world"), "hello world");
        assert_eq!(sql_escape_like_python_json("test123"), "test123");
    }

    #[test]
    fn test_sql_escape_special_chars() {
        // Special characters should be escaped
        assert_eq!(
            sql_escape_like_python_json("hello\\world"),
            "hello\\\\world"
        );
        assert_eq!(
            sql_escape_like_python_json("say \"hello\""),
            "say \\\"hello\\\""
        );
        assert_eq!(sql_escape_like_python_json("line1\nline2"), "line1\\nline2");
        assert_eq!(sql_escape_like_python_json("tab\there"), "tab\\there");
        assert_eq!(
            sql_escape_like_python_json("carriage\rreturn"),
            "carriage\\rreturn"
        );
    }

    #[test]
    fn test_sql_escape_japanese() {
        // Japanese characters should be converted to \uXXXX
        assert_eq!(sql_escape_like_python_json("通販"), "\\u901a\\u8ca9");
        assert_eq!(
            sql_escape_like_python_json("通販する蔵"),
            "\\u901a\\u8ca9\\u3059\\u308b\\u8535"
        );
    }

    #[test]
    fn test_sql_escape_french() {
        // French accented characters should be escaped
        assert_eq!(
            sql_escape_like_python_json("propriétaires"),
            "propri\\u00e9taires"
        );
        assert_eq!(sql_escape_like_python_json("café"), "caf\\u00e9");
    }

    #[test]
    fn test_sql_escape_chinese() {
        // Chinese characters should be converted to \uXXXX
        assert_eq!(
            sql_escape_like_python_json("明日以降"),
            "\\u660e\\u65e5\\u4ee5\\u964d"
        );
    }

    #[test]
    fn test_sql_escape_mixed() {
        // Mixed ASCII and non-ASCII
        assert_eq!(
            sql_escape_like_python_json("Hello 世界"),
            "Hello \\u4e16\\u754c"
        );
        assert_eq!(
            sql_escape_like_python_json("Test \"with\" 日本語"),
            "Test \\\"with\\\" \\u65e5\\u672c\\u8a9e"
        );
    }

    #[test]
    fn test_sql_escape_emoji() {
        // Emoji (outside BMP) should use surrogate pairs
        // 😀 (U+1F600) should be encoded as surrogate pair
        let result = sql_escape_like_python_json("😀");
        // U+1F600 - 0x10000 = 0xF600
        // high = 0xD800 + (0xF600 >> 10) = 0xD800 + 0x3D = 0xD83D
        // low = 0xDC00 + (0xF600 & 0x3FF) = 0xDC00 + 0x200 = 0xDE00
        assert_eq!(result, "\\ud83d\\ude00");
    }

    #[test]
    fn test_sql_escape_control_chars() {
        // Control characters should be escaped
        assert_eq!(sql_escape_like_python_json("\x00"), "\\u0000");
        assert_eq!(sql_escape_like_python_json("\x01"), "\\u0001");
        assert_eq!(sql_escape_like_python_json("\x1f"), "\\u001f");
    }

    #[test]
    fn test_sql_escape_empty() {
        assert_eq!(sql_escape_like_python_json(""), "");
    }
}
