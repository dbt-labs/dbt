//! SQL equivalents for the staging columns the information schema is built from.
//!
//! [`super::schema::INFO_SCHEMA`] declares each output column as a *staging*
//! column — the shape the Rust ingest produces in the staging directory (the
//! flat index at `target/private/index/`, or a private fallback). A view layer over the
//! epoch files has no staging directory, so every referenced staging column needs
//! an expression
//! over the epoch parquet instead. This module is that mapping, declared once in
//! the same style as `spec.rs` rather than written inline in generated SQL: the
//! ~200 JSON extractions are each a fact currently encoded in a Rust row builder
//! in `ingest/metadata_to_parquet.rs`, and re-stating them is 200 chances to
//! pick a key that silently yields an all-null column.
//!
//! Two tests hold the mapping closed. [`tests::every_referenced_column_is_mapped`]
//! walks `INFO_SCHEMA` and fails on a pair with no entry, so a new output column
//! cannot be added without stating where the view layer gets it. The
//! differential test in `tests_epoch.rs` compares the two paths column by column
//! on a real project, so a *wrong* entry fails too.

use std::path::Path;

use crate::epoch_layers;
use crate::ingest::{
    CATALOG_COLUMNS_SUBDIR, COMPILE_CLL_SUBDIR, COMPILE_COLUMNS_SUBDIR, COMPILE_NODES_SUBDIR,
    PARSE_ALIVE, PARSE_COLUMNS_SUBDIR, PARSE_GENERATION, PARSE_NODES_SUBDIR, PARSE_PROJECT,
    PARSE_RESOLVER_STATE, RUN_CATALOG_STATS_SUBDIR, RUN_FRESHNESS_SUBDIR, RUN_INVOCATIONS_SUBDIR,
    RUN_RESULTS_SUBDIR,
};
use crate::parquet::schema_for;

use super::spec::Src;

/// Table alias of the base epoch relation in a generated view.
///
/// Every generated column reference is qualified, because a joined origin has
/// two relations that share column names (`parse/nodes` and `compile/nodes`
/// both have `unique_id` and `ingested_at`) and an unqualified reference to one
/// of those is a query-time ambiguity error rather than a wrong answer we would
/// see in the diff.
pub const BASE: &str = "t";

/// Table alias of the joined relation, where [`Origin::Relation::join`] is set.
pub const JOINED: &str = "j";

/// Qualified name of the unnested element, where [`Origin::Relation::unnest`] is
/// set. `u` is the lateral alias and `e` its single column, so the element is
/// referred to as one string everywhere.
pub const ELEM: &str = "u.e";

/// The alias clause that binds [`ELEM`], for the generated `unnest(..) AS u(e)`.
/// Beside it so the alias and the reference to it cannot drift apart.
pub const ELEM_REL: &str = "u(e)";

/// The `test_metadata` object inside a test node's payload, under either of the
/// two names the payload carries it under.
///
/// A macro rather than a `const` so `concat!` can splice it: three columns read
/// through it and its presence is `test_metadata`'s row predicate, and writing the
/// COALESCE out four times is four chances for the copies to drift apart.
macro_rules! test_metadata_obj {
    () => {
        "COALESCE(json_extract(t.payload, '$.__test_attr__.test_metadata'), \
         json_extract(t.payload, '$.test_metadata'))"
    };
}

