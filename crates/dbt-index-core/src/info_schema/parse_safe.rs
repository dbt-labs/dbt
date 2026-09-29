//! The views a parse-time check reads: the information schema's vocabulary,
//! computed live over the metadata this run parsed.
//!
//! A project quality check is SQL that runs **at parse time**, so it cannot read the
//! materialized information schema — that is written after parse, and only when asked
//! for. It reads the parse metadata epochs instead, through the views declared here:
//! [`epoch_views::parse_safe_statements`](super::epoch_views::parse_safe_statements)
//! narrows the epoch view layer to the names and columns below.
//!
//! Two properties, and each one constrains the other:
//!
//! - **Same names as the information schema.** A view is named after the
//!   information-schema table it mirrors and spells every column the way that table
//!   spells it, so a query written against one runs against the other. The spelling is
//!   not restated here: a view lists the *output* names it exposes, and the index column
//!   behind each is looked up in [`INFO_SCHEMA`](super::schema::INFO_SCHEMA). A rename
//!   there therefore lands here too, instead of leaving the two surfaces disagreeing.
//! - **Only columns that are final at parse.** Zero rows is a pass, so a column that is
//!   still empty when a check runs does not fail the check — it quietly satisfies it.
//!   `WHERE compiled_code IS NULL` finds nothing to complain about at parse not because
//!   the project is fine but because compile has not happened yet. Every such column is
//!   left out, and left out rather than nulled: referencing one is a DuckDB binder error,
//!   which is loud.
//!
//! So a view here is a **strict subset** of the information-schema table of the same
//! name: same names, fewer columns — and literally so, since the published view's own
//! statement is the subquery the projection reads. The set of views is a subset too — a table with no
//! data at parse is not published at all, since an empty table is indistinguishable from
//! a passing check. [`VIEWS`] lists what is left out and why.
//!
//! Every view name is a table name in the information schema, with no exceptions. A view
//! with no counterpart would mean a check author learning a vocabulary that the published
//! artifact does not use, which is the whole thing this file exists to prevent — so the
//! answer to "checks need a view for X" is to publish X in
//! [`INFO_SCHEMA`](super::schema::INFO_SCHEMA) first, then mirror it here.

use std::collections::BTreeSet;
use std::fmt::Write;

use crate::IndexError;
use crate::parquet::schema_for;

use super::schema::{DAG_RESOURCE_TYPES, spec_for};
use super::spec::{Filter, Ns, Src};

/// Schema the views are created in — the one the information schema publishes under, so
/// `dbt.models` means the same thing in a check as it does in `info_schema/v1/`.
pub const VIEW_SCHEMA: &str = "dbt";

/// Schema the relations underneath the views are registered in.
///
/// Deliberately not `dbt`: the views are the contract and what they read is not. Keeping
/// those relations out of `dbt` is also what stops a check from reaching one by hand —
/// `FROM dbt.nodes` fails to bind instead of returning columns that stay empty until
/// compile. The allowlist in the `info_schema()` Jinja helper cannot do that on its own,
/// since check SQL is free to name any relation it likes.
pub const BASE_SCHEMA: &str = "dbt_internal";

/// The node set, which most views project.
const NODES: &str = "nodes";

/// The tables holding DAG-participating resource types that are not in the node set,
/// with the `resource_type` each one stands for. `write_to_nodes` does not admit them,
/// so `dbt.dag_nodes` unions them in.
const DAG_SIDE_TABLES: &[(&str, &str)] = &[
    ("exposures", "exposure"),
    ("metrics", "metric"),
    ("unit_tests", "unit_test"),
];

/// Alias for the single source table of a projected view.
const T: &str = "t";
/// Aliases for the two sides of a joined view.
const L: &str = "l";
const R: &str = "r";

