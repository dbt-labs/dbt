//! DuckDB `extensions` and `extension_repositories` profile entries, from the profile schema
//! through engine initialization.
//!
//! Live tests are ignored by default because they load the DuckDB ADBC driver, and some
//! download extensions. They run on whichever DuckDB driver `SystemThenCdnCache` loads, and
//! each states the DuckDB version it needs:
//! `cargo test -p dbt-adapter --test duckdb_extensions -- --ignored --nocapture`
//! For DuckDB 2.0, put its `libduckdb` on `LD_LIBRARY_PATH` as `libadbc_driver_duckdb.so`.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use dbt_adapter::cache::RelationCache;
use dbt_adapter::engine::query_comment::QueryCommentConfig;
use dbt_adapter::engine::{AdapterEngine, AdbcEngine};
use dbt_adapter::errors::AdapterError;
use dbt_adapter::sql_types::DefaultTypeOps;
use dbt_adapter::stmt_splitter::DefaultStmtSplitter;
use dbt_adapter_core::AdapterType;
use dbt_adbc::{Backend, Connection, database};
use dbt_auth::{AdapterConfig, Auth, AuthError};
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;
use tempfile::TempDir;

/// Throwaway RSA-2048 public keys for offline trust-state tests.
const KEY_A: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAnWWcITru5BDcUrIeJPNp
VEOLNO9N8HV1Af/ztV4MfdaK6ZhVQWTbfLuxhEnCILgmT2NDswSKF3/LCyEyBC73
xYymq9u+AVuIz6k5uCfP/Z1o6/PJlR1Vs1cm7+2nmcH6O3KXIxHf+92QKTvBhiVv
BVD+vkmQROutmziL3C4OH4FtSoNAKOOvabgnI/d3z67Vf8WCsYaNIa5zlte7FpQZ
SeRbcgje4PMA2FCPfVw5EgHGveVn39JwuDD0uPAcbpS0T4j5ltZLvg9SyuQTP6h3
jibWhkf/nLlf0OuEnSgigUr3zhPNeT9UKszrCJTCDyMkWUtp6z8WByVhbySHfvmu
kwIDAQAB
-----END PUBLIC KEY-----
";
const KEY_B: &str = "-----BEGIN PUBLIC KEY-----
MIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAuW4P+hnre9CmVXTjpFWa
fCUjVQzMjU74osVnz6yo7XWzL2wB8E4gEB6CuLOAhIBCTm3nJ9Sez4cMyFy0bf28
gxo3nLFBMUkUnVImCvbvhQZzrCPHRin/lWuPhAX8SaRPPtbzWWdwdaqLffQ8mYFl
a79w2afBsCDwdsEzqpYRoPznbFxoLB4l3O4bD16hAMsmBoVic4LXyNCgeCsi4Rfv
C+0lT9+2SWp8JhFRgFS1csOuKWltcx3Cg5/yWWooNwZehhox3khe0/El1FVnxYbi
GChS5VLbhp+b2C61CgvxtChEnFYiF3XMsn4os6wCsqrcyS3phh87GLE9clyQdSU8
YQIDAQAB
-----END PUBLIC KEY-----
";

/// Adapter config for a DuckDB profile, deserialized through the profile schema as at load time.
fn profile_config(yaml: &str) -> AdapterConfig {
    let profile: DbConfig = dbt_yaml::from_str(yaml).unwrap();
    AdapterConfig::new(profile.to_mapping().unwrap())
}

/// A DuckDB profile declaring one repository, as a YAML string.
fn repository_profile(name: &str, prefix: &str, key: Option<&str>, extensions: &str) -> String {
    let key = key
        .map(|key| format!("    public_key: {key:?}\n"))
        .unwrap_or_default();
    format!(
        "type: duckdb\npath: ':memory:'\nextension_repositories:\n  - name: {name}\n    prefix: {prefix}\n{key}extensions: {extensions}\n"
    )
}

#[test]
fn extension_dictionaries_reach_init_sql_from_profile() {
    let config = profile_config(
        r#"
type: duckdb
path: ':memory:'
extensions:
  - httpfs
  - name: parquet
  - name: jev
    repo: community
"#,
    );
    assert_eq!(
        dbt_auth::generate_duckdb_init_sql(&config).unwrap(),
        vec![
            "INSTALL httpfs",
            "LOAD httpfs",
            "INSTALL parquet",
            "LOAD parquet",
            "INSTALL jev FROM \"community\"",
            "LOAD jev",
        ]
    );
}

