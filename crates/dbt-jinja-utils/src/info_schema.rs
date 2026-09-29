//! The `info_schema()` Jinja helper available to project quality checks.
//!
//! A check is SQL over the metadata index, and it runs **at parse time** — before anything is
//! compiled or materialized. So the helper deliberately exposes only views whose every column has
//! its final value by the end of parse. Anything later is not merely unavailable, it is *quietly*
//! unavailable: an index column that gets filled during compile reads as NULL at parse, and a check
//! filtering on it finds "violations" that are artefacts of timing rather than of the project.

use minijinja::Value;

/// Views a parse-time check may read.
///
/// Each one is named after the table of the same name in the dbt information schema and
/// spells its columns the same way, so a query that runs here runs there. It exposes fewer
/// columns, though: only those that hold their final value by the end of parse. A column that
/// compile or `dbt docs generate` fills reads as NULL at parse, and since zero rows is a pass,
/// a check filtering on one quietly reports nothing rather than failing. Such columns are left
/// out rather than nulled, so naming one is a DuckDB binder error.
///
/// Every name here is a table in the information schema: one vocabulary, whether a query is
/// run by a check or against `target/info_schema/`. `dbt.nodes` is deliberately absent — the
/// index's own tables are registered under `dbt_internal`, out of reach of a check.
///
/// Defined once, in `dbt_index_core::info_schema::parse_safe::VIEWS`, and registered from
/// there by the check adapter — this list only has to name them, not redefine their columns.
/// The two are kept in step by a test in the crate that can see both (`dbt-tasks-sa`); this
/// crate deliberately does not depend on the index.
///
/// The list is the information schema's `dbt` tables minus the ones that hold nothing at parse
/// (`column_lineage` needs static analysis; `classifiers` and `semantic_relationships` have no
/// writer yet). `dbt_rt` is absent entirely: it is written from run artifacts, so at parse it
/// does not exist.
pub const PARSE_SAFE_VIEWS: &[&str] = &[
    "project",
    "packages",
    "project_vars",
    "project_env_vars",
    "models",
    "seeds",
    "snapshots",
    "functions",
    "analyses",
    "hooks",
    "sources",
    "data_tests",
    "unit_tests",
    "checks",
    "macros",
    "groups",
    "exposures",
    "metrics",
    "docs_blocks",
    "saved_queries",
    "semantic_models",
    "semantic_entities",
    "semantic_measures",
    "semantic_dimensions",
    "time_spines",
    "dag_nodes",
    "edges",
    "node_columns",
];

/// The `info_schema('<view>')` helper. Expands to `dbt.<view>` so a check can read the index
/// without hard-coding the schema: `SELECT … FROM {{ info_schema('models') }}`.
///
/// Only [`PARSE_SAFE_VIEWS`] are accepted. An unknown or later-phase name is an error at render
/// time, naming what is available — far better than a check that runs and silently reports nothing,
/// which is what happens when a check reads a table that is empty at parse.
pub fn make_info_schema_fn() -> Value {
    use minijinja::value::Kwargs;
    use minijinja::{Error as MjError, ErrorKind};
    Value::from_function(
        |args: &[Value], _kwargs: Kwargs| -> Result<Value, MjError> {
            let view = args.first().and_then(|v| v.as_str()).ok_or_else(|| {
                MjError::new(
                    ErrorKind::InvalidOperation,
                    "info_schema(view): first argument must be a view name string",
                )
            })?;
            if !PARSE_SAFE_VIEWS.contains(&view) {
                return Err(MjError::new(
                    ErrorKind::InvalidOperation,
                    format!(
                        "info_schema: '{view}' is not available to a parse-time check. \
                         Available views: {}",
                        PARSE_SAFE_VIEWS.join(", ")
                    ),
                ));
            }
            Ok(Value::from(format!("dbt.{view}")))
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(view: &str) -> Result<String, String> {
        let env = minijinja::Environment::new();
        make_info_schema_fn()
            .call(&env.empty_state(), &[Value::from(view)], &[])
            .map(|v| v.to_string())
            .map_err(|e| e.to_string())
    }

    #[test]
    fn expands_every_parse_safe_view_to_the_dbt_schema() {
        for view in PARSE_SAFE_VIEWS {
            assert_eq!(
                call(view).unwrap_or_else(|e| panic!("{view} must expand, got {e}")),
                format!("dbt.{view}"),
            );
        }
    }

    #[test]
    fn rejects_the_raw_tables_the_parse_safe_views_are_projected_from() {
        // `dbt.nodes` mixes parse-safe columns with compile-filled and never-populated ones,
        // and the per-resource-type views are its safe projections; the table itself must stay
        // unreachable. (`node_columns` is not in this list: the information schema publishes a
        // table by that name, so the view carrying its name is the safe projection.)
        for raw in ["nodes", "test_metadata", "generation"] {
            let err = call(raw).expect_err("the raw table must be refused");
            assert!(
                err.contains("not available to a parse-time check"),
                "unexpected message for {raw}: {err}"
            );
        }
    }

    #[test]
    fn rejects_the_names_these_views_had_before_they_took_the_information_schema_s() {
        // A check written against the old names must fail loudly at render, naming what is
        // available now, rather than resolving to a relation that no longer exists.
        for renamed in ["tests", "operations", "columns", "docs", "graph_nodes"] {
            let err = call(renamed).expect_err("the old name must be refused");
            assert!(
                err.contains("not available to a parse-time check"),
                "unexpected message for {renamed}: {err}"
            );
        }
    }

    #[test]
    fn rejects_views_that_are_not_final_at_parse() {
        // `dbt_rt.*` does not exist until run artifacts are written; `generation` is ingest
        // bookkeeping, not project metadata; `column_lineage` waits on static analysis, and
        // `classifiers` / `semantic_relationships` are information-schema tables with no writer
        // behind them yet. All must fail loudly rather than read as empty.
        for later in [
            "column_lineage",
            "catalog_tables",
            "run_results",
            "generation",
            "classifiers",
            "semantic_relationships",
        ] {
            let err = call(later).expect_err("a later-phase view must be refused");
            assert!(
                err.contains("not available to a parse-time check"),
                "unexpected message for {later}: {err}"
            );
        }
    }

    #[test]
    fn rejects_names_that_could_inject_sql() {
        // The result is spliced into the check's SQL, and the allowlist is what makes that safe: no
        // separate escaping is needed because nothing outside the list is ever expanded.
        for bad in [
            "",
            "models; DROP TABLE x",
            "dbt.models",
            "a-b",
            "a b",
            "a'b",
            "MODELS",
        ] {
            assert!(call(bad).is_err(), "expected {bad:?} to be rejected");
        }
    }

    #[test]
    fn error_names_the_available_views() {
        let err = call("nope").expect_err("unknown view");
        assert!(
            err.contains("models"),
            "should list what is available: {err}"
        );
        assert!(
            err.contains("data_tests"),
            "should list what is available: {err}"
        );
    }
}
