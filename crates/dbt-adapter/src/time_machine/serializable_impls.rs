//! TimeMachineSerializable implementations for Object types.

use std::sync::Arc;

use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;
use dbt_schemas::schemas::relations::base::TableFormat;

use crate::relation::{RelationConfig, RelationObject, do_create_relation};

use super::serde::ReplayCallContext;
use super::serializable::{JsonExtractor, TimeMachineSerializable};

/// Defensively strip `__type__` field before passing to serde deserializer.
fn strip_type_field(json: &serde_json::Value) -> serde_json::Value {
    if let Some(obj) = json.as_object() {
        serde_json::Value::Object(
            obj.iter()
                .filter(|(k, _)| *k != "__type__")
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    } else {
        json.clone()
    }
}

impl TimeMachineSerializable for dbt_agate::AgateTable {
    const TYPE_ID: &'static str = "AgateTable";

    fn to_time_machine_json(&self) -> serde_json::Value {
        if let Some(ipc_base64) = table_to_ipc_base64(self) {
            serde_json::json!({
                "__format__": "arrow_ipc_base64",
                "__ipc__": ipc_base64
            })
        } else {
            serde_json::json!({
                "__format__": "metadata_only",
                "num_rows": self.num_rows(),
                "num_columns": self.num_columns(),
                "column_names": self.column_names(),
            })
        }
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        _ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        let ext = JsonExtractor::new(json)?;
        if ext.opt_str("__format__")? != "arrow_ipc_base64" {
            return None;
        }
        let table = ipc_base64_to_table(&ext.opt_str("__ipc__")?)?;
        Some(minijinja::Value::from_object(table))
    }
}

fn table_to_ipc_base64(table: &dbt_agate::AgateTable) -> Option<String> {
    let batch = table.to_record_batch();
    let schema = batch.schema();
    batches_to_ipc_base64(std::slice::from_ref(batch.as_ref()), &schema)
}

/// Deserialize an AgateTable from base64-encoded Arrow IPC bytes.
fn ipc_base64_to_table(ipc_base64: &str) -> Option<dbt_agate::AgateTable> {
    let (batches, _schema) = ipc_base64_to_batches(ipc_base64)?;
    let batch = batches.into_iter().next()?;
    Some(dbt_agate::AgateTable::from_record_batch(Arc::new(batch)))
}

/// Encode `Vec<RecordBatch>` + `SchemaRef` as a base64 Arrow IPC stream (LZ4-compressed).
pub fn batches_to_ipc_base64(
    batches: &[arrow::array::RecordBatch],
    schema: &arrow_schema::SchemaRef,
) -> Option<String> {
    use arrow_ipc::CompressionType;
    use arrow_ipc::writer::{IpcWriteOptions, StreamWriter};
    use base64::Engine;

    let options = IpcWriteOptions::default()
        .try_with_compression(Some(CompressionType::LZ4_FRAME))
        .ok()?;

    let mut buf = Vec::new();
    let mut writer = StreamWriter::try_new_with_options(&mut buf, schema, options).ok()?;
    for batch in batches {
        writer.write(batch).ok()?;
    }
    writer.finish().ok()?;

    Some(base64::engine::general_purpose::STANDARD.encode(&buf))
}

/// Decode a base64 Arrow IPC stream into `(Vec<RecordBatch>, SchemaRef)`.
pub fn ipc_base64_to_batches(
    ipc_base64: &str,
) -> Option<(Vec<arrow::array::RecordBatch>, arrow_schema::SchemaRef)> {
    use arrow_ipc::reader::StreamReader;
    use base64::Engine;

    let ipc_bytes = base64::engine::general_purpose::STANDARD
        .decode(ipc_base64)
        .ok()?;

    let cursor = std::io::Cursor::new(ipc_bytes);
    let reader = StreamReader::try_new(cursor, None).ok()?;
    let schema = reader.schema();
    let batches: Vec<arrow::array::RecordBatch> = reader.filter_map(|r| r.ok()).collect();
    Some((batches, schema))
}

impl TimeMachineSerializable for crate::response::AdapterResponse {
    const TYPE_ID: &'static str = "AdapterResponse";

    fn to_time_machine_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        _ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        let response: crate::response::AdapterResponse =
            serde_json::from_value(strip_type_field(json)).ok()?;
        Some(minijinja::Value::from_object(response))
    }
}

impl TimeMachineSerializable for RelationObject {
    const TYPE_ID: &'static str = "RelationObject";

