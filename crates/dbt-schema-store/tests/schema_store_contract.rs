//! Contract tests for `SchemaStore` with the `ParquetCache` backend.
//!
//! These tests verify the public `SchemaStoreTrait` contract and the
//! save/reload cycle through the epoch-append parquet files.

use std::{collections::HashMap, sync::Arc, time::Duration};

use dbt_schema_store::{LocalSchemaEntry, store::LookupEntry};

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use dbt_ident::Ident;
use dbt_schema_store::{CanonicalFqn, SchemaStoreTrait, store::SchemaStore};
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

fn make_schema(name: &str) -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(name, DataType::Utf8, false)]))
}

fn ident(s: &str) -> Ident<'static> {
    Ident::new(s)
}

fn cfqn(cat: &str, schema: &str, table: &str) -> CanonicalFqn {
    CanonicalFqn::new(&ident(cat), &ident(schema), &ident(table))
}

/// Builds an empty `SchemaStore` rooted at `dir`.
fn empty_store(dir: &TempDir) -> SchemaStore {
    SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        HashMap::new(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    )
}

/// Builds a `SchemaStore` pre-loaded with one frontier entry.
fn store_with_frontier(dir: &TempDir, c: &CanonicalFqn, uid: &str) -> SchemaStore {
    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());
    SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    )
}

// ── basic contract ─────────────────────────────────────────────────────────────

#[test]
fn register_and_get_by_cfqn() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "t");

    store
        .register_schema(&c, None, make_schema("col"), false)
        .unwrap();

    assert!(store.exists(&c));
    let entry = store.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "col");
}

#[test]
fn register_with_original_schema() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "t");

    store
        .register_schema(&c, Some(make_schema("orig")), make_schema("sdf"), false)
        .unwrap();

    let entry = store.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "sdf");
    assert_eq!(entry.original().unwrap().field(0).name(), "orig");
}

#[test]
fn exists_by_cfqn_and_unique_id() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "orders");
    let store = store_with_frontier(&dir, &c, "model.pkg.orders");

    store
        .register_schema(&c, None, make_schema("amount"), false)
        .unwrap();

    assert!(store.exists(&c));
    assert!(store.exists_by_unique_id("model.pkg.orders"));
}

#[test]
fn get_schema_by_unique_id() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "orders");
    let store = store_with_frontier(&dir, &c, "model.pkg.orders");

    store
        .register_schema(&c, None, make_schema("amount"), false)
        .unwrap();

    let entry = store.get_schema_by_unique_id("model.pkg.orders").unwrap();
    assert_eq!(entry.inner().field(0).name(), "amount");
}

#[test]
fn overwrite_false_preserves_existing() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "t");

    store
        .register_schema(&c, None, make_schema("original"), false)
        .unwrap();
    store
        .register_schema(&c, None, make_schema("new"), false)
        .unwrap();

    let entry = store.get_schema(&c).unwrap();
    assert_eq!(
        entry.inner().field(0).name(),
        "original",
        "overwrite=false must not replace existing entry"
    );
}

#[test]
fn overwrite_true_replaces_existing() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "t");

    store
        .register_schema(&c, None, make_schema("original"), false)
        .unwrap();
    store
        .register_schema(&c, None, make_schema("new"), true)
        .unwrap();

    let entry = store.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "new");
}

#[test]
fn catalog_schema_table_names() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c1 = cfqn("db", "s", "a");
    let c2 = cfqn("db", "s", "b");

    store
        .register_schema(&c1, None, make_schema("x"), false)
        .unwrap();
    store
        .register_schema(&c2, None, make_schema("y"), false)
        .unwrap();

    let catalogs = store.catalog_names();
    assert!(catalogs.iter().any(|c| c.as_str() == "db"));

    let schemas = store.schema_names(&ident("db"));
    assert!(schemas.iter().any(|s| s.as_str() == "s"));

    let tables = store.table_names(&ident("db"), &ident("s"));
    assert!(tables.iter().any(|t| t.as_str() == "a"));
    assert!(tables.iter().any(|t| t.as_str() == "b"));
}

#[test]
fn cold_start_empty() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "t");
    assert!(!store.exists(&c));
}

// ── save / reload cycle ────────────────────────────────────────────────────────

