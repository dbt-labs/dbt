use std::collections::BTreeMap;

use dbt_schemas::schemas::profiles::{DbConfig, TargetContext};
use minijinja::value::Value as MinijinjaValue;

/// The parse env globals a connection sets: `target` (also `env`), `database`, `schema`.
pub struct TargetGlobals {
    pub target: BTreeMap<String, MinijinjaValue>,
    pub database: Option<String>,
    pub schema: Option<String>,
}

/// The [`TargetGlobals`] of `db_config` as target `target` of `profile`.
pub fn build_target_globals(
    profile: &str,
    target: &str,
    db_config: DbConfig,
) -> Result<TargetGlobals, String> {
    // DuckDB and LakeCompute default an unset database and schema, as dbt-duckdb does.
    let defaults_from_path = matches!(db_config, DbConfig::DuckDB(_) | DbConfig::LakeCompute(_));
    let mut database = db_config.get_database().cloned();
    let mut schema = db_config.get_schema().cloned();
    let target_context = TargetContext::try_from(db_config)?;
    let target_map = build_target_context_map(profile, target, target_context);
    if defaults_from_path {
        let field = |key: &str| {
            target_map
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
        };
        database = field("database");
        schema = field("schema");
    }
    Ok(TargetGlobals {
        target: target_map,
        database,
        schema,
    })
}

pub fn build_target_context_map(
    profile: &str,
    target: &str,
    target_context: TargetContext,
) -> BTreeMap<String, MinijinjaValue> {
    let target_context_val = dbt_yaml::to_value(&target_context).unwrap();
    let mut target_context_map: BTreeMap<String, MinijinjaValue> =
        dbt_yaml::from_value(target_context_val).unwrap();
    target_context_map.insert("profile_name".to_string(), MinijinjaValue::from(profile));
    target_context_map.insert("name".to_string(), MinijinjaValue::from(target));
    target_context_map.insert("target_name".to_string(), MinijinjaValue::from(target));
    target_context_map
}

#[cfg(test)]
mod tests {
    use dbt_schemas::schemas::profiles::{DatabricksDbConfig, DuckDbConfig, LakeComputeConfig};

    use super::*;

    fn duckdb(path: Option<&str>, database: Option<&str>, schema: Option<&str>) -> DbConfig {
        DbConfig::DuckDB(Box::new(DuckDbConfig {
            path: path.map(str::to_owned),
            database: database.map(str::to_owned),
            schema: schema.map(str::to_owned),
            ..Default::default()
        }))
    }

    fn lake_compute(path: Option<&str>, database: Option<&str>, schema: Option<&str>) -> DbConfig {
        DbConfig::LakeCompute(Box::new(LakeComputeConfig {
            path: path.map(str::to_owned),
            database: database.map(str::to_owned),
            schema: schema.map(str::to_owned),
            ..Default::default()
        }))
    }

    fn globals_of(db_config: DbConfig) -> TargetGlobals {
        build_target_globals("p", "dev", db_config).unwrap()
    }

    fn target_field(globals: &TargetGlobals, key: &str) -> Option<String> {
        globals.target[key].as_str().map(str::to_owned)
    }

    #[test]
    fn duckdb_and_lake_compute_default_database_and_schema_from_the_target() {
        for (path, expected_database) in [
            (Some("/data/jaffle_shop.duckdb"), "jaffle_shop"),
            (Some(":memory:"), "memory"),
            (None, "memory"),
        ] {
            for db_config in [duckdb(path, None, None), lake_compute(path, None, None)] {
                let globals = globals_of(db_config);
                assert_eq!(globals.database.as_deref(), Some(expected_database));
                assert_eq!(globals.schema.as_deref(), Some("main"));
                assert_eq!(globals.database, target_field(&globals, "database"));
                assert_eq!(globals.schema, target_field(&globals, "schema"));
            }
        }
    }

    #[test]
    fn an_authored_database_and_schema_win() {
        for db_config in [
            duckdb(
                Some("/data/jaffle_shop.duckdb"),
                Some("analytics"),
                Some("dbt_x"),
            ),
            lake_compute(
                Some("/data/jaffle_shop.duckdb"),
                Some("analytics"),
                Some("dbt_x"),
            ),
        ] {
            let globals = globals_of(db_config);
            assert_eq!(globals.database.as_deref(), Some("analytics"));
            assert_eq!(globals.schema.as_deref(), Some("dbt_x"));
        }
    }

    #[test]
    fn other_adapters_keep_the_authored_fields() {
        let globals = globals_of(DbConfig::Databricks(Box::new(DatabricksDbConfig {
            schema: Some("dbt_x".to_string()),
            ..Default::default()
        })));
        assert_eq!(globals.database, None);
        assert_eq!(globals.schema.as_deref(), Some("dbt_x"));
        assert_eq!(
            target_field(&globals, "database").as_deref(),
            Some("hive_metastore")
        );
    }
}
