//! Generator for the information schema as views straight over `target/private/metadata/`,
//! with no materialization step.
//!
//! The SQL it produces declares the same schema names, table names, column names
//! and column order as `views.sql` — the difference is only where the rows come
//! from. `views.sql` reads the parquet snapshot beside it; this reads the epoch
//! files live, so a query sees the state of the last invocation without a
//! conversion having run. The differential test in `tests_epoch.rs` keeps that
//! substitution honest.
//!
//! It is *not* written into the shipped info-schema directory: it reads the
//! private `target/private/metadata/` layout through the `dbt_internal` epoch relations,
//! which are not part of the public contract (see `write_info_schema`). The
//! generator exists for that differential test and for a future serve path that
//! queries metadata live; a caller runs [`generate`] and executes or writes the
//! statements itself.
//!
//! Paths are absolute: DuckDB resolves globs against the process CWD, and the
//! metadata directory is relocatable via `DBT_METADATA_DIR`, so this cannot be a
//! static asset shipped with the binary.
//!
//! Tables whose staging columns are not fully mapped in [`super::epoch`] are
//! *skipped* rather than half-emitted. A partially mapped table would publish
//! silently-null columns, which is the exact failure the mapping exists to
//! prevent; skipping keeps the file's contract "every view here is equivalent to
//! its `views.sql` counterpart".

use std::path::Path;

use arrow_schema::{DataType, TimeUnit};

use crate::IndexError;

use super::epoch::{
    self, BASE, ELEM, ELEM_REL, EPOCH_RELATIONS, EpochExpr, EpochJoin, JOINED, Origin, Unnest,
};
use super::project::out_schema;
use super::spec::{Filter, Src, TableSpec};

/// A generated `epoch_views.sql` plus what it could not cover.
pub struct Generated {
    /// The file contents.
    pub sql: String,
    /// The same DDL as individual statements, in order, for a caller that
    /// executes rather than writes it. Kept alongside `sql` rather than split
    /// back out of it, because splitting SQL on `;` is only correct until a
    /// literal contains one.
    pub statements: Vec<String>,
    /// Qualified names of the output tables with no view in `sql`, because some
    /// staging column they read has no epoch expression yet. Empty is the goal;
    /// a test asserts the set only ever shrinks.
    pub skipped: Vec<String>,
}

/// Generate the whole file for one metadata directory.
///
/// `metadata_dir` is absolutized here rather than required to be absolute: it
/// comes from `--metadata-dir` / `--target-path`, which may be relative to the
/// project root, and the generated file embeds the path in every `read_parquet`,
/// so a relative one would only resolve for a client whose cwd happens to match.
/// `std::path::absolute` and not `canonicalize` because the directory is not
/// required to exist — a relation whose files are absent is skipped, along with
/// the output tables that read it, so a project that has only ever parsed gets a
/// file covering the parse tables and nothing about runs.
pub fn generate(metadata_dir: &Path) -> Result<Generated, IndexError> {
    let metadata_dir = &std::path::absolute(metadata_dir)
        .map_err(|e| IndexError::Other(format!("epoch views: {}: {e}", metadata_dir.display())))?;
    let mut sql = String::new();
    sql.push_str(&format!(
        "-- dbt information schema v{}, as views over the epoch files under\n\
         -- {}\n\
         -- Generated; edits are overwritten. Same schemas, tables, columns and\n\
         -- column order as views.sql, which reads the parquet snapshot instead.\n\n",
        super::INFO_SCHEMA_VERSION,
        metadata_dir.display()
    ));
    let mut statements: Vec<String> = Vec::new();
    for ns in super::spec::Ns::ALL {
        let stmt = format!("CREATE SCHEMA IF NOT EXISTS {}", ns.prefix());
        sql.push_str(&format!("{stmt};\n"));
        statements.push(stmt);
    }
    sql.push('\n');

    // Epoch relations first: the public views select from them by name.
    let mut present: Vec<&'static str> = Vec::new();
    for rel in EPOCH_RELATIONS {
        if !rel.has_files(metadata_dir) {
            continue;
        }
        let stmt = rel.create_view_sql(metadata_dir);
        sql.push_str(&format!("{stmt};\n\n"));
        statements.push(stmt);
        present.push(rel.view);
    }

    let alive_exists = present.contains(&"epoch_parse_alive");
    let mut skipped = Vec::new();
    for spec in super::schema::INFO_SCHEMA {
        match table_view_sql(spec, &present, alive_exists)? {
            // A view over a relation with no files on disk would fail at query
            // time, not here — the worst place to find out — so it is skipped.
            Ok((_, Some(reads))) if !present.contains(&reads) => {
                skipped.push(spec.qualified_name())
            }
            Ok((view_sql, _)) => {
                sql.push_str(&format!("{view_sql};\n\n"));
                statements.push(view_sql);
            }
            Err(_why) => skipped.push(spec.qualified_name()),
        }
    }
    Ok(Generated {
        sql,
        statements,
        skipped,
    })
}

/// The statements a parse-time check needs, over `metadata_dir`.
///
/// The same view layer [`generate`] builds, narrowed to what a check may read:
///
/// - only the [`parse_safe::VIEWS`](super::parse_safe::VIEWS) tables are published into
///   `dbt`, and only with the columns those views declare. A column that is empty until
///   compile is *absent*, so naming it is a binder error rather than a NULL that reads as
///   a clean pass.
/// - `dbt_rt` is not registered at all. Checks run before execute, so a runtime table
///   could only ever be empty, and zero rows is a pass.
/// - the published `dbt_internal` tables are dropped as well. Only the epoch relations
///   are registered there, because the projections read them.
///
/// Each published view is derived from `generate`'s own statement for that table rather
/// than from a second code path: that statement becomes the subquery a column projection
/// selects from. So a check and the published information schema cannot disagree about a
/// column's *value* — only about whether the column is offered.
///
/// A view `generate` skipped — its epoch relation has no files, as `node_columns` has none
/// until something writes columns — is simply absent, so a check naming it fails to bind.
/// That is the same outcome as the index path it replaces, and louder than an empty
/// stand-in, which would pass having read nothing.
///
/// Which is why this builds on [`generate`] and not on [`generate_queryable`], despite the
/// latter being the one `dbt show --info` executes: the empty stand-ins it adds are right for
/// a query, whose answer is "no rows", and wrong for a check, whose zero rows *are* the pass.
pub fn parse_safe_statements(metadata_dir: &Path) -> Result<Vec<String>, IndexError> {
    let generated = generate(metadata_dir)?;
    let mut out: Vec<String> = Vec::with_capacity(generated.statements.len());

    for stmt in generated.statements {
        // An allowlist, not a denylist: check SQL may name any relation, so a published
        // table nobody thought about must be *absent* rather than merely undocumented.
        if stmt.starts_with("CREATE SCHEMA")
            || EPOCH_RELATIONS
                .iter()
                .any(|rel| stmt.starts_with(&create_prefix(&format!("dbt_internal.{}", rel.view))))
        {
            out.push(stmt);
            continue;
        }
        let Some(view) = super::parse_safe::VIEWS
            .iter()
            .find(|v| stmt.starts_with(&create_prefix(&format!("dbt.{}", v.name))))
        else {
            continue;
        };

        let prefix = create_prefix(&format!("dbt.{}", view.name));
        let body = stmt.strip_prefix(&prefix).ok_or_else(|| {
            IndexError::Other(format!(
                "parse-safe views: `generate` no longer emits `{prefix}` for {}; the \
                 projection below wraps that statement and cannot be built without it",
                view.name
            ))
        })?;
        let cols = view
            .cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "CREATE OR REPLACE VIEW dbt.{} AS SELECT {cols} FROM (\n{body}\n)",
            view.name
        ));
    }
    Ok(out)
}