    fn to_time_machine_json(&self) -> serde_json::Value {
        let quote_policy = self.quote_policy();
        serde_json::json!({
            "adapter_type": self.adapter_type(),
            "database": self.database().unwrap_or_default(),
            "schema": self.schema().unwrap_or_default(),
            "identifier": self.identifier(),
            "is_table": self.is_table(),
            "is_view": self.is_view(),
            "is_materialized_view": self.is_materialized_view(),
            "is_cte": self.is_cte(),
            "is_dynamic_table": self.is_dynamic_table(),
            "is_interactive_table": self.is_interactive_table(),
            "is_streaming_table": self.is_streaming_table(),
            "is_metric_view": self.is_metric_view(),
            "is_delta": self.is_delta(),
            "is_shallow_clone": self.is_shallow_clone(),
            "quote_policy": {
                "database": quote_policy.database,
                "schema": quote_policy.schema,
                "identifier": quote_policy.identifier,
            },
        })
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        use dbt_adapter_core::AdapterType;
        use dbt_schemas::schemas::common::ResolvedQuoting;
        use dbt_schemas::schemas::relations::{
            DEFAULT_RESOLVED_QUOTING, SNOWFLAKE_RESOLVED_QUOTING,
        };

        let ext = JsonExtractor::new(json)?;

        // Get adapter type from serialized data, falling back to context
        let adapter_type = ext
            .opt_str("adapter_type")
            .and_then(|s| s.parse::<AdapterType>().ok())
            .unwrap_or_else(|| ctx.replay_context().adapter_type);

        // Get quote_policy from serialized data, falling back to adapter-specific defaults.
        let default_quoting = match adapter_type {
            AdapterType::Snowflake => SNOWFLAKE_RESOLVED_QUOTING,
            _ => DEFAULT_RESOLVED_QUOTING,
        };

        let quote_policy = ext
            .opt_object("quote_policy")
            .map(|qp| ResolvedQuoting {
                database: qp
                    .get("database")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(default_quoting.database),
                schema: qp
                    .get("schema")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(default_quoting.schema),
                identifier: qp
                    .get("identifier")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(default_quoting.identifier),
            })
            .unwrap_or(default_quoting);

        let relation_type = if ext.bool_or("is_view", false) {
            Some(RelationType::View)
        } else if ext.bool_or("is_table", false) {
            Some(RelationType::Table)
        } else if ext.bool_or("is_materialized_view", false) {
            Some(RelationType::MaterializedView)
        } else if ext.bool_or("is_cte", false) {
            Some(RelationType::CTE)
        } else if ext.bool_or("is_dynamic_table", false) {
            Some(RelationType::DynamicTable)
        } else if ext.bool_or("is_interactive_table", false) {
            Some(RelationType::InteractiveTable)
        } else if ext.bool_or("is_streaming_table", false) {
            Some(RelationType::StreamingTable)
        } else if ext.bool_or("is_metric_view", false) {
            Some(RelationType::MetricView)
        } else {
            None
        };

        let mut relation = do_create_relation(
            adapter_type,
            ext.str_or("database", ""),
            ext.str_or("schema", ""),
            ext.opt_str("identifier"),
            relation_type,
            quote_policy,
        )
        .ok()?;

        relation.set_is_delta(Some(ext.bool_or("is_delta", false)));
        relation.set_is_shallow_clone(Some(ext.bool_or("is_shallow_clone", false)));

        Some(RelationObject::new(relation.into()).into_value())
    }
}

impl TimeMachineSerializable for crate::catalog_relation::CatalogRelation {
    const TYPE_ID: &'static str = "CatalogRelation";

    fn to_time_machine_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or_default()
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        let ext = JsonExtractor::new(json)?;

        let adapter_type = ext
            .opt_str("adapter_type")
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| ctx.replay_context().adapter_type);

        let adapter_properties = ext
            .opt_object("adapter_properties")
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let catalog = crate::catalog_relation::CatalogRelation {
            adapter_type,
            catalog_name: ext.opt_str("catalog_name"),
            integration_name: ext.opt_str("integration_name"),
            catalog_type: CatalogType::parse_from_str(
                &ext.str_or("catalog_type", ""),
                adapter_type,
            ),
            table_format: if ext
                .str_or("table_format", "")
                .eq_ignore_ascii_case("iceberg")
            {
                TableFormat::Iceberg
            } else {
                TableFormat::Default
            },
            adapter_properties,
            is_transient: ext.opt_bool("is_transient"),
            external_volume: ext.opt_str("external_volume"),
            catalog_database: ext.opt_str("catalog_database"),
            lakehouse_catalog: ext.opt_str("lakehouse_catalog"),
            base_location: ext.opt_str("base_location"),
            file_format: ext.opt_str("file_format"),
        };

        Some(minijinja::Value::from_object(catalog))
    }
}

