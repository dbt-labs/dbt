use dbt_profile::{ResolveArgs, resolve, resolve_metadata};

fn fixture(body: &str) -> (tempfile::TempDir, ResolveArgs) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("profiles.yml"), body).unwrap();
    let args = ResolveArgs {
        profiles_dir: Some(dir.path().to_path_buf()),
        profile: Some("fixture".into()),
        ..Default::default()
    };
    (dir, args)
}

#[test]
fn selection_does_not_render_credentials() {
    let (_dir, args) = fixture(
        "fixture:\n  target: dev\n  outputs:\n    dev:\n      type: snowflake\n      password: \"{{ env_var('PROFILE_METADATA_ABSENT_PASSWORD') }}\"\n",
    );
    assert_eq!(resolve_metadata(&args).unwrap().adapter_type, "snowflake");
    assert!(resolve(&args).is_err());
}

#[test]
fn selection_renders_all_connection_metadata_and_applies_merges() {
    let (_dir, mut args) = fixture(
        r#"
fixture:
  target: dev
  outputs:
    dev:
      - &first
        type: snowflake
        name: "{{ var('first_name') }}"
        password: "{{ env_var('PROFILE_METADATA_ABSENT_PASSWORD') }}"
      - <<: *first
        type: "{{ var('adapter') }}"
        name: "{{ var('second_name') }}"
        default: "{{ var('selected') }}"
"#,
    );
    for (key, value) in [
        ("first_name", "first"),
        ("second_name", "second"),
        ("adapter", "LAKECOMPUTE"),
        ("selected", "true"),
    ] {
        args.vars.insert(key.into(), dbt_yaml::Value::from(value));
    }
    assert_eq!(resolve_metadata(&args).unwrap().adapter_type, "lakecompute");
    args.vars
        .insert("selected".into(), dbt_yaml::Value::from("false"));
    assert!(matches!(
        resolve_metadata(&args),
        Err(dbt_profile::ProfileError::NoDefaultConnection { .. })
    ));
}

#[test]
fn invalid_selection_never_becomes_an_absent_adapter() {
    for output in [
        "type: \"{{ env_var('PROFILE_METADATA_ABSENT_TYPE') }}\"",
        "- type: snowflake\n        default: true\n      - type: bigquery\n        default: true",
        "- type: snowflake\n        name: repeated\n      - type: snowflake\n        name: repeated",
        "type: lake_compute",
    ] {
        let (_dir, args) = fixture(&format!(
            "fixture:\n  outputs:\n    default:\n      {output}\n"
        ));
        assert!(resolve_metadata(&args).is_err());
    }
}

