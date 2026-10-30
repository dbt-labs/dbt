//! DuckDB `extensions` profile entries, from the profile schema through engine initialization.
//!
//! The live test is ignored by default because it loads the DuckDB ADBC driver and downloads
//! an extension. It runs on whichever DuckDB driver `SystemThenCdnCache` loads:
//! `cargo test -p dbt-adapter --test duckdb_extensions -- --ignored --nocapture`

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use dbt_adapter::cache::RelationCache;
use dbt_adapter::engine::query_comment::QueryCommentConfig;
use dbt_adapter::engine::{AdapterEngine, AdbcEngine};
use dbt_adapter::sql_types::DefaultTypeOps;
use dbt_adapter::stmt_splitter::DefaultStmtSplitter;
use dbt_adapter_core::AdapterType;
use dbt_adbc::{Backend, database};
use dbt_auth::{AdapterConfig, Auth, AuthError};
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

/// Adapter config for a DuckDB profile, deserialized through the profile schema as at load time.
fn profile_config(yaml: &str) -> AdapterConfig {
    let profile: DbConfig = dbt_yaml::from_str(yaml).unwrap();
    AdapterConfig::new(profile.to_mapping().unwrap())
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

/// DuckDB's auth, plus `extension_directory` so a live test never touches `~/.duckdb`.
struct IsolatedDuckDbAuth {
    inner: Box<dyn Auth>,
    extension_directory: String,
}

impl Auth for IsolatedDuckDbAuth {
    fn backend(&self) -> Backend {
        self.inner.backend()
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = self.inner.configure(config)?;
        builder.with_named_option("extension_directory", self.extension_directory.as_str())?;
        Ok(builder)
    }
}

#[test]
#[ignore = "loads the DuckDB ADBC driver and downloads a community extension"]
fn extension_dictionary_installs_and_loads_from_community() {
    let extension_directory = tempfile::tempdir().unwrap();
    let config = profile_config(
        r#"
type: duckdb
path: ':memory:'
extensions:
  - name: h3
    repo: community
"#,
    );
    let auth = IsolatedDuckDbAuth {
        inner: dbt_auth::auth_for_backend(Backend::DuckDB),
        extension_directory: extension_directory.path().to_str().unwrap().to_owned(),
    };
    let engine = AdbcEngine::new(
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
    );
    let mut conn = engine
        .new_connection_with_config(&config)
        .unwrap_or_else(|e| panic!("DuckDB initialization, including INSTALL h3, failed: {e}"));

    let mut query = |sql: &str| {
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query(sql).unwrap();
        stmt.execute().unwrap().next().unwrap().unwrap()
    };
    let batch = query("SELECT version()");
    println!(
        "DuckDB version: {}",
        batch.column(0).as_string::<i32>().value(0)
    );
    let batch = query(
        "SELECT loaded, installed_from, install_path FROM duckdb_extensions() \
         WHERE extension_name = 'h3'",
    );
    assert!(batch.column(0).as_boolean().value(0), "h3 must be loaded");
    assert_eq!(batch.column(1).as_string::<i32>().value(0), "community");
    let install_path = batch.column(2).as_string::<i32>().value(0);
    assert!(
        install_path.starts_with(extension_directory.path().to_str().unwrap()),
        "{install_path}"
    );
    let batch = query("SELECT h3_latlng_to_cell(0.0, 0.0, 0) IS NOT NULL");
    assert!(batch.column(0).as_boolean().value(0));
}
