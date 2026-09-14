//! Differential test: the view layer against the materialized layer.
//!
//! [`super::epoch_views`] republishes the information schema as views over the
//! epoch files, restating in SQL what `ingest/metadata_to_parquet.rs` does in
//! Rust. Roughly 200 of those restatements are a JSON path or a column rename,
//! each of which can be wrong in a way no compiler catches: a mistyped key
//! yields an all-null column, a missed `continue` yields extra rows. The only
//! honest check is to build both layers from the same metadata directory and
//! compare them table by table.
//!
//! Driven by `FS_INFO_SCHEMA_DIFF_METADATA`, a path to a real
//! `target/private/metadata/`. Unset, the test skips: the crate's own fixtures do not
//! cover enough of the schema for the comparison to mean anything, and a
//! synthetic corpus would only test the generator against itself. Generate one
//! with `dbt build --write-metadata --write-lineage` on a project — `parse`
//! covers the parse tables, and only a `build` fills in compiled code, column
//! lineage and run results.
//!
//! What the corpus contains decides what the test checks: two empty relations are
//! equal, so a table for a resource type the project does not use passes without
//! its mapping having been read. The run prints which tables were empty on both
//! sides for exactly that reason, and a corpus is worth widening until that list
//! is only the tables nothing writes yet. Note that the legacy top-level
//! `semantic_models:`/`metrics:` YAML is dropped during parsing — a corpus needs
//! the current form, where a model declares `semantic_model:` and its columns
//! carry `entity:`/`dimension:`, or `dbt.semantic_*` and `dbt.metrics` come out
//! empty and appear to agree.
//!
//! Columns excluded from the comparison are listed one by one in [`exempt`], and
//! only for values that cannot agree by construction. A difference the
//! materializing path is simply wrong about is not exempted — it is fixed there.

use std::collections::BTreeSet;
use std::path::Path;

use crate::db::Db;
use crate::format::cell_to_string;
use crate::ingest::{
    PARSE_ALIVE, PARSE_COLUMNS_SUBDIR, PARSE_GENERATION, PARSE_NODES_SUBDIR, PARSE_PROJECT,
    PARSE_RESOLVER_STATE,
};

use super::schema::INFO_SCHEMA;
use super::spec::TableSpec;
use super::{Materializer, epoch, epoch_views, write_info_schema_with};

/// Columns dropped from the comparison, and why.
///
/// Every entry is a value the *materializing* path invents at conversion time
/// rather than reading out of the epoch files, so re-running the conversion
/// changes it. The view layer has no conversion time to stamp, so it reports
/// what the epoch files say. Both are right; they are just not comparable.
///
/// Nothing else belongs here. Where the two layers disagree because the
/// materializing path is wrong, the view layer does not reproduce the bug and the
/// difference is not listed: the point of this module is to keep the two layers
/// exchangeable, and bug-for-bug compatibility would mean writing SQL whose only
/// justification is a Rust implementation detail. Such a failure is a defect to
/// fix in `ingest/metadata_to_parquet.rs`, which is what happened to the four
/// this test found first.
fn exempt(spec: &TableSpec, out_col: &str) -> bool {
    // `write_*` fills most `ingested_at` columns with `now` rather than with the
    // stamp in the epoch row, so this differs by however long the two runs are
    // apart. Matched on the output name as well as the source, because a
    // `Src::Own` spec declares the column with no source at all and its builder
    // reads the same conversion-stamped staging value.
    if out_col == "ingested_at"
        || spec
            .cols
            .iter()
            .any(|c| c.out == out_col && c.src == "ingested_at")
    {
        return true;
    }
    // Both layers now fill this from the generation row's stamp — the view in the
    // `project` arm, `fill_last_full_parse_at` after projection — but they read a
    // different copy of it: the Arrow path reads staging, where
    // `IndexWriter::write_dbt_table` has already overwritten `ingested_at` with
    // the ingest's clock. Same reason `ingested_at` itself is exempt above.
    // `schema_version` is *not* exempt: the view inlines the same constant, so the
    // two layers agree.
    if spec.qualified_name() == "dbt.project" && out_col == "last_full_parse_at" {
        return true;
    }
    false
}

