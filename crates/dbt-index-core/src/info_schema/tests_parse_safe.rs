//! Tests for the parse-safe views. Like the schema tests, these need no fixtures: the
//! views are a pure function of [`VIEWS`] and [`INFO_SCHEMA`].

use std::collections::HashSet;

use super::parse_safe::{BASE_SCHEMA, VIEWS, base_tables};
use super::schema::spec_for;
use super::spec::{Ns, Src};
use crate::parquet::schema_for;

/// Source columns that exist in the index but hold nothing usable at parse, by table.
///
/// On `nodes`, compile fills the first group and nothing fills the second (the real values
/// are inside the `config` JSON blob); `classifiers` is the third case and the nastiest —
/// written in part at parse and completed later, so a check reading it sees a subset and
/// passes. On the rest, the column is in the row schema and the parse-time writer simply
/// never sets it, which reads the same way: NULL for every row.
///
/// Named by *source* column, since that is what a view ultimately selects.
const DEAD_AT_PARSE: &[(&str, &[&str])] = &[
    (
        "nodes",
        &[
            "compiled_code",
            "compiled_code_hash",
            "compiled_path",
            "compiled_at",
            "extra_ctes",
            "search_text",
            "raw_code_hash",
            // `raw_code` is deliberately not listed: it is dead for *model* rows only, which a
            // table-keyed list cannot express. See `raw_code_is_exposed_only_where_a_node_has_sql`.
            "grain",
            "grain_declared",
            "grain_tested",
            "grain_inferred",
            "table_role",
            "classifiers",
            "incremental_strategy",
            "on_schema_change",
            "unique_key",
            "full_refresh",
            "persist_docs",
            "pre_hook",
            "post_hook",
            "grants",
            "node_constraints",
            "docs_show",
            "time_spine",
            "ai_context",
            "loaded_at_query",
            "freshness",
            "external_config",
            "source_meta",
            "quoting",
        ],
    ),
    // Type inference and `dbt docs generate` fill the type columns; the rest arrive with
    // the compile and catalog merges.
    (
        "node_columns",
        &[
            "inferred_type",
            "catalog_type",
            "data_type",
            "catalog_comment",
            "classifiers",
            "column_index",
            "label",
            "expression",
            "quote",
            "granularity",
            "meta",
            "column_constraints",
            "tests",
        ],
    ),
    ("test_metadata", &["test_where", "test_limit"]),
    (
        "project",
        &["project_id", "description", "quoting", "ai_context"],
    ),
    (
        "packages",
        &[
            "package_source",
            "version",
            "git_url",
            "git_revision",
            "local_path",
        ],
    ),
    (
        "groups",
        &["package_name", "file_path", "original_file_path", "config"],
    ),
    ("exposures", &["meta", "config"]),
    ("metrics", &["semantic_model_name", "ai_context"]),
    ("saved_queries", &["config"]),
    ("unit_tests", &["config", "created_at"]),
    (
        "semantic_models",
        &["package_name", "file_path", "original_file_path", "config"],
    ),
    ("semantic_entities", &["config"]),
    ("semantic_measures", &["config"]),
    ("semantic_dimensions", &["config"]),
];

/// Resolve a view's columns to the source column each one reads.
fn source_columns(view: &super::parse_safe::ParseSafeView) -> Vec<(&'static str, &'static str)> {
    let spec = spec_for(Ns::Dbt, view.vocabulary)
        .unwrap_or_else(|| panic!("{}: unknown vocabulary '{}'", view.name, view.vocabulary));
    view.cols
        .iter()
        .map(|out| {
            let col = spec
                .cols
                .iter()
                .find(|c| c.out == *out)
                .unwrap_or_else(|| panic!("{}: '{out}' is not in '{}'", view.name, spec.name));
            (*out, if col.src.is_empty() { col.out } else { col.src })
        })
        .collect()
}

/// The whole point of the file: a check and the information schema must use one
/// vocabulary. Every column a view exposes is a column of the information-schema table it
/// claims to mirror, spelled the same way.
#[test]
fn every_column_is_an_information_schema_column() {
    for view in VIEWS {
        // Panics with the offending name if it is not.
        source_columns(view);
    }
}

/// And the other end: the source column each output name resolves to has to be in the
/// index, or the view is a `CREATE VIEW` that fails at registration time.
#[test]
fn every_source_column_exists_in_the_index() {
    for view in VIEWS {
        for (out, src) in source_columns(view) {
            let found = view
                .base_tables()
                .iter()
                .any(|t| schema_for(t).field_with_name(src).is_ok());
            assert!(
                found,
                "{}.{out} reads '{src}', which is in none of {:?}",
                view.name,
                view.base_tables(),
            );
        }
    }
}