#[test]
fn extension_dictionary_unknown_fields_are_rejected_from_profile() {
    for (entry, field) in [
        ("{name: httpfs, repository: community}", "repository"),
        ("{name: httpfs, version: v1}", "version"),
    ] {
        let config = profile_config(&format!("type: duckdb\nextensions: [httpfs, {entry}]"));
        let err = dbt_auth::generate_duckdb_init_sql(&config).unwrap_err();
        assert_eq!(
            err.msg(),
            format!("extensions[1]: unknown field '{field}'"),
            "{entry}"
        );
    }
}

#[test]
fn extension_repository_unknown_fields_are_rejected_from_profile() {
    let config = profile_config(
        "type: duckdb\nextension_repositories: [{name: repo_a, prefix: x, pubkey: ABC=}]",
    );
    let err = dbt_auth::DuckDbInit::from_config(&config).err().unwrap();
    assert_eq!(
        err.msg(),
        "extension_repositories[0]: unknown field 'pubkey'"
    );
}

/// DuckDB's auth, plus startup options that keep live tests out of `~/.duckdb`.
struct IsolatedDuckDbAuth {
    inner: Box<dyn Auth>,
    options: Vec<(&'static str, String)>,
}

impl Auth for IsolatedDuckDbAuth {
    fn backend(&self) -> Backend {
        self.inner.backend()
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = self.inner.configure(config)?;
        for (name, value) in &self.options {
            builder.with_named_option(name, value.as_str())?;
        }
        Ok(builder)
    }
}

/// Extension and repository storage in temporary directories. Engines built from the same
/// storage open separate databases that share it, like consecutive dbt runs.
struct Storage {
    extensions: TempDir,
    repositories: TempDir,
}

impl Storage {
    fn new() -> Self {
        Storage {
            extensions: tempfile::tempdir().unwrap(),
            repositories: tempfile::tempdir().unwrap(),
        }
    }

    /// Keeps extensions here.
    fn extension_options(&self) -> Vec<(&'static str, String)> {
        vec![("extension_directory", path(&self.extensions))]
    }

    /// Also keeps extension repositories here (a DuckDB 2.0 startup option).
    fn repository_options(&self) -> Vec<(&'static str, String)> {
        let mut options = self.extension_options();
        options.push(("extension_repository_directory", path(&self.repositories)));
        options
    }
}

fn path(dir: &TempDir) -> String {
    dir.path().to_str().unwrap().to_owned()
}

fn find_file(dir: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        if path.is_dir() {
            find_file(&path, name)
        } else {
            (path.file_name()? == name).then_some(path)
        }
    })
}