/// Whether `metadata_dir` holds the parse epochs the check views are built from.
///
/// The parse-time gate consults this before running anything, because
/// [`parse_safe_statements`] *skips* a view whose relation has no files: with the parse
/// epochs missing, every check would fail to bind and the run would report twenty binder
/// errors instead of the one fact that explains them.
///
/// `epoch_parse_nodes` alone, and not every relation a view might read: it is the node set
/// the views project, so its absence means nothing has been written, while a project that
/// has parsed but never compiled legitimately lacks the rest.
pub fn parse_epochs_present(metadata_dir: &Path) -> bool {
    EPOCH_RELATIONS
        .iter()
        .any(|rel| rel.view == "epoch_parse_nodes" && rel.has_files(metadata_dir))
}

/// The prefix every generated view statement starts with, per the two `header`s above.
fn create_prefix(qualified: &str) -> String {
    format!("CREATE OR REPLACE VIEW {qualified} AS\n")
}

/// Output tables the mapping cannot express at all.
///
/// A subset of [`Generated::skipped`], and the half of it that is a property of
/// this crate rather than of a metadata directory: the rest of that set is
/// tables whose epoch relation has no files yet, which says only that the
/// project has never done the corresponding kind of run. Computed by generating
/// against a hypothetical directory where every relation is present, so it can
/// be asserted on in CI, where no real metadata directory exists.
pub fn unmappable_tables() -> Result<Vec<String>, IndexError> {
    let all: Vec<&'static str> = EPOCH_RELATIONS.iter().map(|r| r.view).collect();
    let mut out = Vec::new();
    for spec in super::schema::INFO_SCHEMA {
        if table_view_sql(spec, &all, true)?.is_err() {
            out.push(spec.qualified_name());
        }
    }
    Ok(out)
}

/// Every statement needed to make the whole published surface queryable straight from
/// `metadata_dir`: [`generate`]'s views, plus a column-shaped empty view for each table it
/// skipped.
///
/// The stubs are what keep this equivalent to querying the parquet snapshot. A table whose
/// epoch source produced nothing is *published empty* there, so `dbt show --info <that table>`
/// answers with zero rows; without a stub the view would simply not exist and the same command
/// would fail to bind. An absent table and an empty one are different answers, and only one of
/// them is true.
pub fn generate_queryable(metadata_dir: &Path) -> Result<Vec<String>, IndexError> {
    let generated = generate(metadata_dir)?;
    let mut statements = generated.statements;
    for spec in super::schema::INFO_SCHEMA {
        if generated
            .skipped
            .iter()
            .any(|s| *s == spec.qualified_name())
        {
            statements.push(empty_view_sql(spec)?);
        }
    }
    Ok(statements)
}

/// `CREATE VIEW` for a table's declared columns with no rows.
fn empty_view_sql(spec: &TableSpec) -> Result<String, IndexError> {
    let out = out_schema(spec)?;
    let cols = spec
        .cols
        .iter()
        .zip(out.fields())
        .map(|(col, field)| column_sql(EpochExpr::Null, col.out, field.data_type(), None))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(format!(
        "CREATE OR REPLACE VIEW {} AS SELECT {} WHERE FALSE",
        spec.qualified_name(),
        cols.join(", ")
    ))
}

/// DuckDB type for an Arrow type, for the `CAST(NULL AS ...)` columns.
///
/// Only the types the information schema actually declares are covered — an
/// unhandled type is a hard error rather than a guess, so adding a column of a
/// new type cannot silently produce a `VARCHAR` that then fails the differential
/// test with a confusing message.
fn duckdb_type(ty: &DataType) -> Result<String, IndexError> {
    Ok(match ty {
        DataType::Utf8 | DataType::LargeUtf8 => "VARCHAR".into(),
        DataType::Boolean => "BOOLEAN".into(),
        DataType::Int32 => "INTEGER".into(),
        DataType::Int64 => "BIGINT".into(),
        DataType::Float64 => "DOUBLE".into(),
        DataType::Timestamp(TimeUnit::Microsecond, Some(_)) => "TIMESTAMP WITH TIME ZONE".into(),
        DataType::Timestamp(TimeUnit::Microsecond, None) => "TIMESTAMP".into(),
        DataType::List(f) | DataType::LargeList(f) => format!("{}[]", duckdb_type(f.data_type())?),
        other => {
            return Err(IndexError::Other(format!(
                "epoch views: no DuckDB type for Arrow type {other:?}"
            )));
        }
    })
}

