//! Sources in properties files: one entry for each table of a source.

use crate::resolve::properties_index::{
    IndexCtx, IndexedName, PackageProperties, PropertiesEntry, validate_resource_name,
};
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::properties::{MinimalSchemaValue, MinimalTableValue};
use dbt_yaml::Value;
use minijinja::Value as MinijinjaValue;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// Between the name of a source and the name of its table, in the name a table is found under.
/// Names found under it sort as the pairs of source and table do.
const SEPARATOR: char = '\0';

/// The names that the source `item` defines: one for each of its tables, in their order.
pub(crate) fn index_source(
    key: &'static str,
    item: &Value,
    ctx: &IndexCtx<'_>,
) -> FsResult<Vec<IndexedName>> {
    // Pre-render the `tables` field if it is a single Jinja expression
    // (e.g. `{{ var('source_tables') }}`). MinimalSchemaValue wraps
    // `tables` in `Verbatim` so that table *contents* are not rendered
    // prematurely, but this also blocks expanding the field value itself
    // from a string into a sequence.
    // See: https://github.com/dbt-labs/dbt-fusion/issues/982
    let item = with_tables_rendered(item, ctx.jinja_env, ctx.base_ctx);
    let source: MinimalSchemaValue = ctx.typed(&item)?;
    let Some(tables) = &*source.tables else {
        emit_warn_log_message(
            ErrorCode::NoTablesInSource,
            format!(
                "No tables defined for source '{}' in file '{}'.",
                source.name,
                ctx.properties_path.display()
            ),
        );
        return Ok(Vec::new());
    };
    validate_resource_name(&source.name)?;
    tables
        .iter()
        .map(|table| {
            let table: MinimalTableValue = ctx.typed(table)?;
            Ok(IndexedName {
                key,
                name: format!("{}{SEPARATOR}{}", source.name, table.name.as_ref()),
                name_span: table.name.span().clone(),
            })
        })
        .collect()
}