fn engine(config: &AdapterConfig, options: Vec<(&'static str, String)>) -> AdbcEngine {
    let auth = IsolatedDuckDbAuth {
        inner: dbt_auth::auth_for_backend(Backend::DuckDB),
        options,
    };
    AdbcEngine::new(
        AdapterType::DuckDB,
        Arc::new(auth),
        config.clone(),
        DEFAULT_RESOLVED_QUOTING,
        QueryCommentConfig::from_query_comment(None, AdapterType::DuckDB, false, None),
        Arc::new(DefaultTypeOps::new(AdapterType::DuckDB)),
        Arc::new(DefaultStmtSplitter),
        Arc::new(RelationCache::default()),
        BTreeMap::new(),
        None,
        None,
    )
}

/// Opens a connection through engine initialization, in a new database.
fn connect(
    yaml: &str,
    options: Vec<(&'static str, String)>,
) -> Result<Box<dyn Connection>, AdapterError> {
    let config = profile_config(yaml);
    engine(&config, options).new_connection_with_config(&config)
}

fn query_string(conn: &mut Box<dyn Connection>, sql: &str) -> String {
    let mut stmt = conn.new_statement().unwrap();
    stmt.set_sql_query(sql).unwrap();
    let batch = stmt.execute().unwrap().next().unwrap().unwrap();
    batch.column(0).as_string::<i32>().value(0).to_owned()
}

/// Panics unless the loaded DuckDB driver's major version is `major`, so that a test run on
/// the wrong driver fails clearly instead of testing something else.
fn require_duckdb_major(major: &str) {
    let mut conn = connect("type: duckdb\npath: ':memory:'", vec![]).unwrap();
    let version = query_string(&mut conn, "SELECT version()");
    println!("DuckDB version: {version}");
    assert!(
        version.starts_with(&format!("v{major}.")),
        "this test needs DuckDB {major}.x, but the loaded driver is {version}"
    );
}

fn access(conn: &mut Box<dyn Connection>) -> String {
    query_string(
        conn,
        "SELECT current_setting('allow_extension_repositories')",
    )
}

#[test]
#[ignore = "loads the DuckDB ADBC driver and downloads a community extension"]
fn extension_dictionary_installs_and_loads_from_community() {
    let storage = Storage::new();
    let mut conn = connect(
        r#"
type: duckdb
path: ':memory:'
extensions:
  - name: h3
    repo: community
"#,
        storage.extension_options(),
    )
    .unwrap_or_else(|e| panic!("DuckDB initialization, including INSTALL h3, failed: {e}"));

    println!(
        "DuckDB version: {}",
        query_string(&mut conn, "SELECT version()")
    );
    let mut stmt = conn.new_statement().unwrap();
    stmt.set_sql_query(
        "SELECT loaded, installed_from, install_path FROM duckdb_extensions() \
         WHERE extension_name = 'h3'",
    )
    .unwrap();
    let batch = stmt.execute().unwrap().next().unwrap().unwrap();
    assert!(batch.column(0).as_boolean().value(0), "h3 must be loaded");
    assert_eq!(batch.column(1).as_string::<i32>().value(0), "community");
    let install_path = batch.column(2).as_string::<i32>().value(0);
    assert!(
        install_path.starts_with(&path(&storage.extensions)),
        "{install_path}"
    );
    assert_eq!(
        query_string(
            &mut conn,
            "SELECT (h3_latlng_to_cell(0.0, 0.0, 0) IS NOT NULL)::VARCHAR"
        ),
        "true"
    );
}

#[test]
#[ignore = "needs a DuckDB driver older than 2.0"]
fn extension_repositories_need_duckdb_2() {
    require_duckdb_major("1");
    let storage = Storage::new();
    let err = connect(
        &repository_profile("repo_a", "https://a.invalid", Some(KEY_A), "[]"),
        storage.extension_options(),
    )
    .err()
    .unwrap();
    assert!(
        err.message()
            .contains("extension_repositories requires DuckDB 2.0 or later, but the loaded DuckDB driver is v1."),
        "{err}"
    );
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn extension_repository_is_registered_then_reused() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let declared = repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), "[]");

    let mut conn = connect(&declared, storage.repository_options()).unwrap();
    assert_eq!(
        query_string(
            &mut conn,
            "SELECT repository_name || ' ' || prefix FROM duckdb_extension_repositories() \
             WHERE type = 'USER_PROVIDED'"
        ),
        "test_repo https://repo-a.invalid"
    );
    // Creating the repository opted in.
    assert_eq!(access(&mut conn), "allowed");

    // A later run reuses the matching registration without opting in again, including with
    // the key omitted (no fetch from the `.invalid` prefix), a trailing slash, or another case.
    for profile in [
        declared.clone(),
        repository_profile("test_repo", "https://repo-a.invalid", None, "[]"),
        repository_profile("test_repo", "https://repo-a.invalid/", Some(KEY_A), "[]"),
        repository_profile("Test_Repo", "https://repo-a.invalid", None, "[]"),
    ] {
        let mut conn = connect(&profile, storage.repository_options())
            .unwrap_or_else(|e| panic!("{profile}\n{e}"));
        assert_eq!(access(&mut conn), "undecided", "{profile}");
    }
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn extension_repository_trust_changes_are_rejected() {
    require_duckdb_major("2");
    let storage = Storage::new();
    connect(
        &repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), "[]"),
        storage.repository_options(),
    )
    .unwrap();

    let both_keys = format!("{KEY_A}{KEY_B}");
    for (profile, expected) in [
        (
            repository_profile("test_repo", "https://repo-b.invalid", None, "[]"),
            "already registered with prefix 'https://repo-a.invalid'",
        ),
        (
            repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_B), "[]"),
            "already registered with public keys [sha256:",
        ),
        (
            repository_profile(
                "test_repo",
                "https://repo-a.invalid",
                Some(&both_keys),
                "[]",
            ),
            "already registered with public keys [sha256:",
        ),
    ] {
        let err = connect(&profile, storage.repository_options())
            .err()
            .unwrap_or_else(|| panic!("must fail: {profile}"));
        assert!(err.message().contains(expected), "{profile}\n{err}");
        assert!(err.message().contains("DROP EXTENSION REPOSITORY"), "{err}");
        assert!(
            !err.message().contains("MIIB"),
            "keys must not be echoed: {err}"
        );
    }
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn extension_repositories_are_rejected_when_forbidden() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let declared = repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), "[]");
    let forbidden = || {
        let mut options = storage.repository_options();
        options.push(("allow_extension_repositories", "forbidden".to_owned()));
        options
    };

    // Missing, then already registered.
    for _ in 0..2 {
        let err = connect(&declared, forbidden()).err().unwrap();
        assert!(
            err.message()
                .contains("allow_extension_repositories is 'forbidden'"),
            "{err}"
        );
        connect(&declared, storage.repository_options()).unwrap();
    }

    // A late `settings` value that DuckDB refuses after the opt-in fails with DuckDB's error
    // rather than being reordered or overridden.
    let mut profile = repository_profile("other_repo", "https://repo-b.invalid", Some(KEY_B), "[]");
    profile.push_str("settings:\n  allow_extension_repositories: undecided\n");
    let err = connect(&profile, storage.repository_options())
        .err()
        .unwrap();
    assert!(
        err.message()
            .contains("allow_extension_repositories can only be changed"),
        "{err}"
    );
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn extension_repository_is_registered_before_installing_from_it() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let installs = "[{name: a5, repo: test_repo}]";
    // On the first run the repository is created; on the second it is reused, and installing
    // from it still needs 'allowed'. Either way the install reaches the repository's prefix,
    // which is unreachable here, instead of failing on an unknown repository or a permission.
    for _ in 0..2 {
        let err = connect(
            &repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), installs),
            storage.repository_options(),
        )
        .err()
        .unwrap();
        assert!(err.message().contains("repo-a.invalid"), "{err}");
        assert!(!err.message().contains("not a known"), "{err}");
        assert!(
            !err.message()
                .contains("requires allow_extension_repositories"),
            "{err}"
        );
    }
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn cached_database_is_not_reused_for_other_repository_declarations() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let first = profile_config(&repository_profile(
        "test_repo",
        "https://repo-a.invalid",
        Some(KEY_A),
        "[]",
    ));
    let changed = profile_config(&repository_profile(
        "test_repo",
        "https://repo-b.invalid",
        Some(KEY_A),
        "[]",
    ));
    let engine = engine(&first, storage.repository_options());
    engine.new_connection_with_config(&first).unwrap();
    // Same database options, so the same cached database, but different declarations.
    let err = engine.new_connection_with_config(&changed).err().unwrap();
    assert!(
        err.message()
            .contains("already initialized in this process"),
        "{err}"
    );
    engine.new_connection_with_config(&first).unwrap();
}

