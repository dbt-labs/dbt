#![allow(clippy::disallowed_methods)]

use std::collections::BTreeMap;

use dbt_profile::{ResolveArgs, resolve};

// ── Dependency guardrails ────────────────────────────────────────────
// dbt-profile must stay lightweight — no dbt-common or dbt-jinja-utils.
// These tests parse Cargo.toml and fail if a banned dependency appears.

const CARGO_TOML: &str = include_str!("../Cargo.toml");

const BANNED_DEPS: &[&str] = &["dbt-common", "dbt-jinja-utils"];

#[test]
fn no_banned_dependencies() {
    for dep in BANNED_DEPS {
        assert!(
            !CARGO_TOML.contains(dep),
            "dbt-profile must not depend on `{dep}`. \
             This crate is intentionally standalone — see the module doc in lib.rs."
        );
    }
}

fn write_file(dir: &std::path::Path, name: &str, content: &str) {
    std::fs::write(dir.join(name), content).unwrap();
}

#[test]
fn test_resolve_simple_profile() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: snowflake
      account: test_account
      user: test_user
      password: test_pass
      warehouse: COMPUTE_WH
      database: MY_DB
      schema: PUBLIC
      role: ACCOUNTADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.profile_name, "my_project");
    assert_eq!(result.target_name, "dev");
    assert_eq!(result.adapter_type, "snowflake");
    assert_eq!(result.get_str("account"), Some("test_account"));
    assert_eq!(result.get_str("user"), Some("test_user"));
    assert_eq!(result.get_str("warehouse"), Some("COMPUTE_WH"));
    assert_eq!(result.database(), Some("MY_DB"));
    assert_eq!(result.schema(), Some("PUBLIC"));
}

#[test]
fn test_resolve_with_env_var() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    unsafe {
        std::env::set_var("DBT_PROFILE_TEST_ACCOUNT", "env_account");
        std::env::set_var("DBT_PROFILE_TEST_USER", "env_user");
        std::env::set_var("DBT_PROFILE_TEST_PASS", "env_pass");
    };

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: snowflake
      account: "{{ env_var('DBT_PROFILE_TEST_ACCOUNT') }}"
      user: "{{ env_var('DBT_PROFILE_TEST_USER') }}"
      password: "{{ env_var('DBT_PROFILE_TEST_PASS') }}"
      warehouse: "{{ env_var('DBT_PROFILE_TEST_WH', 'DEFAULT_WH') }}"
      database: MY_DB
      schema: PUBLIC
      role: ACCOUNTADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.get_str("account"), Some("env_account"));
    assert_eq!(result.get_str("user"), Some("env_user"));
    assert_eq!(result.get_str("password"), Some("env_pass"));
    assert_eq!(result.get_str("warehouse"), Some("DEFAULT_WH"));

    unsafe {
        std::env::remove_var("DBT_PROFILE_TEST_ACCOUNT");
        std::env::remove_var("DBT_PROFILE_TEST_USER");
        std::env::remove_var("DBT_PROFILE_TEST_PASS");
    };
}

#[test]
fn test_rendered_query_tags_json_becomes_a_mapping() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    unsafe {
        std::env::set_var(
            "DBT_PROFILE_TEST_QUERY_TAGS",
            r#"{"team":"analytics","cost_center":"3000"}"#,
        );
    }

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: databricks
      query_tags: "{{ env_var('DBT_PROFILE_TEST_QUERY_TAGS') }}"
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    let query_tags = result
        .credentials
        .get("query_tags")
        .expect("query_tags should be present");
    assert_eq!(
        query_tags.get("team").and_then(dbt_yaml::Value::as_str),
        Some("analytics")
    );
    assert_eq!(
        query_tags
            .get("cost_center")
            .and_then(dbt_yaml::Value::as_str),
        Some("3000")
    );

    unsafe {
        std::env::remove_var("DBT_PROFILE_TEST_QUERY_TAGS");
    }
}

#[test]
fn test_query_tags_mapping_renders_values() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: databricks
      query_tags:
        team: "{{ env_var('DBT_PROFILE_TEST_QUERY_TAG_TEAM', 'analytics') }}"
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(
        result
            .credentials
            .get("query_tags")
            .and_then(|value| value.get("team"))
            .and_then(dbt_yaml::Value::as_str),
        Some("analytics")
    );
}

