//! Canonical definition of the index view surface and its DDL generator.
//!
//! The single owner of *which views exist* over the index parquet: one base
//! view per table, plus the analytical convenience views. Consumers generate
//! their statements from here rather than carrying their own lists (fs#13788):
//! live registration in [`crate::backend`], the `views.sql` file, and the
//! `dbt-index` ingest bootstrap all render from these definitions.
//!
//! The parse-safe views a check may read are not part of this surface: they
//! carry the information schema's vocabulary and are declared next to it, in
//! [`crate::info_schema`]'s `parse_safe` module, looked up from `INFO_SCHEMA`.
//! The legacy graph-family views (`dbt.graph_nodes`, `dbt.models`, ...,
//! `dbt.columns`) remain part of this surface for live connections and the
//! ingest bootstrap, but are absent from `views.sql`, mirroring the split
//! made when checks moved to the information schema's names.

use std::fmt::Write;
use std::path::Path;
use std::sync::LazyLock;

use crate::db::{DBT_RT_TABLES, DBT_TABLES};

/// A parquet-backed base table.
pub struct BaseTable {
    /// SQL schema the view is created in: `dbt` or `dbt_rt`.
    pub schema: &'static str,
    pub name: &'static str,
}

/// A view defined as SQL over other views/tables rather than over a file.
pub struct DerivedView {
    pub schema: &'static str,
    pub name: &'static str,
}

static BASE_TABLES: LazyLock<Vec<BaseTable>> = LazyLock::new(|| {
    let dbt = DBT_TABLES.iter().map(|name| BaseTable {
        schema: "dbt",
        name,
    });
    let dbt_rt = DBT_RT_TABLES.iter().map(|name| BaseTable {
        schema: "dbt_rt",
        name,
    });
    dbt.chain(dbt_rt).collect()
});

/// Every base table, in `views.sql` order (`dbt` then `dbt_rt`).
pub fn base_tables() -> impl Iterator<Item = &'static BaseTable> {
    BASE_TABLES.iter()
}

/// Metadata for every derived view, in creation order (views that build on
/// other derived views come after them). The order and count must match the
/// statements in `ANALYTICAL_VIEWS` — pinned by
/// `derived_statements_align_with_metadata`.
pub const DERIVED_VIEWS: &[DerivedView] = &[
    DerivedView {
        schema: "dbt_rt",
        name: "run_results_latest",
    },
    DerivedView {
        schema: "dbt_rt",
        name: "dag_validity",
    },
    DerivedView {
        schema: "dbt_rt",
        name: "node_status",
    },
    DerivedView {
        schema: "dbt",
        name: "nodes_enriched",
    },
    DerivedView {
        schema: "dbt",
        name: "tests_enriched",
    },
    DerivedView {
        schema: "dbt",
        name: "graph_nodes",
    },
    DerivedView {
        schema: "dbt",
        name: "models",
    },
    DerivedView {
        schema: "dbt",
        name: "seeds",
    },
    DerivedView {
        schema: "dbt",
        name: "tests",
    },
    DerivedView {
        schema: "dbt",
        name: "snapshots",
    },
    DerivedView {
        schema: "dbt",
        name: "sources",
    },
    DerivedView {
        schema: "dbt",
        name: "analyses",
    },
    DerivedView {
        schema: "dbt",
        name: "operations",
    },
    DerivedView {
        schema: "dbt",
        name: "functions",
    },
    DerivedView {
        schema: "dbt",
        name: "columns",
    },
];

/// The legacy graph-family views: enumerated parse-final columns of
/// `dbt.nodes`, the per-resource-type filters over that projection, and the
/// parse-populated slice of `dbt.node_columns`. Served on live connections
/// and by the ingest bootstrap; not written into `views.sql`.
const GRAPH_FAMILY_VIEWS_DDL: &str = "
CREATE OR REPLACE VIEW dbt.graph_nodes AS
SELECT
    unique_id, name, resource_type, package_name, file_path, original_file_path,
    fqn, alias, checksum, description, raw_code, database_name, schema_name,
    relation_name, identifier, enabled, materialized, config, access_level,
    group_name, contract_enforced, version, latest_version, deprecation_date,
    primary_key, patch_path, tags, meta, source_name, source_description,
    loader, loaded_at_field, ingested_at