impl TimeMachineSerializable for crate::column::Column {
    const TYPE_ID: &'static str = "Column";

    fn to_time_machine_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name(),
            "dtype": self.dtype(),
            "data_type": self.data_type(),
            "char_size": self.char_size(),
            "numeric_precision": self.numeric_precision(),
            "numeric_scale": self.numeric_scale(),
            "comment": self.comment(),
        })
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        let ext = JsonExtractor::new(json)?;
        let comment = ext.opt_str("comment");
        let adapter_type = ctx.replay_context().adapter_type;

        // Choose which recorded string to feed back into `Column::new` as the
        // `original_sql_str`. For most adapters `dtype` (the core dtype) is the
        // right choice. For BigQuery, `dtype` is a lossy label (e.g. "RECORD")
        // that does not round-trip: nested/repeated columns record their full
        // shape in `data_type` (e.g. "ARRAY<STRUCT<`inner` INT64>>"), which
        // `Column::new` re-parses into the same `core_dtype`/`core_data_type`.
        // Feeding `dtype` instead would collapse the type to "STRUCT" and break
        // same-version replay. See `test_column_reserialization_fixed_point_bigquery_repeated_struct`.
        let sql_str = match adapter_type {
            dbt_adapter_core::AdapterType::Bigquery => ext
                .opt_str("data_type")
                .unwrap_or_else(|| ext.str_or("dtype", "")),
            _ => ext.str_or("dtype", ""),
        };

        let column = crate::column::Column::new(
            adapter_type,
            ext.opt_str("name")?,
            sql_str,
            ext.opt_u32("char_size"),
            ext.opt_u64("numeric_precision"),
            ext.opt_u64("numeric_scale"),
        )
        .with_comment(comment);
        Some(minijinja::Value::from_object(column))
    }
}

impl TimeMachineSerializable for RelationConfig {
    const TYPE_ID: &'static str = "RelationConfig";

    fn to_time_machine_json(&self) -> serde_json::Value {
        let components = self
            .components()
            .filter_map(|(name, component)| {
                serde_json::to_value(component.to_jinja())
                    .ok()
                    .map(|value| ((*name).to_string(), value))
            })
            .collect();
        serde_json::Value::Object(components)
    }