/// One `(column_name, column_type)` pair per column, in declaration order.
fn describe(db: &mut Db, target: &str) -> Vec<(String, String)> {
    let batches = db
        .execute_query(&format!("DESCRIBE {target}"))
        .unwrap_or_else(|e| panic!("DESCRIBE {target}: {e}"));
    let mut out = Vec::new();
    for batch in &batches {
        let name = batch.schema().index_of("column_name").expect("column_name");
        let ty = batch.schema().index_of("column_type").expect("column_type");
        for row in 0..batch.num_rows() {
            out.push((
                cell_to_string(batch, row, name),
                cell_to_string(batch, row, ty),
            ));
        }
    }
    out
}

#[test]
#[ignore = "differential: needs FS_INFO_SCHEMA_DIFF_METADATA"]
fn view_layer_matches_materialized_layer() {
    let Ok(metadata_dir) = std::env::var("FS_INFO_SCHEMA_DIFF_METADATA") else {
        eprintln!("SKIP: FS_INFO_SCHEMA_DIFF_METADATA is not set");
        return;
    };
    // Absolute, not canonical: `generate` needs an absolute path because DuckDB
    // resolves globs against the CWD, and `std::fs::canonicalize` is banned
    // workspace-wide for breaking on Windows.
    let metadata_dir = std::path::absolute(&metadata_dir).expect("metadata dir");

    // Materialized layer, into a scratch directory so the project's own
    // `target/info_schema/` is left alone.
    let tmp = tempfile::tempdir().expect("tempdir");
    let out_root = tmp.path().join("info_schema");
    write_info_schema_with(
        Materializer::Copy,
        &metadata_dir,
        &out_root,
        &tmp.path().join(super::STAGING_DIR_NAME),
    )
    .expect("write_info_schema COPY");
    let parquet_dir = super::versioned_dir(&out_root);

    // View layer, executed statement by statement rather than by splitting the
    // generated file on `;`.
    let generated = epoch_views::generate(&metadata_dir).expect("generate");
    let mut db = Db::open_memory().expect("open_memory");
    for stmt in &generated.statements {
        db.execute_update(stmt)
            .unwrap_or_else(|e| panic!("executing\n{stmt}\n{e}"));
    }

    let skipped: BTreeSet<&str> = generated.skipped.iter().map(String::as_str).collect();
    let mut failures: Vec<String> = Vec::new();
    let mut compared = 0;
    let mut empty: Vec<String> = Vec::new();
    for spec in INFO_SCHEMA {
        if skipped.contains(spec.qualified_name().as_str()) {
            continue;
        }
        compared += 1;
        let name = spec.qualified_name();
        if count(&mut db, &name) == 0 {
            empty.push(name);
        }
        if let Err(why) = compare_table(&mut db, spec, &parquet_dir) {
            failures.push(why);
        }
    }
    assert!(compared > 0, "no table was compared");
    assert!(
        failures.is_empty(),
        "{} of {compared} tables differ:\n\n{}",
        failures.len(),
        failures.join("\n\n")
    );
    eprintln!("{compared} tables match, {} skipped", skipped.len());
    // Two empty relations are equal, so a table the corpus has no rows for
    // passes without its mapping having been exercised at all. Not a failure —
    // no one corpus contains every resource type — but it is the difference
    // between "verified" and "compiles", so it is named rather than counted as a
    // pass. Widening the corpus is what shortens this list.
    if !empty.is_empty() {
        eprintln!(
            "{} of those are empty on both sides, so their mapping is unverified:\n  {}",
            empty.len(),
            empty.join("\n  ")
        );
    }
}

