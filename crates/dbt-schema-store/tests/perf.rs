//! Performance smoke tests for the parquet schema cache.
//!
//! These are not micro-benchmarks — they measure wall-clock time for realistic
//! N values and print results so regressions are visible in CI output.
//! Run with: cargo xtask test --llm --no-external-deps -p dbt-schema-store perf

use std::{collections::HashMap, hint::black_box, sync::Arc, time::Instant};

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use dbt_ident::Ident;
use dbt_schema_store::{CanonicalFqn, SchemaStoreTrait, store::SchemaStore};
use tempfile::TempDir;

fn make_schema(n_cols: usize) -> SchemaRef {
    let fields: Vec<Field> = (0..n_cols)
        .map(|i| Field::new(format!("col_{i}"), DataType::Utf8, true))
        .collect();
    Arc::new(Schema::new(fields))
}

fn cfqn(cat: &str, schema: &str, table: &str) -> CanonicalFqn {
    CanonicalFqn::new(&Ident::new(cat), &Ident::new(schema), &Ident::new(table))
}

/// Builds a store pre-registered with `n` frontier entries and their schemas,
/// then saves and reloads. Prints timing and entry count.
fn run_perf(label: &str, n: usize, cols_per_schema: usize) {
    let dir = TempDir::new().unwrap();

    // ── write ──────────────────────────────────────────────────────────────────
    let mut frontier: HashMap<CanonicalFqn, String> = HashMap::new();
    for i in 0..n {
        frontier.insert(
            cfqn("db", "s", &format!("t{i}")),
            format!("source.pkg.t{i}"),
        );
    }

    let schema = make_schema(cols_per_schema);

    let t0 = Instant::now();
    let store = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier.clone(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    let init_ms = t0.elapsed().as_millis();

    let t1 = Instant::now();
    for c in frontier.keys() {
        store
            .register_schema(c, None, schema.clone(), false)
            .unwrap();
    }
    let register_ms = t1.elapsed().as_millis();

    let t2 = Instant::now();
    store.save(dir.path()).unwrap();
    let save_ms = t2.elapsed().as_millis();

    // ── read ───────────────────────────────────────────────────────────────────
    let t3 = Instant::now();
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier.clone(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    let load_ms = t3.elapsed().as_millis();

    // Force schema deserialization for all entries (lazy load).
    let t4 = Instant::now();
    for c in frontier.keys() {
        let _ = black_box(store2.get_schema(c));
    }
    let deser_ms = t4.elapsed().as_millis();

    println!(
        "[perf/{label}] n={n} cols={cols_per_schema} | \
         init={init_ms}ms register={register_ms}ms save={save_ms}ms | \
         load(bytes)={load_ms}ms deser_all={deser_ms}ms"
    );

    // Sanity check: all entries round-trip.
    let loaded = store2.exists(frontier.keys().next().unwrap());
    assert!(loaded, "entries must survive save/reload");
}

#[test]
#[ignore = "timing only — run locally with: cargo test -p dbt-schema-store -- --ignored"]
fn perf_100_schemas_20_cols() {
    run_perf("small", 100, 20);
}

#[test]
#[ignore = "timing only — run locally with: cargo test -p dbt-schema-store -- --ignored"]
fn perf_1000_schemas_20_cols() {
    run_perf("medium", 1000, 20);
}

#[test]
#[ignore = "timing only — run locally with: cargo test -p dbt-schema-store -- --ignored"]
fn perf_5000_schemas_20_cols() {
    run_perf("large", 5000, 20);
}

#[test]
#[ignore = "timing only — run locally with: cargo test -p dbt-schema-store -- --ignored"]
fn perf_1000_schemas_100_cols() {
    run_perf("wide", 1000, 100);
}
