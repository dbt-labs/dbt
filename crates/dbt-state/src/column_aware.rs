//! Pure helpers for column-aware reuse (`compare_selected_columns`).

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use arrow_schema::Schema;
use dbt_adapter::errors::AdapterResult;
use dbt_adapter::sql_types::TypeOps;

use crate::metadata_cache::RunCacheMetadataCache;
use crate::proto::query_cache::table_modified_info::DependencyKind;
use crate::proto::query_cache::{ColumnInfo, TableModifiedInfo, TableSchema};

pub type TableColumns = Arc<[ColumnInfo]>;

/// What to cache for one table from a `list_relations_schemas` result, in
/// the shape of `RunCacheMetadataCache::table_schema`:
/// - `None`: the table's fetch errored. Cache nothing, so a later request
///   retries; the cache can outlive the invocation.
/// - `Some(None)`: cache as absent. The table is missing from the result, or
///   has no columns or a column type that can't be rendered.
/// - `Some(Some(columns))`: the rendered columns.
pub fn columns_from_fetch(
    type_ops: &dyn TypeOps,
    result: Option<&AdapterResult<Arc<Schema>>>,
) -> Option<Option<TableColumns>> {
    match result {
        None => Some(None),
        Some(Err(_)) => None,
        Some(Ok(schema)) => Some(render_columns(type_ops, schema).map(Arc::<[ColumnInfo]>::from)),
    }
}

/// Name-sorted cached schemas for the requested tables; absent and uncached
/// are dropped.
pub fn cached_table_schemas(
    cache: &RunCacheMetadataCache,
    fqns: &BTreeSet<String>,
) -> Vec<TableSchema> {
    fqns.iter()
        .filter_map(|fqn| {
            let columns = cache.table_schema(fqn)??;
            Some(TableSchema {
                name: fqn.clone(),
                columns: columns.to_vec(),
            })
        })
        .collect()
}

/// Renders columns in ordinal order. `None` if empty or any field fails to render.
pub fn render_columns(type_ops: &dyn TypeOps, schema: &Schema) -> Option<Vec<ColumnInfo>> {
    if schema.fields().is_empty() {
        return None;
    }
    schema
        .fields()
        .iter()
        .map(|field| {
            type_ops
                .get_original_sql_type_from_field(field)
                .ok()
                .map(|ty| ColumnInfo {
                    name: field.name().clone(),
                    r#type: ty.into_owned(),
                })
        })
        .collect()
}

/// `seen_tables - (selected_targets - {own_target})`.
pub fn edge_tables(
    seen_tables: &BTreeSet<String>,
    selected_targets: &BTreeSet<String>,
    own_target: &str,
) -> BTreeSet<String> {
    seen_tables
        .iter()
        .filter(|t| t.as_str() == own_target || !selected_targets.contains(*t))
        .cloned()
        .collect()
}

/// A name seen with two different kinds maps to `Unknown`.
pub fn dependency_kinds(
    entries: impl IntoIterator<Item = (String, DependencyKind)>,
) -> HashMap<String, DependencyKind> {
    let mut out: HashMap<String, DependencyKind> = HashMap::new();
    for (name, kind) in entries {
        out.entry(name)
            .and_modify(|existing| {
                if *existing != kind {
                    *existing = DependencyKind::Unknown;
                }
            })
            .or_insert(kind);
    }
    out
}