/// One parse-safe view.
///
/// [`name`](Self::name), [`vocabulary`](Self::vocabulary) and [`cols`](Self::cols) are what
/// the published views are built from. [`src`](Self::src) and [`filter`](Self::filter) no
/// longer render the SQL a check executes — [`create_view_sql`](Self::create_view_sql) built
/// that while checks read the index — and are kept as the cross-check they also were: their
/// tests resolve every declared column against the index schema and against the
/// information-schema spec, so a rename in either place fails here rather than at query time.
pub struct ParseSafeView {
    /// View name: `dbt.<name>`, and the argument `info_schema('<name>')` takes.
    pub name: &'static str,
    /// Information-schema table this view mirrors, and where its column names come from.
    /// Always equal to [`name`](Self::name) — the field stays because the lookup reads
    /// better named, and because a mismatch is a test failure rather than a silent
    /// divergence.
    pub vocabulary: &'static str,
    /// Index table(s) the view reads. Restated rather than taken from the
    /// information-schema table because the two can differ: the information schema
    /// assembles some tables in code ([`Src::Own`]) where a view has to project one.
    pub src: Src,
    /// Rows to keep. Restated for the same reason as [`src`](Self::src).
    pub filter: Filter,
    /// Output columns, named as the information schema names them.
    pub cols: &'static [&'static str],
}

/// Columns every node-derived view exposes, in information-schema spelling.
///
/// `$extra` is spliced in before `ingested_at` so that column stays last, matching
/// [`super::schema`]'s `node_cols!`. Textual expansion for the same reason: const slices
/// cannot be concatenated.
///
/// Absent, and deliberately: `compiled_code`, `compiled_path`, `grain*`, `layer_inferred`
/// and `search_text` (written during compile); `classifiers` (partially written at parse,
/// which is worse — a check filtering on it sees a subset and passes); `unique_key`,
/// `pre_hook`, `post_hook`, `grants`, `persist_docs`, `incremental_strategy`,
/// `on_schema_change`, `full_refresh`, `constraints`, `docs_show`, `time_spine` and
/// `ai_context` (no writer fills them — their real values are inside the `config` JSON,
/// which is here). `file_path` and `checksum` are absent because the information schema
/// does not have them.
///
/// `raw_code` is not here either, and it is the one column left out by *resource type* rather
/// than by phase, which is why it cannot live in this macro at all: the views that carry it name
/// it themselves.
///
/// It was excluded from `models` because the index left it NULL there — `trim_model_payload`
/// drops it from `__common_attr__` for models only, their payloads being multi-KB mostly
/// because of SQL — while every other node type with SQL kept it. Reading the epoch payloads
/// removes that asymmetry: `raw_code` is populated for models there, which is why the
/// information schema has always published it, and a rule developed in `dbt show --info` no
/// longer returns rows there and nothing as a check. Offering it on `models` is a widening of
/// the check surface rather than part of the port, so it is still left out here.
macro_rules! node_cols {
    ($($extra:expr),* $(,)?) => {
        &[
            "unique_id",
            "name",
            "resource_type",
            "package_name",
            "original_file_path",
            "fqn",
            "alias",
            "description",
            "node_language",
            "database_name",
            "schema_name",
            "relation_name",
            "identifier",
            "enabled",
            "materialized",
            "config",
            "access",
            "group",
            "contract_enforced",
            "version",
            "latest_version",
            "deprecation_date",
            "primary_key",
            "properties_yml_file_path",
            "tags",
            "meta",
            $($extra,)*
            "ingested_at",
        ]
    };
}