FROM dbt.nodes;

CREATE OR REPLACE VIEW dbt.models AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'model';
CREATE OR REPLACE VIEW dbt.seeds AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'seed';
CREATE OR REPLACE VIEW dbt.tests AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'test';
CREATE OR REPLACE VIEW dbt.snapshots AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'snapshot';
CREATE OR REPLACE VIEW dbt.sources AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'source';
CREATE OR REPLACE VIEW dbt.analyses AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'analysis';
CREATE OR REPLACE VIEW dbt.operations AS SELECT * FROM dbt.graph_nodes WHERE resource_type IN ('operation', 'sql_operation');
CREATE OR REPLACE VIEW dbt.functions AS SELECT * FROM dbt.graph_nodes WHERE resource_type = 'function';

CREATE OR REPLACE VIEW dbt.columns AS
SELECT unique_id, column_name, declared_type, description, tags, ingested_at
FROM dbt.node_columns;
";

/// Splits a `;`-separated DDL script into individual statements, stripping
/// `--` line comments (respecting `'...'` string literals so a comment marker
/// inside a string is not mistaken for one). Registration and the ingest
/// bootstrap run one statement per call rather than a whole script.
fn split_ddl_statements(ddl: &str) -> Vec<String> {
    let stripped: String = ddl
        .lines()
        .map(|line| {
            let mut in_string = false;
            let mut chars = line.char_indices().peekable();
            while let Some((i, ch)) = chars.next() {
                if ch == '\'' {
                    // `''` is SQL's escaped quote within a string literal, not
                    // two delimiters: consume the pair without toggling, so an
                    // escaped quote doesn't prematurely close the string and
                    // expose a later `--` as a real comment marker.
                    if chars.peek() == Some(&(i + 1, '\'')) {
                        chars.next();
                        continue;
                    }
                    in_string = !in_string;
                } else if ch == '-' && !in_string && chars.peek().map(|&(_, c)| c) == Some('-') {
                    return &line[..i];
                }
            }
            line
        })
        .collect::<Vec<&str>>()
        .join("\n");

    stripped
        .split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

static DERIVED_STATEMENTS: LazyLock<Vec<(&'static DerivedView, String)>> = LazyLock::new(|| {
    let statements = split_ddl_statements(crate::db::ANALYTICAL_VIEWS)
        .into_iter()
        .chain(split_ddl_statements(GRAPH_FAMILY_VIEWS_DDL));
    DERIVED_VIEWS.iter().zip(statements).collect()
});

/// Each derived view paired with its `CREATE OR REPLACE VIEW` statement.
///
/// The SQL bodies live in the `db.rs` `ANALYTICAL_VIEWS` constant; this zips
/// them with [`DERIVED_VIEWS`] so callers can order and report without a
/// second copy of either side. The count and order invariant between the two
/// is pinned by `derived_statements_align_with_metadata`, in CI rather than
/// at runtime.
pub fn derived_statements() -> &'static [(&'static DerivedView, String)] {
    &DERIVED_STATEMENTS
}

pub(crate) fn quote_sql_string(path: &str) -> String {
    path.replace('\'', "''")
}

/// The `CREATE OR REPLACE VIEW` statement for one base table.
///
/// With an `index_dir` the `read_parquet` path is absolute, which live
/// connections need (their working directory is wherever the user invoked dbt
/// from). Without one the path is the bare file name, which is only correct
/// for `views.sql`, executed from inside the index directory.
pub fn base_view_statement(table: &BaseTable, index_dir: Option<&Path>) -> String {
    let file = format!("{}.{}.parquet", table.schema, table.name);
    let path = match index_dir {
        Some(dir) => quote_sql_string(&dir.join(&file).to_string_lossy()),
        None => file,
    };
    format!(
        "CREATE OR REPLACE VIEW {}.{} AS SELECT * FROM read_parquet('{}')",
        table.schema, table.name, path
    )
}