/// SQL for one output column, given the staging column it reads.
///
/// Always cast, never inferred. The declared type is the contract — it comes
/// from the staging schema, which is what the materialized layer publishes —
/// while the epoch parquet's own type is an implementation detail of whichever
/// writer produced it, and the two do differ (`run/invocations.node_count` is
/// `INTEGER` on disk, `BIGINT` in the schema). Without the cast that difference
/// reaches consumers as a view whose column types depend on which file the glob
/// happened to match.
/// `guard` is a row predicate the column is null outside of — the join-partner
/// case described in [`table_view_sql`].
fn column_sql(
    expr: EpochExpr,
    out_name: &str,
    ty: &DataType,
    guard: Option<&str>,
) -> Result<String, IndexError> {
    let quoted = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
    let ty_sql = duckdb_type(ty)?;
    let body = match expr {
        EpochExpr::Same => format!("{BASE}.{}", quoted(out_name)),
        EpochExpr::Col(c) => format!("{BASE}.{}", quoted(c)),
        EpochExpr::Json(path) => format!("json_extract_string({BASE}.payload, '$.{path}')"),
        EpochExpr::JsonRaw(path) => format!("json_extract({BASE}.payload, '$.{path}')"),
        // The cast goes *inside* the COALESCE as well as around it: the extract
        // is always VARCHAR, so `COALESCE(<varchar>, TRUE)` is a binder error
        // rather than a defaulted boolean.
        EpochExpr::JsonOr(path, default) => format!(
            "COALESCE(CAST(json_extract_string({BASE}.payload, '$.{path}') AS {ty_sql}), {default})"
        ),
        EpochExpr::JsonFirst(paths, default) => {
            let mut args: Vec<String> = paths
                .iter()
                .map(|p| format!("CAST(json_extract_string({BASE}.payload, '$.{p}') AS {ty_sql})"))
                .collect();
            args.push(default.to_string());
            format!("COALESCE({})", args.join(", "))
        }
        EpochExpr::JsonList(path) => {
            format!("COALESCE(json_extract_string({BASE}.payload, '$.{path}[*]'), [])")
        }
        EpochExpr::TrimmedJson(path) => format!(
            "CASE WHEN {BASE}.resource_type = 'model' THEN NULL \
             ELSE json_extract_string({BASE}.payload, '$.{path}') END"
        ),
        EpochExpr::TrimmedJsonRaw(path) => format!(
            "CASE WHEN {BASE}.resource_type = 'model' THEN NULL \
             ELSE json_extract({BASE}.payload, '$.{path}') END"
        ),
        EpochExpr::Elem => ELEM.to_string(),
        EpochExpr::ElemJson(path) => format!("json_extract_string({ELEM}, '$.{path}')"),
        EpochExpr::ElemJsonRaw(path) => format!("json_extract({ELEM}, '$.{path}')"),
        EpochExpr::ElemJsonFirst(paths, default) => {
            let mut args: Vec<String> = paths
                .iter()
                .map(|p| format!("CAST(json_extract_string({ELEM}, '$.{p}') AS {ty_sql})"))
                .collect();
            args.push(default.to_string());
            format!("COALESCE({})", args.join(", "))
        }
        EpochExpr::JoinCol(c) => format!("{JOINED}.{}", quoted(c)),
        // `json_extract_string(j, '$[*]')` yields `VARCHAR[]`, and NULL for a
        // column that is null or is not a JSON array — the two cases the Rust
        // path's `unwrap_or_default()` also collapses to the empty list.
        EpochExpr::JoinJsonList(c) => format!(
            "COALESCE(json_extract_string({JOINED}.{}, '$[*]'), [])",
            quoted(c)
        ),
        EpochExpr::Sql(sql) | EpochExpr::SqlJson(sql) => sql.to_string(),
        EpochExpr::Null => "NULL".to_string(),
        EpochExpr::EmptyList => "[]".to_string(),
        EpochExpr::True => "TRUE".to_string(),
    };
    let body = match guard {
        Some(g) => format!("CASE WHEN {g} THEN {body} END"),
        None => body,
    };
    Ok(format!("CAST({body} AS {ty_sql}) AS {}", quoted(out_name)))
}