    fn from_time_machine_json(
        json: &serde_json::Value,
        ctx: &ReplayCallContext,
    ) -> Option<minijinja::Value> {
        match ctx.replay_context().adapter_type {
            dbt_adapter_core::AdapterType::Databricks => {
                let relation_type = ctx.relation_type()?;
                let config = crate::relation::databricks::config::relation_types::relation_config_from_recorded(
                    ctx.replay_context().adapter_type,
                    relation_type,
                    json,
                )
                .ok()?;
                Some(minijinja::Value::from_object(config))
            }
            // TODO: Add typed reconstruction as adapter-specific recorded formats are supported.
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::relation::{RelationConfig, RelationObject};
    use crate::time_machine::serde::{ReplayContext, json_to_value_with_context, values_match};
    use crate::time_machine::serializable::serialize_object;

    use super::*;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::common::ResolvedQuoting;

    fn ctx() -> ReplayCallContext {
        ReplayContext {
            adapter_type: AdapterType::Snowflake,
            quoting: ResolvedQuoting::default(),
        }
        .into()
    }

    fn databricks_ctx(relation_type: RelationType) -> ReplayCallContext {
        let ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Databricks,
            quoting: ResolvedQuoting::default(),
        }
        .into();
        ctx.with_relation_type(Some(relation_type))
    }

    fn relation_config_payload() -> serde_json::Value {
        serde_json::json!({
            "column_comments": {
                "comments": {"event_id": "A UUID for this event."},
                "persist": true
            },
            "column_tags": {"set_column_tags": {"event_id": {"sensitivity": "internal"}}},
            "comment": {"comment": "An event sent when a purchase occurs.", "persist": true},
            "tags": {
                "set_tags": {
                    "asset_owner": "Warehouse Analytics",
                    "asset_state": "ACTIVE"
                }
            },
            "tblproperties": {"tblproperties": {"delta.parquet.compression.codec": "zstd"}}
        })
    }

    #[test]
    fn test_type_ids_are_stable() {
        assert_eq!(dbt_agate::AgateTable::TYPE_ID, "AgateTable");
        assert_eq!(crate::response::AdapterResponse::TYPE_ID, "AdapterResponse");
        assert_eq!(RelationObject::TYPE_ID, "RelationObject");
        assert_eq!(
            crate::catalog_relation::CatalogRelation::TYPE_ID,
            "CatalogRelation"
        );
        assert_eq!(crate::column::Column::TYPE_ID, "Column");
        assert_eq!(RelationConfig::TYPE_ID, "RelationConfig");
    }

    #[test]
    fn test_relation_config_roundtrip() {
        let cases = [
            (RelationType::Table, relation_config_payload()),
            (
                RelationType::View,
                serde_json::json!({
                    "column_tags": {"set_column_tags": {"id": {"sensitivity": "internal"}}}
                }),
            ),
            (
                RelationType::MaterializedView,
                serde_json::json!({
                    "partitioned_by": {"partition_by": ["event_date"]}
                }),
            ),
            (
                RelationType::StreamingTable,
                serde_json::json!({
                    "comment": {"comment": "streaming events", "persist": true},
                    "partitioned_by": {"partition_by": ["event_date"]}
                }),
            ),
            (
                RelationType::MetricView,
                serde_json::json!({
                    "tags": {"set_tags": {"team": "analytics"}},
                    "tblproperties": {
                        "tblproperties": {"quality": "gold"},
                        "pipeline_id": null
                    },
                    "query": {"query": "version: 1.1\nsource: orders"}
                }),
            ),
        ];

        for (relation_type, payload) in cases {
            let ctx = databricks_ctx(relation_type);
            let original = RelationConfig::from_time_machine_json(&payload, &ctx)
                .expect("RelationConfig should deserialize");
            let recorded = serialize_object(&original).expect("RelationConfig should serialize");
            let restored = json_to_value_with_context(&recorded, &ctx);
            assert_eq!(
                serialize_object(&restored),
                Some(recorded),
                "RelationConfig should roundtrip for {relation_type:?}"
            );
        }
    }

    /// Reproduces the Databricks same-version replay break.
    ///
    /// `tbl_properties::to_jinja` emits TWO keys for the `tblproperties` component:
    /// the `tblproperties` sub-map (with `pipelines.pipelineId` filtered out) AND a
    /// separate top-level `pipeline_id`. But `component_from_recorded` only reads back
    /// `val.get("tblproperties")` — the `pipeline_id` is silently dropped on deserialize.
    ///
    /// For any DLT-backed relation (streaming tables, materialized views), the recorded
    /// `pipeline_id` is a non-null string. On replay the recorded result is deserialized
    /// (dropping `pipeline_id`), flows into a later adapter call as an argument, and is
    /// reserialized — now with `pipeline_id: null`. The recorded-vs-actual comparison
    /// then mismatches (a non-zero string on the expected side vs null), and `values_match`
    /// cannot tolerate it. Replay fails on the SAME version that made the recording.
    #[test]
    fn test_relation_config_tblproperties_pipeline_id_fixed_point() {
        // Shape of a real recorded `get_relation_config` payload for a pipeline-backed
        // streaming table: tblproperties component carries a non-null pipeline_id.
        let recorded = serde_json::json!({
            "tblproperties": {
                "tblproperties": {"delta.parquet.compression.codec": "zstd"},
                "pipeline_id": "pipeline-abc-123"
            }
        });

        let ctx = databricks_ctx(RelationType::StreamingTable);
        let original = RelationConfig::from_time_machine_json(&recorded, &ctx)
            .expect("RelationConfig should deserialize");
        let reserialized = serialize_object(&original).expect("RelationConfig should serialize");

        // The reserialized pipeline_id must remain the recorded non-null value.
        // (Before the fix it came back as null, which `values_match` cannot tolerate
        // because a non-zero string on the expected side is not a zero value.)
        assert_eq!(
            reserialized["tblproperties"]["pipeline_id"],
            serde_json::json!("pipeline-abc-123"),
            "tblproperties pipeline_id must survive deserialize->serialize"
        );

        // Model the real replay comparison: `values_match` ignores the injected
        // `__type__` field but requires the pipeline_id to match.
        assert!(
            values_match(&recorded, &reserialized),
            "reserialized config must match the recording under replay comparison"
        );
    }

    #[test]
    fn test_relation_config_non_databricks_context_falls_back_to_map() {
        let payload = serde_json::json!({
            "__type__": "RelationConfig",
            "tags": {"set_tags": {"owner": "analytics"}}
        });
        let replay_ctx = ReplayContext {
            adapter_type: AdapterType::Snowflake,
            quoting: ResolvedQuoting::default(),
        };
        let ctx: ReplayCallContext = replay_ctx.into();
        let ctx = ctx.with_relation_type(Some(RelationType::Table));

        assert!(RelationConfig::from_time_machine_json(&payload, &ctx).is_none());
        let value = json_to_value_with_context(&payload, &ctx);
        assert!(value.downcast_object::<RelationConfig>().is_none());
        assert_eq!(
            value
                .get_attr("tags")
                .unwrap()
                .get_attr("set_tags")
                .unwrap()
                .get_attr("owner")
                .unwrap()
                .as_str(),
            Some("analytics")
        );
    }

    #[test]
    fn test_relation_config_requires_supported_relation_type_context() {
        let payload = relation_config_payload();
        let ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Databricks,
            quoting: ResolvedQuoting::default(),
        }
        .into();
        assert!(RelationConfig::from_time_machine_json(&payload, &ctx).is_none());

        let ctx = ctx.with_relation_type(Some(RelationType::CTE));
        assert!(RelationConfig::from_time_machine_json(&payload, &ctx).is_none());
    }