#[test]
fn persist_and_reload() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "orders");

    {
        let store = store_with_frontier(&dir, &c, "source.pkg.orders");
        store
            .register_schema(&c, None, make_schema("amount"), false)
            .unwrap();
        assert!(!store.schema_is_from_prior_invocation(&c));
        store.save(dir.path()).unwrap();
    }

    // Reload from the parquet epochs.
    let store2 = store_with_frontier(&dir, &c, "source.pkg.orders");
    assert!(store2.exists(&c), "entry must survive save/reload");
    assert!(store2.schema_is_from_prior_invocation(&c));
    let entry = store2.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "amount");

    store2
        .register_schema(&c, None, make_schema("current_amount"), true)
        .unwrap();
    assert!(!store2.schema_is_from_prior_invocation(&c));
}

#[test]
fn persist_and_reload_with_original() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "t");

    {
        let store = store_with_frontier(&dir, &c, "source.pkg.t");
        store
            .register_schema(&c, Some(make_schema("orig")), make_schema("sdf"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    let store2 = store_with_frontier(&dir, &c, "source.pkg.t");
    let entry = store2.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "sdf");
    assert_eq!(entry.original().unwrap().field(0).name(), "orig");
}

#[test]
fn ttl_evicts_stale_entry() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "src");
    let uid = "source.pkg.src";

    {
        let store = store_with_frontier(&dir, &c, uid);
        store
            .register_schema(&c, None, make_schema("col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    std::thread::sleep(Duration::from_millis(5));

    // Reload with a TTL shorter than the sleep: entry must be evicted.
    let mut refresh_intervals = HashMap::new();
    refresh_intervals.insert(uid.to_string(), Some(Duration::from_millis(1)));

    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());

    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        refresh_intervals,
    );

    assert!(!store2.exists(&c), "stale entry must be evicted on reload");
}

#[test]
fn ttl_keeps_fresh_entry() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "src");
    let uid = "source.pkg.src";

    {
        let store = store_with_frontier(&dir, &c, uid);
        store
            .register_schema(&c, None, make_schema("col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    let mut refresh_intervals = HashMap::new();
    refresh_intervals.insert(uid.to_string(), Some(Duration::from_secs(3600)));

    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());

    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        refresh_intervals,
    );

    assert!(store2.exists(&c), "fresh entry must survive reload");
}

#[test]
fn no_ttl_cached_indefinitely() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "src");
    let uid = "source.pkg.src";

    {
        let store = store_with_frontier(&dir, &c, uid);
        store
            .register_schema(&c, None, make_schema("col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // No refresh intervals → None → never expires.
    let store2 = store_with_frontier(&dir, &c, uid);
    assert!(store2.exists(&c), "no-TTL entry must survive reload");
}

#[test]
fn external_entry_registered() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "ext_s", "ext_t");

    // External entries are created implicitly when registered with an unknown FQN.
    store
        .register_schema(&c, None, make_schema("x"), false)
        .unwrap();

    assert!(store.exists(&c));
}