#[test]
fn test_literal_query_tags_json_remains_a_string() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: databricks
      query_tags: '{"team":"analytics"}'
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(
        result.get_str("query_tags"),
        Some(r#"{"team":"analytics"}"#)
    );
}

#[test]
fn test_resolve_with_vars() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: postgres
      host: localhost
      port: 5432
      user: "{{ var('db_user', 'default_user') }}"
      password: "{{ var('db_pass') }}"
      dbname: my_database
      schema: public
"#,
    );

    let mut vars = BTreeMap::new();
    vars.insert(
        "db_pass".to_owned(),
        dbt_yaml::Value::string("secret123".to_owned()),
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        vars,
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.adapter_type, "postgres");
    assert_eq!(result.get_str("user"), Some("default_user"));
    assert_eq!(result.get_str("password"), Some("secret123"));
    assert_eq!(result.database(), Some("my_database"));
}

#[test]
fn test_resolve_target_override() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.db
      schema: main
    prod:
      type: snowflake
      account: prod_account
      user: prod_user
      password: prod_pass
      warehouse: PROD_WH
      database: PROD_DB
      schema: PROD_SCHEMA
      role: SYSADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        target: Some("prod".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.target_name, "prod");
    assert_eq!(result.adapter_type, "snowflake");
    assert_eq!(result.get_str("account"), Some("prod_account"));
}

#[test]
fn test_resolve_profile_name_from_project() {
    let tmp = tempfile::tempdir().unwrap();
    let project_dir = tmp.path();

    write_file(
        project_dir,
        "dbt_project.yml",
        r#"
name: my_cool_project
version: '1.0.0'
profile: 'cool_profile'
"#,
    );

    write_file(
        project_dir,
        "profiles.yml",
        r#"
cool_profile:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.db
      schema: main
      database: memory
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(project_dir.to_path_buf()),
        project_dir: Some(project_dir.to_path_buf()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.profile_name, "cool_profile");
    assert_eq!(result.adapter_type, "duckdb");
}

#[test]
fn test_resolve_credentials_json() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
proj:
  target: dev
  outputs:
    dev:
      type: snowflake
      account: my_acct
      user: my_user
      password: my_pass
      warehouse: WH
      database: DB
      schema: SCH
      role: ADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("proj".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    let json_str = result.credentials_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed["type"], "snowflake");
    assert_eq!(parsed["account"], "my_acct");
    assert_eq!(parsed["user"], "my_user");
}

#[test]
fn test_resolve_missing_profile_error() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
other_project:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.db
      schema: main
      database: memory
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("nonexistent".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        format!("{err}").contains("nonexistent"),
        "Error should mention the missing profile name: {err}"
    );
}

#[test]
fn test_resolve_missing_target_error() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
proj:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.db
      schema: main
      database: memory
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("proj".to_owned()),
        target: Some("staging".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        format!("{err}").contains("staging"),
        "Error should mention the missing target: {err}"
    );
}

#[test]
fn test_resolve_with_as_number_filter() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    unsafe { std::env::set_var("DBT_PROFILE_TEST_PORT2", "5432") };

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
proj:
  target: dev
  outputs:
    dev:
      type: postgres
      host: localhost
      port: "{{ env_var('DBT_PROFILE_TEST_PORT2') | as_number }}"
      user: test
      password: test
      dbname: testdb
      schema: public
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("proj".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    let port = result.credentials.get("port").unwrap();
    assert!(
        port.as_i64().is_some() || (port.as_str() == Some("5432")),
        "port should be a number or '5432', got: {port:?}"
    );

    unsafe { std::env::remove_var("DBT_PROFILE_TEST_PORT2") };
}

#[test]
fn test_resolve_no_profiles_yml() {
    let tmp = tempfile::tempdir().unwrap();

    let args = ResolveArgs {
        profiles_dir: Some(tmp.path().to_path_buf()),
        profile: Some("anything".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args);
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("profiles.yml"),
        "Error should mention profiles.yml: {err}"
    );
}