/// Compare one output table: first its shape, then its rows.
///
/// Shape first because a column-order difference makes every row differ, and
/// reporting 40k row diffs for one transposed column buries the cause.
fn compare_table(db: &mut Db, spec: &TableSpec, parquet_dir: &Path) -> Result<(), String> {
    let name = spec.qualified_name();
    let path = parquet_dir.join(spec.file_name());
    let parquet = format!(
        "(SELECT * FROM read_parquet('{}'))",
        path.display().to_string().replace('\'', "''")
    );

    let view_shape = describe(db, &name);
    let file_shape = describe(db, &parquet);
    if view_shape != file_shape {
        return Err(format!(
            "{name}: shape differs\n{}",
            shape_diff(&view_shape, &file_shape)
        ));
    }

    // Exempt columns are dropped from both sides rather than compared and
    // ignored, so a NULL on one side cannot mask a difference elsewhere in the
    // row: `EXCEPT` compares whole rows. JSON-valued columns come out too, and
    // are compared parsed by `compare_json_col` — see [`epoch::is_json_text`].
    let mut cols: Vec<String> = Vec::new();
    let mut json_cols: Vec<String> = Vec::new();
    for (c, _) in &view_shape {
        if exempt(spec, c) {
            continue;
        }
        let quoted = format!("\"{}\"", c.replace('"', "\"\""));
        if is_json_col(spec, c) {
            json_cols.push(quoted);
        } else {
            cols.push(quoted);
        }
    }
    let mut failures: Vec<String> = Vec::new();
    for col in &json_cols {
        if let Some(why) = compare_json_col(db, &name, &parquet, col) {
            failures.push(why);
        }
    }
    if cols.is_empty() {
        return json_failures(&name, failures);
    }
    let list = cols.join(", ");

    let (in_view, in_file) = (count(db, &name), count(db, &parquet));

    // Symmetric multiset difference. `EXCEPT ALL` rather than `EXCEPT` so a
    // duplicated row on one side is a difference, not a coincidence.
    let diff = format!(
        "(SELECT {list} FROM {name} EXCEPT ALL SELECT {list} FROM {parquet})\n\
         UNION ALL\n\
         (SELECT {list} FROM {parquet} EXCEPT ALL SELECT {list} FROM {name})"
    );
    let differing = count(db, &format!("({diff})"));
    if differing == 0 {
        return json_failures(&name, failures);
    }
    failures.insert(
        0,
        format!(
            "{differing} rows differ ({in_view} in view, {in_file} in file)\n{}",
            blame(db, &name, &parquet, &cols)
        ),
    );
    json_failures(&name, failures)
}

/// Collect per-column failures under the table's name, or `Ok` if there are none.
fn json_failures(name: &str, failures: Vec<String>) -> Result<(), String> {
    if failures.is_empty() {
        return Ok(());
    }
    Err(format!("{name}: {}", failures.join("\n")))
}

/// Whether an output column holds serialised JSON, per the epoch mapping.
fn is_json_col(spec: &TableSpec, out_col: &str) -> bool {
    spec.cols
        .iter()
        .find(|c| c.out == out_col && c.ty.is_none())
        .and_then(|c| epoch_views::owning_table(spec, c.src).map(|t| (t, c.src)))
        .is_some_and(|(table, src)| epoch::is_json_text(table, src))
}

/// Compare one JSON-valued column by parsing both sides.
///
/// Keyed on `unique_id` where the table has one, so a mismatch names the row.
/// Otherwise the two sides are compared as multisets of canonical JSON, which is
/// weaker — it cannot say which row is wrong — but still catches a value present
/// on one side only.
fn compare_json_col(db: &mut Db, view: &str, parquet: &str, col: &str) -> Option<String> {
    let canon = |s: &str| -> String {
        match serde_json::from_str::<serde_json::Value>(s) {
            // Re-serialising through `Value` sorts object keys on both sides, so
            // what remains is a difference in structure or in a leaf.
            Ok(v) => v.to_string(),
            // Not JSON on one side is itself the difference, so it is compared as
            // the text it is rather than silently skipped.
            Err(_) => s.to_string(),
        }
    };
    let keyed = describe(db, view).iter().any(|(c, _)| c == "unique_id");
    if keyed {
        let rows = db
            .execute_query(&format!(
                "SELECT v.unique_id, v.{col}, f.{col} FROM {view} v \
                 JOIN {parquet} f ON v.unique_id = f.unique_id \
                 WHERE v.{col} IS DISTINCT FROM f.{col}"
            ))
            .ok()?;
        let mut n = 0;
        let mut first: Option<(String, String, String)> = None;
        for batch in &rows {
            for row in 0..batch.num_rows() {
                let (a, b) = (cell_to_string(batch, row, 1), cell_to_string(batch, row, 2));
                if canon(&a) == canon(&b) {
                    continue;
                }
                n += 1;
                if first.is_none() {
                    first = Some((cell_to_string(batch, row, 0), a, b));
                }
            }
        }
        let (uid, a, b) = first?;
        return Some(format!(
            "  {col}: {n} differ as JSON\n    at: {uid}\n    view: {}\n    file: {}",
            clip(&a),
            clip(&b)
        ));
    }
    let side = |db: &mut Db, rel: &str| -> Vec<String> {
        let mut out = Vec::new();
        for batch in db
            .execute_query(&format!("SELECT {col} FROM {rel}"))
            .unwrap_or_default()
        {
            for row in 0..batch.num_rows() {
                out.push(canon(&cell_to_string(&batch, row, 0)));
            }
        }
        out.sort();
        out
    };
    let (v0, f0) = (side(db, view), side(db, parquet));
    if v0 == f0 {
        return None;
    }
    let n = v0.len().abs_diff(f0.len()).max(1);
    // Only-on-one-side values, each computed against the *original* other side.
    let v: Vec<&String> = v0.iter().filter(|x| !f0.contains(x)).collect();
    let f: Vec<&String> = f0.iter().filter(|x| !v0.contains(x)).collect();
    Some(format!(
        "  {col}: {n} differ as JSON\n    view: {}\n    file: {}",
        clip(v.first().map(|s| s.as_str()).unwrap_or("<none>")),
        clip(f.first().map(|s| s.as_str()).unwrap_or("<none>"))
    ))
}