/// `settings` that forbid repositories, as DuckDB would read them: setting names are
/// case-insensitive.
const FORBIDDING_SETTINGS: [&str; 3] = [
    "settings:\n  allow_extension_repositories: forbidden\n",
    "settings:\n  ALLOW_EXTENSION_REPOSITORIES: forbidden\n",
    "settings:\n  Allow_Extension_Repositories: Forbidden\n",
];

fn assert_forbidding_conflict(err: &AdapterError) {
    assert!(
        err.message()
            .contains("settings.allow_extension_repositories: 'forbidden' conflicts"),
        "{err}"
    );
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn forbidding_setting_is_rejected_before_registration() {
    require_duckdb_major("2");
    let storage = Storage::new();
    for settings in FORBIDDING_SETTINGS {
        let mut profile =
            repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), "[]");
        profile.push_str(settings);
        let err = connect(&profile, storage.repository_options())
            .err()
            .unwrap_or_else(|| panic!("must fail: {profile}"));
        assert_forbidding_conflict(&err);
    }
    // Nothing was registered: no repository file, and DuckDB lists no user repository.
    assert_eq!(
        std::fs::read_dir(storage.repositories.path())
            .unwrap()
            .count(),
        0
    );
    let mut conn = connect(
        "type: duckdb\npath: ':memory:'",
        storage.repository_options(),
    )
    .unwrap();
    assert_eq!(
        query_string(
            &mut conn,
            "SELECT count(*)::VARCHAR FROM duckdb_extension_repositories() \
             WHERE type = 'USER_PROVIDED'"
        ),
        "0"
    );
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; offline"]
fn cached_database_is_not_reused_for_a_rejected_configuration() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let declared = repository_profile("test_repo", "https://repo-a.invalid", Some(KEY_A), "[]");
    let first = profile_config(&declared);
    let engine = engine(&first, storage.repository_options());
    engine.new_connection_with_config(&first).unwrap();
    // Same database options and declarations, so the same cached database, but settings that
    // initialization rejects.
    for settings in FORBIDDING_SETTINGS {
        let rejected = profile_config(&format!("{declared}{settings}"));
        let err = engine
            .new_connection_with_config(&rejected)
            .err()
            .unwrap_or_else(|| panic!("must fail: {settings}"));
        assert_forbidding_conflict(&err);
    }
    let mut conn = engine.new_connection_with_config(&first).unwrap();
    assert_eq!(access(&mut conn), "allowed");
}