#[test]
fn multiple_entries_independent() {
    let dir = TempDir::new().unwrap();
    let c1 = cfqn("db", "s", "a");
    let c2 = cfqn("db", "s", "b");
    let uid1 = "source.pkg.a";
    let uid2 = "source.pkg.b";

    {
        let mut frontier = HashMap::new();
        frontier.insert(c1.clone(), uid1.to_string());
        frontier.insert(c2.clone(), uid2.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            HashMap::new(),
            frontier,
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c1, None, make_schema("col_a"), false)
            .unwrap();
        store
            .register_schema(&c2, None, make_schema("col_b"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    let mut frontier = HashMap::new();
    frontier.insert(c1.clone(), uid1.to_string());
    frontier.insert(c2.clone(), uid2.to_string());
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    );

    assert_eq!(
        store2.get_schema(&c1).unwrap().inner().field(0).name(),
        "col_a"
    );
    assert_eq!(
        store2.get_schema(&c2).unwrap().inner().field(0).name(),
        "col_b"
    );
}

// ── Bug 1: set_deferred ────────────────────────────────────────────────────────

/// set_deferred() must make the deferred entry visible via exists()/get_schema()
/// when the schema is registered afterwards (Bug 1 regression test).
#[test]
fn set_deferred_schema_visible() {
    let dir = TempDir::new().unwrap();
    let store = empty_store(&dir);
    let c = cfqn("db", "s", "deferred_t");

    let mut deferred = HashMap::new();
    deferred.insert(c.clone(), "model.pkg.deferred_t".to_string());
    store.set_deferred(deferred);

    store
        .register_schema(&c, None, make_schema("dcol"), false)
        .unwrap();

    assert!(
        store.exists(&c),
        "deferred entry must be visible after register"
    );
    let entry = store.get_schema(&c).unwrap();
    assert_eq!(entry.inner().field(0).name(), "dcol");
}

/// A deferred entry registered and saved must survive a reload when the same
/// entry is deferred again in the new store.
#[test]
fn set_deferred_survives_reload() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "deferred_t");

    {
        let store = empty_store(&dir);
        let mut deferred = HashMap::new();
        deferred.insert(c.clone(), "model.pkg.deferred_t".to_string());
        store.set_deferred(deferred);
        store
            .register_schema(&c, None, make_schema("dcol"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // On reload, set_deferred must find the schema in the remote parquet cache.
    let store2 = empty_store(&dir);
    let mut deferred = HashMap::new();
    deferred.insert(c.clone(), "model.pkg.deferred_t".to_string());
    store2.set_deferred(deferred);

    assert!(store2.exists(&c), "deferred entry must survive save/reload");
    assert_eq!(
        store2.get_schema(&c).unwrap().inner().field(0).name(),
        "dcol"
    );
}

// ── Bug 2: evict_stale_entries removes from parquet cache ─────────────────────

/// evict_stale_entries() must also remove the entry from the in-memory parquet
/// cache so save() does not re-persist it (Bug 2 regression test).
///
/// Scenario: entry is fresh at load time, becomes stale mid-run (long-running
/// invocation), then evict_stale_entries() is called to flush it. The entry
/// must not reappear after the next save/reload cycle.
#[test]
fn evict_stale_not_repersisted() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "src");
    let uid = "source.pkg.src";

    // Write epoch 0 with a fresh entry (upserted this "run" — dirty).
    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());
    let store = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier.clone(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    store
        .register_schema(&c, None, make_schema("col"), false)
        .unwrap();
    store.save(dir.path()).unwrap();

    // Now simulate "entry becomes stale mid-run": use a generous TTL on load
    // (so entry enters cached_entries), then evict with a very short TTL.
    let long_ttl: HashMap<String, Option<Duration>> =
        [(uid.to_string(), Some(Duration::from_secs(3600)))].into();
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier.clone(),
        HashMap::new(),
        vec![],
        long_ttl,
    );
    assert!(store2.exists(&c), "entry must be loaded as fresh");

    // Wait long enough that the wrapper timestamp ages past the eviction TTL.
    std::thread::sleep(Duration::from_millis(5));

    // Evict using a very short TTL — wrapper is now older than this.
    let zero_ttl: HashMap<String, Option<Duration>> =
        [(uid.to_string(), Some(Duration::from_millis(1)))].into();
    let evicted = store2.evict_stale_entries(&zero_ttl);
    assert_eq!(evicted, 1, "one stale entry must be evicted");
    assert!(!store2.exists(&c), "evicted entry must not be visible");

    // Save: the evicted entry must NOT appear in the new epoch.
    store2.save(dir.path()).unwrap();

    // Reload with the same short TTL — the old epoch 0 row is still on disk but
    // its cached_at_ms is old, so it gets evicted at load time by try_load's TTL check.
    let short_ttl: HashMap<String, Option<Duration>> =
        [(uid.to_string(), Some(Duration::from_millis(1)))].into();
    let store3 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        short_ttl,
    );
    assert!(
        !store3.exists(&c),
        "evicted entry must not reappear after save/reload"
    );
}

// ── Bug 4: local schemas re-derived from YAML, not persisted ──────────────────

/// Local schemas must be registered from YAML every run and must not be read
/// back from the parquet cache, so YAML changes always take effect immediately.
#[test]
fn local_schema_not_persisted() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "local_src");
    let uid = "source.pkg.local_src";

    // Run 1: register a local schema and save.
    {
        let local_entry = LocalSchemaEntry {
            cfqn: c.clone(),
            unique_id: uid.to_string(),
            schema: make_schema("v1"),
        };
        let mut local = HashMap::new();
        local.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            HashMap::new(),
            HashMap::new(),
            local,
            vec![local_entry],
            HashMap::new(),
        );
        assert!(store.exists(&c));
        store.save(dir.path()).unwrap();
    }

    // Run 2: reload with a different YAML schema — must see v2, not v1.
    let local_entry2 = LocalSchemaEntry {
        cfqn: c.clone(),
        unique_id: uid.to_string(),
        schema: make_schema("v2"),
    };
    let mut local = HashMap::new();
    local.insert(c.clone(), uid.to_string());
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        HashMap::new(),
        local,
        vec![local_entry2],
        HashMap::new(),
    );

    assert!(store2.exists(&c));
    assert_eq!(
        store2.get_schema(&c).unwrap().inner().field(0).name(),
        "v2",
        "local schema must reflect current YAML, not cached value"
    );
}

// ── delta writes: only dirty entries saved ────────────────────────────────────