type ViewSql = (String, Option<&'static str>);

/// The select list of a hand-written view: one cast column per field of
/// [`out_schema`], in declaration order, with the body given per output name.
///
/// A column with no body is a typed null. Driving the list off `out_schema`
/// rather than off the literal SQL is what makes the shape check in the
/// differential test hard to fail by accident: a column added to the spec appears
/// here as a null rather than as a missing column, and the name, order and type
/// cannot drift from what the materialized layer publishes.
fn cast_cols(spec: &TableSpec, bodies: &[(&str, String)]) -> Result<Vec<String>, IndexError> {
    let out = out_schema(spec)?;
    out.fields()
        .iter()
        .map(|field| {
            let body = bodies
                .iter()
                .find(|(name, _)| name == field.name())
                .map_or("NULL", |(_, sql)| sql.as_str());
            Ok(format!(
                "CAST({body} AS {}) AS \"{}\"",
                duckdb_type(field.data_type())?,
                field.name().replace('"', "\"\"")
            ))
        })
        .collect()
}

/// `[{key, val}, ..]` for a JSON object held as text — DuckDB has no `json_each`,
/// and `json_keys` alone gives no way back to the values without building a path
/// string per key and quoting it.
///
/// `json_extract(.., '$.*')` returns the values in key order, so zipping the two
/// pairs each key with its own value. `list_zip` yields an *unnamed* struct, hence
/// the transform that names the fields: everything downstream reads `.key` and
/// `.val` rather than a positional index.
fn json_entries(obj: &str) -> String {
    format!(
        "list_transform(list_zip(json_keys({obj}), json_extract({obj}, '$.*')), \
         z -> struct_pack(key := struct_extract(z, 1), val := struct_extract(z, 2)))"
    )
}

/// One side of the `node_columns` merge: a projection of an epoch relation, or a
/// typed empty relation when that relation has no files.
///
/// Substituting an empty relation rather than generating a different query shape
/// is what keeps the merge testable: a FULL join against an empty side is exactly
/// "this source contributed nothing", which is what the Rust path does when the
/// directory is missing, and the shape a corpus with all three sources exercises
/// is then the shape every project gets.
///
/// `cols` is `(expression, name, DuckDB type)` per output column. The type is only
/// used for the empty case, and is the column's type in the merge rather than in
/// the output schema — the output cast happens once, in [`cast_cols`].
fn merge_side(
    view: &'static str,
    present: &[&'static str],
    cols: &[(&str, &str, &str)],
    wheres: &[String],
    dedup_by: Option<&str>,
) -> String {
    if !present.contains(&view) {
        let list = cols
            .iter()
            .map(|(_, name, ty)| format!("CAST(NULL AS {ty}) AS {name}"))
            .collect::<Vec<_>>()
            .join(", ");
        return format!("SELECT {list} WHERE FALSE");
    }
    let list = cols
        .iter()
        .map(|(expr, name, _)| format!("{expr} AS {name}"))
        .collect::<Vec<_>>()
        .join(", ");
    let mut sql = format!("SELECT {list} FROM dbt_internal.{view}");
    if !wheres.is_empty() {
        sql.push_str(&format!(" WHERE {}", wheres.join(" AND ")));
    }
    // The Rust merges key their lookup map on one row per key, so a second row
    // with the same key overwrites rather than duplicating. A join without this
    // would multiply the other side's rows instead.
    if let Some(key) = dedup_by {
        sql.push_str(&format!(
            " QUALIFY row_number() OVER (PARTITION BY {key}) = 1"
        ));
    }
    sql
}

/// The one project-level row `write_parse_project` assembles, as a subquery
/// aliased [`BASE`]: `parse/generation`'s fields with `parse/resolver_state`'s git
/// fields beside them, or no row at all.
///
/// Both sources are single-row snapshots overwritten on every parse, and the Rust
/// path reads row 0 of each — hence `LIMIT 1` rather than an aggregate, which
/// would quietly pick a different row from a file that somehow had two.
///
/// `NULLIF(.., '')` throughout because the Rust path's field reads all reject the
/// empty string as well as null (`read_str_opt!`), and treat "still empty" as
/// grounds to try the next source. `project_name` empty is the one that decides
/// the row exists at all: `write_parse_project` returns before writing any of the
/// three tables.
///
/// The legacy `parse/project.parquet` key/value fallback is *not* reproduced. It
/// covers metadata directories written before `generation.parquet` carried these
/// fields, and such a directory predates epoch layers entirely — `parse/nodes` has
/// no `v<n>_*.parquet` in it, so there is nothing for the view layer to read and
/// no view for the fallback to feed.
fn project_snapshot(present: &[&'static str]) -> String {
    // `LEFT JOIN .. ON TRUE`, not a comma join: the Rust path's git fields stay
    // `None` when `resolver_state.parquet` is missing or empty, where a cross
    // join with an empty right side would drop `dbt.project`'s only row.
    let resolver = if present.contains(&"epoch_parse_resolver_state") {
        "SELECT * FROM dbt_internal.epoch_parse_resolver_state LIMIT 1".to_string()
    } else {
        "SELECT CAST(NULL AS VARCHAR) AS vars_json, CAST(NULL AS VARCHAR) AS env_vars_json, \
         CAST(NULL AS VARCHAR) AS git_sha, CAST(NULL AS VARCHAR) AS git_branch, \
         CAST(NULL AS INTEGER) AS git_is_dirty, CAST(NULL AS VARCHAR) AS pkg_kinds_json"
            .to_string()
    };
    format!(
        "(\n\
         \x20 SELECT\n\
         \x20   COALESCE(NULLIF(g.project_name, ''), '') AS project_name,\n\
         \x20   COALESCE(NULLIF(g.dbt_version, ''), '') AS dbt_version,\n\
         \x20   COALESCE(NULLIF(g.adapter_type, ''), '') AS adapter_type,\n\
         \x20   COALESCE(NULLIF(g.vars_json, ''), NULLIF(r.vars_json, '')) AS vars_json,\n\
         \x20   COALESCE(NULLIF(g.env_vars_json, ''), NULLIF(r.env_vars_json, '')) \
         AS env_vars_json,\n\
         \x20   NULLIF(r.git_sha, '') AS git_sha,\n\
         \x20   NULLIF(r.git_branch, '') AS git_branch,\n\
         \x20   r.git_is_dirty <> 0 AS git_is_dirty,\n\
         \x20   NULLIF(r.pkg_kinds_json, '') AS pkg_kinds_json,\n\
         \x20   COALESCE(json_keys(NULLIF(r.pkg_kinds_json, '')), []) AS installed_packages,\n\
         \x20   g.ingested_at AS ingested_at\n\
         \x20 FROM (SELECT * FROM dbt_internal.epoch_parse_generation LIMIT 1) g\n\
         \x20 LEFT JOIN ({resolver}) r ON TRUE\n\
         \x20 WHERE NULLIF(g.project_name, '') IS NOT NULL\n\
         ) {BASE}"
    )
}

/// The output tables written out by hand, rather than derived from the per-column
/// mapping in [`super::epoch`].
///
/// Two kinds end up here. `Src::Own` tables have no staging column to map: the
/// Rust path assembles them from whatever it needs. So do the tables whose
/// staging source is a single-row snapshot the Rust path reads field by field
/// (`dbt.project` and friends) — those are `Src::Table`, but their staging table's
/// [`Origin`] is [`Origin::Custom`], which is the same statement. Column names,
/// order and types still have to match [`out_schema`] exactly; [`cast_cols`] is
/// what holds that, and it is why every column is cast rather than left to
/// inference.
///
/// `None` for a table not covered yet, which `generate` reports as skipped.
fn own_sql(
    spec: &TableSpec,
    present: &[&'static str],
    alive_exists: bool,
) -> Result<Option<ViewSql>, IndexError> {
    let header = |cols: &[&str]| {
        format!(
            "CREATE OR REPLACE VIEW {} AS\nSELECT\n{}\n",
            spec.qualified_name(),
            cols.iter()
                .map(|c| format!("  {c}"))
                .collect::<Vec<_>>()
                .join(",\n")
        )
    };
    Ok(match spec.name {
        // No source of taxonomy data on either path — `build_own` returns no rows
        // at all — so the shape is published and the rows are not. The typed nulls
        // come from the same `resolve_cols` every other table uses, since every
        // column of this spec is spec-declared null.
        "classifiers" => {
            let cols = match resolve_cols(spec, false, &[])? {
                Ok(cols) => cols,
                Err(why) => return Err(IndexError::Other(why)),
            };
            let list: Vec<&str> = cols.iter().map(|c| c.sql.as_str()).collect();
            Some((format!("{}WHERE FALSE", header(&list)), None))
        }
        // Every live, enabled node whose resource type participates in the DAG.
        //
        // The Rust builder gets there in two pieces because its inputs are two
        // pieces: the staging `nodes` table holds only the eight types
        // `write_to_nodes` admits, so `exposure`, `metric` and `unit_test` are
        // unioned in from their own tables. All three come from `parse/nodes` in
        // the first place, so one pass over that relation is the same row set —
        // and `resource_type IN (DAG_RESOURCE_TYPES)` is what drops the types that
        // are in `nodes` but not in the DAG, `operation` being the one today.
        //
        // `enabled` comes through the same inverted `is_disabled` column the
        // `nodes` mapping reads, and a missing value means enabled. Applied to
        // every type, matching the Rust path, which filters the side tables on
        // their own `enabled` column too.
        "dag_nodes" => {
            let types = super::schema::DAG_RESOURCE_TYPES
                .iter()
                .map(|t| format!("'{t}'"))
                .collect::<Vec<_>>()
                .join(", ");
            let cols = cast_cols(
                spec,
                &[
                    ("unique_id", format!("{BASE}.unique_id")),
                    ("resource_type", format!("{BASE}.resource_type")),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let mut sql = header(&cols.iter().map(String::as_str).collect::<Vec<_>>());
            sql.push_str(&format!("FROM dbt_internal.epoch_parse_nodes {BASE}\n"));
            let mut wheres = vec![
                format!("{BASE}.resource_type IN ({types})"),
                format!("COALESCE({BASE}.is_disabled = 0, TRUE)"),
            ];
            if alive_exists {
                wheres.insert(
                    0,
                    format!(
                        "{BASE}.unique_id IN \
                         (SELECT unique_id FROM dbt_internal.epoch_parse_alive)"
                    ),
                );
            }
            for (i, w) in wheres.iter().enumerate() {
                let lead = if i == 0 { "WHERE" } else { "  AND" };
                sql.push_str(&format!("{lead} {w}\n"));
            }
            Some((sql.trim_end().to_string(), Some("epoch_parse_nodes")))
        }
        // The warehouse's own view of each materialized relation. This is the
        // one table the epoch layer publishes more directly than the Rust path
        // does: `run/catalog_stats` already carries one wide row per relation,
        // so this is a projection, while `build_relations` has to undo the
        // split the staging ingest applied on the way in.
        //
        // No `alive` filter: a disabled node keeps its last known catalog row
        // rather than losing it. The epoch relation is `LatestGroup`, not
        // `KeepAll` like `dbt_rt.run_results` — this is the warehouse's current
        // state for a relation, not an invocation log, so a node re-catalogued
        // by a later `docs generate` replaces its row here instead of
        // accumulating one per invocation.
        "relations" => {
            let cols = cast_cols(
                spec,
                &[
                    ("unique_id", format!("{BASE}.unique_id")),
                    ("table_type", format!("{BASE}.table_type")),
                    ("table_owner", format!("{BASE}.table_owner")),
                    ("database_name", format!("{BASE}.database_name")),
                    ("schema_name", format!("{BASE}.schema_name")),
                    ("table_name", format!("{BASE}.table_name")),
                    ("row_count", format!("{BASE}.row_count")),
                    ("bytes", format!("{BASE}.bytes")),
                    ("last_modified", format!("{BASE}.last_modified")),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let list: Vec<&str> = cols.iter().map(String::as_str).collect();
            let sql = format!(
                "{}FROM dbt_internal.epoch_run_catalog_stats {BASE}\nWHERE {BASE}.unique_id IS NOT NULL",
                header(&list)
            );
            Some((sql, Some("epoch_run_catalog_stats")))
        }
        // The single-row project snapshot. `quoting` and `ai_context` are left
        // null: the staging schema declares them and `write_parse_project` writes
        // neither.
        "project" => {
            let cols = cast_cols(
                spec,
                &[
                    ("project_name", format!("{BASE}.project_name")),
                    ("dbt_version", format!("{BASE}.dbt_version")),
                    ("adapter_type", format!("{BASE}.adapter_type")),
                    ("git_sha", format!("{BASE}.git_sha")),
                    ("git_branch", format!("{BASE}.git_branch")),
                    ("git_uncommitted_changes", format!("{BASE}.git_is_dirty")),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                    // The generation row's own stamp, which is what "last full
                    // parse" means. `fill_last_full_parse_at` does this on the
                    // Arrow path; without it here the production COPY path left
                    // the column null, and every consumer's staleness label with
                    // it.
                    ("last_full_parse_at", format!("{BASE}.ingested_at")),
                    // Same Rust constant the writer stamps in
                    // (`fill_schema_version`), inlined: it describes the shape of
                    // the relations this file declares, so a view layer generated
                    // by this build reports this build's version.
                    ("schema_version", super::INFO_SCHEMA_VERSION.to_string()),
                ],
            )?;
            let list: Vec<&str> = cols.iter().map(String::as_str).collect();
            let sql = format!("{}FROM {}", header(&list), project_snapshot(present));
            Some((sql, Some("epoch_parse_generation")))
        }
        // One row per key of the snapshot's `env_vars_json`. The value — the list
        // of nodes the variable is used in — is dropped by the spec, so only the
        // keys are needed and `json_keys` is the whole of it. A null or absent
        // `env_vars_json` unnests to no rows, matching the `unwrap_or_default()`
        // on the Rust side.
        "project_env_vars" => {
            let cols = cast_cols(
                spec,
                &[
                    ("env_var_name", ELEM.to_string()),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let list: Vec<&str> = cols.iter().map(String::as_str).collect();
            let sql = format!(
                "{}FROM {}\n  , unnest(json_keys({BASE}.env_vars_json)) AS {ELEM_REL}",
                header(&list),
                project_snapshot(present)
            );
            Some((sql, Some("epoch_parse_generation")))
        }
        // One row per (package, variable) of the snapshot's `{package: {var: value}}`
        // map, which the ingest writes in the same shape: each package's map is
        // already resolved for that package, so this walks it rather than
        // un-nesting it.
        //
        // Two kinds of key are dropped, matching `write_parse_project`: an internal
        // package (`internal_packages`, whose contents depend on the adapter, so it
        // is built from the snapshot's `adapter_type` rather than being a literal),
        // and a nested object whose key names an installed package -- that is a
        // scope declaration, and the scope's effect is already in that package's own
        // row. `COALESCE(.., '')` on the conditional skip entry because a NULL
        // inside `NOT IN` makes the whole predicate NULL, which would drop every
        // package rather than none.
        "project_vars" => {
            let skip = format!(
                "'dbt', 'dbt_' || {BASE}.adapter_type, \
                 COALESCE(CASE {BASE}.adapter_type \
                 WHEN 'redshift' THEN 'dbt_postgres' \
                 WHEN 'databricks' THEN 'dbt_spark' END, '')"
            );
            // Plain strings bare, everything else as JSON: `serde_json::Value`'s
            // `String` arm versus `to_string()`.
            let value = "CASE WHEN json_type(q.val) = 'VARCHAR' \
                         THEN json_extract_string(q.val, '$') \
                         ELSE CAST(q.val AS VARCHAR) END";
            let installed = format!("{BASE}.installed_packages");
            let is_scope =
                format!("json_type(q.val) = 'OBJECT' AND list_contains({installed}, q.key)");
            let own_vars = format!(
                "list_filter({vars}, q -> NOT ({is_scope}))",
                vars = json_entries("p.val"),
            );
            let list = format!(
                "flatten(list_transform(\
                 list_filter({packages}, p -> p.key NOT IN ({skip})), \
                 p -> list_transform({own_vars}, \
                 q -> struct_pack(package := p.key, name := q.key, value := {value}))))",
                packages = json_entries(&format!("{BASE}.vars_json")),
            );
            let cols = cast_cols(
                spec,
                &[
                    ("project_name", format!("{ELEM}.package")),
                    ("var_name", format!("{ELEM}.name")),
                    ("var_value", format!("{ELEM}.value")),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
            let sql = format!(
                "{}FROM {}\n  , unnest({list}) AS {ELEM_REL}",
                header(&cols),
                project_snapshot(present)
            );
            Some((sql, Some("epoch_parse_generation")))
        }
        // One row per key of `pkg_kinds_json`. The other columns have no epoch
        // source yet — `write_parse_project` writes `package_name` only — so
        // they stay null, matching the Rust path.
        "packages" => {
            let cols = cast_cols(
                spec,
                &[
                    ("package_name", ELEM.to_string()),
                    ("package_source", "NULL".to_string()),
                    ("version", "NULL".to_string()),
                    ("git_url", "NULL".to_string()),
                    ("git_revision", "NULL".to_string()),
                    ("local_path", "NULL".to_string()),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let cols: Vec<&str> = cols.iter().map(String::as_str).collect();
            let sql = format!(
                "{}FROM {}\n  , unnest({BASE}.installed_packages) AS {ELEM_REL}",
                header(&cols),
                project_snapshot(present)
            );
            Some((sql, Some("epoch_parse_generation")))
        }
        // The three-way merge: `parse/columns` supplies the documented column,
        // `compile/columns` its inferred type and position, `catalog/columns` what
        // the warehouse actually has. Each is a separate writer on the Rust side
        // (`write_parse_columns` then two `merge_*_into_node_columns` passes over
        // the file it wrote), and each pass both updates matching rows and appends
        // its own unmatched ones — which is a FULL join, twice, in that order.
        //
        // The compile join is case-insensitive on the column name and the catalog
        // join is not, mirroring the two merges' map keys: Snowflake uppercases
        // unquoted identifiers in the inferred schema, so a case-sensitive compile
        // key would append a duplicate row instead of filling in the parse row.
        //
        // `data_type` is set *only* where a catalog row matched, so it is null for
        // every project that has never fetched a catalog even though
        // `inferred_type` is populated. That is what the Rust path writes.
        //
        // Both column sources are newest-epoch-wins per *node* on both sides:
        // `dedup_epoch_groups` on the Rust side, `Supersede::LatestGroup` on the
        // relations these read. The `dedup_by` keys below are per (node, column)
        // and do a different job — one row per join key, so a FULL JOIN cannot
        // multiply the other side. No corpus has a catalog, so the catalog side of
        // this is unexercised on real data.
        "node_columns" => {
            let alive = |col: &str| {
                alive_exists.then(|| {
                    format!("{col} IN (SELECT unique_id FROM dbt_internal.epoch_parse_alive)")
                })
            };
            let parse = merge_side(
                "epoch_parse_columns",
                present,
                &[
                    ("unique_id", "unique_id", "VARCHAR"),
                    // `get_str(..).unwrap_or_default()`: a null name is stored as
                    // the empty string, which is a value the join can match on.
                    ("COALESCE(column_name, '')", "column_name", "VARCHAR"),
                    ("declared_type", "declared_type", "VARCHAR"),
                    ("description", "description", "VARCHAR"),
                    ("COALESCE(tags, [])", "tags", "VARCHAR[]"),
                    ("ingested_at", "ingested_at", "TIMESTAMP WITH TIME ZONE"),
                ],
                &["unique_id IS NOT NULL".to_string()]
                    .into_iter()
                    .chain(alive("unique_id"))
                    .collect::<Vec<_>>(),
                None,
            );
            let compile = merge_side(
                "epoch_compile_columns",
                present,
                &[
                    ("unique_id", "unique_id", "VARCHAR"),
                    ("column_name", "column_name", "VARCHAR"),
                    ("CAST(column_index AS BIGINT)", "column_index", "BIGINT"),
                    ("column_type", "inferred_type", "VARCHAR"),
                    ("description", "description", "VARCHAR"),
                    ("COALESCE(classifiers, [])", "classifiers", "VARCHAR[]"),
                    ("ingested_at", "ingested_at", "TIMESTAMP WITH TIME ZONE"),
                ],
                &[
                    "unique_id IS NOT NULL".to_string(),
                    "column_name IS NOT NULL".to_string(),
                ]
                .into_iter()
                .chain(alive("unique_id"))
                .collect::<Vec<_>>(),
                Some("unique_id, lower(column_name)"),
            );
            let catalog = merge_side(
                "epoch_catalog_columns",
                present,
                &[
                    ("unique_id", "unique_id", "VARCHAR"),
                    ("column_name", "column_name", "VARCHAR"),
                    ("CAST(column_index AS BIGINT)", "column_index", "BIGINT"),
                    ("catalog_type", "catalog_type", "VARCHAR"),
                    ("catalog_comment", "catalog_comment", "VARCHAR"),
                    ("ingested_at", "ingested_at", "TIMESTAMP WITH TIME ZONE"),
                ],
                &[
                    "unique_id IS NOT NULL".to_string(),
                    "column_name IS NOT NULL".to_string(),
                ]
                .into_iter()
                .chain(alive("unique_id"))
                .collect::<Vec<_>>(),
                Some("unique_id, column_name"),
            );
            let cols = cast_cols(
                spec,
                &[
                    ("node_unique_id", format!("{BASE}.unique_id")),
                    ("column_name", format!("{BASE}.column_name")),
                    ("column_index", format!("{BASE}.column_index")),
                    ("data_type_declared", format!("{BASE}.declared_type")),
                    ("data_type_inferred", format!("{BASE}.inferred_type")),
                    ("data_type_actual", format!("{BASE}.catalog_type")),
                    ("data_type", format!("{BASE}.data_type")),
                    ("description", format!("{BASE}.description")),
                    ("tags", format!("{BASE}.tags")),
                    ("classifiers", format!("{BASE}.classifiers")),
                    ("comment", format!("{BASE}.catalog_comment")),
                    ("ingested_at", format!("{BASE}.ingested_at")),
                ],
            )?;
            let list: Vec<&str> = cols.iter().map(String::as_str).collect();
            // `label`, `expression`, `quote`, `granularity`, `meta`, `constraints`
            // and `tests` are left to `cast_cols`' typed nulls. `parse/columns`
            // carries `meta`, `constraints` and `granularity`, but
            // `write_parse_columns` does not read them.
            let sql = format!(
                "{}FROM (\n\
                 \x20 SELECT\n\
                 \x20   COALESCE(m.unique_id, k.unique_id) AS unique_id,\n\
                 \x20   COALESCE(m.column_name, k.column_name) AS column_name,\n\
                 \x20   COALESCE(k.column_index, m.column_index) AS column_index,\n\
                 \x20   m.declared_type AS declared_type,\n\
                 \x20   m.inferred_type AS inferred_type,\n\
                 \x20   k.catalog_type AS catalog_type,\n\
                 \x20   CASE WHEN k.unique_id IS NULL THEN NULL\n\
                 \x20        WHEN k.catalog_type IS NOT NULL THEN k.catalog_type\n\
                 \x20        ELSE m.inferred_type END AS data_type,\n\
                 \x20   m.description AS description,\n\
                 \x20   COALESCE(m.tags, []) AS tags,\n\
                 \x20   COALESCE(m.classifiers, []) AS classifiers,\n\
                 \x20   k.catalog_comment AS catalog_comment,\n\
                 \x20   COALESCE(m.ingested_at, k.ingested_at) AS ingested_at\n\
                 \x20 FROM (\n\
                 \x20   SELECT\n\
                 \x20     COALESCE(p.unique_id, c.unique_id) AS unique_id,\n\
                 \x20     COALESCE(p.column_name, c.column_name) AS column_name,\n\
                 \x20     c.column_index AS column_index,\n\
                 \x20     p.declared_type AS declared_type,\n\
                 \x20     c.inferred_type AS inferred_type,\n\
                 \x20     COALESCE(c.description, p.description) AS description,\n\
                 \x20     COALESCE(p.tags, []) AS tags,\n\
                 \x20     COALESCE(c.classifiers, []) AS classifiers,\n\
                 \x20     COALESCE(p.ingested_at, c.ingested_at) AS ingested_at\n\
                 \x20   FROM ({parse}) p\n\
                 \x20   FULL JOIN ({compile}) c\n\
                 \x20     ON p.unique_id = c.unique_id\n\
                 \x20    AND lower(p.column_name) = lower(c.column_name)\n\
                 \x20 ) m\n\
                 \x20 FULL JOIN ({catalog}) k\n\
                 \x20   ON m.unique_id = k.unique_id AND m.column_name = k.column_name\n\
                 ) {BASE}",
                header(&list)
            );
            Some((sql, None))
        }
        _ => None,
    })
}

/// `CREATE OR REPLACE VIEW <ns>.<name> AS ...` plus the epoch relation it reads,
/// or the reason the table cannot be published from the epoch files.
fn table_view_sql(
    spec: &TableSpec,
    present: &[&'static str],
    alive_exists: bool,
) -> Result<Result<ViewSql, String>, IndexError> {
    // Bespoke SQL first. A `Src::Own` table has no staging column to resolve —
    // every column is spec-declared null and the Rust path fills them in by hand
    // — so publishing it from `resolve_cols` alone would emit an empty view for a
    // table that has rows. The single-row snapshots are `Src::Table` but no more
    // derivable, and are recognised by name here rather than by their staging
    // table's `Custom` origin, which the fall-through below reports.
    if let Some(sql) = own_sql(spec, present, alive_exists)? {
        return Ok(Ok(sql));
    }
    if matches!(spec.src, Src::Own) {
        return Ok(Err(format!(
            "{} is assembled in Rust",
            spec.qualified_name()
        )));
    }
    // Every staging column of one output table must come from one base epoch
    // relation. `dbt.data_tests` is the only joined spec and both its sides read
    // `parse/nodes`, so this holds today; a spec that broke it would need a
    // second join here rather than a silently wrong single-relation SELECT.
    let base_table = match spec.src {
        Src::Table(t) => t,
        Src::Join { left, .. } => left,
        Src::Own => unreachable!("returned above"),
    };
    let mut view: Option<&'static str> = None;
    let mut keep: Option<&'static str> = None;
    let mut alive_on: Option<&'static str> = None;
    let mut join: Option<EpochJoin> = None;
    let mut unnest: Option<Unnest> = None;
    let mut guards: Vec<(&'static str, &'static str)> = Vec::new();
    let mut all_empty = true;
    for table in staging_tables(spec) {
        match epoch::origin(table) {
            Some(Origin::Relation {
                view: v,
                keep: k,
                alive_on: a,
                join: j,
                unnest: u,
            }) => {
                all_empty = false;
                if view.is_some_and(|prev| prev != v) {
                    return Ok(Err(format!(
                        "{} reads two epoch relations",
                        spec.qualified_name()
                    )));
                }
                view = Some(v);
                // The row predicate does one of two jobs depending on which side
                // of the spec its table is on. For the base table it selects rows,
                // as the `continue` in the Rust row builder does. For the *right*
                // side of a `Src::Join` it cannot: the join is a LEFT join, so a
                // node with no row on the right still appears — with every one of
                // that table's columns null. There the same predicate guards the
                // columns instead, which is not the same as letting each column's
                // own path come back null: `test_metadata.severity` reads
                // `deprecated_config`, which a test node carries whether or not it
                // has any `test_metadata` at all.
                if table == base_table {
                    keep = keep.or(k);
                    alive_on = alive_on.or(a);
                } else if let Some(k) = k {
                    guards.push((table, k));
                }
                join = join.or(j);
                unnest = unnest.or(u);
            }
            Some(Origin::Empty) => {}
            Some(Origin::Custom) => {
                return Ok(Err(format!("{table} needs bespoke SQL")));
            }
            None => return Ok(Err(format!("{table} has no declared origin"))),
        }
    }
    // A join whose relation has no files is dropped rather than skipping the
    // table: `load_compile_nodes_map` returns an *empty map* for a missing
    // `compile/nodes`, so the Rust path publishes the same table with the
    // compiled columns unset. A parse-only project is the common case.
    let join = join.filter(|j| present.contains(&j.view));

    let cols = match resolve_cols(spec, join.is_some(), &guards)? {
        Ok(cols) => cols,
        Err(why) => return Ok(Err(why)),
    };
    let select = cols
        .iter()
        .map(|c| format!("  {}", c.sql))
        .collect::<Vec<_>>()
        .join(",\n");
    let header = format!(
        "CREATE OR REPLACE VIEW {} AS\nSELECT\n{select}\n",
        spec.qualified_name()
    );

    // No relation at all: publish the shape, no rows. `WHERE FALSE` keeps the
    // declared types without inventing a row.
    let Some(view) = view.filter(|_| !all_empty) else {
        return Ok(Ok((format!("{header}WHERE FALSE"), None)));
    };

    let mut sql = format!("{header}FROM dbt_internal.{view} {BASE}\n");
    // The lateral goes between the base relation and the join, so the join's ON
    // clause is still attached to the join. No spec needs both today, and the
    // combination is refused rather than emitted untested: a comma-join followed
    // by a `LEFT JOIN` is exactly where SQL's precedence surprises live.
    if let Some(u) = unnest {
        if join.is_some() {
            return Ok(Err(format!(
                "{} would unnest and join at once",
                spec.qualified_name()
            )));
        }
        sql.push_str(&format!("  , unnest({}) AS {ELEM_REL}\n", u.list));
    }
    // LEFT, never INNER: the Rust path's `compile_map.get(&uid)` leaves the
    // fields unset on a miss rather than dropping the node.
    if let Some(j) = join {
        sql.push_str(&format!(
            "LEFT JOIN dbt_internal.{} {JOINED} ON {BASE}.{on} = {JOINED}.{on}\n",
            j.view,
            on = j.on
        ));
    }

    // Liveness is a semi-join on `parse/alive`, which only exists once a parse
    // has written it. The Rust path treats a missing file as "prune nothing"
    // (`load_alive_ids` returns `None`), so the join is omitted rather than
    // emitted against a missing file — which would drop every row.
    let mut wheres: Vec<String> = Vec::new();
    if let (Some(on), true) = (alive_on, alive_exists) {
        wheres.push(format!(
            "{BASE}.{on} IN (SELECT unique_id FROM dbt_internal.epoch_parse_alive)"
        ));
    }
    if let Some(k) = keep {
        wheres.push(k.to_string());
    }
    // The element predicate filters the unnested rows, not the base rows, so it
    // belongs in the same WHERE: a base row is already multiplied by the time
    // this is applied, and dropping one element leaves the others.
    if let Some(k) = unnest.and_then(|u| u.keep) {
        wheres.push(k.to_string());
    }
    if let Filter::ResourceTypeIn(types) = spec.filter {
        let list = types
            .iter()
            .map(|t| format!("'{t}'"))
            .collect::<Vec<_>>()
            .join(", ");
        wheres.push(format!("{BASE}.resource_type IN ({list})"));
    }
    for (i, w) in wheres.iter().enumerate() {
        let lead = if i == 0 { "WHERE" } else { "  AND" };
        sql.push_str(&format!("{lead} {w}\n"));
    }
    Ok(Ok((sql.trim_end().to_string(), Some(view))))
}

/// The staging tables a spec's columns resolve against, deduplicated.
fn staging_tables(spec: &TableSpec) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = spec
        .cols
        .iter()
        .filter(|c| c.ty.is_none())
        .filter_map(|c| owning_table(spec, c.src))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// One output column, resolved down to the SQL that produces it.
struct ResolvedCol {
    sql: String,
}

/// Resolve every column of a spec, or report the first staging column with no
/// epoch expression.
///
/// `joined` is false when the join relation has no files on disk, in which case
/// its columns degrade to what the Rust path produces from an empty lookup map.
/// `guards` carries the per-staging-table row predicates described in
/// [`table_view_sql`].
fn resolve_cols(
    spec: &TableSpec,
    joined: bool,
    guards: &[(&'static str, &'static str)],
) -> Result<Result<Vec<ResolvedCol>, String>, IndexError> {
    let out = out_schema(spec)?;
    let mut resolved = Vec::with_capacity(spec.cols.len());
    for (col, field) in spec.cols.iter().zip(out.fields()) {
        // A spec-declared null column has no staging source on either path.
        if col.ty.is_some() {
            resolved.push(ResolvedCol {
                sql: column_sql(EpochExpr::Null, col.out, field.data_type(), None)?,
            });
            continue;
        }
        let Some(table) = owning_table(spec, col.src) else {
            return Ok(Err(format!(
                "{} is assembled in Rust",
                spec.qualified_name()
            )));
        };
        // A staging table nothing writes needs no per-column mapping: every
        // column of it is null on both paths.
        let expr = if matches!(epoch::origin(table), Some(Origin::Empty)) {
            EpochExpr::Null
        } else {
            match epoch::epoch_expr(table, col.src) {
                Some(expr) => expr,
                None => return Ok(Err(format!("{}.{} is unmapped", table, col.src))),
            }
        };
        let expr = match expr {
            // `Same` names the *staging* column, which for a renamed output
            // column is not `col.out`.
            EpochExpr::Same => EpochExpr::Col(col.src),
            EpochExpr::JoinCol(_) if !joined => EpochExpr::Null,
            EpochExpr::JoinJsonList(_) if !joined => EpochExpr::EmptyList,
            other => other,
        };
        let guard = guards.iter().find(|(t, _)| *t == table).map(|(_, g)| *g);
        resolved.push(ResolvedCol {
            sql: column_sql(expr, col.out, field.data_type(), guard)?,
        });
    }
    Ok(Ok(resolved))
}

/// The staging table an output column resolves against, mirroring
/// [`super::project::project_join`]'s left-then-right order.
pub(super) fn owning_table(spec: &TableSpec, src_col: &str) -> Option<&'static str> {
    match spec.src {
        Src::Table(t) => Some(t),
        Src::Join { left, right, .. } => {
            if crate::parquet::schema_for(left)
                .field_with_name(src_col)
                .is_ok()
            {
                Some(left)
            } else {
                Some(right)
            }
        }
        Src::Own => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// The output tables the mapping cannot express, pinned so the set can only
    /// shrink.
    ///
    /// Adding a name here is a regression: it means an output table that used to
    /// be queryable from the epoch files no longer is. Removing one is the point
    /// of the exercise.
    #[test]
    fn unmappable_tables_only_shrink() {
        const SKIPPED: &[&str] = &[];
        let unmappable = unmappable_tables().expect("unmappable_tables");
        let got: BTreeSet<&str> = unmappable.iter().map(String::as_str).collect();
        let want: BTreeSet<&str> = SKIPPED.iter().copied().collect();
        let new: Vec<&&str> = got.difference(&want).collect();
        assert!(new.is_empty(), "newly unmappable output tables: {new:?}");
        let fixed: Vec<&&str> = want.difference(&got).collect();
        assert!(
            fixed.is_empty(),
            "these tables are now covered — remove them from SKIPPED: {fixed:?}"
        );
    }

    /// Scaffolding while the mapping is being filled in: print the generated
    /// file for a real metadata directory, with what it could not cover.
    #[test]
    #[ignore = "scaffolding: needs FS_INFO_SCHEMA_BENCH_METADATA"]
    fn print_generated() {
        let Ok(dir) = std::env::var("FS_INFO_SCHEMA_BENCH_METADATA") else {
            eprintln!("SKIP: FS_INFO_SCHEMA_BENCH_METADATA is not set");
            return;
        };
        let g = generate(Path::new(&dir)).expect("generate");
        println!("{}", g.sql);
        println!("-- skipped ({}):", g.skipped.len());
        for t in &g.skipped {
            println!("--   {t}");
        }
    }
}