/// Which columns the difference is in, with both values from one row.
///
/// A whole-row diff of a 60-column table says nothing useful — the interesting
/// fact is *which* column is wrong. Where the table has a `unique_id` the two
/// sides are joined on it, so the two printed values are the same row's and can
/// be read against each other; the values are otherwise sampled independently
/// and only the column name means anything.
fn blame(db: &mut Db, view: &str, parquet: &str, cols: &[String]) -> String {
    let keyed = cols.iter().any(|c| c == "\"unique_id\"");
    let mut out = String::new();
    for col in cols {
        let (n, sample) = if keyed {
            let pair = format!(
                "SELECT v.{col} AS a, f.{col} AS b FROM {view} v \
                 JOIN {parquet} f ON v.unique_id = f.unique_id \
                 WHERE v.{col} IS DISTINCT FROM f.{col}"
            );
            let n = count(db, &format!("({pair})"));
            let row = db
                .execute_query(&format!("{pair} LIMIT 1"))
                .ok()
                .and_then(|b| {
                    let batch = b.into_iter().find(|b| b.num_rows() > 0)?;
                    Some((cell_to_string(&batch, 0, 0), cell_to_string(&batch, 0, 1)))
                })
                .unwrap_or_default();
            (n, row)
        } else {
            let one = |db: &mut Db, rel: &str, other: &str| {
                db.query_scalar(
                    &format!(
                        "SELECT {col} FROM \
                         (SELECT {col} FROM {rel} EXCEPT ALL SELECT {col} FROM {other}) LIMIT 1"
                    ),
                    0,
                )
                .unwrap_or_default()
            };
            let n = count(
                db,
                &format!(
                    "((SELECT {col} FROM {view} EXCEPT ALL SELECT {col} FROM {parquet}) \
                     UNION ALL \
                     (SELECT {col} FROM {parquet} EXCEPT ALL SELECT {col} FROM {view}))"
                ),
            );
            let v = one(db, view, parquet);
            let f = one(db, parquet, view);
            (n, (v, f))
        };
        if n == 0 {
            continue;
        }
        out.push_str(&format!(
            "  {col}: {n} differ\n    view: {}\n    file: {}\n",
            clip(&sample.0),
            clip(&sample.1)
        ));
    }
    if out.is_empty() {
        // Every column matches on its own but the rows do not, so the difference
        // is in how values are paired up — a join fanning out, or rows present
        // on one side only.
        out.push_str("  no single column differs; the rows are combined differently\n");
    }
    out
}

/// `SELECT count(*) FROM <rel>`, or -1 if the query fails.
fn count(db: &mut Db, rel: &str) -> i64 {
    db.query_count(&format!("SELECT count(*) FROM {rel}"))
        .parse()
        .unwrap_or(-1)
}

