//! Snapshot tests for `compose_v2_catalog_attach_stmts`.
//!
//! Each scenario under `tests/duckdb_attach_fixtures/<scenario>/catalogs.yml`
//! is parsed and the joined ATTACH (and optional `INSTALL ducklake`) statements
//! are snapshotted to a sibling `output.snap` for side-by-side reviewability.
//! Update goldens: `cargo insta review` or `cargo insta accept`.
//! Run: `cargo xtask test --llm --no-external-deps -p dbt-adapter duckdb_attach_fixtures`

use dbt_adapter::engine::duckdb_attach::compose_v2_catalog_attach_stmts;
use dbt_schemas::schemas::dbt_catalogs::DbtCatalogs;

const SCENARIOS: &[&str] = &[
    "alias_collision_error",
    "ducklake_full_options",
    "ducklake_minimal",
    "empty_alias_error",
    "glue_endpoint_type",
    "glue_explicit_endpoint",
    "horizon_duckdb",
    "horizon_duckdb_user_overrides",
    "iceberg_rest_endpoint_type_glue",
    "iceberg_rest_full_options",
    "iceberg_rest_minimal",
    "iceberg_rest_string_bool_options",
    "local_filesystem_no_attach",
    "multi_catalog_iceberg_rest",
    "multi_catalog_with_ducklake",
    "s3_tables_endpoint_type",
    "unity_duckdb",
];

fn render(yaml: &str) -> String {
    let parsed: dbt_yaml::Value = dbt_yaml::from_str(yaml).expect("valid YAML");
    let dbt_yaml::Value::Mapping(repr, span) = parsed else {
        panic!("fixture must be a top-level mapping");
    };
    let catalogs = DbtCatalogs::new(repr, span);
    let view = catalogs.view_v2().expect("valid v2 catalog view");
    match compose_v2_catalog_attach_stmts(&view, "duckdb") {
        Ok(stmts) => stmts.join("\n"),
        Err(e) => format!("error: {:?}: {}", e.kind(), e),
    }
}

#[test]
fn duckdb_attach_fixtures() {
    let fixtures_root =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/duckdb_attach_fixtures");
    for scenario in SCENARIOS {
        let scenario_dir = fixtures_root.join(scenario);
        let yaml = std::fs::read_to_string(scenario_dir.join("catalogs.yml"))
            .unwrap_or_else(|_| panic!("read fixture: {scenario}"));
        insta::with_settings!(
            {
                prepend_module_to_snapshot => false,
                snapshot_path => &scenario_dir,
                snapshot_suffix => "",
                omit_expression => true,
            },
            { insta::assert_snapshot!("output", render(&yaml)) }
        );
    }
}