#[test]
fn test_resolve_config_key_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
config:
  send_anonymous_usage_stats: false

my_project:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: dev.db
      schema: main
      database: memory
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args);
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("config") && err.contains("Unexpected"),
        "Error should reject unexpected config key: {err}"
    );
}

#[test]
fn test_resolve_jinja_in_target_name() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    unsafe { std::env::set_var("DBT_PROFILE_TEST_TARGET", "production") };

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
proj:
  target: "{{ env_var('DBT_PROFILE_TEST_TARGET') }}"
  outputs:
    production:
      type: snowflake
      account: prod_acct
      user: prod_user
      password: prod_pass
      warehouse: WH
      database: PROD
      schema: PUBLIC
      role: ADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("proj".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.target_name, "production");
    assert_eq!(result.get_str("account"), Some("prod_acct"));

    unsafe { std::env::remove_var("DBT_PROFILE_TEST_TARGET") };
}

// Regression test for https://github.com/dbt-labs/dbt-fusion/issues/1564
// YAML merge keys (<<: *anchor) must be resolved so inherited fields are visible.
#[test]
fn test_resolve_yaml_anchor_merge_keys() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev

  outputs:
    dev: &default_config
      type: bigquery
      method: oauth
      project: my-project-dev
      dataset: my_dataset
      threads: 2
      timeout_seconds: 300
      location: US

    prod:
      <<: *default_config
      project: my-project-prod
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        target: Some("prod".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.target_name, "prod");
    // type must be inherited from the anchor via <<: *default_config
    assert_eq!(result.adapter_type, "bigquery");
    assert_eq!(result.get_str("method"), Some("oauth"));
    // overridden field must take the local value
    assert_eq!(result.get_str("project"), Some("my-project-prod"));
    // inherited field not overridden
    assert_eq!(result.get_str("location"), Some("US"));
}

// Regression test for https://github.com/dbt-labs/dbt-fusion/issues/XXXX
// DBT_ENV_SECRET_* variables must be resolved in profiles.yml, not blocked.
#[test]
fn test_resolve_secret_env_var() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    unsafe {
        std::env::set_var("DBT_ENV_SECRET_PROFILE_TEST_PASS", "super_secret");
        std::env::set_var("DBT_ENV_SECRET_PROFILE_TEST_TOKEN", "secret_token");
    }

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: snowflake
      account: my_account
      user: my_user
      password: "{{ env_var('DBT_ENV_SECRET_PROFILE_TEST_PASS') }}"
      private_key_passphrase: "{{ env_var('DBT_ENV_SECRET_PROFILE_TEST_TOKEN') }}"
      warehouse: WH
      database: DB
      schema: SCH
      role: ADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();
    assert_eq!(result.adapter_type, "snowflake");
    // Secret values must be resolved to their real values
    assert_eq!(result.get_str("password"), Some("super_secret"));
    assert_eq!(
        result.get_str("private_key_passphrase"),
        Some("secret_token")
    );

    unsafe {
        std::env::remove_var("DBT_ENV_SECRET_PROFILE_TEST_PASS");
        std::env::remove_var("DBT_ENV_SECRET_PROFILE_TEST_TOKEN");
    }
}

#[test]
fn test_resolve_secret_env_var_missing() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: snowflake
      account: my_account
      user: my_user
      password: "{{ env_var('DBT_ENV_SECRET_MISSING_VAR') }}"
      warehouse: WH
      database: DB
      schema: SCH
      role: ADMIN
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args);
    assert!(result.is_err());
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("DBT_ENV_SECRET_MISSING_VAR"),
        "Error should mention the missing variable: {err}"
    );
}

// ── Multi-adapter targets ────────────────────────────────────────────
// A target's output block may be a mapping of adapter type to a list of
// connections instead of one flat mapping. These exercise the shape end-to-end
// through `resolve`, not just the parser, so the wiring in
// `resolve_with_env_ext` is covered.