#[test]
fn scoped_values_select_targets_and_resolve_secrets_without_process_mutation() {
    let (_dir, mut args) = fixture(
        r#"
fixture:
  target: dev
  outputs:
    dev:
      type: duckdb
    prod:
      type: "{{ env_var('PROFILE_METADATA_ADAPTER') }}"
      password: "{{ env_var('DBT_ENV_SECRET_METADATA_FIRST') }}:{{ env_var('DBT_ENV_SECRET_METADATA_SECOND') }}"
      account: "{{ env_var('PROFILE_METADATA_EMPTY', 'fallback') }}"
      user: "{{ env_var('PROFILE_METADATA_ABSENT', 'fallback') }}"
"#,
    );
    args.env_overrides = [
        ("DBT_TARGET", "prod"),
        ("PROFILE_METADATA_ADAPTER", "snowflake"),
        ("DBT_ENV_SECRET_METADATA_FIRST", "first"),
        ("DBT_ENV_SECRET_METADATA_SECOND", "second"),
        ("PROFILE_METADATA_EMPTY", ""),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    let resolved = resolve(&args).unwrap();
    assert_eq!(resolved.adapter_type, "snowflake");
    assert_eq!(resolved.get_str("password"), Some("first:second"));
    // Preserve YAML rendering's existing empty-string-to-null conversion.
    assert!(resolved.credentials.get("account").unwrap().is_null());
    assert_eq!(resolved.get_str("user"), Some("fallback"));
    assert_eq!(resolve_metadata(&args).unwrap().target_name, "prod");
    args.target = Some("dev".into());
    assert_eq!(resolve_metadata(&args).unwrap().adapter_type, "duckdb");
    assert!(std::env::var("DBT_ENV_SECRET_METADATA_FIRST").is_err());
    args.target = None;
    args.env_overrides.insert("DBT_TARGET".into(), "".into());
    assert_eq!(resolve_metadata(&args).unwrap().adapter_type, "duckdb");
}

#[test]
fn concurrent_scopes_do_not_share_values() {
    let (_dir, args) = fixture(
        "fixture:\n  outputs:\n    default:\n      type: \"{{ env_var('PROFILE_METADATA_CONCURRENT') }}\"\n",
    );
    let threads: Vec<_> = ["snowflake", "bigquery"]
        .into_iter()
        .map(|adapter| {
            let mut args = args.clone();
            args.env_overrides
                .insert("PROFILE_METADATA_CONCURRENT".into(), adapter.into());
            std::thread::spawn(move || {
                for _ in 0..20 {
                    assert_eq!(resolve_metadata(&args).unwrap().adapter_type, adapter);
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    assert!(std::env::var("PROFILE_METADATA_CONCURRENT").is_err());
}

#[cfg(unix)]
#[test]
fn unix_scoped_environment_keeps_differently_cased_names_distinct() {
    let (_dir, mut args) = fixture(
        r#"
fixture:
  target: dev
  outputs:
    dev:
      type: "{{ env_var('PROFILE_METADATA_CASE_ADAPTER') }}"
      account: "{{ env_var('profile_metadata_case_adapter') }}"
      password: "{{ env_var('DBT_ENV_SECRET_CASE_PASSWORD') }}"
    prod:
      type: bigquery
"#,
    );
    args.env_overrides = [
        ("DBT_TARGET", ""),
        ("dbt_target", "prod"),
        ("PROFILE_METADATA_CASE_ADAPTER", "snowflake"),
        ("profile_metadata_case_adapter", "lowercase"),
        ("DBT_ENV_SECRET_CASE_PASSWORD", "uppercase-secret"),
        ("dbt_env_secret_case_password", "lowercase-secret"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    assert_eq!(resolve_metadata(&args).unwrap().target_name, "dev");
    let resolved = resolve(&args).unwrap();
    assert_eq!(resolved.adapter_type, "snowflake");
    assert_eq!(resolved.get_str("account"), Some("lowercase"));
    assert_eq!(resolved.get_str("password"), Some("uppercase-secret"));
}

#[cfg(windows)]
#[test]
fn windows_scoped_environment_matches_spawned_process() {
    use std::process::Command;

    const CHILD_MARKER: &str = "DBT_PROFILE_WINDOWS_SCOPED_ENV_CHILD";
    if std::env::var_os(CHILD_MARKER).is_none() {
        // Seed an isolated test process so the resolver inherits known values
        // without mutating the environment of concurrent tests.
        let output = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "windows_scoped_environment_matches_spawned_process",
                "--nocapture",
            ])
            .env(CHILD_MARKER, "1")
            .env("DBT_TARGET", "dev")
            .env("PROFILE_METADATA_CASE_ADAPTER", "duckdb")
            .env("DBT_ENV_SECRET_CASE_PASSWORD", "inherited-secret")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "child test failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let (_dir, mut args) = fixture(
        r#"
fixture:
  target: dev
  outputs:
    dev:
      type: duckdb
    prod:
      type: "{{ env_var('PROFILE_METADATA_CASE_ADAPTER') }}"
      password: "{{ env_var('DBT_ENV_SECRET_CASE_PASSWORD') }}"
"#,
    );
    args.env_overrides = [
        ("dbt_target", "prod"),
        ("profile_metadata_case_adapter", "snowflake"),
        ("dbt_env_secret_case_password", "scoped-secret"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();

    for duplicate_keys in [false, true] {
        if duplicate_keys {
            args.env_overrides.insert("DBT_TARGET".into(), "dev".into());
            args.env_overrides
                .insert("PROFILE_METADATA_CASE_ADAPTER".into(), "duckdb".into());
            args.env_overrides
                .insert("DBT_ENV_SECRET_CASE_PASSWORD".into(), "other-secret".into());
        }
        let child = Command::new("cmd.exe")
            .args([
                "/d",
                "/c",
                "echo %DBT_TARGET%;%PROFILE_METADATA_CASE_ADAPTER%;%DBT_ENV_SECRET_CASE_PASSWORD%",
            ])
            .envs(&args.env_overrides)
            .output()
            .unwrap();
        assert!(child.status.success());
        let child_values = String::from_utf8(child.stdout).unwrap();
        assert_eq!(child_values.trim(), "prod;snowflake;scoped-secret");

        let resolved = resolve(&args).unwrap();
        assert_eq!(
            format!(
                "{};{};{}",
                resolved.target_name,
                resolved.adapter_type,
                resolved.get_str("password").unwrap()
            ),
            child_values.trim()
        );
        assert_eq!(resolve_metadata(&args).unwrap().target_name, "prod");
    }
    args.target = Some("dev".into());
    assert_eq!(resolve_metadata(&args).unwrap().target_name, "dev");
    assert_eq!(std::env::var("DBT_TARGET").unwrap(), "dev");
}