pub fn apply_dependency_kinds(
    tables: &mut [TableModifiedInfo],
    kinds: &HashMap<String, DependencyKind>,
) {
    for table in tables {
        table.kind = kinds
            .get(&table.name)
            .copied()
            .unwrap_or(DependencyKind::Unknown) as i32;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arrow_schema::{DataType, Field, UnionFields, UnionMode};
    use dbt_adapter::AdapterType;
    use dbt_adapter::errors::{AdapterError, AdapterErrorKind};
    use dbt_adapter::sql_types::DefaultTypeOps;

    fn ops() -> DefaultTypeOps {
        DefaultTypeOps::new(AdapterType::Snowflake)
    }

    fn meta_field(name: &str, ty: &str) -> Field {
        Field::new(name, DataType::Decimal128(38, 0), true).with_metadata(HashMap::from([(
            "SNOWFLAKE:type".to_string(),
            ty.to_string(),
        )]))
    }

    fn cols(names: &[&str]) -> Option<TableColumns> {
        Some(
            names
                .iter()
                .map(|n| ColumnInfo {
                    name: n.to_string(),
                    r#type: "T".into(),
                })
                .collect::<Vec<_>>()
                .into(),
        )
    }

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn render_columns_prefers_original_type_metadata() {
        let schema = Schema::new(vec![
            meta_field("ID", "NUMBER(38,0)"),
            meta_field("AMOUNT", "NUMBER(10,2)"),
        ]);
        let got = render_columns(&ops(), &schema).unwrap();
        let got: Vec<_> = got
            .iter()
            .map(|c| (c.name.as_str(), c.r#type.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![("ID", "NUMBER(38,0)"), ("AMOUNT", "NUMBER(10,2)")]
        );
    }

    #[test]
    fn render_columns_falls_back_to_dialect_type() {
        let field = Field::new("X", DataType::Int64, true);
        let mut expected = String::new();
        ops()
            .format_arrow_type_as_sql(field.data_type(), field.is_nullable(), &mut expected)
            .unwrap();
        let got = render_columns(&ops(), &Schema::new(vec![field])).unwrap();
        assert!(!got[0].r#type.is_empty());
        assert_eq!(got[0].r#type, expected);
    }

    /// One unrenderable column makes the whole table absent, never a partial
    /// schema: a partial one would hide a column the SQL may read. Snowflake's
    /// `DefaultTypeOps` renders every Arrow type, so this uses Postgres's,
    /// which rejects a union.
    #[test]
    fn columns_from_fetch_caches_absent_but_not_errors() {
        let ops = DefaultTypeOps::new(AdapterType::Postgres);
        let renderable = Field::new("ID", DataType::Int64, true);
        let unrenderable = Field::new(
            "U",
            DataType::Union(
                UnionFields::try_new(vec![0], vec![Field::new("a", DataType::Int32, true)])
                    .unwrap(),
                UnionMode::Sparse,
            ),
            true,
        );
        assert!(ops.get_original_sql_type_from_field(&renderable).is_ok());
        assert!(ops.get_original_sql_type_from_field(&unrenderable).is_err());

        let ok = |fields: Vec<Field>| -> AdapterResult<Arc<Schema>> {
            Ok(Arc::new(Schema::new(fields)))
        };
        let got = columns_from_fetch(&ops, Some(&ok(vec![renderable.clone()])));
        assert_eq!(got.flatten().map(|c| c.len()), Some(1));
        let errored: AdapterResult<Arc<Schema>> =
            Err(AdapterError::new(AdapterErrorKind::Internal, "boom"));
        assert_eq!(
            columns_from_fetch(&ops, Some(&errored)),
            None,
            "an errored fetch is not cached"
        );
        assert_eq!(columns_from_fetch(&ops, None), Some(None), "missing");
        assert_eq!(
            columns_from_fetch(&ops, Some(&ok(vec![]))),
            Some(None),
            "empty"
        );
        assert_eq!(
            columns_from_fetch(&ops, Some(&ok(vec![renderable, unrenderable]))),
            Some(None),
            "unrenderable"
        );
    }

    #[test]
    fn cached_table_schemas_sorted_and_drops_absent() {
        let cache = RunCacheMetadataCache::new();
        cache.insert_table_schema("b", cols(&["x"]));
        cache.insert_table_schema("a", cols(&["y"]));
        cache.insert_table_schema("c", None);
        let got = cached_table_schemas(&cache, &set(&["a", "b", "c", "d"]));
        let names: Vec<_> = got.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn edge_tables_keeps_own_target() {
        let got = edge_tables(
            &set(&["src", "up", "me", "other_sel"]),
            &set(&["me", "other_sel"]),
            "me",
        );
        assert_eq!(got, set(&["src", "up", "me"]));
    }

    #[test]
    fn dependency_kinds_conflict_is_unknown() {
        let got = dependency_kinds([
            ("x".to_string(), DependencyKind::Source),
            ("x".to_string(), DependencyKind::Model),
            ("y".to_string(), DependencyKind::Seed),
            ("y".to_string(), DependencyKind::Seed),
        ]);
        assert_eq!(got["x"], DependencyKind::Unknown);
        assert_eq!(got["y"], DependencyKind::Seed);
    }

    #[test]
    fn apply_dependency_kinds_defaults_unknown() {
        let mut tables = vec![
            TableModifiedInfo {
                name: "x".into(),
                last_modified_epoch: Some(1),
                kind: 0,
            },
            TableModifiedInfo {
                name: "z".into(),
                last_modified_epoch: Some(1),
                kind: 3,
            },
        ];
        let kinds = HashMap::from([("x".to_string(), DependencyKind::Seed)]);
        apply_dependency_kinds(&mut tables, &kinds);
        assert_eq!(tables[0].kind, DependencyKind::Seed as i32);
        assert_eq!(tables[1].kind, DependencyKind::Unknown as i32);
    }
}