/// Every view a parse-time check may read: one per information-schema table in the `dbt`
/// namespace that holds data at parse.
///
/// The information-schema tables deliberately absent, and why:
///
/// - `column_lineage` — needs static analysis, so it is empty at parse.
/// - `classifiers` — the information schema publishes the shape, but nothing writes the
///   taxonomy yet.
/// - `semantic_relationships` — same: a table in the schema with no writer behind it.
///
/// Absent for reasons unrelated to the information schema:
///
/// - the `dbt_rt` schema — written from run artifacts, so it holds nothing at parse.
/// - `generation` — ingest bookkeeping rather than project metadata. Parse-stable, but
///   exposing it invites checks that depend on ingest internals.
/// - `catalog_tables`, `column_stats` and the rest of the catalog tables — they are not in
///   the information schema either, and are empty until `dbt docs generate` runs.
pub const VIEWS: &[ParseSafeView] = &[
    // ── project metadata ────────────────────────────────────────────────
    // `schema_version` and `last_full_parse_at` are in the information schema but not
    // here: the writer fills them, there is no index column to read them from. `quoting`
    // and `ai_context` are in the index's own project row and never written to it.
    ParseSafeView {
        name: "project",
        vocabulary: "project",
        src: Src::Table("project"),
        filter: Filter::All,
        cols: &[
            "project_name",
            "dbt_version",
            "adapter_type",
            "git_sha",
            "git_branch",
            "git_uncommitted_changes",
            "ingested_at",
        ],
    },
    // Which packages are installed, and nothing else: the ingest has no source for the
    // other columns (`package_source`, `version`, `git_url`, `git_revision`,
    // `local_path`) and writes the name alone.
    ParseSafeView {
        name: "packages",
        vocabulary: "packages",
        src: Src::Table("packages"),
        filter: Filter::All,
        cols: &["package_name", "ingested_at"],
    },
    ParseSafeView {
        name: "project_vars",
        vocabulary: "project_vars",
        src: Src::Table("project_vars"),
        filter: Filter::All,
        cols: &["project_name", "var_name", "var_value", "ingested_at"],
    },
    ParseSafeView {
        name: "project_env_vars",
        vocabulary: "project_env_vars",
        src: Src::Table("project_env_vars"),
        filter: Filter::All,
        cols: &["env_var_name", "ingested_at"],
    },
    // ── one view per resource type ──────────────────────────────────────
    ParseSafeView {
        name: "models",
        vocabulary: "models",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["model"]),
        cols: node_cols![],
    },
    ParseSafeView {
        name: "seeds",
        vocabulary: "seeds",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["seed"]),
        cols: node_cols![],
    },
    ParseSafeView {
        name: "snapshots",
        vocabulary: "snapshots",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["snapshot"]),
        cols: node_cols!["raw_code"],
    },
    ParseSafeView {
        name: "functions",
        vocabulary: "functions",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["function"]),
        cols: node_cols!["raw_code"],
    },
    ParseSafeView {
        name: "analyses",
        vocabulary: "analyses",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["analysis"]),
        cols: node_cols!["raw_code"],
    },
    ParseSafeView {
        name: "hooks",
        vocabulary: "hooks",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["operation", "sql_operation"]),
        cols: node_cols!["raw_code"],
    },
    // The source-only columns live here and nowhere else, as in the information schema:
    // on a model row they would always be NULL. `loaded_at_query`, `freshness`,
    // `external`, `source_meta` and `quoting` are the source columns no writer fills.
    ParseSafeView {
        name: "sources",
        vocabulary: "sources",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["source"]),
        cols: node_cols![
            "source_name",
            "source_description",
            "loader",
            "loaded_at_field",
        ],
    },
    // Node columns joined with the generic-test detail, as one view rather than a node
    // view plus a `test_metadata` one, because that is how the information schema
    // publishes it. LEFT join — a singular test has a node row and no detail row and must
    // still appear. `where` and `limit` are in the information schema but not here: the
    // index has the columns and nothing writes them.
    ParseSafeView {
        name: "data_tests",
        vocabulary: "data_tests",
        src: Src::Join {
            left: "nodes",
            right: "test_metadata",
            left_on: "unique_id",
            right_on: "unique_id",
        },
        filter: Filter::ResourceTypeIn(&["test"]),
        cols: &[
            "unique_id",
            "name",
            "package_name",
            "original_file_path",
            "fqn",
            "description",
            "database_name",
            "schema_name",
            "relation_name",
            "enabled",
            "materialized",
            "config",
            "tags",
            "meta",
            "group",
            "properties_yml_file_path",
            "test_name",
            "test_definition_package",
            "arguments",
            "column_name",
            "node_unique_id",
            "severity",
            "warn_if",
            "error_if",
            "fail_calc",
            "store_failures",
            "store_failures_as",
            "ingested_at",
        ],
    },
    // `config` and `created_at` are in the information schema but unwritten here.
    ParseSafeView {
        name: "unit_tests",
        vocabulary: "unit_tests",
        src: Src::Table("unit_tests"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "model",
            "description",
            "package_name",
            "original_file_path",
            "fqn",
            "given",
            "expect",
            "overrides",
            "versions",
            "ingested_at",
        ],
    },
    // Checks are node rows like any other, so a check can assert something about the
    // checks themselves.
    ParseSafeView {
        name: "checks",
        vocabulary: "checks",
        src: Src::Table("nodes"),
        filter: Filter::ResourceTypeIn(&["check"]),
        cols: node_cols!["raw_code"],
    },
    // ── everything else declared in YAML ────────────────────────────────
    ParseSafeView {
        name: "macros",
        vocabulary: "macros",
        src: Src::Table("macros"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "package_name",
            "original_file_path",
            "macro_sql",
            "description",
            "depends_on_macros",
            "arguments",
            "docs_show",
            "properties_yml_file_path",
            "meta",
            "created_at",
            "ingested_at",
        ],
    },
    // The group row carries who owns the group and nothing about where it was declared:
    // `package_name`, `original_file_path` and `config` are in the information schema and
    // unwritten in the index.
    ParseSafeView {
        name: "groups",
        vocabulary: "groups",
        src: Src::Table("groups"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "description",
            "owner_name",
            "owner_email",
            "ingested_at",
        ],
    },
    // `meta` and `config` are in the information schema but unwritten here.
    ParseSafeView {
        name: "exposures",
        vocabulary: "exposures",
        src: Src::Table("exposures"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "exposure_type",
            "label",
            "owner_name",
            "owner_email",
            "url",
            "maturity",
            "description",
            "package_name",
            "original_file_path",
            "fqn",
            "depends_on",
            "tags",
            "created_at",
            "ingested_at",
        ],
    },
    // `semantic_model_name` and `ai_context` are in the information schema but unwritten
    // here.
    ParseSafeView {
        name: "metrics",
        vocabulary: "metrics",
        src: Src::Table("metrics"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "label",
            "metric_type",
            "description",
            "package_name",
            "original_file_path",
            "fqn",
            "type_params",
            "metric_filter",
            "time_granularity",
            "input_metric_names",
            "group",
            "tags",
            "meta",
            "config",
            "created_at",
            "ingested_at",
        ],
    },
    ParseSafeView {
        name: "docs_blocks",
        vocabulary: "docs_blocks",
        src: Src::Table("docs"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "package_name",
            "original_file_path",
            "content",
            "ingested_at",
        ],
    },
    // `config` is in the information schema but unwritten here.
    ParseSafeView {
        name: "saved_queries",
        vocabulary: "saved_queries",
        src: Src::Table("saved_queries"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "label",
            "description",
            "package_name",
            "original_file_path",
            "fqn",
            "query_params",
            "exports",
            "depends_on",
            "group",
            "tags",
            "created_at",
            "ingested_at",
        ],
    },
    // ── semantic layer ──────────────────────────────────────────────────
    // `config` is unwritten on all four of these, and the semantic model row also has no
    // `package_name` or `original_file_path` — it is built from the payload, which does
    // not carry them.
    ParseSafeView {
        name: "semantic_models",
        vocabulary: "semantic_models",
        src: Src::Table("semantic_models"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "model",
            "label",
            "description",
            "fqn",
            "node_relation",
            "primary_entity",
            "defaults",
            "group",
            "created_at",
            "ingested_at",
        ],
    },
    ParseSafeView {
        name: "semantic_entities",
        vocabulary: "semantic_entities",
        src: Src::Table("semantic_entities"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "entity_type",
            "description",
            "label",
            "entity_role",
            "expr",
            "ingested_at",
        ],
    },
    ParseSafeView {
        name: "semantic_measures",
        vocabulary: "semantic_measures",
        src: Src::Table("semantic_measures"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "agg",
            "description",
            "label",
            "expr",
            "create_metric",
            "agg_time_dimension",
            "agg_params",
            "non_additive_dimension",
            "ingested_at",
        ],
    },
    ParseSafeView {
        name: "semantic_dimensions",
        vocabulary: "semantic_dimensions",
        src: Src::Table("semantic_dimensions"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "name",
            "dimension_type",
            "description",
            "label",
            "expr",
            "is_partition",
            "time_granularity",
            "validity_params",
            "ingested_at",
        ],
    },
    ParseSafeView {
        name: "time_spines",
        vocabulary: "time_spines",
        src: Src::Table("time_spines"),
        filter: Filter::All,
        cols: &[
            "unique_id",
            "primary_column",
            "primary_granularity",
            "custom_granularities",
            "node_relation",
            "ingested_at",
        ],
    },
    // ── graph ───────────────────────────────────────────────────────────
    // Every enabled resource that participates in the DAG. Assembled rather than
    // projected: the node set holds only the types `write_to_nodes` admits, so
    // exposures, metrics and unit tests come from their own tables. Three columns — a
    // membership list, not a cross-type node surface. See [`Self::dag_nodes_sql`].
    ParseSafeView {
        name: "dag_nodes",
        vocabulary: "dag_nodes",
        src: Src::Own,
        filter: Filter::All,
        cols: &["unique_id", "resource_type", "ingested_at"],
    },
    ParseSafeView {
        name: "edges",
        vocabulary: "edges",
        src: Src::Table("edges"),
        filter: Filter::All,
        cols: &["parent_unique_id", "child_unique_id", "ingested_at"],
    },
    // `data_type_inferred` needs static analysis, `data_type_actual` (and so the resolved
    // `data_type`) needs `dbt docs generate`, and `classifiers` is only partly written at
    // parse. `column_index`, `label`, `quote`, `granularity`, `constraints`, `tests` and
    // `meta` are filled by the same later merges. What is left is what YAML declared.
    ParseSafeView {
        name: "node_columns",
        vocabulary: "node_columns",
        src: Src::Table("node_columns"),
        filter: Filter::All,
        cols: &[
            "node_unique_id",
            "column_name",
            "data_type_declared",
            "description",
            "tags",
            "ingested_at",
        ],
    },
];

