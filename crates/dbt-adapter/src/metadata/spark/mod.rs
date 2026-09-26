//! Spark metadata queries.
//!
//! Relation listing is based on `SHOW TABLE EXTENDED`, which reports the
//! relation type and storage provider for every relation in a schema in one
//! query. Unlike `system.information_schema` (a Databricks Unity Catalog
//! feature) it exists on every Spark backend, including Microsoft Fabric
//! Lakehouses, where it works for both classic lakehouses (2-part
//! `schema.table` naming) and schema-enabled lakehouses (3-part
//! `lakehouse.schema.table`, with the lakehouse in the catalog slot).

use std::sync::Arc;

use arrow_array::*;
use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::relations::base::BaseRelation;

use crate::AdapterEngine;
use crate::errors::AdapterResult;
use crate::metadata::CatalogAndSchema;
use crate::record_batch::RecordBatchExt;
use crate::relation::Relation;
use dbt_adbc::{Connection, QueryCtx};

/// Extracts the value of a `Key: value` line from `SHOW TABLE EXTENDED`'s
/// `information` column.
fn information_value<'a>(information: &'a str, key: &str) -> Option<&'a str> {
    information.lines().find_map(|line| {
        line.strip_prefix(key)
            .and_then(|rest| rest.strip_prefix(": "))
            .map(str::trim)
    })
}

/// Returns true when a query error reports that the target schema does not
/// exist, in which case it has no relations (dbt will create it).
///
/// The Livy driver surfaces Spark's error class in the message text; Spark's
/// class for this condition is `SCHEMA_NOT_FOUND` ([SQLSTATE 42704]).
fn is_schema_not_found(message: &str) -> bool {
    message.contains("[SCHEMA_NOT_FOUND]") || message.contains("SCHEMA_NOT_FOUND")
}

/// Converts a `SHOW TABLE EXTENDED IN <ns> LIKE '*'` result batch into
/// relations. Separated from query execution so it can be unit tested with
/// synthetic batches.
fn relations_from_show_table_extended(
    adapter_type: AdapterType,
    quoting: ResolvedQuoting,
    db_schema: &CatalogAndSchema,
    batch: &RecordBatch,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    if batch.num_rows() == 0 {
        return Ok(Vec::new());
    }

    let catalog = &db_schema.resolved_catalog;
    let schema = &db_schema.resolved_schema;

    let names = batch.column_values::<StringArray>("tableName")?;
    let is_temporary = batch.column_values::<BooleanArray>("isTemporary")?;
    let information = batch.column_values::<StringArray>("information")?;

    let mut relations = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        if is_temporary.value(i) {
            continue;
        }
        let info = information.value(i);
        // `Type:` is MANAGED, EXTERNAL or VIEW; `Provider:` is the storage
        // format (`delta` for regular lakehouse tables) and absent for views.
        let table_type = information_value(info, "Type").unwrap_or("MANAGED");
        let is_delta = information_value(info, "Provider") == Some("delta");

        let relation = Arc::new(
            Relation::new(
                adapter_type,
                (!catalog.is_empty()).then(|| catalog.to_string()),
                schema.to_string(),
                names.value(i).to_string(),
            )
            .with_relation_type(RelationType::from_adapter_type(adapter_type, table_type))
            .with_quoting(quoting)
            .with_is_delta(is_delta),
        ) as Arc<dyn BaseRelation>;
        relations.push(relation);
    }

    Ok(relations)
}

pub fn list_relations(
    engine: &dyn AdapterEngine,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    db_schema: &CatalogAndSchema,
    token: CancellationToken,
) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
    let catalog = &db_schema.resolved_catalog;
    let schema = &db_schema.resolved_schema;
    let namespace = if catalog.is_empty() {
        format!("`{schema}`")
    } else {
        format!("`{catalog}`.`{schema}`")
    };

    let sql = format!("SHOW TABLE EXTENDED IN {namespace} LIKE '*'");
    let batch = match engine.execute(None, conn, ctx, &sql, token) {
        // A schema that doesn't exist yet has no relations; dbt will create it.
        Err(e) if is_schema_not_found(&e.to_string()) => return Ok(Vec::new()),
        other => other?,
    };

    relations_from_show_table_extended(
        engine.adapter_type(),
        engine.quoting(),
        db_schema,
        &batch,
    )
}

#[cfg(test)]
mod show_table_extended_tests {
    use super::*;
    use arrow_schema::{DataType, Field, Schema};

    #[test]
    fn information_value_parses_show_table_extended_lines() {
        let info = "Catalog: spark_catalog\nDatabase: `WS`.`lh`.`dbo`\nTable: probe\nType: MANAGED\nProvider: delta\nLocation: abfss://...\n";
        assert_eq!(information_value(info, "Type"), Some("MANAGED"));
        assert_eq!(information_value(info, "Provider"), Some("delta"));
        assert_eq!(information_value(info, "Missing"), None);

        let view_info = "Catalog: spark_catalog\nTable: v\nType: VIEW\nView Text: SELECT 1\n";
        assert_eq!(information_value(view_info, "Type"), Some("VIEW"));
        assert_eq!(information_value(view_info, "Provider"), None);
    }

    #[test]
    fn schema_not_found_matches_fabric_error_text() {
        // Verbatim shape of the error Fabric returns for a missing schema
        // (captured from a live lakehouse; identifiers replaced).
        let fabric_error = "[spark] livy: query error: Error: [SCHEMA_NOT_FOUND] \
The schema `my_lakehouse.missing_schema` cannot be found. Verify the spelling \
and correctness of the schema and catalog.";
        assert!(is_schema_not_found(fabric_error));

        assert!(!is_schema_not_found(
            "[spark] livy: query error: Error: [TABLE_OR_VIEW_NOT_FOUND] ..."
        ));
        assert!(!is_schema_not_found("connection reset by peer"));
    }