#[test]
fn test_resolve_target_as_a_connection_list() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: prod
  outputs:
    prod:
      - type: snowflake
        account: test_account
        user: test_user
        password: test_pass
        warehouse: COMPUTE_WH
        database: MY_DB
        schema: PUBLIC
      - type: duckdb
        default: true
        path: ./scratch.db
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();

    assert_eq!(result.target_name, "prod");
    let types: Vec<&str> = result
        .adapters
        .iter()
        .map(|a| a.adapter_type.as_str())
        .collect();
    assert_eq!(types, vec!["snowflake", "duckdb"]);

    // `adapter_type`/`credentials` describe the *default* adapter, which here is
    // the second entry -- proving the marker is honoured rather than "first wins".
    assert_eq!(result.adapter_type, "duckdb");
    assert_eq!(
        result.credentials.get("path").and_then(|v| v.as_str()),
        Some("./scratch.db")
    );

    for adapter in &result.adapters {
        let credentials = &adapter.default_connection().credentials;
        // Bookkeeping keys must not leak into connection config.
        assert!(
            credentials.get("name").is_none(),
            "`name` leaked into credentials for {}",
            adapter.adapter_type
        );
        assert!(
            credentials.get("default").is_none(),
            "`default` leaked into credentials for {}",
            adapter.adapter_type
        );
        // ...but `type` must be present, taken from the key, since that is what
        // `DbConfig` deserialization is tagged by.
        assert_eq!(
            credentials.get("type").and_then(|v| v.as_str()),
            Some(adapter.adapter_type.as_str()),
            "`type` should be injected from the adapter key"
        );
    }
}

/// The legacy single-mapping shape must keep resolving identically, and now also
/// surface as one adapter with one connection.
#[test]
fn test_resolve_legacy_target_yields_single_default_adapter() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: dev
  outputs:
    dev:
      type: duckdb
      path: ./dev.db
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let result = resolve(&args).unwrap();

    assert_eq!(result.adapter_type, "duckdb");
    assert_eq!(result.adapters.len(), 1);
    assert_eq!(result.adapters[0].adapter_type, "duckdb");
    assert_eq!(result.adapters[0].connections.len(), 1);
    assert!(result.adapters[0].default_connection().is_default);
}

/// Which adapter every unannotated node runs on must not depend on YAML ordering.
#[test]
fn test_resolve_several_adapters_without_default_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: prod
  outputs:
    prod:
      - type: duckdb
        path: ./a.db
      - type: snowflake
        account: acc
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    let err = format!("{}", resolve(&args).unwrap_err());
    assert!(
        err.contains("default: true") && err.contains("duckdb") && err.contains("snowflake"),
        "error should name the candidates: {err}"
    );
}

/// The alternate compute target is not a separate `x_alt_target` output -- it is
/// an `lake_compute` adapter in the active target. This is the seam `dbt-loader` reads to
/// build `DbtProfile::adapters`.
#[test]
fn test_resolve_surfaces_lake_compute_adapter_as_non_default() {
    let tmp = tempfile::tempdir().unwrap();
    let profiles_dir = tmp.path();

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: prod
  outputs:
    prod: []
"#,
    );

    let args = ResolveArgs {
        profiles_dir: Some(profiles_dir.to_path_buf()),
        profile: Some("my_project".to_owned()),
        ..Default::default()
    };

    // A target must declare at least one connection.
    let err = format!("{}", resolve(&args).unwrap_err());
    assert!(err.contains("empty list of connections"), "got: {err}");

    write_file(
        profiles_dir,
        "profiles.yml",
        r#"
my_project:
  target: prod
  outputs:
    prod:
      - type: snowflake
        default: true
        account: test_account
        database: MY_DB
        schema: PUBLIC
      - type: lakecompute
"#,
    );

    let result = resolve(&args).unwrap();

    // The default stays the warehouse, so every existing consumer of
    // `adapter_type`/`credentials` is unaffected.
    assert_eq!(result.adapter_type, "snowflake");
    assert_eq!(result.default_adapter().adapter_type, "snowflake");

    let non_default: Vec<&str> = result
        .non_default_adapters()
        .map(|a| a.adapter_type.as_str())
        .collect();
    assert_eq!(non_default, vec!["lakecompute"]);
    assert!(result.adapter_by_type("lakecompute").is_some());
    assert!(result.adapter_by_type("nope").is_none());
}