/// Zero rows is a pass, so exposing a column that is empty at parse turns a check into a
/// silent no-op. The exclusions are stated as a list rather than left implicit in the
/// column enumeration: dropping one back in should have to be deliberate.
#[test]
fn no_view_exposes_a_column_that_is_empty_at_parse() {
    for view in VIEWS {
        for (out, src) in source_columns(view) {
            for (table, dead) in DEAD_AT_PARSE {
                if view.base_tables().contains(table) {
                    assert!(
                        !dead.contains(&src),
                        "{}.{out} exposes {table}.{src}, which holds nothing at parse",
                        view.name,
                    );
                }
            }
        }
    }
}

/// A dead column has to be dead *in the index*, not merely absent from a view: if the
/// ingest starts filling one, the exclusion above is stale rather than protective.
#[test]
fn the_excluded_columns_still_exist_in_the_index() {
    for (table, dead) in DEAD_AT_PARSE {
        let schema = schema_for(table);
        for col in *dead {
            assert!(
                schema.field_with_name(col).is_ok(),
                "{table}.{col} is gone; drop it from the exclusion list",
            );
        }
    }
}

#[test]
fn view_names_are_unique() {
    let mut seen = HashSet::new();
    for view in VIEWS {
        assert!(seen.insert(view.name), "duplicate view '{}'", view.name);
    }
}

/// Only the tables the ingest actually writes can be registered.
#[test]
fn base_tables_are_index_tables() {
    for table in base_tables() {
        assert!(
            crate::db::DBT_TABLES.contains(&table),
            "'{table}' is not a table of the index",
        );
    }
}

/// Reading the generated SQL is the fastest way to see what a check sees, so one view is
/// pinned in full. `models` covers the interesting parts: the rename to the information
/// schema's `access`, a quoted keyword (`group`), and the resource-type filter.
#[test]
fn projected_view_sql() {
    let models = VIEWS.iter().find(|v| v.name == "models").unwrap();
    assert_eq!(
        models.create_view_sql().unwrap(),
        concat!(
            r#"CREATE OR REPLACE VIEW dbt."models" AS SELECT "#,
            r#"t."unique_id" AS "unique_id", t."name" AS "name", "#,
            r#"t."resource_type" AS "resource_type", t."package_name" AS "package_name", "#,
            r#"t."original_file_path" AS "original_file_path", t."fqn" AS "fqn", "#,
            r#"t."alias" AS "alias", t."description" AS "description", "#,
            r#"t."node_language" AS "node_language", "#,
            r#"t."database_name" AS "database_name", t."schema_name" AS "schema_name", "#,
            r#"t."relation_name" AS "relation_name", t."identifier" AS "identifier", "#,
            r#"t."enabled" AS "enabled", t."materialized" AS "materialized", "#,
            r#"t."config" AS "config", t."access_level" AS "access", "#,
            r#"t."group_name" AS "group", t."contract_enforced" AS "contract_enforced", "#,
            r#"t."version" AS "version", t."latest_version" AS "latest_version", "#,
            r#"t."deprecation_date" AS "deprecation_date", t."primary_key" AS "primary_key", "#,
            r#"t."patch_path" AS "properties_yml_file_path", t."tags" AS "tags", "#,
            r#"t."meta" AS "meta", t."ingested_at" AS "ingested_at" "#,
            r#"FROM dbt_internal."nodes" AS t WHERE t."resource_type" IN ('model')"#,
        ),
    );
}