/// Values in this schema include whole compiled models and whole config blobs.
fn clip(s: &str) -> String {
    const MAX: usize = 160;
    match s.char_indices().nth(MAX) {
        Some((i, _)) => format!("{}… ({} bytes)", &s[..i], s.len()),
        None => s.to_string(),
    }
}

/// The first column position where the two shapes disagree, with a few columns
/// of context either side. Printing both full 58-column shapes for one renamed
/// column is unreadable.
fn shape_diff(view: &[(String, String)], file: &[(String, String)]) -> String {
    let at = (0..view.len().max(file.len())).find(|&i| view.get(i) != file.get(i));
    let one = |s: &[(String, String)], i: usize| match s.get(i) {
        Some((c, t)) => format!("{c} {t}"),
        None => "<absent>".to_string(),
    };
    match at {
        Some(i) => format!(
            "  first difference at column {i}:\n    view: {}\n    file: {}",
            one(view, i),
            one(file, i)
        ),
        // Unreachable while the lists are compared elementwise, but a shape
        // mismatch with no differing element would be worth seeing rather than
        // silently reported as identical.
        None => format!("  {} vs {} columns", view.len(), file.len()),
    }
}

/// `Supersede::LatestGroup` replaces a whole group, not row by row.
///
/// The rule the column and lineage sources publish under: an epoch republishes a
/// node's entire column set (or a target's entire incoming edge set), so the
/// newest epoch that mentions the node replaces everything the older ones said
/// about it. `Supersede::LatestBy("unique_id, column_name")` — what these
/// relations used before — cannot express that: a column only the older epoch has
/// wins the partition it is the only member of, and a dropped column stays
/// visible forever. This is the corpus that tells the two apart, and it is
/// synthetic because no real one has two epochs for the same node with different
/// column sets.
#[test]
fn latest_group_supersedes_the_whole_group() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let dir = tmp.path().join("compile/columns");
    std::fs::create_dir_all(&dir).expect("create_dir_all");

    let mut db = Db::open_memory().expect("open_memory");
    db.execute_update("CREATE SCHEMA IF NOT EXISTS dbt_internal")
        .expect("create schema");
    // Epoch 0: model `a` has three columns, model `b` one.
    // Epoch 1: model `a` is recompiled and now has two — `c` is gone. `b` is not
    // mentioned at all, so its epoch-0 row has to survive.
    let mut write = |file: &str, rows: &str| {
        let path = dir.join(file);
        db.execute_update(&format!(
            "COPY ({rows}) TO '{}' (FORMAT parquet)",
            path.display()
        ))
        .unwrap_or_else(|e| panic!("writing {file}: {e}"));
    };
    write(
        "v1_0.parquet",
        "SELECT * FROM (VALUES ('a', 'x'), ('a', 'y'), ('a', 'c'), ('b', 'z')) \
         AS t(unique_id, column_name)",
    );
    write(
        "v1_1.parquet",
        "SELECT * FROM (VALUES ('a', 'x'), ('a', 'y')) AS t(unique_id, column_name)",
    );

    let rel = epoch::EpochRelation {
        view: "epoch_group_test",
        dir: "compile/columns",
        single_file: false,
        supersede: epoch::Supersede::LatestGroup("unique_id"),
    };
    let stmt = rel.create_view_sql(tmp.path());
    db.execute_update(&stmt)
        .unwrap_or_else(|e| panic!("executing\n{stmt}\n{e}"));

    let rows = db
        .execute_query(
            "SELECT unique_id || '.' || column_name FROM dbt_internal.epoch_group_test \
             ORDER BY 1",
        )
        .expect("query");
    let mut got: Vec<String> = Vec::new();
    for batch in &rows {
        for i in 0..batch.num_rows() {
            got.push(cell_to_string(batch, i, 0));
        }
    }
    assert_eq!(
        got,
        vec!["a.x".to_string(), "a.y".to_string(), "b.z".to_string()],
        "the newest epoch that mentions `a` replaces every row it had — `a.c` is \
         dropped and not duplicated — while `b`, which epoch 1 does not mention, \
         keeps its epoch-0 row"
    );
}