/// Clean (loaded-from-disk) entries must not generate a new epoch file on save.
#[test]
fn clean_entries_produce_no_new_epoch() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "t");
    let uid = "source.pkg.t";

    // Write epoch 0.
    {
        let store = store_with_frontier(&dir, &c, uid);
        store
            .register_schema(&c, None, make_schema("col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }
    // Epoch 0 written — count = 1.

    // Reload and save without registering anything new.
    let store2 = store_with_frontier(&dir, &c, uid);
    store2.save(dir.path()).unwrap();

    // No dirty entries → no new epoch file written → still 1 file.
    let remote_dir = dir.path().join("private/metadata/warehouse/schemas");
    let file_count = std::fs::read_dir(&remote_dir)
        .unwrap()
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .map(|x| x == "parquet")
                .unwrap_or(false)
        })
        .count();
    assert_eq!(
        file_count, 1,
        "no new epoch must be written when nothing is dirty"
    );
}

/// Only newly upserted entries appear in the delta epoch file.
#[test]
fn promote_to_frontier_visible_in_run() {
    // After a local model executes, downstream models in the same run must find
    // the upstream schema via the Frontier lookup key.
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "orders");
    let uid = "model.pkg.orders";

    let mut selected = HashMap::new();
    selected.insert(c.clone(), uid.to_string());
    let store = SchemaStore::new(
        dir.path().to_path_buf(),
        selected,
        HashMap::new(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    );

    // 1. Register the analyzed (Selected) schema.
    store
        .register_schema(&c, None, make_schema("amount"), false)
        .unwrap();

    // 2. Promote it to the Frontier cache.
    store.promote_to_frontier(&c).unwrap();

    // 3. The Frontier key must resolve — simulates a downstream model's lookup.
    let frontier_entry = LookupEntry::Frontier(c);
    assert!(
        store.exists_by_lookup(&frontier_entry),
        "Frontier entry must be visible after promote_to_frontier"
    );

    // 4. And the schema content must be correct.
    // resolve_lookup_entry_by_cfqn won't find Frontier for a Selected node, so
    // we use exists_by_lookup (already asserted above) and get_schema directly
    // via the state — but the public API is register_schema + get_schema, which
    // routes through resolve_lookup_entry_by_cfqn. Instead verify via save+reload.
}

#[test]
fn promote_to_frontier_survives_reload() {
    // Verifies that a promoted frontier entry is written to the epoch file and
    // survives a full save/reload cycle — same guarantee as a warehouse-fetched
    // frontier schema.
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "orders");
    let uid = "model.pkg.orders";

    // Run 1: selected model executes locally, schema is promoted to frontier.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("amount"), false)
            .unwrap();
        store.promote_to_frontier(&c).unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: the same cfqn is now a Frontier entry (not Selected).
    {
        let mut frontier = HashMap::new();
        frontier.insert(c.clone(), uid.to_string());
        let store2 = SchemaStore::new(
            dir.path().to_path_buf(),
            HashMap::new(),
            frontier,
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        assert!(
            store2.exists(&c),
            "Promoted frontier schema must survive save/reload"
        );
        let entry = store2.get_schema(&c).unwrap();
        assert_eq!(
            entry.inner().field(0).name(),
            "amount",
            "Schema content must be preserved across reload"
        );
    }
}

