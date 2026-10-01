//! `views.sql` generation.
//!
//! Driven off `INFO_SCHEMA`, so a table cannot be added without its view.
//!
//! Statement generation is pure and lives here; `views.sql` and a live
//! connection are two renderings of it. That is the same split
//! [`crate::view_defs`] makes for the index, and for the same reason: the two
//! differ only in whether the `read_parquet` path is absolute, and everything
//! else must not be able to drift.

use std::fmt::Write;
use std::path::Path;

use crate::IndexError;
use crate::view_defs::quote_sql_string;

use super::schema::INFO_SCHEMA;
use super::spec::{Filter, Ns, Src, TableSpec};

/// The `CREATE OR REPLACE VIEW` statement for one information-schema table.
///
/// With a `dir` the `read_parquet` path is absolute, which live connections
/// need — their working directory is wherever the user invoked dbt from.
/// Without one the path is the bare file name, correct only for `views.sql`,
/// which is executed from inside the directory it sits in.
pub fn base_view_statement(table: &TableSpec, dir: Option<&Path>) -> String {
    let file = table.file_name();
    let path = match dir {
        Some(dir) => quote_sql_string(&dir.join(&file).to_string_lossy()),
        None => file,
    };
    format!(
        "CREATE OR REPLACE VIEW {} AS SELECT * FROM read_parquet('{}')",
        table.qualified_name(),
        path
    )
}

/// The latest result per node.
///
/// Compile-only error rows (`status = 'error'` with no execution time) are
/// excluded — they record a failed compile, not a node execution.
const RUN_RESULTS_LATEST: &str = "CREATE OR REPLACE VIEW dbt_rt.run_results_latest AS
SELECT * FROM dbt_rt.run_results
WHERE NOT (status = 'error' AND execution_time = 0)
QUALIFY ROW_NUMBER() OVER (PARTITION BY unique_id ORDER BY created_at DESC) = 1";

/// Whether a table's rows come from the node set.
///
/// The per-resource-type tables project `nodes` directly; `data_tests` joins it
/// to `test_metadata`. Matching on the source rather than on a list of names is
/// what makes [`resources_statement`] pick up a new resource-type table on its
/// own.
fn reads_the_node_set(table: &TableSpec) -> bool {
    match table.src {
        Src::Table(name) => name == "nodes",
        Src::Join { left, .. } => left == "nodes",
        Src::Own => false,
    }
}

/// `dbt_internal.resources`: every resource in the project, one row each.
///
/// The information schema splits the node set into one table per resource type,
/// which is the right shape for anything that knows what it is looking for and
/// the wrong shape for the surfaces that do not: counting resources by type,
/// listing every file, resolving a name for an arbitrary node in the graph,
/// searching across types. `dbt.dag_nodes` does not answer those either — it
/// carries three columns, keeps only enabled rows, and only for the types that
/// participate in the DAG.
///
/// Deliberately in `dbt_internal`: the view is fully derivable from the public
/// tables, so publishing it would be a promise to keep for nothing gained.
///
/// `UNION ALL BY NAME` rather than `UNION ALL`, because the branches do not
/// share a column set — `sources` and `data_tests` each carry their own extras,
/// which arrive NULL on the branches that lack them instead of failing to bind.
///
/// **Reading this view reads every underlying parquet.** Query the typed table
/// when the resource type is known; that matters most for the docs site, which
/// fetches each artifact over HTTP.
fn resources_statement() -> String {
    let branches: Vec<String> = INFO_SCHEMA
        .iter()
        .filter(|t| t.ns == Ns::Dbt && reads_the_node_set(t))
        .map(|t| match resource_type_literal(t) {
            Some(rt) => format!(
                "SELECT *, '{rt}' AS resource_type FROM {}",
                t.qualified_name()
            ),
            None => format!("SELECT * FROM {}", t.qualified_name()),
        })
        .collect();

    format!(
        "CREATE OR REPLACE VIEW dbt_internal.resources AS\n{}",
        branches.join("\nUNION ALL BY NAME ")
    )
}

/// The `resource_type` a branch has to supply as a literal, if any.
///
/// Most node-backed tables carry `resource_type` as a column, because
/// `node_cols!` does. `dbt.data_tests` does not — the table *is* the type, so
/// repeating it there would be noise. In the union it stops being noise: every
/// cross-type consumer groups or filters on `resource_type`, and a branch that
/// left it NULL would be a resource type that quietly counts as none. The value
/// comes from the spec's own row filter, so it cannot disagree with the rows.
///
/// `None` means the table has the column; a table with neither is a spec that
/// cannot be unioned, which `resources_view_supplies_a_resource_type` catches.
fn resource_type_literal(table: &TableSpec) -> Option<&'static str> {
    if table.cols.iter().any(|c| c.out == "resource_type") {
        return None;
    }
    match table.filter {
        Filter::ResourceTypeIn([only]) => Some(only),
        _ => None,
    }
}

/// The derived views, in creation order: a view's prerequisites come first, so
/// a caller executing the list top to bottom never binds against a view that
/// does not exist yet.
///
/// Each entry is `(qualified name, statement)`; the name is what a registration
/// pass reports as registered or skipped.
pub fn derived_statements() -> Vec<(String, String)> {
    vec![
        (
            "dbt_rt.run_results_latest".to_string(),
            RUN_RESULTS_LATEST.to_string(),
        ),
        ("dbt_internal.resources".to_string(), resources_statement()),
    ]
}

/// Write `views.sql` next to the parquet files.
pub fn write_views_sql(dir: &Path) -> Result<(), IndexError> {
    let mut sql = String::from(
        "-- dbt information schema. Generated; do not edit.\n\
         --\n\
         -- Query with:\n\
         --   duckdb -cmd \".read views.sql\"\n\
         --\n\
         -- Objects in dbt_internal are not part of the public contract and may\n\
         -- change without notice.\n\n",
    );

    for ns in Ns::ALL {
        writeln!(sql, "CREATE SCHEMA IF NOT EXISTS {};", ns.prefix()).unwrap();
    }
    sql.push('\n');

    for ns in Ns::ALL {
        for table in INFO_SCHEMA.iter().filter(|t| t.ns == *ns) {
            writeln!(sql, "{};", base_view_statement(table, None)).unwrap();
        }
        sql.push('\n');
    }

    for (_, statement) in derived_statements() {
        writeln!(sql, "{statement};\n").unwrap();
    }

    std::fs::write(dir.join("views.sql"), sql)?;
    Ok(())
}