    #[test]
    fn test_adapter_response_roundtrip() {
        let original = crate::response::AdapterResponse::new()
            .with_message("SUCCESS 42")
            .with_code("SUCCESS")
            .with_rows_affected(42)
            .with_query_id("query-123");

        let json = original.to_time_machine_json();
        assert_eq!(json["message"], "SUCCESS 42");
        assert_eq!(json["rows_affected"], 42);

        let value =
            crate::response::AdapterResponse::from_time_machine_json(&json, &ctx()).unwrap();
        let response = value
            .downcast_object::<crate::response::AdapterResponse>()
            .unwrap();
        assert_eq!(response.message(), original.message());
        assert_eq!(response.rows_affected(), original.rows_affected());
    }

    #[test]
    fn test_catalog_relation_roundtrip() {
        use std::collections::BTreeMap;

        let original = crate::catalog_relation::CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: Some("my_catalog".to_string()),
            integration_name: Some("my_integration".to_string()),
            catalog_type: CatalogType::SnowflakeBuiltIn,
            table_format: TableFormat::Iceberg,
            adapter_properties: BTreeMap::from([("key1".to_string(), "value1".to_string())]),
            is_transient: Some(false),
            external_volume: Some("my_volume".to_string()),
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: Some("/path/to/data".to_string()),
            file_format: None,
        };

        let json = original.to_time_machine_json();
        assert_eq!(json["catalog_name"], "my_catalog");
        assert_eq!(json["table_format"], "iceberg");