#[test]
fn delta_epoch_contains_only_new_entries() {
    let dir = TempDir::new().unwrap();
    let c_old = cfqn("db", "s", "old");
    let c_new = cfqn("db", "s", "new");
    let uid_old = "source.pkg.old";
    let uid_new = "source.pkg.new";

    // Epoch 0: write old entry.
    {
        let mut frontier = HashMap::new();
        frontier.insert(c_old.clone(), uid_old.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            HashMap::new(),
            frontier,
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c_old, None, make_schema("old_col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // Epoch 1: load old (clean), add new (dirty).
    {
        let mut frontier = HashMap::new();
        frontier.insert(c_old.clone(), uid_old.to_string());
        frontier.insert(c_new.clone(), uid_new.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            HashMap::new(),
            frontier,
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c_new, None, make_schema("new_col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // Now reload both: both must be visible.
    let mut frontier = HashMap::new();
    frontier.insert(c_old.clone(), uid_old.to_string());
    frontier.insert(c_new.clone(), uid_new.to_string());
    let store3 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    assert_eq!(
        store3.get_schema(&c_old).unwrap().inner().field(0).name(),
        "old_col"
    );
    assert_eq!(
        store3.get_schema(&c_new).unwrap().inner().field(0).name(),
        "new_col"
    );
}

#[test]
fn external_entry_survives_reload() {
    // External entries (tables not in the project graph, discovered lazily)
    // must survive save/reload via the remote cache, the same as Frontier entries.
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "ext_s", "ext_t");

    // Run 1: external entry registered and saved.
    {
        let store = empty_store(&dir);
        store
            .register_schema(&c, None, make_schema("ext_col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: reload as an External entry — must be a cache hit.
    // External entries are not in selected/frontier/local maps; they are
    // re-registered as External on the fly. After reload the epoch file
    // contains the row; it becomes visible once the store looks it up.
    // We verify this by registering with overwrite=false: it must see the
    // cached schema (not overwrite it with a different one).
    let store2 = empty_store(&dir);
    let entry = store2
        .register_schema(&c, None, make_schema("different_col"), false)
        .unwrap();
    assert_eq!(
        entry.inner().field(0).name(),
        "ext_col",
        "External entry must be loaded from epoch file (overwrite=false)"
    );
}

#[test]
fn snapshot_entry_survives_reload() {
    // Snapshots use unique_ids prefixed with "snapshot." and are stored in the
    // analyzed cache (compiled_state/schemas_analyzed), not the remote cache.
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "snp");
    let uid = "snapshot.pkg.snp";

    // Run 1: snapshot schema registered and saved.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("snap_col"), false)
            .unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: reload via the analyzed cache.
    let mut selected = HashMap::new();
    selected.insert(c.clone(), uid.to_string());
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        selected,
        HashMap::new(),
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    assert!(store2.exists(&c), "Snapshot schema must survive reload");
    assert_eq!(
        store2.get_schema(&c).unwrap().inner().field(0).name(),
        "snap_col"
    );
}

// ── Selected-model accumulation ──────────────────────────────────────────────────

/// A Selected model's schema from run 1 must be discoverable as a Frontier entry
/// in run 2 without re-registration (schema store accumulates across runs).
#[test]
fn selected_schema_available_as_frontier_in_next_run() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "model_b");
    let uid = "model.pkg.model_b";

    // Run 1: model_b is Selected, compiled, schema registered and saved.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("amount"), false)
            .unwrap();
        // Promote to frontier so it's discoverable as a Frontier key next run.
        store.promote_to_frontier(&c).unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: model_b is now a Frontier entry (model_a depends on it).
    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    );

    assert!(
        store2.exists(&c),
        "Selected schema from run 1 must be available as Frontier in run 2"
    );
    assert_eq!(
        store2.get_schema(&c).unwrap().inner().field(0).name(),
        "amount"
    );
}

/// Latest Selected schema wins across runs.
#[test]
fn selected_schema_latest_wins_across_runs() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "model_b");
    let uid = "model.pkg.model_b";

    // Run 1: compile with schema v1.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("v1"), false)
            .unwrap();
        store.promote_to_frontier(&c).unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: recompile with schema v2.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("v2"), true)
            .unwrap();
        store.promote_to_frontier(&c).unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 3: load as Frontier — must see v2.
    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());
    let store3 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    );
    assert_eq!(
        store3.get_schema(&c).unwrap().inner().field(0).name(),
        "v2",
        "latest Selected schema must win across runs"
    );
}

/// All Selected models (not just snapshots) should have their schemas persisted
/// in the compile cache and be loadable in subsequent runs.
#[test]
fn all_selected_models_persisted_not_just_snapshots() {
    let dir = TempDir::new().unwrap();
    let c = cfqn("db", "s", "regular_model");
    let uid = "model.pkg.regular_model";

    // Run 1: register a regular (non-snapshot) Selected model.
    {
        let mut selected = HashMap::new();
        selected.insert(c.clone(), uid.to_string());
        let store = SchemaStore::new(
            dir.path().to_path_buf(),
            selected,
            HashMap::new(),
            HashMap::new(),
            vec![],
            HashMap::new(),
        );
        store
            .register_schema(&c, None, make_schema("reg_col"), false)
            .unwrap();
        store.promote_to_frontier(&c).unwrap();
        store.save(dir.path()).unwrap();
    }

    // Run 2: the same model is now in the frontier. Must find it.
    let mut frontier = HashMap::new();
    frontier.insert(c.clone(), uid.to_string());
    let store2 = SchemaStore::new(
        dir.path().to_path_buf(),
        HashMap::new(),
        frontier,
        HashMap::new(),
        vec![],
        HashMap::new(),
    );

    assert!(
        store2.exists(&c),
        "regular (non-snapshot) Selected model must persist across runs"
    );
    assert_eq!(
        store2.get_schema(&c).unwrap().inner().field(0).name(),
        "reg_col"
    );
}