/// A metadata directory holding the parse relations and nothing else.
///
/// The files are empty: `generate` decides which views to emit from whether an epoch file
/// *exists*, and the tests below read the statements rather than run them.
fn parse_corpus() -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    for sub in [PARSE_NODES_SUBDIR, PARSE_COLUMNS_SUBDIR] {
        let dir = tmp.path().join(sub);
        std::fs::create_dir_all(&dir).expect("create_dir_all");
        std::fs::write(dir.join("v1_0.parquet"), b"").expect("write epoch");
    }
    for file in [
        PARSE_ALIVE,
        PARSE_PROJECT,
        PARSE_RESOLVER_STATE,
        PARSE_GENERATION,
    ] {
        std::fs::write(tmp.path().join(file), b"").expect("write epoch");
    }
    tmp
}

/// The check surface: what [`epoch_views::parse_safe_statements`] registers, and nothing else.
///
/// Statement-level rather than differential, so it needs no corpus and runs everywhere. Only
/// the *presence* of an epoch file decides which views `generate` emits, so touching empty
/// ones is enough to make it emit the parse tables — the statements are inspected, never
/// executed.
///
/// What it pins is a boundary, which is why it is an allowlist and not a scan for the two
/// namespaces a reviewer would think of. Check SQL may name any relation, so a table nobody
/// remembered — `dbt_internal.node_input_files` is published and empty at parse — must be
/// *absent* rather than merely undocumented, and absent is what makes naming it a binder
/// error instead of a clean pass over no rows.
#[test]
fn parse_safe_statements_publish_only_the_check_views() {
    let tmp = parse_corpus();
    let dir = tmp.path();

    let statements = epoch_views::parse_safe_statements(dir).expect("parse-safe statements");
    let parse_safe: BTreeSet<&str> = super::parse_safe::VIEWS.iter().map(|v| v.name).collect();
    let epochs: BTreeSet<&str> = epoch::EPOCH_RELATIONS.iter().map(|r| r.view).collect();

    let mut published: BTreeSet<String> = BTreeSet::new();
    for stmt in &statements {
        if stmt.starts_with("CREATE SCHEMA") {
            continue;
        }
        let target = stmt
            .strip_prefix("CREATE OR REPLACE VIEW ")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_else(|| panic!("not a view statement:\n{stmt}"));
        let (schema, name) = target.split_once('.').expect("qualified name");
        match schema {
            "dbt_internal" => assert!(
                epochs.contains(name),
                "only the epoch relations belong in dbt_internal, not {target}:\n{stmt}"
            ),
            "dbt" => {
                assert!(
                    parse_safe.contains(name),
                    "{target} is not a parse-safe view and must not be registered:\n{stmt}"
                );
                published.insert(name.to_string());
            }
            _ => panic!("unexpected schema in {target}:\n{stmt}"),
        }
    }

    // Not equality with `parse_safe`: a view whose relation has no files is skipped, and this
    // corpus has the parse relations only. `dbt.models` is the one every check reaches for.
    assert!(
        published.contains("models"),
        "the parse corpus should publish dbt.models; got {published:?}"
    );
}

/// The columns a check may read are the ones [`super::parse_safe`] declares — no more, and in
/// that order.
///
/// The restriction is the reason the views exist: zero rows is a pass, so a column still empty
/// at parse does not fail a check, it quietly satisfies one. `WHERE compiled_code IS NULL`
/// must fail to bind rather than match every row.
#[test]
fn each_check_view_offers_exactly_its_declared_columns() {
    let tmp = parse_corpus();
    let dir = tmp.path();

    let statements = epoch_views::parse_safe_statements(dir).expect("parse-safe statements");
    let mut seen = 0;
    for view in super::parse_safe::VIEWS {
        let prefix = format!("CREATE OR REPLACE VIEW dbt.{} AS SELECT ", view.name);
        let Some(stmt) = statements.iter().find(|s| s.starts_with(&prefix)) else {
            continue;
        };
        seen += 1;
        let projection = stmt[prefix.len()..]
            .split(" FROM (")
            .next()
            .expect("projection");
        let expected = view
            .cols
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(", ");
        assert_eq!(
            projection, expected,
            "dbt.{} must offer exactly its declared columns",
            view.name
        );
    }
    assert!(seen > 0, "no parse-safe view was published to compare");
}