    /// Builds a synthetic `SHOW TABLE EXTENDED` result batch.
    fn show_table_extended_batch(rows: &[(&str, bool, &str)]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("namespace", DataType::Utf8, true),
            Field::new("tableName", DataType::Utf8, true),
            Field::new("isTemporary", DataType::Boolean, true),
            Field::new("information", DataType::Utf8, true),
        ]));
        let namespaces: StringArray = rows.iter().map(|_| Some("ns")).collect();
        let names: StringArray = rows.iter().map(|(n, _, _)| Some(*n)).collect();
        let temps: BooleanArray = rows.iter().map(|(_, t, _)| Some(*t)).collect();
        let infos: StringArray = rows.iter().map(|(_, _, i)| Some(*i)).collect();
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(namespaces),
                Arc::new(names),
                Arc::new(temps),
                Arc::new(infos),
            ],
        )
        .unwrap()
    }

    const MANAGED_DELTA: &str = "Type: MANAGED\nProvider: delta\n";
    const EXTERNAL_PARQUET: &str = "Type: EXTERNAL\nProvider: parquet\n";
    const VIEW: &str = "Type: VIEW\nView Text: SELECT 1\n";

    fn relations_for(catalog: &str, rows: &[(&str, bool, &str)]) -> Vec<Arc<dyn BaseRelation>> {
        let batch = show_table_extended_batch(rows);
        let db_schema = CatalogAndSchema {
            rendered_catalog: catalog.to_string(),
            rendered_schema: "dbo".to_string(),
            resolved_catalog: catalog.to_string(),
            resolved_schema: "dbo".to_string(),
        };
        relations_from_show_table_extended(
            AdapterType::Spark,
            ResolvedQuoting::trues(),
            &db_schema,
            &batch,
        )
        .unwrap()
    }

    #[test]
    fn maps_relation_types_and_delta_provider() {
        let relations = relations_for(
            "lakehouse",
            &[
                ("t_managed", false, MANAGED_DELTA),
                ("t_external", false, EXTERNAL_PARQUET),
                ("v_view", false, VIEW),
            ],
        );
        assert_eq!(relations.len(), 3);

        let managed = &relations[0];
        assert_eq!(managed.identifier_as_str().unwrap(), "t_managed");
        assert_eq!(managed.relation_type(), Some(RelationType::Table));
        assert!(managed.is_delta());

        let external = &relations[1];
        assert_eq!(external.relation_type(), Some(RelationType::Table));
        assert!(!external.is_delta());

        let view = &relations[2];
        assert_eq!(view.relation_type(), Some(RelationType::View));
        assert!(!view.is_delta());
    }

    #[test]
    fn excludes_temporary_relations() {
        let relations = relations_for(
            "lakehouse",
            &[
                ("t_perm", false, MANAGED_DELTA),
                ("t_temp", true, MANAGED_DELTA),
            ],
        );
        assert_eq!(relations.len(), 1);
        assert_eq!(relations[0].identifier_as_str().unwrap(), "t_perm");
    }

    #[test]
    fn schema_enabled_lakehouses_get_three_part_relations() {
        let relations = relations_for("lakehouse", &[("t", false, MANAGED_DELTA)]);
        assert_eq!(relations[0].database(), Some("lakehouse"));
        assert_eq!(relations[0].schema_as_str().unwrap(), "dbo");
    }

    #[test]
    fn classic_lakehouses_get_two_part_relations() {
        let relations = relations_for("", &[("t", false, MANAGED_DELTA)]);
        // classic lakehouses are single-database: no database component
        assert_eq!(relations[0].database(), None);
        assert_eq!(relations[0].schema_as_str().unwrap(), "dbo");
    }

    #[test]
    fn empty_batch_yields_no_relations() {
        assert!(relations_for("lakehouse", &[]).is_empty());
    }
}

use crate::column::Column;

/// Keep only the leading column names from a Spark `describe extended` result,
/// discarding all columns from a separator column name (empty string or starts
/// with `#`)
///
/// Mirrors dbt-core's `SparkAdapter.parse_describe_extended` from dbt-adapters/dbt-spark
pub(crate) fn truncate_at_describe_extended_separator(columns: Vec<Column>) -> Vec<Column> {
    let end = columns
        .iter()
        .position(|c| c.name().is_empty() || c.name().starts_with('#'))
        .unwrap_or(columns.len());
    columns.into_iter().take(end).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter_core::AdapterType;

    #[test]
    fn test_truncate_at_describe_extended_separator() {
        let mk = |name: &str| {
            Column::new(
                AdapterType::Spark,
                name.to_string(),
                "string".to_string(),
                None,
                None,
                None,
            )
        };

        // Real columns, then the blank separator, then the extended-metadata section.
        let columns = vec![
            mk("id"),
            mk("name"),
            mk(""),
            mk("# Detailed Table Information"),
            mk("Catalog"),
            mk("Database"),
        ];
        let kept = truncate_at_describe_extended_separator(columns);
        let names: Vec<&str> = kept.iter().map(|c| c.name()).collect();
        assert_eq!(names, vec!["id", "name"]);

        // No separator: every column is kept.
        let columns = vec![mk("id"), mk("name")];
        assert_eq!(truncate_at_describe_extended_separator(columns).len(), 2);

        // A leading '#' row truncates to empty.
        let columns = vec![mk("# Detailed Table Information")];
        assert!(truncate_at_describe_extended_separator(columns).is_empty());

        // Empty input stays empty.
        assert!(truncate_at_describe_extended_separator(vec![]).is_empty());
    }
}