impl ParseSafeView {
    /// Index tables this view reads.
    ///
    /// An *empty* table is fine — the view returns no rows. A *missing* one means the
    /// view cannot be created at all, which is why callers register the tables first and
    /// then create only the views whose tables are all there.
    pub fn base_tables(&self) -> Vec<&'static str> {
        match self.src {
            Src::Table(table) => vec![table],
            Src::Join { left, right, .. } => vec![left, right],
            // The one assembled view. Returning its tables rather than nothing is what
            // keeps the caller's "create it only if every table registered" check honest.
            Src::Own => {
                let mut tables = vec![NODES];
                tables.extend(DAG_SIDE_TABLES.iter().map(|(table, _)| *table));
                tables
            }
        }
    }

    /// The view's `CREATE OR REPLACE VIEW` statement, reading [`BASE_SCHEMA`].
    ///
    /// Errors describe a mistake in [`VIEWS`] rather than anything about the project, and
    /// the tests in this module are what should catch them; they are returned rather than
    /// panicked so a bad entry takes down one check, not the process.
    pub fn create_view_sql(&self) -> Result<String, IndexError> {
        // Assembled rather than projected. `info_schema::build_own` and
        // `epoch_views::own_sql` special-case the same table by name; this is the third
        // place that has to, for the same reason.
        if matches!(self.src, Src::Own) {
            return self.dag_nodes_sql();
        }

        let spec = spec_for(Ns::Dbt, self.vocabulary).ok_or_else(|| {
            self.err(format!(
                "no information-schema table named '{}'",
                self.vocabulary
            ))
        })?;

        let mut selects = Vec::with_capacity(self.cols.len());
        for out in self.cols {
            let col =
                spec.cols.iter().find(|c| c.out == *out).ok_or_else(|| {
                    self.err(format!("'{out}' is not a column of '{}'", spec.name))
                })?;
            // A column the information schema declares but assembles in code has no
            // source name to borrow; the index column is the one that shares its name.
            let src = if col.src.is_empty() { col.out } else { col.src };
            selects.push(format!("{} AS {}", self.qualify(src)?, quote(out)));
        }

        let from = match self.src {
            Src::Table(table) => format!("{BASE_SCHEMA}.{} AS {T}", quote(table)),
            Src::Join {
                left,
                right,
                left_on,
                right_on,
            } => format!(
                "{BASE_SCHEMA}.{} AS {L} LEFT JOIN {BASE_SCHEMA}.{} AS {R} ON {L}.{} = {R}.{}",
                quote(left),
                quote(right),
                quote(left_on),
                quote(right_on),
            ),
            Src::Own => return Err(self.err("must read an index table".to_string())),
        };

        let mut sql = format!(
            "CREATE OR REPLACE VIEW {VIEW_SCHEMA}.{} AS SELECT {} FROM {from}",
            quote(self.name),
            selects.join(", "),
        );
        if let Filter::ResourceTypeIn(types) = self.filter {
            let list = types
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(", ");
            write!(sql, " WHERE {} IN ({list})", self.qualify("resource_type")?)
                .expect("writing to a String cannot fail");
        }
        Ok(sql)
    }

    /// `dbt.dag_nodes`: the node set unioned with the tables holding the DAG-participating
    /// resource types that are not in it.
    ///
    /// Mirrors `info_schema::build_dag_nodes` rather than inventing a rule: keep the node
    /// rows whose `resource_type` is a DAG type, keep every row of the side tables (their
    /// type is one by definition), and treat a missing `enabled` as enabled, matching the
    /// config default. The side tables' `enabled` is in the index even though the
    /// information schema does not publish it — a view reads the table, not the artifact.
    fn dag_nodes_sql(&self) -> Result<String, IndexError> {
        // Written per output column so a column added to the information schema's
        // `dag_nodes` fails here instead of silently not appearing.
        let expr = |col: &str, resource_type: Option<&str>| -> Result<String, IndexError> {
            Ok(match (col, resource_type) {
                ("resource_type", Some(literal)) => format!("'{literal}'"),
                ("unique_id" | "resource_type" | "ingested_at", _) => {
                    format!("{T}.{}", quote(col))
                }
                _ => return Err(self.err(format!("no expression for '{col}'"))),
            })
        };
        let select = |table: &str, resource_type: Option<&str>| -> Result<String, IndexError> {
            let cols = self
                .cols
                .iter()
                .map(|col| Ok(format!("{} AS {}", expr(col, resource_type)?, quote(col))))
                .collect::<Result<Vec<_>, IndexError>>()?
                .join(", ");
            Ok(format!(
                "SELECT {cols} FROM {BASE_SCHEMA}.{} AS {T} WHERE COALESCE({T}.\"enabled\", TRUE)",
                quote(table),
            ))
        };

        let types = DAG_RESOURCE_TYPES
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        let mut arms = vec![format!(
            "{} AND {T}.\"resource_type\" IN ({types})",
            select(NODES, None)?
        )];
        for (table, resource_type) in DAG_SIDE_TABLES {
            arms.push(select(table, Some(resource_type))?);
        }

        Ok(format!(
            "CREATE OR REPLACE VIEW {VIEW_SCHEMA}.{} AS {}",
            quote(self.name),
            arms.join(" UNION ALL "),
        ))
    }

    /// Qualify a source column with the alias of the table it comes from. For a join the
    /// left table wins, as it does in the information schema's own projection.
    fn qualify(&self, src: &str) -> Result<String, IndexError> {
        match self.src {
            Src::Table(_) => Ok(format!("{T}.{}", quote(src))),
            Src::Join { left, .. } if has_column(left, src) => Ok(format!("{L}.{}", quote(src))),
            Src::Join { right, .. } if has_column(right, src) => Ok(format!("{R}.{}", quote(src))),
            Src::Join { left, right, .. } => Err(self.err(format!(
                "neither '{left}' nor '{right}' has a column '{src}'"
            ))),
            Src::Own => Err(self.err("must read an index table".to_string())),
        }
    }

    fn err(&self, msg: String) -> IndexError {
        IndexError::Other(format!("parse-safe view '{}': {msg}", self.name))
    }
}

/// Every index table [`VIEWS`] reads, deduplicated.
pub fn base_tables() -> Vec<&'static str> {
    VIEWS
        .iter()
        .flat_map(|v| v.base_tables())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Whether the index's `table` has a column named `col`.
fn has_column(table: &str, col: &str) -> bool {
    schema_for(table).field_with_name(col).is_ok()
}

/// Quote an identifier, so a column named `group` or `where` is still a column name.
fn quote(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}