        let value = crate::catalog_relation::CatalogRelation::from_time_machine_json(&json, &ctx())
            .unwrap();
        let catalog = value
            .downcast_object::<crate::catalog_relation::CatalogRelation>()
            .unwrap();
        assert_eq!(catalog.catalog_name, original.catalog_name);
        assert_eq!(catalog.table_format, original.table_format);
    }

    #[test]
    fn test_column_roundtrip() {
        let original = crate::column::Column::new(
            AdapterType::Snowflake,
            "my_column".to_string(),
            "VARCHAR".to_string(),
            Some(255),
            None,
            None,
        )
        .with_comment(Some("A useful column".to_string()));

        let json = original.to_time_machine_json();
        assert_eq!(json["name"], "my_column");
        assert_eq!(json["dtype"], "VARCHAR");
        assert_eq!(json["comment"], "A useful column");

        let value = crate::column::Column::from_time_machine_json(&json, &ctx()).unwrap();
        let column = value.downcast_object::<crate::column::Column>().unwrap();
        assert_eq!(column.name(), original.name());
        assert_eq!(column.dtype(), original.dtype());
        assert_eq!(column.comment(), original.comment());
    }

    #[test]
    fn test_relation_object_roundtrip_with_quoting() {
        use dbt_schemas::dbt_types::RelationType;

        // Create a relation with custom quoting
        let custom_quoting = ResolvedQuoting {
            database: false,
            schema: false,
            identifier: true,
        };

        let relation = do_create_relation(
            AdapterType::Snowflake,
            "MY_DB".to_string(),
            "MY_SCHEMA".to_string(),
            Some("my_table".to_string()),
            Some(RelationType::Table),
            custom_quoting,
        )
        .unwrap();

        let original = RelationObject::from(relation);

        let json = original.to_time_machine_json();

        // Verify quoting is serialized
        assert_eq!(json["quote_policy"]["database"], false);
        assert_eq!(json["quote_policy"]["schema"], false);
        assert_eq!(json["quote_policy"]["identifier"], true);
        assert_eq!(json["adapter_type"], "snowflake");

        // Deserialize with a DIFFERENT context quoting - should use serialized quoting
        let different_ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Snowflake,
            quoting: ResolvedQuoting {
                database: true, // Different quoting
                schema: true,
                identifier: false,
            },
        }
        .into();

        let value = RelationObject::from_time_machine_json(&json, &different_ctx).unwrap();
        let restored = value.downcast_object::<RelationObject>().unwrap();

        // Verify the restored relation uses the serialized quoting
        assert!(!restored.quote_policy().database);
        assert!(!restored.quote_policy().schema);
        assert!(restored.quote_policy().identifier);

        // Verify adapter type is also restored from serialized data
        assert!(matches!(restored.adapter_type(), AdapterType::Snowflake));
    }

    /// The relation type survives replay only if it has both a serialized flag and a matching
    /// arm in the reconstruction chain; without either one it silently comes back as `None`.
    #[test]
    fn test_relation_object_roundtrip_preserves_relation_type() {
        use dbt_schemas::dbt_types::RelationType;

        for relation_type in [
            RelationType::Table,
            RelationType::View,
            RelationType::MaterializedView,
            RelationType::DynamicTable,
            RelationType::InteractiveTable,
            RelationType::StreamingTable,
            RelationType::MetricView,
        ] {
            let relation = do_create_relation(
                AdapterType::Snowflake,
                "MY_DB".to_string(),
                "MY_SCHEMA".to_string(),
                Some("my_table".to_string()),
                Some(relation_type),
                ResolvedQuoting::default(),
            )
            .unwrap();

            let json = RelationObject::from(relation).to_time_machine_json();
            let value = RelationObject::from_time_machine_json(&json, &ctx()).unwrap();
            let restored = value.downcast_object::<RelationObject>().unwrap();

            assert_eq!(
                restored.relation_type(),
                Some(relation_type),
                "{relation_type:?} did not survive replay"
            );
        }
    }

    #[test]
    fn test_databricks_relation_roundtrip_preserves_is_delta() {
        use dbt_schemas::dbt_types::RelationType;

        let custom_quoting = ResolvedQuoting {
            database: false,
            schema: false,
            identifier: false,
        };

        // Create a Databricks relation with is_delta=true (as would come from a real warehouse)
        let mut relation = do_create_relation(
            AdapterType::Databricks,
            "my_catalog".to_string(),
            "my_schema".to_string(),
            Some("my_table".to_string()),
            Some(RelationType::Table),
            custom_quoting,
        )
        .unwrap();
        relation.set_is_delta(Some(true));

        let original = RelationObject::from(relation);
        assert!(original.is_delta(), "original should have is_delta=true");

        let json = original.to_time_machine_json();
        assert_eq!(json["is_delta"], true, "is_delta should be serialized");

        let databricks_ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Databricks,
            quoting: custom_quoting,
        }
        .into();

        let value = RelationObject::from_time_machine_json(&json, &databricks_ctx).unwrap();
        let restored = value.downcast_object::<RelationObject>().unwrap();

        assert!(
            restored.is_delta(),
            "restored relation must preserve is_delta=true"
        );
    }

    #[test]
    fn test_databricks_relation_backward_compat_missing_is_delta() {
        // Old recordings won't have is_delta in the JSON — should default to false
        let old_format_json = serde_json::json!({
            "adapter_type": "databricks",
            "database": "my_catalog",
            "schema": "my_schema",
            "identifier": "my_table",
            "is_table": true,
        });

        let databricks_ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Databricks,
            quoting: ResolvedQuoting::default(),
        }
        .into();

        let value =
            RelationObject::from_time_machine_json(&old_format_json, &databricks_ctx).unwrap();
        let restored = value.downcast_object::<RelationObject>().unwrap();

        assert!(
            !restored.is_delta(),
            "missing is_delta should default to false for backward compat"
        );
    }

    #[test]
    fn test_relation_object_backward_compat_postgres_defaults() {
        // Same test but for Postgres, which has different default quoting (all true)
        let old_format_json = serde_json::json!({
            "database": "mydb",
            "schema": "public",
            "identifier": "users",
            "is_table": true,
        });

        let ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Postgres,
            quoting: ResolvedQuoting {
                database: false, // Context has different quoting
                schema: false,
                identifier: false,
            },
        }
        .into();

        let value = RelationObject::from_time_machine_json(&old_format_json, &ctx).unwrap();
        let restored = value.downcast_object::<RelationObject>().unwrap();

        // Should use Postgres defaults (all true), NOT context quoting (all false)
        assert!(restored.quote_policy().database);
        assert!(restored.quote_policy().schema);
        assert!(restored.quote_policy().identifier);
    }

    // =========================================================================
    // Fixed-point (reserialization idempotency) tests.
    //
    // These model the replay data-flow that actually breaks Fusion→Fusion
    // replay on the SAME version: an object returned by one adapter call is
    // reconstructed via `json_to_value_with_context` (engine.rs), then passed
    // as an ARGUMENT to a later adapter call, where it is re-serialized via
    // `serialize_object`/`serialize_args` and compared against the later
    // event's recorded args.
    //
    // For that to match, serialization must be a fixed point:
    //
    //     let j1 = serialize(original);
    //     let restored = deserialize(j1);
    //     let j2 = serialize(restored);
    //     assert_eq!(j1, j2);   // <-- the invariant replay depends on
    //
    // The existing `*_roundtrip` tests only check accessors after ONE
    // deserialize; they do NOT assert this fixed point.
    // =========================================================================

    /// Assert `serialize(deserialize(serialize(original))) == serialize(original)`.
    ///
    /// `original` must already be a serializable jinja Value (i.e. `serialize_object`
    /// returns `Some`). Returns the first serialization for optional further checks.
    fn assert_reserialization_fixed_point(
        original: &minijinja::Value,
        ctx: &ReplayCallContext,
    ) -> serde_json::Value {
        let j1 = serialize_object(original)
            .expect("original value should be serializable via the time-machine registry");
        let restored = json_to_value_with_context(&j1, ctx);
        let j2 = serialize_object(&restored).expect(
            "reconstructed value should re-serialize via the time-machine registry \
             (if this is None, deserialize produced a non-registry type such as a plain map)",
        );
        assert_eq!(
            j1, j2,
            "reserialization must be a fixed point; serialize/deserialize are asymmetric.\n\
             first:  {j1:#}\n\
             second: {j2:#}"
        );
        j1
    }

    #[test]
    fn test_column_reserialization_fixed_point_snowflake_varchar() {
        // A VARCHAR(255) column as returned by e.g. get_columns_in_relation.
        let original = crate::column::Column::new(
            AdapterType::Snowflake,
            "my_column".to_string(),
            "VARCHAR".to_string(),
            Some(255),
            None,
            None,
        )
        .with_comment(Some("A useful column".to_string()));

        assert_reserialization_fixed_point(&minijinja::Value::from_object(original), &ctx());
    }

    #[test]
    fn test_column_reserialization_fixed_point_snowflake_numeric() {
        // NUMBER(38,2): data_type() is computed from core_dtype + precision/scale.
        let original = crate::column::Column::new(
            AdapterType::Snowflake,
            "amount".to_string(),
            "NUMBER".to_string(),
            None,
            Some(38),
            Some(2),
        );

        assert_reserialization_fixed_point(&minijinja::Value::from_object(original), &ctx());
    }

    #[test]
    fn test_column_reserialization_fixed_point_bigquery_repeated_struct() {
        // BigQuery REPEATED / STRUCT columns carry _fields + mode that are dropped
        // on deserialize. Before the fix the recomputed data_type() shape differed on
        // re-serialize (e.g. "RECORD"/"ARRAY<STRUCT<`inner` INT64>>" -> "STRUCT"),
        // breaking Fusion->Fusion replay on the SAME version when such a column is
        // returned by one adapter call and passed as an arg to a later one.
        //
        // A real repeated-struct column carries the full SQL type as its original_sql_str
        // and the nested fields; both `dtype` (RECORD) and `data_type`
        // (ARRAY<STRUCT<...>>) are derived from _fields + mode.
        use crate::column::BigqueryColumnMode;

        let inner = crate::column::Column::new_bigquery(
            "inner".to_string(),
            "INT64".to_string(),
            Vec::new(),
            BigqueryColumnMode::Nullable,
        );
        let original = crate::column::Column::new_bigquery(
            "events".to_string(),
            "ARRAY<STRUCT<`inner` INT64>>".to_string(),
            vec![inner],
            BigqueryColumnMode::Repeated,
        );

        let bq_ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Bigquery,
            quoting: ResolvedQuoting::default(),
        }
        .into();

        assert_reserialization_fixed_point(&minijinja::Value::from_object(original), &bq_ctx);
    }

    #[test]
    fn test_column_reserialization_fixed_point_bigquery_nested_struct() {
        // A non-repeated (NULLABLE) STRUCT column. dtype = "RECORD",
        // data_type = "STRUCT<`a` INT64, `b` STRING>". Same drift class as the
        // repeated case: reconstruct must rebuild from data_type, not dtype.
        use crate::column::BigqueryColumnMode;

        let a = crate::column::Column::new_bigquery(
            "a".to_string(),
            "INT64".to_string(),
            Vec::new(),
            BigqueryColumnMode::Nullable,
        );
        let b = crate::column::Column::new_bigquery(
            "b".to_string(),
            "STRING".to_string(),
            Vec::new(),
            BigqueryColumnMode::Nullable,
        );
        let original = crate::column::Column::new_bigquery(
            "payload".to_string(),
            "STRUCT<`a` INT64, `b` STRING>".to_string(),
            vec![a, b],
            BigqueryColumnMode::Nullable,
        );

        let bq_ctx: ReplayCallContext = ReplayContext {
            adapter_type: AdapterType::Bigquery,
            quoting: ResolvedQuoting::default(),
        }
        .into();

        assert_reserialization_fixed_point(&minijinja::Value::from_object(original), &bq_ctx);
    }

    #[test]
    fn test_relation_object_reserialization_fixed_point_table() {
        // A plain table relation should be a clean fixed point (single is_table flag).
        let relation = do_create_relation(
            AdapterType::Snowflake,
            "MY_DB".to_string(),
            "MY_SCHEMA".to_string(),
            Some("my_table".to_string()),
            Some(RelationType::Table),
            dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING,
        )
        .unwrap();
        let original = RelationObject::from(relation);

        assert_reserialization_fixed_point(&original.into_value(), &ctx());
    }

    #[test]
    fn test_relation_object_reserialization_fixed_point_dynamic_table() {
        // is_dynamic_table sits BELOW is_table/is_view in the deserialize priority
        // chain. A dynamic table (which also reports is_table == true on some
        // adapters) is at risk of collapsing to RelationType::Table on the way back.
        let ctx = ctx();
        let relation = do_create_relation(
            AdapterType::Snowflake,
            "MY_DB".to_string(),
            "MY_SCHEMA".to_string(),
            Some("my_dynamic_table".to_string()),
            Some(RelationType::DynamicTable),
            dbt_schemas::schemas::relations::SNOWFLAKE_RESOLVED_QUOTING,
        )
        .unwrap();
        let original = RelationObject::from(relation);

        // NOTE: This currently PASSES. All RelationObject `is_*` flags are mutually
        // exclusive derivations of a single `relation_type()` (see
        // dbt-schemas relations/base.rs), so exactly one flag is ever true and the
        // deserialize priority chain reconstructs the same RelationType. The flag
        // collapse is therefore not a replay drift source in practice.
        assert_reserialization_fixed_point(&original.into_value(), &ctx);
    }

    #[test]
    fn test_agate_table_reserialization_fixed_point() {
        // AgateTable is compared by the verbatim base64 `__ipc__` string
        // (values_match treats it as opaque). decode -> from_record_batch ->
        // to_record_batch -> LZ4 re-encode must reproduce byte-identical base64
        // for a replayed table passed back as an argument to match.
        use std::sync::Arc;

        use arrow::array::{Int64Array, RecordBatch, StringArray};
        use arrow_schema::{DataType, Field, Schema};

        let schema = Schema::new(vec![
            Field::new("id", DataType::Int64, false),
            Field::new("name", DataType::Utf8, true),
        ]);
        let batch = RecordBatch::try_new(
            Arc::new(schema),
            vec![
                Arc::new(Int64Array::from(vec![1, 2, 3])),
                Arc::new(StringArray::from(vec![Some("a"), None, Some("c")])),
            ],
        )
        .unwrap();
        let table = dbt_agate::AgateTable::from_record_batch(Arc::new(batch));

        assert_reserialization_fixed_point(&minijinja::Value::from_object(table), &ctx());
    }
}
