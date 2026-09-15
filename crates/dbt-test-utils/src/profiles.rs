use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_profile::{ProfileEnvironment, resolve_with_env};
use dbt_schemas::schemas::profiles::{DbConfig, DbTargets};

use dbt_schemas::schemas::serde::yaml_to_fs_error;
use dbt_yaml;
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::path::{Path, PathBuf};

const TEST_PROFILE: &str = "fusion_tests";

/// Load the db config from a 'test' profiles.yml at the default profile path (~/.dbt)
/// and set schema and database values
pub fn load_db_config_from_test_profile_with_database(
    target: &str,
    schema: &str,
    database: &str,
) -> FsResult<DbConfig> {
    let mut db_config = load_db_config_from_test_profile(target, schema)?;
    match &mut db_config {
        DbConfig::Postgres(pg) => {
            pg.database = Some(database.to_string());
        }
        DbConfig::Snowflake(sf) => {
            sf.database = Some(database.to_string());
        }
        DbConfig::Redshift(rs) => {
            rs.database = Some(database.to_string());
        }
        DbConfig::Bigquery(bq) => {
            bq.database = Some(database.to_string());
        }
        DbConfig::Databricks(db) => {
            db.database = Some(database.to_string());
        }
        DbConfig::ClickHouse(ch) => {
            ch.database = Some(database.to_string());
        }
        DbConfig::Athena(at) => {
            at.database = Some(database.to_string());
        }
        _ => {}
    }

    Ok(db_config)
}

/// Load the db config from a 'test' profiles.yml at the default profile path (~/.dbt)
/// and set schema value
pub fn load_db_config_from_test_profile(target: &str, schema: &str) -> FsResult<DbConfig> {
    let home_dir = dirs::home_dir().expect("home dir exists");
    // ! This must be consistent with what's written out from the init_creds.rs (from xtask crate)
    let profile_path = home_dir.join(".dbt").join("profiles.yml");
    load_db_config(target, schema, &profile_path)
}

/// Load the target db config from a profiles.yml at a give directory
pub fn load_db_config<P: AsRef<Path>>(
    target: &str,
    schema: &str,
    profile_path: P,
) -> FsResult<DbConfig> {
    let penv = ProfileEnvironment::new(BTreeMap::new());
    let resolved = resolve_with_env(&penv, profile_path.as_ref(), TEST_PROFILE, Some(target))
        .map_err(|e| fs_err!(ErrorCode::InvalidConfig, "{}", e))?;

    let credentials_value =
        dbt_yaml::Value::Mapping(resolved.credentials, dbt_yaml::Span::default());
    let mut db_config: DbConfig = dbt_yaml::from_value(credentials_value).map_err(|e| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "Failed to parse profiles.yml: {}",
            e
        )
    })?;

    match &mut db_config {
        DbConfig::Postgres(pg) => {
            pg.schema = Some(schema.to_string());
        }
        DbConfig::Snowflake(sf) => {
            sf.schema = Some(schema.to_string());
        }
        DbConfig::Redshift(rs) => {
            rs.schema = Some(schema.to_string());
        }
        DbConfig::Bigquery(bq) => {
            bq.schema = Some(schema.to_string());
        }
        DbConfig::Databricks(db) => {
            db.schema = Some(schema.to_string());
        }
        DbConfig::DuckDB(duck) => {
            duck.schema = Some(schema.to_string());
        }
        DbConfig::Spark(s) => {
            s.schema = Some(schema.to_string());
        }
        DbConfig::ClickHouse(ch) => {
            ch.schema = Some(schema.to_string());
        }
        DbConfig::Athena(at) => {
            at.schema = Some(schema.to_string());
        }
        _ => {}
    }

    Ok(db_config)
}

/// Write the db config to a 'test' profiles.yml at a give directory
pub fn write_db_config_to_test_profile(
    db_config: DbConfig,
    profile_dir: &Path,
) -> FsResult<PathBuf> {
    let profile_path = profile_dir.join("profiles.yml");
    let mut file = File::create(&profile_path)?;

    let adapter_type = db_config.adapter_type().to_string();

    let profile = HashMap::from([(
        TEST_PROFILE,
        DbTargets {
            default_target: adapter_type.to_string(),
            outputs: HashMap::from([(
                adapter_type,
                dbt_yaml::to_value(db_config).map_err(|e| {
                    fs_err!(
                        ErrorCode::InvalidConfig,
                        "Failed to serialize db config: {}",
                        e
                    )
                })?,
            )]),
        },
    )]);
    dbt_yaml::to_writer(&mut file, &profile)
        .map_err(|e| yaml_to_fs_error(e, Some(&profile_path)))?;
    Ok(profile_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_test_primitives::assert_contains;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    #[ignore = "This test is for local debugging but unnecessary for CI, since the functions are covered in other tests"]
    fn test_load_and_write_profile_roundtrip() -> FsResult<()> {
        // load the test profile
        let db_config = load_db_config_from_test_profile("postgres", "test_schema")?;

        let temp_dir = tempdir()?;
        let profile_path = write_db_config_to_test_profile(db_config, temp_dir.path())?;

        assert!(profile_path.exists());
        let profile_contents = fs::read_to_string(&profile_path)?;
        assert_contains!(profile_contents, "test_schema");
        assert_contains!(profile_contents, "postgres");
        assert_contains!(profile_contents, "test:");
        assert!(!profile_contents.is_empty());

        // Clean up
        temp_dir.close()?;

        Ok(())
    }
}