/// How one staging column is computed from its epoch source.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EpochExpr {
    /// A column of the base relation, under the same name.
    Same,
    /// A column of the base relation, under a different name.
    Col(&'static str),
    /// `json_extract_string(t.payload, '$.<path>')` — a scalar out of the node
    /// payload, as text.
    Json(&'static str),
    /// `json_extract(t.payload, '$.<path>')` — a sub-object or array kept as
    /// JSON text, for columns the staging schema also stores as serialised JSON.
    JsonRaw(&'static str),
    /// [`EpochExpr::Json`] with a fallback, for the payload fields the Rust path
    /// reads through `str_field_default` or `unwrap_or`. The second field is SQL,
    /// so `"''"` for a defaulted string and `"TRUE"` for a defaulted boolean —
    /// which is the difference between an absent field and a null one, and the
    /// staging column is frequently non-nullable.
    JsonOr(&'static str, &'static str),
    /// A JSON array of strings inside the payload, as a list. Absent, null or not
    /// an array is the empty list, matching the `unwrap_or_default()` the Rust
    /// path applies to the same field.
    JsonList(&'static str),
    /// The first non-null of several payload paths, then the fallback SQL — the
    /// `or_else` chains the Rust path walks for fields that have moved between
    /// payload shapes. Order matters and matches the Rust order; the fallback is
    /// `"NULL"` where the Rust path has none.
    JsonFirst(&'static [&'static str], &'static str),
    /// Like [`EpochExpr::Json`], but null for `model` nodes.
    ///
    /// `write_parse_nodes` trims a model's payload to `config` and
    /// `__model_attr__` before reading it — a deliberate optimisation,
    /// the model payload being the multi-KB one — so every other payload field
    /// is unset for models in the materialized layer. The view layer reads the
    /// untrimmed payload and would otherwise publish *more* data than its
    /// counterpart, which is still a difference: `epoch_views.sql` promises to
    /// be substitutable for `views.sql`, not to be better than it. Reproducing
    /// the trim keeps the two exchangeable and makes lifting it a single change
    /// on both sides.
    TrimmedJson(&'static str),
    /// [`EpochExpr::JsonRaw`] under the model trim described on
    /// [`EpochExpr::TrimmedJson`].
    TrimmedJsonRaw(&'static str),
    /// The unnested element itself, where [`Origin::Relation::unnest`] is set.
    /// Only meaningful for a list of scalars; for a list of objects the element
    /// is read through the `Elem*` variants below.
    Elem,
    /// `json_extract_string(u.e, '$.<path>')` — a scalar out of the unnested
    /// element, as text. [`EpochExpr::Json`] one level down.
    ElemJson(&'static str),
    /// `json_extract(u.e, '$.<path>')` — a sub-object of the unnested element,
    /// kept as JSON text. [`EpochExpr::JsonRaw`] one level down.
    ElemJsonRaw(&'static str),
    /// The first non-null of several paths within the element, then the fallback
    /// SQL. [`EpochExpr::JsonFirst`] one level down, and the only defaulting form
    /// the element variants need: a single path with a fallback is the one-element
    /// case.
    ElemJsonFirst(&'static [&'static str], &'static str),
    /// A column of the joined relation, under the name given.
    JoinCol(&'static str),
    /// A column of the joined relation holding a JSON array of strings as text,
    /// as a list. Absent or unparseable is the empty list rather than null,
    /// matching the `serde_json::from_str(..).unwrap_or_default()` the Rust path
    /// applies to the same column.
    JoinJsonList(&'static str),
    /// Arbitrary SQL over the relations. The escape hatch for the handful of
    /// columns that are assembled rather than extracted; nothing is substituted,
    /// so the expression must qualify its own columns with [`BASE`] or
    /// [`JOINED`].
    Sql(&'static str),
    /// [`EpochExpr::Sql`] for a column that holds serialised JSON. Distinguished
    /// only so [`is_json_text`] can see it.
    SqlJson(&'static str),
    /// No source in the epoch files. Emitted as a typed null, matching what the
    /// Rust path writes for the same column.
    Null,
    /// The empty list. For a non-nullable list column the Rust path fills with
    /// `vec![]` rather than leaving unset — a distinction `Null` would lose,
    /// since an empty list and a null list are different values.
    EmptyList,
    /// A literal `true`. For the non-nullable booleans the Rust path hardcodes.
    True,
}

/// How rows of an epoch relation supersede each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supersede {
    /// Keep every row. The append-only `run/*` sources, where each invocation
    /// adds history that stays queryable.
    KeepAll,
    /// One row per key, from the highest epoch that has it. Mirrors the ingest,
    /// which walks epochs newest-first and keeps the first `unique_id` it sees
    /// (`write_parse_nodes`).
    LatestBy(&'static str),
    /// Every row of the highest epoch that mentions the key, and none of the
    /// rows of any older one — a *wholesale* replacement, where `LatestBy` is
    /// per row.
    ///
    /// The difference shows when a group shrinks: `LatestBy("unique_id,
    /// column_name")` on a model whose newest epoch dropped a column keeps the
    /// dropped column, because it wins the partition it is the only member of.
    /// This is the semantics of the sources whose unit of publication is the
    /// whole group — a node's column set (`dedup_epoch_groups`) and a node's
    /// incoming lineage edges (`WriteMode::ReplaceColumnLineage`).
    LatestGroup(&'static str),
}

/// A relation over epoch parquet: one glob, plus how its rows supersede.
///
/// `dir` is relative to the metadata directory, and is either an epoch
/// *directory* (globbed over `v<n>_*.parquet`) or a single snapshot file.
#[derive(Debug, Clone, Copy)]
pub struct EpochRelation {
    /// View name, created in the `dbt_internal` schema under an `epoch_` prefix
    /// so the generated file's public views read as plain projections.
    pub view: &'static str,
    /// Path under the metadata directory: `parse/nodes` or `parse/alive.parquet`.
    pub dir: &'static str,
    /// True when `dir` names one file rather than an epoch directory.
    pub single_file: bool,
    pub supersede: Supersede,
}

/// Every epoch relation the view layer reads, in the order the generated file
/// declares them. One entry per reader in `cold_ingest`, so a source the ingest
/// consumes and the view layer does not is visible as an absence here.
pub const EPOCH_RELATIONS: &[EpochRelation] = &[
    EpochRelation {
        view: "epoch_parse_nodes",
        dir: PARSE_NODES_SUBDIR,
        single_file: false,
        supersede: Supersede::LatestBy("unique_id"),
    },
    EpochRelation {
        view: "epoch_parse_columns",
        dir: PARSE_COLUMNS_SUBDIR,
        single_file: false,
        // A node's column set is republished whole, so the newest epoch that
        // mentions a node replaces every column row it had.
        supersede: Supersede::LatestGroup("unique_id"),
    },
    EpochRelation {
        view: "epoch_parse_alive",
        dir: PARSE_ALIVE,
        single_file: true,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_parse_project",
        dir: PARSE_PROJECT,
        single_file: true,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_parse_resolver_state",
        dir: PARSE_RESOLVER_STATE,
        single_file: true,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_parse_generation",
        dir: PARSE_GENERATION,
        single_file: true,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_compile_nodes",
        dir: COMPILE_NODES_SUBDIR,
        single_file: false,
        supersede: Supersede::LatestBy("unique_id"),
    },
    EpochRelation {
        view: "epoch_compile_columns",
        dir: COMPILE_COLUMNS_SUBDIR,
        single_file: false,
        supersede: Supersede::LatestGroup("unique_id"),
    },
    EpochRelation {
        view: "epoch_compile_cll",
        dir: COMPILE_CLL_SUBDIR,
        single_file: false,
        // Column lineage is republished per *target* node — `ReplaceColumnLineage`
        // deletes every edge into a recomputed target before inserting — so the
        // whole edge set of the newest epoch that mentions a target wins.
        supersede: Supersede::LatestGroup("to_node_unique_id"),
    },
    EpochRelation {
        view: "epoch_catalog_columns",
        dir: CATALOG_COLUMNS_SUBDIR,
        single_file: false,
        supersede: Supersede::LatestGroup("unique_id"),
    },
    EpochRelation {
        view: "epoch_run_invocations",
        dir: RUN_INVOCATIONS_SUBDIR,
        single_file: false,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_run_results",
        dir: RUN_RESULTS_SUBDIR,
        single_file: false,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_run_freshness",
        dir: RUN_FRESHNESS_SUBDIR,
        single_file: false,
        supersede: Supersede::KeepAll,
    },
    EpochRelation {
        view: "epoch_run_catalog_stats",
        dir: RUN_CATALOG_STATS_SUBDIR,
        single_file: false,
        // Unlike `run_results`, this is not an invocation log: it is the
        // warehouse's current state for a relation. A re-catalogued relation
        // replaces its old row rather than appending to it, so the newest
        // epoch's row per `unique_id` wins.
        supersede: Supersede::LatestGroup("unique_id"),
    },
];

impl EpochRelation {
    /// The `read_parquet(...)` call for this relation, with `metadata_dir`
    /// absolute. DuckDB resolves globs against the process CWD and the metadata
    /// directory is relocatable (`DBT_METADATA_DIR`), so the path cannot be
    /// relative and the file cannot be a static asset.
    ///
    /// `union_by_name` because epochs written by different dbt versions may
    /// differ in column set; without it a widened schema is a read error.
    fn read_parquet(&self, metadata_dir: &Path) -> String {
        let path = metadata_dir.join(self.dir);
        let pattern = if self.single_file {
            path.display().to_string()
        } else {
            path.join(format!("{}_*.parquet", epoch_layers::SCHEMA_VERSION))
                .display()
                .to_string()
        };
        format!(
            "read_parquet('{}', union_by_name = true, filename = true)",
            pattern.replace('\'', "''")
        )
    }

    /// Whether this relation has anything to read. A glob that matches no file
    /// is a query-time error in DuckDB, so the generator omits both the relation
    /// and the views over it rather than emitting SQL that fails on use.
    pub fn has_files(&self, metadata_dir: &Path) -> bool {
        let path = metadata_dir.join(self.dir);
        if self.single_file {
            path.is_file()
        } else {
            !epoch_layers::existing_epochs(&path).is_empty()
        }
    }

    /// `CREATE OR REPLACE VIEW dbt_internal.<view> AS ...` — one statement, no
    /// trailing semicolon, so a caller can either execute it directly or write
    /// it into a `.sql` file.
    ///
    /// Includes the
    /// latest-wins wrapper when the relation supersedes by key.
    ///
    /// Precedence is by epoch number parsed out of the file name, not by
    /// `ingested_at`: the epoch number is what the ingest orders on
    /// (`write_parse_nodes` walks epochs newest-first), and two rows written in
    /// the same second would otherwise tie.
    pub fn create_view_sql(&self, metadata_dir: &Path) -> String {
        let read = self.read_parquet(metadata_dir);
        match self.supersede {
            Supersede::KeepAll => format!(
                "CREATE OR REPLACE VIEW dbt_internal.{} AS\n\
                 SELECT * EXCLUDE (filename) FROM {read}",
                self.view
            ),
            Supersede::LatestBy(key) => format!(
                "CREATE OR REPLACE VIEW dbt_internal.{} AS\n\
                 SELECT * EXCLUDE (filename, _epoch_rn) FROM (\n\
                 \x20 SELECT *, ROW_NUMBER() OVER (\n\
                 \x20   PARTITION BY {key}\n\
                 \x20   ORDER BY {EPOCH_NUMBER} DESC\n\
                 \x20 ) AS _epoch_rn\n\
                 \x20 FROM {read}\n\
                 ) WHERE _epoch_rn = 1",
                self.view
            ),
            // `max(epoch) OVER (PARTITION BY key)` rather than a row number: the
            // filter has to keep *every* row of the winning epoch, and a row
            // number would keep one.
            Supersede::LatestGroup(key) => format!(
                "CREATE OR REPLACE VIEW dbt_internal.{} AS\n\
                 SELECT * EXCLUDE (filename, _epoch, _group_epoch) FROM (\n\
                 \x20 SELECT *, {EPOCH_NUMBER} AS _epoch,\n\
                 \x20   max({EPOCH_NUMBER}) OVER (PARTITION BY {key}) AS _group_epoch\n\
                 \x20 FROM {read}\n\
                 ) WHERE _epoch = _group_epoch",
                self.view
            ),
        }
    }
}

/// The epoch number of the file a row came from, out of `filename`. Ordering on
/// this rather than on the string keeps `v1_10` above `v1_9`.
const EPOCH_NUMBER: &str = r"CAST(regexp_extract(filename, '_([0-9]+)\.parquet$', 1) AS BIGINT)";

/// A second relation joined in to supply columns the base relation lacks.
///
/// Always a LEFT join and always many-to-one: the Rust path's equivalent is a
/// `HashMap` lookup that either hits or leaves the fields `None`, so a join that
/// could match twice would produce rows the Rust path cannot.
#[derive(Debug, Clone, Copy)]
pub struct EpochJoin {
    /// An [`EPOCH_RELATIONS`] view name.
    pub view: &'static str,
    /// Column present in both relations, joined on equality. The map key in the
    /// Rust path.
    pub on: &'static str,
}

/// A list inside one base row, expanded to one output row per element.
///
/// The Rust path's equivalent is a `for` loop inside the pass over the nodes,
/// pushing a row per element of a list it read out of the node's payload. In SQL
/// that is a lateral `unnest`, which multiplies the base row rather than
/// filtering it — so a base row whose list is empty or null contributes no
/// output rows at all, exactly as a loop over an empty vector does.
#[derive(Debug, Clone, Copy)]
pub struct Unnest {
    /// SQL producing the list, qualified with [`BASE`]. Either a real list column
    /// of the relation or a `json_extract(.., '$..[*]')`, which yields `JSON[]`.
    pub list: &'static str,
    /// Predicate on the element, qualified with [`ELEM`], matching the
    /// `filter_map` the Rust path applies to the same list. An element the Rust
    /// path drops has to be dropped here too, or the differential test sees an
    /// extra row.
    pub keep: Option<&'static str>,
}

/// Where a staging table's rows come from in the epoch files.
#[derive(Debug, Clone, Copy)]
pub enum Origin {
    /// One row per row of `view`, columns given by [`epoch_expr`].
    Relation {
        /// An [`EPOCH_RELATIONS`] view name.
        view: &'static str,
        /// Row filter, matching the `continue`s in the Rust row builder. A row
        /// the ingest skips has to be skipped here too or the differential test
        /// sees an extra row.
        keep: Option<&'static str>,
        /// Column to match against `parse/alive.unique_id`, keeping only live
        /// nodes — invariant 4 of the epoch architecture, replacing
        /// `prune_by_alive`. `None` for sources that are not node-scoped.
        alive_on: Option<&'static str>,
        /// Second relation supplying [`EpochExpr::JoinCol`] columns.
        join: Option<EpochJoin>,
        /// List to expand, for the tables with many rows per base row.
        unnest: Option<Unnest>,
    },
    /// Assembled by bespoke SQL in `epoch_views.rs` — the tables the Rust path
    /// also assembles rather than projects (`Src::Own`), plus the ones whose
    /// source is a single-row snapshot read field by field.
    Custom,
    /// No epoch source at all. The Rust path never writes this staging table, so
    /// the view is a typed empty relation: the shape is published, the data does
    /// not exist yet.
    Empty,
}

/// One of a semantic model's `entities`, `measures` or `dimensions` lists,
/// expanded to one row per element.
///
/// A macro rather than a function because `concat!` needs the list name as a
/// literal, the JSON path being built at compile time like every other path in
/// this module. Expands inside [`origin`], whose `node_unnest` it calls.
macro_rules! semantic_sub {
    ($list:literal) => {
        node_unnest(
            Some("t.resource_type = 'semantic_model'"),
            Unnest {
                list: concat!(
                    "json_extract(t.payload, '$.__semantic_model_attr__.",
                    $list,
                    "[*]')"
                ),
                // `.get("name").and_then(|v| v.as_str())?` — absent, null, or
                // present but not a string all drop the element, which is what
                // `json_type` distinguishes and `json_extract_string` would not:
                // it renders a numeric name as text rather than yielding NULL.
                keep: Some("json_type(u.e, '$.name') = 'VARCHAR'"),
            },
        )
    };
}

/// Origin of every staging table the information schema reads. Exhaustive over
/// the tables [`referenced_pairs`] can name; an unlisted table is a bug the
/// totality test reports rather than a silent empty view.
pub fn origin(staging_table: &str) -> Option<Origin> {
    use Origin::{Custom, Empty, Relation};
    // One row per `parse/nodes` row that `keep` admits, pruned by liveness.
    //
    // Every node-derived table is built inside `extract_parse_nodes_batch`'s one
    // pass over the nodes, under a `match rt` or an `if rt ==`. That resource-type
    // test is the row filter, and it has to be restated here: without it the view
    // would publish, say, every node as a macro.
    const fn node_derived(keep: &'static str) -> Origin {
        Relation {
            view: "epoch_parse_nodes",
            keep: Some(keep),
            alive_on: Some("unique_id"),
            join: None,
            unnest: None,
        }
    }
    /// [`node_derived`], one output row per element of a list inside the node.
    /// `keep` is `None` where the list itself is the only filter — `depends_on`
    /// is carried by every resource type, so `dbt.edges` selects no subset.
    const fn node_unnest(keep: Option<&'static str>, unnest: Unnest) -> Origin {
        Relation {
            view: "epoch_parse_nodes",
            keep,
            alive_on: Some("unique_id"),
            join: None,
            unnest: Some(unnest),
        }
    }
    Some(match staging_table {
        // `dbt.nodes` is the one table assembled from two epoch sources: the
        // compiled code, table role, grains and classifiers are looked up per
        // node in `load_compile_nodes_map`, which is a left join by any other
        // name.
        "nodes" => Relation {
            view: "epoch_parse_nodes",
            keep: None,
            alive_on: Some("unique_id"),
            join: Some(EpochJoin {
                view: "epoch_compile_nodes",
                on: "unique_id",
            }),
            unnest: None,
        },
        // ── parse/nodes, one row per node ────────────────────────────────
        "macros" => node_derived("t.resource_type = 'macro'"),
        "docs" => node_derived("t.resource_type = 'docs_macro'"),
        "groups" => node_derived("t.resource_type = 'group'"),
        "exposures" => node_derived("t.resource_type = 'exposure'"),
        "metrics" => node_derived("t.resource_type = 'metric'"),
        "unit_tests" => node_derived("t.resource_type = 'unit_test'"),
        "saved_queries" => node_derived("t.resource_type = 'saved_query'"),
        "semantic_models" => node_derived("t.resource_type = 'semantic_model'"),
        // A test node with no `test_metadata` in its payload gets no row here
        // (`if let Some(tm) = tm`) — which, this being the right side of a LEFT
        // join, nulls the columns rather than dropping the node.
        "test_metadata" => node_derived(concat!(
            "t.resource_type = 'test' AND ",
            test_metadata_obj!(),
            " IS NOT NULL"
        )),
        // Time spines hang off a model rather than being their own resource type.
        // `json_type` rather than `IS NOT NULL`: the field serialises as JSON
        // `null` on a model that has no time spine, and `json_extract` returns
        // that as a JSON null value — not as SQL NULL. The Rust path rejects it
        // with `.filter(|v| !v.is_null())`, and `json_type` is how SQL sees the
        // same distinction. `json_type` is itself NULL when the path is absent.
        "time_spines" => node_derived(
            "t.resource_type = 'model' \
             AND COALESCE(json_type(t.payload, '$.__model_attr__.time_spine'), 'NULL') <> 'NULL'",
        ),
        // ── parse/nodes, many rows per node ──────────────────────────────
        // One row per element of a list belonging to the node: a `for` loop in
        // the Rust pass, a lateral `unnest` here.
        //
        // `depends_on` is a real list column of the relation rather than
        // something inside the payload, so its elements are already VARCHAR and
        // need no extraction. Pruned by liveness on the *child* only, which is
        // the node the row is attached to. The Rust path's delta ingest
        // additionally drops an edge whose parent is not alive; its full ingest,
        // the one a conversion from scratch always takes, does not. The two agree
        // whenever the epoch state is consistent — a node that still refers to a
        // deleted node would not have re-parsed — and restating the full path is
        // what keeps this view substitutable for the parquet the same run writes.
        "edges" => node_unnest(
            None,
            Unnest {
                list: "t.depends_on",
                keep: None,
            },
        ),
        // The three lists inside a semantic model's payload. Each element is a
        // JSON object read through the `Elem*` variants, and each list is
        // `filter_map`ped on a string `name` — an unnamed element is not a row.
        "semantic_entities" => semantic_sub!("entities"),
        "semantic_measures" => semantic_sub!("measures"),
        "semantic_dimensions" => semantic_sub!("dimensions"),
        // ── other epoch sources ──────────────────────────────────────────
        "node_columns" => Custom,
        // Alive-pruned on the *target* node, as `write_compile_cll` does.
        "column_lineage" => Relation {
            view: "epoch_compile_cll",
            keep: None,
            alive_on: Some("to_node_unique_id"),
            join: None,
            unnest: None,
        },
        "invocations" => Relation {
            view: "epoch_run_invocations",
            keep: Some("t.invocation_id IS NOT NULL"),
            alive_on: None,
            join: None,
            unnest: None,
        },
        // Both keys are `let Some(..) else { continue }` in `write_run_results`.
        "run_results" => Relation {
            view: "epoch_run_results",
            keep: Some("t.unique_id IS NOT NULL AND t.invocation_id IS NOT NULL"),
            alive_on: None,
            join: None,
            unnest: None,
        },
        "source_freshness" => Relation {
            view: "epoch_run_freshness",
            keep: Some("t.unique_id IS NOT NULL"),
            alive_on: None,
            join: None,
            unnest: None,
        },
        // Single-row snapshots read field by field, with fallbacks.
        "project" | "project_vars" | "project_env_vars" | "packages" => Custom,
        // ── written by nothing ───────────────────────────────────────────
        // `semantic_relationships` has a schema and spec but no writer in
        // `metadata_to_parquet.rs`; `diagnostics`, `adapter_queries` and
        // `node_input_files` have no epoch source at all.
        "semantic_relationships" | "diagnostics" | "adapter_queries" | "node_input_files" => Empty,
        _ => return None,
    })
}

/// `dbt_rt.run_results` ← `run/results`. Straight passthrough apart from four
/// columns the Rust path hardcodes to `None`.
const RUN_RESULTS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("invocation_id", EpochExpr::Same),
    ("status", EpochExpr::Same),
    ("execution_time", EpochExpr::Same),
    ("thread_id", EpochExpr::Same),
    ("message", EpochExpr::Same),
    ("failures", EpochExpr::Same),
    // `compiled`, `batch_results` and `rows_affected` are not in the epoch
    // payload; `write_run_results` sets all three to `None`.
    ("compiled", EpochExpr::Null),
    ("batch_results", EpochExpr::Null),
    ("rows_affected", EpochExpr::Null),
    ("compiled_code_hash", EpochExpr::Same),
    ("relation_name", EpochExpr::Same),
    ("adapter_response", EpochExpr::Same),
    ("timing", EpochExpr::Same),
    ("created_at", EpochExpr::Col("ingested_at")),
];

/// `dbt_rt.invocations` ← `run/invocations`.
const INVOCATIONS: &[(&str, EpochExpr)] = &[
    ("invocation_id", EpochExpr::Same),
    ("command", EpochExpr::Same),
    ("selector", EpochExpr::Same),
    // `write_run_invocations` uses `unwrap_or_default()` here, so an absent
    // version is the empty string rather than null.
    ("dbt_version", EpochExpr::Sql("COALESCE(dbt_version, '')")),
    ("generated_at", EpochExpr::Col("ingested_at")),
    ("elapsed_time", EpochExpr::Col("elapsed_secs")),
    ("node_count", EpochExpr::Same),
    ("target_name", EpochExpr::Same),
    ("target_type", EpochExpr::Col("adapter_type")),
    ("git_sha", EpochExpr::Same),
    ("git_branch", EpochExpr::Same),
    // Stored as an int in the epoch, boolean in the index.
    ("git_is_dirty", EpochExpr::Sql("git_is_dirty = 1")),
    // Not carried in the epoch: `write_run_invocations` sets each to `None`.
    ("args", EpochExpr::Null),
    ("target_database", EpochExpr::Null),
    ("target_schema", EpochExpr::Null),
    ("target_threads", EpochExpr::Null),
    ("vars_override", EpochExpr::Null),
];

/// `dbt.source_freshness` ← `run/freshness`, published as `dbt_rt.freshness`.
///
/// The staging table keeps its name; only the published one is `freshness`. Rows are
/// sources *and* models, told apart by `resource_type`.
const SOURCE_FRESHNESS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("resource_type", EpochExpr::Same),
    ("invocation_id", EpochExpr::Same),
    ("status", EpochExpr::Same),
    ("max_loaded_at", EpochExpr::Same),
    ("snapshotted_at", EpochExpr::Same),
    ("max_loaded_at_time_ago", EpochExpr::Same),
    ("execution_time", EpochExpr::Same),
    ("warn_after_count", EpochExpr::Same),
    ("warn_after_period", EpochExpr::Same),
    ("error_after_count", EpochExpr::Same),
    ("error_after_period", EpochExpr::Same),
    ("ingested_at", EpochExpr::Same),
    // Declared in the staging schema, never written by `write_run_freshness`.
    ("adapter_response", EpochExpr::Null),
    ("created_at", EpochExpr::Null),
    ("error", EpochExpr::Null),
    ("freshness_filter", EpochExpr::Null),
    ("thread_id", EpochExpr::Null),
    ("timing", EpochExpr::Null),
];

/// `dbt.column_lineage` ← `compile/column_lineage`. `project_cll_batches` is a
/// pure column rename-free projection, so every column keeps its name.
const COLUMN_LINEAGE: &[(&str, EpochExpr)] = &[
    ("from_node_unique_id", EpochExpr::Same),
    ("from_column_name", EpochExpr::Same),
    ("to_node_unique_id", EpochExpr::Same),
    ("to_column_name", EpochExpr::Same),
    ("lineage_kind", EpochExpr::Same),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.nodes` ← `parse/nodes` LEFT JOIN `compile/nodes`, the widest and most
/// hand-written of the row builders (`extract_parse_nodes_batch` plus the
/// `OwnedNodeRow` → `NodeRow` conversion).
///
/// Three groups: columns the epoch parquet carries under another name, columns
/// dug out of the JSON payload, and columns the Rust conversion hardcodes. The
/// last group is the majority — most of `NodeRow` is `None` at the conversion,
/// not for want of data in the payload but because nothing reads it yet.
const NODES: &[(&str, EpochExpr)] = &[
    // ── carried by the epoch parquet ─────────────────────────────────────
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::Same),
    ("resource_type", EpochExpr::Same),
    ("package_name", EpochExpr::Same),
    ("original_file_path", EpochExpr::Col("original_path")),
    ("alias", EpochExpr::Same),
    ("description", EpochExpr::Same),
    ("relation_name", EpochExpr::Same),
    (
        "identifier",
        EpochExpr::Sql(
            "COALESCE(t.identifier, \
             json_extract_string(t.payload, '$.__source_attr__.identifier'), t.alias)",
        ),
    ),
    ("materialized", EpochExpr::Col("materialization")),
    (
        "access_level",
        EpochExpr::Sql(
            "COALESCE(t.access, json_extract_string(t.payload, '$.__model_attr__.access'))",
        ),
    ),
    (
        "group_name",
        EpochExpr::Sql(
            "COALESCE(t.group_name, json_extract_string(t.payload, '$.__model_attr__.group'))",
        ),
    ),
    // `unwrap_or_default()` on a `&str`: absent is the empty string, not null.
    (
        "database_name",
        EpochExpr::Sql("COALESCE(t.\"database\", '')"),
    ),
    ("schema_name", EpochExpr::Sql("COALESCE(t.\"schema\", '')")),
    // Non-nullable lists: `get_list` yields `vec![]` for a null column.
    ("fqn", EpochExpr::Sql("COALESCE(t.fqn, [])")),
    ("tags", EpochExpr::Sql("COALESCE(t.tags, [])")),
    // Stored inverted, and a missing value means enabled.
    (
        "enabled",
        EpochExpr::Sql("COALESCE(t.is_disabled = 0, TRUE)"),
    ),
    // Column first, payload second — the order of the `or_else` in the builder.
    (
        "source_name",
        EpochExpr::Sql(
            "COALESCE(t.source_name, json_extract_string(t.payload, '$.__source_attr__.source_name'), \
             json_extract_string(t.payload, '$.source_name'))",
        ),
    ),
    ("ingested_at", EpochExpr::Same),
    // ── dug out of the JSON payload ──────────────────────────────────────
    (
        "checksum",
        EpochExpr::JsonFirst(
            &["__common_attr__.checksum.checksum", "checksum.checksum"],
            "NULL",
        ),
    ),
    (
        "raw_code",
        EpochExpr::JsonFirst(&["__common_attr__.raw_code", "raw_code"], "NULL"),
    ),
    (
        "source_description",
        EpochExpr::JsonFirst(
            &["__source_attr__.source_description", "source_description"],
            "NULL",
        ),
    ),
    (
        "loader",
        EpochExpr::JsonFirst(&["__source_attr__.loader", "loader"], "NULL"),
    ),
    (
        "loaded_at_field",
        EpochExpr::JsonFirst(
            &["__source_attr__.loaded_at_field", "loaded_at_field"],
            "NULL",
        ),
    ),
    (
        "patch_path",
        EpochExpr::JsonFirst(&["__common_attr__.patch_path", "patch_path"], "NULL"),
    ),
    (
        "deprecation_date",
        EpochExpr::JsonFirst(
            &["__model_attr__.deprecation_date", "deprecation_date"],
            "NULL",
        ),
    ),
    (
        "meta",
        EpochExpr::SqlJson(
            "COALESCE(json_extract(t.payload, '$.__common_attr__.meta'), \
             json_extract(t.payload, '$.meta'))",
        ),
    ),
    (
        "version",
        EpochExpr::SqlJson(
            "COALESCE(json_extract(t.payload, '$.__model_attr__.version'), \
             json_extract(t.payload, '$.version'))",
        ),
    ),
    (
        "latest_version",
        EpochExpr::SqlJson(
            "COALESCE(json_extract(t.payload, '$.__model_attr__.latest_version'), \
             json_extract(t.payload, '$.latest_version'))",
        ),
    ),
    // `config` survives the model trim, but as a *raw JSON string* rather than
    // a parsed object, and the builder then serialises whatever it holds — so a
    // model's `config` is the JSON encoding of a string containing JSON, one
    // level deeper than every other node's. `to_json` of the text reproduces
    // that; the `ELSE` branch is the ordinary parsed-object form.
    (
        "config",
        EpochExpr::SqlJson(
            "CASE WHEN t.resource_type = 'model' \
             THEN to_json(CAST(json_extract(t.payload, '$.config') AS VARCHAR)) \
             ELSE json_extract(t.payload, '$.config') END",
        ),
    ),
    // Three payload shapes in `or_else` order, the first two of which survive
    // the model trim. `unwrap_or(false)`, so absent is false rather than null.
    //
    // The COALESCE is over the three `contract` *objects*, with one read of
    // `enforced` after it — not over three `enforced` values. The Rust path
    // commits to the first contract it finds, so a `contract: {}` on the config
    // of a model whose `__model_attr__.contract.enforced` is true yields false;
    // coalescing the values would fall through to the attr and yield true.
    (
        "contract_enforced",
        EpochExpr::Sql(
            "COALESCE(CAST(json_extract(COALESCE(\
             json_extract(t.payload, '$.config.contract'), \
             json_extract(t.payload, '$.__model_attr__.contract'), \
             json_extract(t.payload, '$.contract')), '$.enforced') AS BOOLEAN), \
             FALSE)",
        ),
    ),
    // Read from `__model_attr__`, which only model payloads carry; for every
    // other resource type the path is absent and the column is an empty list.
    (
        "primary_key",
        EpochExpr::Sql(
            "COALESCE(json_extract_string(t.payload, \
             '$.__model_attr__.primary_key[*]'), [])",
        ),
    ),
    // Not read by any output table, but the staging schema declares it and the
    // builder fills it, so leaving it unmapped would be a hole in the totality
    // test rather than a decision.
    (
        "file_path",
        EpochExpr::Sql(
            "CASE WHEN t.resource_type = 'model' THEN '' \
             ELSE COALESCE(json_extract_string(t.payload, \
             '$.__common_attr__.path'), '') END",
        ),
    ),
    // ── from compile/nodes, via the left join ────────────────────────────
    ("compiled_code", EpochExpr::JoinCol("compiled_code")),
    (
        "compiled_code_hash",
        EpochExpr::JoinCol("compiled_code_hash"),
    ),
    ("compiled_path", EpochExpr::JoinCol("compiled_path")),
    ("table_role", EpochExpr::JoinCol("table_role")),
    ("grain", EpochExpr::JoinJsonList("grain")),
    ("grain_declared", EpochExpr::JoinJsonList("grain_declared")),
    ("grain_tested", EpochExpr::JoinJsonList("grain_tested")),
    ("classifiers", EpochExpr::JoinJsonList("classifiers")),
    // ── hardcoded by the conversion ──────────────────────────────────────
    // Non-nullable and always `true` / `vec![]` in `NodeRow`.
    ("docs_show", EpochExpr::True),
    ("grain_inferred", EpochExpr::EmptyList),
    // Declared in the staging schema, never populated: the payload has the data
    // for most of these, but nothing reads it into `OwnedNodeRow`.
    ("node_language", EpochExpr::Json("__common_attr__.language")),
    ("incremental_strategy", EpochExpr::Null),
    ("on_schema_change", EpochExpr::Null),
    ("unique_key", EpochExpr::Null),
    ("full_refresh", EpochExpr::Null),
    ("persist_docs", EpochExpr::Null),
    ("pre_hook", EpochExpr::Null),
    ("post_hook", EpochExpr::Null),
    ("grants", EpochExpr::Null),
    ("node_constraints", EpochExpr::Null),
    ("time_spine", EpochExpr::Null),
    ("ai_context", EpochExpr::Null),
    ("loaded_at_query", EpochExpr::Null),
    ("freshness", EpochExpr::Null),
    ("external_config", EpochExpr::Null),
    ("source_meta", EpochExpr::Null),
    ("quoting", EpochExpr::Null),
    ("extra_ctes", EpochExpr::Null),
    ("compiled_at", EpochExpr::Null),
    ("raw_code_hash", EpochExpr::Null),
    ("search_text", EpochExpr::Null),
];

/// `dbt.edges` ← one row per element of a node's `depends_on`.
///
/// The node is the *child*: the loop is `for parent in &deps`, pushing an edge
/// from each dependency to the node it belongs to. `edge_type` is the literal
/// `'ref'` the Rust path writes for every edge — `depends_on` does not record
/// which kind of reference produced it, so there is nothing to distinguish.
const EDGES: &[(&str, EpochExpr)] = &[
    ("parent_unique_id", EpochExpr::Elem),
    ("child_unique_id", EpochExpr::Col("unique_id")),
    ("edge_type", EpochExpr::Sql("'ref'")),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.semantic_entities` ← one row per `__semantic_model_attr__.entities`
/// element.
///
/// `entity_role` is the element's `role`, not `entity_role`: the staging column
/// was named for what it means rather than for the payload key. `config` has no
/// source — the row builder never sets it — so it is null on both paths.
const SEMANTIC_ENTITIES: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::ElemJson("name")),
    (
        "entity_type",
        EpochExpr::ElemJsonFirst(&["entity_type", "type"], "'unknown'"),
    ),
    ("description", EpochExpr::ElemJson("description")),
    ("label", EpochExpr::ElemJson("label")),
    ("entity_role", EpochExpr::ElemJson("role")),
    ("expr", EpochExpr::ElemJson("expr")),
    ("config", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.semantic_measures` ← one row per `__semantic_model_attr__.measures`
/// element.
///
/// `agg` defaults to `'sum'` and `create_metric` to false, both from the Rust
/// path's `unwrap_or`, so an absent key is a value rather than a null.
const SEMANTIC_MEASURES: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::ElemJson("name")),
    ("agg", EpochExpr::ElemJsonFirst(&["agg"], "'sum'")),
    ("description", EpochExpr::ElemJson("description")),
    ("label", EpochExpr::ElemJson("label")),
    ("expr", EpochExpr::ElemJson("expr")),
    (
        "create_metric",
        EpochExpr::ElemJsonFirst(&["create_metric"], "FALSE"),
    ),
    (
        "agg_time_dimension",
        EpochExpr::ElemJson("agg_time_dimension"),
    ),
    ("agg_params", EpochExpr::ElemJsonRaw("agg_params")),
    (
        "non_additive_dimension",
        EpochExpr::ElemJsonRaw("non_additive_dimension"),
    ),
    ("config", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.semantic_dimensions` ← one row per `__semantic_model_attr__.dimensions`
/// element.
///
/// `time_granularity` is nested one level further, under the element's
/// `type_params`, unlike every other column here.
const SEMANTIC_DIMENSIONS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::ElemJson("name")),
    (
        "dimension_type",
        EpochExpr::ElemJsonFirst(&["dimension_type", "type"], "'categorical'"),
    ),
    ("description", EpochExpr::ElemJson("description")),
    ("label", EpochExpr::ElemJson("label")),
    ("expr", EpochExpr::ElemJson("expr")),
    (
        "is_partition",
        EpochExpr::ElemJsonFirst(&["is_partition"], "FALSE"),
    ),
    (
        "time_granularity",
        EpochExpr::ElemJson("type_params.time_granularity"),
    ),
    ("validity_params", EpochExpr::ElemJsonRaw("validity_params")),
    ("config", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.docs` ← `parse/nodes` where `resource_type = 'docs_macro'`.
///
/// `DbtDocsMacro` serialises flat — no `__common_attr__` — so the payload keys
/// are read at the top level. `name` and `package_name` come from the payload
/// rather than from the parse/nodes columns of the same name, because
/// [`ParsedDoc`](crate::ingest::payload::ParsedDoc) reads the payload; the two
/// agree in practice but this mapping states what the Rust path does, not what it
/// could have done.
const DOCS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::JsonOr("name", "''")),
    ("package_name", EpochExpr::JsonOr("package_name", "''")),
    (
        "original_file_path",
        EpochExpr::JsonOr("original_file_path", "''"),
    ),
    ("block_contents", EpochExpr::Json("block_contents")),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.groups` ← `parse/nodes` where `resource_type = 'group'`.
const GROUPS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::JsonOr("__common_attr__.name", "''")),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    // `build_group_from_payload` does not set it, so the staging column — which
    // exists — is null on every row.
    ("package_name", EpochExpr::Null),
    ("original_file_path", EpochExpr::Null),
    ("config", EpochExpr::Null),
    ("owner_name", EpochExpr::Json("__group_attr__.owner.name")),
    ("owner_email", EpochExpr::Json("__group_attr__.owner.email")),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.macros` ← `parse/nodes` where `resource_type = 'macro'`.
///
/// `DbtMacro` serialises flat, like `DbtDocsMacro` above.
const MACROS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    ("name", EpochExpr::JsonOr("name", "''")),
    ("package_name", EpochExpr::JsonOr("package_name", "''")),
    (
        "original_file_path",
        EpochExpr::JsonOr("original_file_path", "''"),
    ),
    // Both are `Option<String>` narrowed to `""` at the row rather than at the
    // parse, so absent and empty are the same value here.
    ("macro_sql", EpochExpr::JsonOr("macro_sql", "''")),
    ("description", EpochExpr::JsonOr("description", "''")),
    (
        "depends_on_macros",
        EpochExpr::JsonList("depends_on.macros"),
    ),
    // `arguments` with `args` as the fallback name.
    (
        "arguments",
        EpochExpr::SqlJson(
            "COALESCE(json_extract(t.payload, '$.arguments'), \
             json_extract(t.payload, '$.args'))",
        ),
    ),
    ("docs_show", EpochExpr::JsonOr("docs.show", "TRUE")),
    ("patch_path", EpochExpr::Json("patch_path")),
    ("meta", EpochExpr::JsonRaw("meta")),
    ("created_at", EpochExpr::Json("created_at")),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.data_tests`'s generic-test half ← `parse/nodes` where
/// `resource_type = 'test'` and the payload carries `test_metadata`.
///
/// The severity and threshold columns come from `deprecated_config`, not from the
/// `test_metadata` object — but they are still guarded by its presence, because
/// the Rust path only ever emits this table's row inside `if let Some(tm)`.
const TEST_METADATA: &[(&str, EpochExpr)] = &[
    (
        "test_name",
        EpochExpr::Sql(concat!(
            "json_extract_string(",
            test_metadata_obj!(),
            ", '$.name')"
        )),
    ),
    (
        "test_namespace",
        EpochExpr::Sql(concat!(
            "json_extract_string(",
            test_metadata_obj!(),
            ", '$.namespace')"
        )),
    ),
    (
        "kwargs",
        EpochExpr::SqlJson(concat!(
            "json_extract(",
            test_metadata_obj!(),
            ", '$.kwargs')"
        )),
    ),
    ("column_name", EpochExpr::Json("__test_attr__.column_name")),
    // `file_key_name` is the last fallback the Rust path tries, so it is the last
    // branch here too.
    (
        "attached_node",
        EpochExpr::Sql(
            "COALESCE(json_extract_string(t.payload, '$.__test_attr__.attached_node'), \
             json_extract_string(t.payload, '$.attached_node'), \
             json_extract_string(t.payload, '$.file_key_name'))",
        ),
    ),
    (
        "severity",
        EpochExpr::Sql(
            "COALESCE(json_extract_string(t.payload, '$.deprecated_config.severity'), \
             json_extract_string(t.payload, '$.config.severity'))",
        ),
    ),
    ("warn_if", EpochExpr::Json("deprecated_config.warn_if")),
    ("error_if", EpochExpr::Json("deprecated_config.error_if")),
    ("fail_calc", EpochExpr::Json("deprecated_config.fail_calc")),
    (
        "store_failures",
        EpochExpr::Json("deprecated_config.store_failures"),
    ),
    (
        "store_failures_as",
        EpochExpr::Json("deprecated_config.store_failures_as"),
    ),
    // The staging schema has both, and `test_meta_rows` sets neither.
    ("test_limit", EpochExpr::Null),
    ("test_where", EpochExpr::Null),
];

/// `dbt.unit_tests` ← `parse/nodes` where `resource_type = 'unit_test'`.
const UNIT_TESTS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    (
        "name",
        EpochExpr::JsonFirst(&["__common_attr__.name", "name"], "''"),
    ),
    ("model", EpochExpr::JsonOr("__unit_test_attr__.model", "''")),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    (
        "package_name",
        EpochExpr::JsonOr("__common_attr__.package_name", "''"),
    ),
    (
        "original_file_path",
        EpochExpr::JsonOr("__common_attr__.original_file_path", "''"),
    ),
    ("fqn", EpochExpr::JsonList("__base_attr__.fqn")),
    ("given", EpochExpr::JsonRaw("__unit_test_attr__.given")),
    ("expect", EpochExpr::JsonRaw("__unit_test_attr__.expect")),
    (
        "overrides",
        EpochExpr::JsonRaw("__unit_test_attr__.overrides"),
    ),
    (
        "versions",
        EpochExpr::JsonRaw("__unit_test_attr__.versions"),
    ),
    // `unit_test_rows` sets neither, though the staging schema has both.
    ("config", EpochExpr::Null),
    ("created_at", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.time_spines` ← `__model_attr__.time_spine` of a model node.
///
/// The Rust path reads the *time spine object* rather than the payload, so every
/// path here is relative to it.
///
/// This is the table the differential test earned its keep on: the model trim
/// used to keep only `__model_attr__.contract`, so `time_spine` was gone by the
/// time `extract_parse_nodes_batch` looked for it and the materialized table was
/// unconditionally empty. Rather than reproduce that with a `keep` of `FALSE` —
/// publishing a table known to have no rows — the view layer reads the untrimmed
/// payload, and the first corpus with a time spine in it failed the comparison
/// and pointed at the trim.
const TIME_SPINES: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    // `primary_column` is either an object with a `name` or the name itself.
    (
        "primary_column",
        EpochExpr::JsonFirst(
            &[
                "__model_attr__.time_spine.primary_column.name",
                "__model_attr__.time_spine.primary_column",
            ],
            "NULL",
        ),
    ),
    (
        "primary_granularity",
        EpochExpr::Json("__model_attr__.time_spine.primary_column.time_granularity"),
    ),
    (
        "custom_granularities",
        EpochExpr::JsonRaw("__model_attr__.time_spine.custom_granularities"),
    ),
    (
        "node_relation",
        EpochExpr::JsonRaw("__model_attr__.time_spine.node_relation"),
    ),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.exposures` ← `parse/nodes` where `resource_type = 'exposure'`.
///
/// `tags` is the only column not read from the payload: `build_exposure_from_
/// payload` is handed the node's `tags` *column*, so this reads it too.
const EXPOSURES: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    (
        "name",
        EpochExpr::JsonFirst(&["__common_attr__.name", "name"], "''"),
    ),
    (
        "exposure_type",
        EpochExpr::JsonOr("__exposure_attr__.type", "'dashboard'"),
    ),
    ("label", EpochExpr::Json("__exposure_attr__.label")),
    (
        "owner_name",
        EpochExpr::Json("__exposure_attr__.owner.name"),
    ),
    (
        "owner_email",
        EpochExpr::Json("__exposure_attr__.owner.email"),
    ),
    ("url", EpochExpr::Json("__exposure_attr__.url")),
    ("maturity", EpochExpr::Json("__exposure_attr__.maturity")),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    (
        "package_name",
        EpochExpr::JsonOr("__common_attr__.package_name", "''"),
    ),
    (
        "original_file_path",
        EpochExpr::JsonOr("__common_attr__.original_file_path", "''"),
    ),
    ("fqn", EpochExpr::JsonList("__base_attr__.fqn")),
    ("depends_on_nodes", EpochExpr::JsonList("depends_on.nodes")),
    ("tags", EpochExpr::Sql("COALESCE(t.tags, [])")),
    (
        "created_at",
        EpochExpr::Json("__exposure_attr__.created_at"),
    ),
    // `exposure_rows` sets neither, though the staging schema has both.
    ("config", EpochExpr::Null),
    ("meta", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.saved_queries` ← `parse/nodes` where `resource_type = 'saved_query'`.
const SAVED_QUERIES: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    (
        "name",
        EpochExpr::JsonFirst(&["__common_attr__.name", "name"], "''"),
    ),
    ("label", EpochExpr::Json("__saved_query_attr__.label")),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    (
        "package_name",
        EpochExpr::JsonOr("__common_attr__.package_name", "''"),
    ),
    (
        "original_file_path",
        EpochExpr::JsonOr("__common_attr__.original_file_path", "''"),
    ),
    ("fqn", EpochExpr::JsonList("__base_attr__.fqn")),
    (
        "query_params",
        EpochExpr::JsonRaw("__saved_query_attr__.query_params"),
    ),
    (
        "exports",
        EpochExpr::JsonRaw("__saved_query_attr__.exports"),
    ),
    ("depends_on_nodes", EpochExpr::JsonList("depends_on.nodes")),
    // Unlike every other node type here, a saved query's tags come from its
    // payload rather than from the node's `tags` column.
    (
        "tags",
        EpochExpr::Sql(
            "COALESCE(json_extract_string(t.payload, '$.config.tags[*]'), \
             json_extract_string(t.payload, '$.tags[*]'), [])",
        ),
    ),
    ("group_name", EpochExpr::Json("__saved_query_attr__.group")),
    (
        "created_at",
        EpochExpr::Json("__saved_query_attr__.created_at"),
    ),
    ("config", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.semantic_models` ← `parse/nodes` where `resource_type =
/// 'semantic_model'`.
const SEMANTIC_MODELS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    // No default here, unlike the other node types: `name` is `Option<String>`
    // on `ParsedSemanticModel` rather than narrowed to `""`.
    (
        "name",
        EpochExpr::JsonFirst(&["__common_attr__.name", "name"], "NULL"),
    ),
    (
        "model",
        EpochExpr::JsonOr("__semantic_model_attr__.model", "''"),
    ),
    ("label", EpochExpr::Json("__semantic_model_attr__.label")),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    ("fqn", EpochExpr::JsonList("__base_attr__.fqn")),
    (
        "node_relation",
        EpochExpr::JsonRaw("__semantic_model_attr__.node_relation"),
    ),
    (
        "primary_entity",
        EpochExpr::Json("__semantic_model_attr__.primary_entity"),
    ),
    (
        "defaults",
        EpochExpr::JsonRaw("__semantic_model_attr__.defaults"),
    ),
    (
        "group_name",
        EpochExpr::Json("__semantic_model_attr__.group"),
    ),
    (
        "created_at",
        EpochExpr::Json("__semantic_model_attr__.created_at"),
    ),
    // `sm_rows` sets none of the three, though the staging schema has them.
    ("package_name", EpochExpr::Null),
    ("original_file_path", EpochExpr::Null),
    ("config", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// `dbt.metrics` ← `parse/nodes` where `resource_type = 'metric'`.
const METRICS: &[(&str, EpochExpr)] = &[
    ("unique_id", EpochExpr::Same),
    // `build_metric_from_payload` falls back to the node's `name` *column* when
    // the payload's name is empty, which is the one place a `__metric_attr__`
    // reader crosses back to the parse/nodes columns. `NULLIF` is the empty-string
    // test; the trailing `''` is `unwrap_or_default` on the column.
    (
        "name",
        EpochExpr::Sql(
            "COALESCE(NULLIF(COALESCE(\
             json_extract_string(t.payload, '$.__common_attr__.name'), \
             json_extract_string(t.payload, '$.name'), ''), ''), t.name, '')",
        ),
    ),
    ("label", EpochExpr::Json("__metric_attr__.label")),
    // Either the type name or a single-key object wrapping its parameters. The
    // object case takes the first key, which is where the two layers can disagree:
    // `serde_json`'s map is sorted and `json_keys` is in source order, so a
    // multi-key object would pick differently. dbt only ever writes one key.
    (
        "metric_type",
        EpochExpr::Sql(
            "CASE json_type(t.payload, '$.__metric_attr__.metric_type') \
             WHEN 'VARCHAR' THEN json_extract_string(t.payload, '$.__metric_attr__.metric_type') \
             WHEN 'OBJECT' THEN json_keys(t.payload, '$.__metric_attr__.metric_type')[1] END",
        ),
    ),
    (
        "description",
        EpochExpr::Json("__common_attr__.description"),
    ),
    (
        "package_name",
        EpochExpr::JsonOr("__common_attr__.package_name", "''"),
    ),
    (
        "original_file_path",
        EpochExpr::JsonOr("__common_attr__.original_file_path", "''"),
    ),
    ("fqn", EpochExpr::JsonList("__base_attr__.fqn")),
    (
        "type_params",
        EpochExpr::JsonRaw("__metric_attr__.type_params"),
    ),
    (
        "metric_filter",
        EpochExpr::JsonRaw("__metric_attr__.filter"),
    ),
    (
        "time_granularity",
        EpochExpr::Json("__metric_attr__.time_granularity"),
    ),
    // Elements are either a name or an object carrying one. `$.name` is tried
    // first, unlike the Rust path's `as_str().or_else(..)`, because
    // `json_extract_string(e, '$')` on an *object* returns its JSON text rather
    // than NULL — so leading with it would name every object element after its
    // own serialisation. A string element has no `$.name`, so the result is the
    // same for both shapes.
    (
        "input_metric_names",
        EpochExpr::Sql(
            "COALESCE(list_filter(list_transform(json_extract(COALESCE(\
             json_extract(t.payload, '$.__metric_attr__.type_params.input_measures'), \
             json_extract(t.payload, '$.__metric_attr__.type_params.input_metrics')), '$[*]'), \
             e -> COALESCE(json_extract_string(e, '$.name'), json_extract_string(e, '$'))), \
             x -> x IS NOT NULL), [])",
        ),
    ),
    ("group_name", EpochExpr::Json("__metric_attr__.group")),
    ("tags", EpochExpr::Sql("COALESCE(t.tags, [])")),
    ("meta", EpochExpr::JsonRaw("__metric_attr__.meta")),
    ("config", EpochExpr::JsonRaw("__metric_attr__.config")),
    ("created_at", EpochExpr::Json("__metric_attr__.created_at")),
    // `metric_rows` sets neither, though the staging schema has both.
    ("ai_context", EpochExpr::Null),
    ("semantic_model_name", EpochExpr::Null),
    ("ingested_at", EpochExpr::Same),
];

/// The expression for one staging column, or `None` when the mapping does not
/// cover it yet. An uncovered column means the view layer cannot publish that
/// output table; see [`super::epoch_views::unmappable_tables`].
pub fn epoch_expr(staging_table: &str, staging_column: &str) -> Option<EpochExpr> {
    let table = match staging_table {
        "run_results" => RUN_RESULTS,
        "invocations" => INVOCATIONS,
        "source_freshness" => SOURCE_FRESHNESS,
        "column_lineage" => COLUMN_LINEAGE,
        "nodes" => NODES,
        "edges" => EDGES,
        "semantic_entities" => SEMANTIC_ENTITIES,
        "semantic_measures" => SEMANTIC_MEASURES,
        "semantic_dimensions" => SEMANTIC_DIMENSIONS,
        "docs" => DOCS,
        "groups" => GROUPS,
        "macros" => MACROS,
        "test_metadata" => TEST_METADATA,
        "unit_tests" => UNIT_TESTS,
        "time_spines" => TIME_SPINES,
        "exposures" => EXPOSURES,
        "saved_queries" => SAVED_QUERIES,
        "semantic_models" => SEMANTIC_MODELS,
        "metrics" => METRICS,
        _ => return None,
    };
    table
        .iter()
        .find(|(col, _)| *col == staging_column)
        .map(|(_, expr)| *expr)
}

/// True when the staging column holds serialised JSON rather than a scalar.
///
/// The two layers serialise the same object differently. The Rust path builds a
/// `serde_json::Value` and calls `to_string()`, and `Value`'s map is a `BTreeMap`,
/// so its keys come out alphabetically; DuckDB's `json_extract` hands back the
/// source bytes, so its keys keep the order the epoch writer used. Object key
/// order carries no meaning in JSON, so the differential test compares these
/// columns parsed rather than byte for byte — the alternative would be sorting
/// keys in SQL, which DuckDB has no function for and which would make the view
/// layer's output depend on a detail of the Rust path's data structures.
pub fn is_json_text(staging_table: &str, staging_column: &str) -> bool {
    matches!(
        epoch_expr(staging_table, staging_column),
        Some(
            EpochExpr::JsonRaw(_)
                | EpochExpr::TrimmedJsonRaw(_)
                | EpochExpr::ElemJsonRaw(_)
                | EpochExpr::SqlJson(_)
        )
    )
}

/// Every `(staging_table, staging_column)` pair the information schema reads.
///
/// Resolution mirrors [`super::project::project_join`]: a joined spec resolves a
/// column against the left table first and the right only if the left has no
/// such field. Null-typed columns and `Src::Own` tables contribute nothing —
/// they have no staging column to translate.
pub fn referenced_pairs() -> Vec<(&'static str, &'static str)> {
    let mut out = Vec::new();
    for spec in super::schema::INFO_SCHEMA {
        for col in spec.cols {
            if col.ty.is_some() {
                continue;
            }
            let owner = match spec.src {
                Src::Table(t) => Some(t),
                Src::Join { left, right, .. } => {
                    if schema_for(left).field_with_name(col.src).is_ok() {
                        Some(left)
                    } else {
                        Some(right)
                    }
                }
                Src::Own => None,
            };
            if let Some(table) = owner {
                out.push((table, col.src));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    /// Every staging column the information schema reads has an epoch source.
    ///
    /// The static half of the mapping's contract: a new output column cannot be
    /// added to `INFO_SCHEMA` without saying where the view layer gets it. The
    /// differential test in `tests_epoch.rs` is the other half — it needs a real
    /// project, so it is `#[ignore]`d, and this one is what runs in CI.
    ///
    /// Tables whose rows [`Origin::Custom`] assembles are exempt per column:
    /// their SQL is written whole in `epoch_views.rs`, so there is no per-column
    /// expression to look up. [`Origin::Empty`] likewise — nothing writes the
    /// table on either path.
    #[test]
    fn every_referenced_column_is_mapped() {
        let mut missing_table: BTreeSet<&str> = BTreeSet::new();
        let mut missing_col: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (table, col) in referenced_pairs() {
            match origin(table) {
                Some(Origin::Relation { .. }) => {
                    if epoch_expr(table, col).is_none() {
                        missing_col.entry(table).or_default().push(col);
                    }
                }
                Some(Origin::Custom | Origin::Empty) => {}
                None => {
                    missing_table.insert(table);
                }
            }
        }
        let mut report = String::new();
        for table in &missing_table {
            report.push_str(&format!(
                "  staging table `{table}` has no entry in `origin`\n"
            ));
        }
        for (table, cols) in &missing_col {
            report.push_str(&format!(
                "  {table}: {} unmapped column(s): {}\n",
                cols.len(),
                cols.join(", ")
            ));
        }
        assert!(
            report.is_empty(),
            "the epoch mapping does not cover every staging column the \
             information schema reads:\n{report}"
        );
    }

    /// The enumeration the map has to cover, grouped by staging table. Kept as
    /// scaffolding for extending the map: the totality test says *what* is
    /// missing, this says what the whole surface looks like.
    #[test]
    #[ignore = "scaffolding: prints the mapping surface"]
    fn print_referenced_pairs() {
        let mut by_table: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for (table, col) in referenced_pairs() {
            by_table.entry(table).or_default().push(col);
        }
        let total: usize = by_table.values().map(|v| v.len()).sum();
        println!("{} pairs across {} staging tables", total, by_table.len());
        for (table, cols) in &by_table {
            println!("\n-- {} ({})\n{}", table, cols.len(), cols.join(", "));
        }
    }
}