fn assert_a5_loaded_with_signature_verification(conn: &mut Box<dyn Connection>, run: &str) {
    assert_eq!(
        query_string(
            conn,
            "SELECT current_setting('allow_unsigned_extensions')::VARCHAR"
        ),
        "false"
    );
    assert_eq!(
        query_string(
            conn,
            "SELECT loaded::VARCHAR FROM duckdb_extensions() WHERE extension_name = 'a5'"
        ),
        "true",
        "{run}"
    );
    let functions = query_string(
        conn,
        "SELECT count(*)::VARCHAR FROM duckdb_functions() WHERE function_name LIKE 'a5%'",
    );
    assert_ne!(functions, "0", "{run}");
    println!(
        "{run}: {functions} a5 functions, keys {}",
        query_string(
            conn,
            "SELECT key_fingerprints::VARCHAR FROM duckdb_extension_repositories() \
             WHERE repository_name = 'query_farm'"
        )
    );
}

#[test]
#[ignore = "needs a DuckDB 2.0 driver; downloads A5 and its signing key from Query Farm"]
fn query_farm_a5_installs_and_loads_with_signature_verification() {
    require_duckdb_major("2");
    let storage = Storage::new();
    let profile = repository_profile(
        "query_farm",
        "https://duckdb-extensions.query.farm",
        None,
        "[{name: a5, repo: query_farm}]",
    );
    // A fresh run, then a run that reuses the registration and the installed extension.
    for run in ["fresh", "reused"] {
        let mut conn = connect(&profile, storage.repository_options())
            .unwrap_or_else(|e| panic!("{run} run failed: {e}"));
        assert_a5_loaded_with_signature_verification(&mut conn, run);
    }
    // `duckdb_extensions()` does not report per-repository installs in DuckDB 2.0.0
    // (`installed` is false), so look for the file in the isolated storage instead.
    assert!(
        find_file(storage.extensions.path(), "a5.duckdb_extension")
            .is_some_and(|path| path.to_string_lossy().contains("repositories/query_farm")),
        "a5 must be installed under the isolated extension directory"
    );
    for (profile, expected) in [
        (
            repository_profile(
                "query_farm",
                "https://duckdb-extensions.query.farm.invalid",
                None,
                "[{name: a5, repo: query_farm}]",
            ),
            "already registered with prefix",
        ),
        (
            repository_profile(
                "query_farm",
                "https://duckdb-extensions.query.farm",
                Some(KEY_A),
                "[{name: a5, repo: query_farm}]",
            ),
            "already registered with public keys",
        ),
    ] {
        let err = connect(&profile, storage.repository_options())
            .err()
            .unwrap_or_else(|| panic!("must fail: {profile}"));
        assert!(err.message().contains(expected), "{err}");
    }
}