/// Takes the entries of the tables under `sources:`, by the names of the source and the table.
///
/// The entry of a table has the table's YAML, and a copy of its source's YAML without the
/// tables.
pub(crate) fn take_source_table_entries(
    properties: &mut PackageProperties,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> BTreeMap<(String, String), PropertiesEntry> {
    let mut entries = BTreeMap::new();
    // The tables of each source read so far, taken out of it, by the file and the position of
    // the source.
    let mut tables_of = BTreeMap::new();
    for indexed in properties.take_indexed("sources") {
        let location = indexed.location;
        let Some((source_name, table_name)) = indexed.name.split_once(SEPARATOR) else {
            continue;
        };
        let tables = match tables_of.entry((location.file, location.item)) {
            Entry::Occupied(tables) => tables.into_mut(),
            Entry::Vacant(tables) => {
                let source = properties.item_mut(&location);
                pre_render_tables_field(source, jinja_env, base_ctx);
                // Set only the tables field to null while preserving all other fields and their
                // spans.
                let taken = source.as_mapping_mut().and_then(|source| {
                    source.insert(Value::string("tables".to_string()), Value::null())
                });
                tables.insert(match taken {
                    Some(Value::Sequence(tables, ..)) => tables,
                    _ => Vec::new(),
                })
            }
        };
        let path = properties.path(&location).to_path_buf();
        for duplicate_path in &indexed.duplicate_paths {
            emit_warn_log_message(
                ErrorCode::DuplicateSourceTable,
                format!(
                    "Duplicate definition for table '{}' in source '{}' found in file '{}'. Using definition from '{}'.",
                    table_name,
                    source_name,
                    duplicate_path.display(),
                    path.display()
                ),
            );
        }
        let entry = PropertiesEntry {
            name: table_name.to_string(),
            name_span: indexed.name_span,
            relative_path: path,
            schema_value: properties.item(&location).clone(),
            table_value: Some(
                tables
                    .get_mut(location.part)
                    .map(std::mem::take)
                    .unwrap_or_default(),
            ),
            version_info: None,
            duplicate_paths: indexed.duplicate_paths,
        };
        entries.insert((source_name.to_string(), table_name.to_string()), entry);
    }
    entries
}

/// `source`, with its `tables` rendered if they are a single Jinja expression.
fn with_tables_rendered<'a>(
    source: &'a Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> Cow<'a, Value> {
    if source
        .get("tables")
        .is_some_and(|tables| tables.as_str().is_some())
    {
        let mut source = source.clone();
        pre_render_tables_field(&mut source, jinja_env, base_ctx);
        Cow::Owned(source)
    } else {
        Cow::Borrowed(source)
    }
}

/// If the `tables` field of a source YAML mapping is a single Jinja expression
/// (e.g. `"{{ var('source_tables') }}"`), evaluate it and replace the string
/// with the rendered sequence in-place. This is necessary because
/// `MinimalSchemaValue.tables` is wrapped in `Verbatim` (to protect table
/// *contents* from premature rendering), which also blocks the field-level
/// Jinja transform that would normally expand the string into a sequence.
fn pre_render_tables_field(
    source_value: &mut Value,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) {
    let Some(mapping) = source_value.as_mapping_mut() else {
        return;
    };
    let tables_key = Value::string("tables".to_string());
    let jinja_expr = mapping
        .get(&tables_key)
        .and_then(|v| v.as_str())
        .filter(|s| dbt_jinja_utils::serde::check_single_expression_without_whitepsace_control(s))
        .map(|s| s[2..s.len() - 2].trim().to_string());

    if let Some(expr) = jinja_expr {
        if let Ok(compiled) = jinja_env.compile_expression(&expr) {
            if let Ok(result) = compiled.eval(base_ctx, &[]) {
                if let Ok(val) = dbt_yaml::to_value(&result) {
                    if val.is_sequence() {
                        mapping.insert(tables_key, val);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::properties_extensions::NoExtensions;
    use crate::resolve::properties_index::PropertiesFile;
    use crate::resolve::resolve_sources::Sources;
    use std::path::Path;
    use std::sync::Arc;

    /// The entries of the tables of the sources in `files`, each a path and its content.
    async fn table_entries(
        files: &[(&str, &str)],
    ) -> FsResult<BTreeMap<(String, String), PropertiesEntry>> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let mut properties = PackageProperties::default();
        for (path, input) in files {
            let file = PropertiesFile::read(
                input,
                Path::new(path),
                &Path::new("/project").join(path),
                None,
                &["sources"],
                &[],
            )?;
            properties
                .index_file(
                    &[&Sources::<NoExtensions>::default()],
                    file,
                    &IndexCtx::new(&jinja_env, &base_ctx),
                )
                .await?;
        }
        Ok(take_source_table_entries(
            &mut properties,
            &jinja_env,
            &base_ctx,
        ))
    }

    fn key(source: &str, table: &str) -> (String, String) {
        (source.to_string(), table.to_string())
    }

    fn string<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
        value.get(field)?.as_str()
    }

    #[dbt_runtime::test]
    async fn a_source_is_one_entry_for_each_table() -> FsResult<()> {
        let entries = table_entries(&[(
            "a.yml",
            "sources:
  - name: raw
    database: lake
    tables:
      - name: orders
        identifier: orders_v2
      - name: customers
  - name: raw_events
    tables:
      - name: clicks
",
        )])
        .await?;
        // In the order of the pairs of source and table.
        assert_eq!(
            entries.keys().cloned().collect::<Vec<_>>(),
            [
                key("raw", "customers"),
                key("raw", "orders"),
                key("raw_events", "clicks")
            ]
        );
        let orders = &entries[&key("raw", "orders")];
        assert_eq!(orders.name, "orders");
        assert!(orders.name_span.is_valid());
        assert_eq!(orders.relative_path, Path::new("a.yml"));
        // The source's YAML, without its tables.
        assert_eq!(string(&orders.schema_value, "database"), Some("lake"));
        assert!(
            orders
                .schema_value
                .get("tables")
                .is_some_and(Value::is_null)
        );
        // The table's YAML.
        let table = orders.table_value.as_ref().expect("a table");
        assert_eq!(string(table, "identifier"), Some("orders_v2"));
        let customers = entries[&key("raw", "customers")].table_value.as_ref();
        assert_eq!(
            customers.and_then(|table| string(table, "name")),
            Some("customers")
        );
        Ok(())
    }

    #[dbt_runtime::test]
    async fn the_first_definition_of_a_table_is_its_entry() -> FsResult<()> {
        let entries = table_entries(&[
            ("a.yml", "sources:\n  - name: raw\n    tables:\n      - name: orders\n        identifier: first\n"),
            ("b.yml", "sources:\n  - name: raw\n    tables:\n      - name: orders\n        identifier: second\n"),
        ])
        .await?;
        let orders = &entries[&key("raw", "orders")];
        assert_eq!(orders.relative_path, Path::new("a.yml"));
        assert_eq!(orders.duplicate_paths, [Path::new("b.yml")]);
        let table = orders.table_value.as_ref().expect("a table");
        assert_eq!(string(table, "identifier"), Some("first"));
        Ok(())
    }

    #[dbt_runtime::test]
    async fn a_source_without_tables_has_no_entries() -> FsResult<()> {
        let entries = table_entries(&[("a.yml", "sources:\n  - name: raw\n")]).await?;
        assert!(entries.is_empty());
        Ok(())
    }

    #[dbt_runtime::test]
    async fn tables_written_as_one_expression_are_rendered() -> FsResult<()> {
        let entries = table_entries(&[(
            "a.yml",
            "sources:\n  - name: raw\n    tables: \"{{ [{'name': 'orders'}, {'name': 'customers'}] }}\"\n",
        )])
        .await?;
        assert_eq!(
            entries.keys().cloned().collect::<Vec<_>>(),
            [key("raw", "customers"), key("raw", "orders")]
        );
        let orders = &entries[&key("raw", "orders")];
        assert!(
            orders
                .schema_value
                .get("tables")
                .is_some_and(Value::is_null)
        );
        let table = orders.table_value.as_ref().expect("a table");
        assert_eq!(string(table, "name"), Some("orders"));
        Ok(())
    }
}