static VIEWS_SQL_DOCUMENT: LazyLock<String> = LazyLock::new(|| {
    let mut sql = String::from(
        "-- Auto-generated by dbt-index. Query with:\n\
         --   duckdb -cmd \".read views.sql\"\n\n\
         CREATE SCHEMA IF NOT EXISTS dbt;\n\
         CREATE SCHEMA IF NOT EXISTS dbt_rt;\n\n",
    );
    for table in base_tables().filter(|t| t.schema == "dbt") {
        writeln!(sql, "{};", base_view_statement(table, None)).unwrap();
    }
    writeln!(sql).unwrap();
    for table in base_tables().filter(|t| t.schema == "dbt_rt") {
        writeln!(sql, "{};", base_view_statement(table, None)).unwrap();
    }
    // The parse-safe views are deliberately not here: they carry the
    // information schema's names (`dbt.macros`, `dbt.edges`, ...), which in
    // this file are already taken by the source tables above. They are
    // registered where they are read, over `dbt_internal` — see
    // `info_schema::parse_safe`.
    sql.push_str(crate::db::ANALYTICAL_VIEWS);
    sql
});

/// The full `views.sql` document: header comment, relative-path base views
/// (`dbt` then `dbt_rt`), then the analytical-view blob verbatim.
pub fn views_sql_document() -> &'static str {
    &VIEWS_SQL_DOCUMENT
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The blob statements and [`DERIVED_VIEWS`] must describe the same views
    /// in the same order, or reporting would attach the wrong name to a
    /// statement.
    #[test]
    fn derived_statements_align_with_metadata() {
        // zip() truncates silently, so the count must be pinned explicitly —
        // here, not at runtime (a mismatch is a source bug, not a user error).
        assert_eq!(derived_statements().len(), DERIVED_VIEWS.len());
        for (view, statement) in derived_statements() {
            let marker = format!("VIEW {}.{} AS", view.schema, view.name);
            assert!(
                statement.contains(&marker),
                "statement for {}.{} does not create it: {}",
                view.schema,
                view.name,
                statement.lines().next().unwrap_or_default()
            );
        }
    }

    /// Absolute style renders the directory into every base view and escapes
    /// single quotes, so a path cannot break out of the SQL string literal.
    #[test]
    fn absolute_paths_are_rendered_and_escaped() {
        let dir = Path::new("/tmp/it's here");
        let table = BaseTable {
            schema: "dbt",
            name: "nodes",
        };
        let sql = base_view_statement(&table, Some(dir));
        assert!(sql.contains("it''s here"));
        assert!(sql.contains("dbt.nodes.parquet"));
    }
}

#[cfg(test)]
mod split_ddl_statements_tests {
    use super::split_ddl_statements;

    #[test]
    fn strips_trailing_comment() {
        let ddl = "SELECT 1; -- a comment\nSELECT 2;";
        assert_eq!(split_ddl_statements(ddl), vec!["SELECT 1", "SELECT 2"]);
    }

    #[test]
    fn preserves_comment_marker_inside_string_literal() {
        let ddl = "SELECT '--not a comment' AS x;";
        assert_eq!(
            split_ddl_statements(ddl),
            vec!["SELECT '--not a comment' AS x"]
        );
    }

    #[test]
    fn escaped_quote_does_not_prematurely_close_string() {
        // A doubled `''` inside the literal is SQL's escaped quote, not the
        // string's end — a `--` later in the same string must not be treated
        // as a real comment marker.
        let ddl = "SELECT 'it''s -- odd' AS x;";
        assert_eq!(
            split_ddl_statements(ddl),
            vec!["SELECT 'it''s -- odd' AS x"]
        );
    }
}