/// The joined view resolves each column against the side that has it, left first — the
/// rule the information schema's own projection uses.
#[test]
fn joined_view_sql_resolves_each_side() {
    let tests = VIEWS.iter().find(|v| v.name == "data_tests").unwrap();
    let sql = tests.create_view_sql().unwrap();
    assert!(
        sql.contains(r#"FROM dbt_internal."nodes" AS l LEFT JOIN dbt_internal."test_metadata" AS r ON l."unique_id" = r."unique_id""#),
        "{sql}",
    );
    // `unique_id` and `ingested_at` are on both sides; the node row wins.
    assert!(sql.contains(r#"l."unique_id" AS "unique_id""#), "{sql}");
    assert!(sql.contains(r#"l."ingested_at" AS "ingested_at""#), "{sql}");
    // Test detail only exists on the right.
    assert!(
        sql.contains(r#"r."attached_node" AS "node_unique_id""#),
        "{sql}"
    );
    assert!(sql.contains(r#"r."kwargs" AS "arguments""#), "{sql}");
}

/// Every view reads [`BASE_SCHEMA`], never `dbt` — a view over `dbt.nodes` would both
/// collide with the index's own table of that name and put the raw columns back within a
/// check's reach.
#[test]
fn views_read_only_the_base_schema() {
    for view in VIEWS {
        let sql = view.create_view_sql().unwrap();
        let (_, from) = sql
            .split_once(" FROM ")
            .expect("a view selects from something");
        for table in view.base_tables() {
            assert!(
                from.contains(&format!("{BASE_SCHEMA}.\"{table}\"")),
                "{}: expected {BASE_SCHEMA}.{table} in: {from}",
                view.name,
            );
        }
    }
}

/// The invariant the whole surface rests on: a check author and someone querying
/// `target/info_schema/` use one vocabulary. Every view name is a table in the
/// information schema — no checks-only views, no exceptions. A view for something the
/// information schema does not publish has to be published there first.
#[test]
fn every_view_mirrors_an_information_schema_table() {
    for view in VIEWS {
        assert_eq!(
            view.name, view.vocabulary,
            "{} borrows another table's columns; give it its own",
            view.name,
        );
        assert!(
            spec_for(Ns::Dbt, view.name).is_some(),
            "the information schema has no '{}' table",
            view.name,
        );
    }
}

/// `raw_code` is per resource type, not per table, so it cannot be settled by
/// [`DEAD_AT_PARSE`] or by `node_cols!`.
///
/// `trim_model_payload` drops it from model payloads only, so it is NULL for `models` and
/// real SQL for every other node type that has any. Seeds and sources have no SQL at all
/// and arrive as an empty string, which reads to a check exactly like the NULL does -- a
/// `like` never matches, so the rule passes -- so they do not carry it either.
///
/// Measured per type against the index: model NULL; snapshot, check, analysis, operation
/// and function populated; seed and source empty.
#[test]
fn raw_code_is_exposed_only_where_a_node_has_sql() {
    let exposes = |name: &str| {
        VIEWS
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("no view '{name}'"))
            .cols
            .contains(&"raw_code")
    };

    for view in ["snapshots", "checks", "analyses", "hooks", "functions"] {
        assert!(
            exposes(view),
            "{view} nodes carry their SQL, so a rule about that SQL must be writable",
        );
    }
    for view in ["models", "seeds", "sources"] {
        assert!(
            !exposes(view),
            "{view}.raw_code is NULL or empty for every row, which only reports false passes",
        );
    }
}

/// `dag_nodes` is the one view assembled rather than projected, and the two name-keyed
/// places that know it (`create_view_sql`, `base_tables`) are only correct while that
/// holds. A second assembled view would silently get `dag_nodes`' SQL.
#[test]
fn dag_nodes_is_the_only_assembled_view() {
    let assembled: Vec<_> = VIEWS
        .iter()
        .filter(|v| matches!(v.src, Src::Own))
        .map(|v| v.name)
        .collect();
    assert_eq!(assembled, vec!["dag_nodes"]);
}

/// The union, pinned: node rows filtered to DAG types, then one arm per side table with
/// its resource type as a literal. `enabled` is read from each table, with NULL meaning
/// enabled — the config default, and what `info_schema::build_dag_nodes` does.
#[test]
fn assembled_view_sql() {
    let dag = VIEWS.iter().find(|v| v.name == "dag_nodes").unwrap();
    let sql = dag.create_view_sql().unwrap();
    for expected in [
        r#"CREATE OR REPLACE VIEW dbt."dag_nodes" AS"#,
        r#"FROM dbt_internal."nodes" AS t WHERE COALESCE(t."enabled", TRUE) AND t."resource_type" IN ("#,
        r#"UNION ALL SELECT t."unique_id" AS "unique_id", 'exposure' AS "resource_type""#,
        r#"FROM dbt_internal."exposures" AS t WHERE COALESCE(t."enabled", TRUE)"#,
        r#"'metric' AS "resource_type""#,
        r#"'unit_test' AS "resource_type""#,
    ] {
        assert!(sql.contains(expected), "missing {expected}\nin: {sql}");
    }
    // Every DAG resource type reaches the filter, or rows go missing silently.
    for ty in super::schema::DAG_RESOURCE_TYPES {
        assert!(sql.contains(&format!("'{ty}'")), "{ty} not in: {sql}");
    }
}
