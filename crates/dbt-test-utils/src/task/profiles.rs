//! Tasks for working with dbt profiles.

use crate::profiles::{
    load_db_config, load_db_config_from_test_profile, write_db_config_to_test_profile,
};
use crate::task::{ProjectEnv, Task, TestEnv, TestResult};

use async_trait::async_trait;
use dbt_schemas::schemas::profiles::DbConfig;
use merge::Merge;

/// Used to load a fully resolved profiles.yml from the default directory (~/.dbt)
/// and write it to the project env.
#[derive(Debug, Clone)]
pub struct HydrateProfilesTask {
    pub schema: String,
    pub target: String,
}

#[async_trait]
impl Task for HydrateProfilesTask {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        _test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        // the test project doesn't have a profiles.yml, attempts to use the one at .dbt
        let original_profiles_yml = project_env.absolute_project_dir.join("profiles.yml");

        if !original_profiles_yml.exists() {
            let mut db_config = load_db_config_from_test_profile(&self.target, &self.schema)?;

            let override_profiles_path = project_env
                .absolute_project_dir
                .join("_profiles.override.yml");

            if override_profiles_path.exists() {
                // The override file may only contain entries for some adapters
                // (e.g. only duckdb). Silently skip if the target isn't present.
                if let Ok(override_db_config) =
                    load_db_config(&self.target, &self.schema, &override_profiles_path)
                {
                    override_with(&mut db_config, override_db_config);
                }
            }

            write_db_config_to_test_profile(db_config, &project_env.absolute_project_dir)?;
        }
        Ok(())
    }
}

fn override_with(original: &mut DbConfig, override_: DbConfig) {
    match (original, override_) {
        (DbConfig::Bigquery(self_bigquery), DbConfig::Bigquery(other_bigquery)) => {
            self_bigquery.merge(*other_bigquery);
        }
        (DbConfig::DuckDB(self_duckdb), DbConfig::DuckDB(other_duckdb)) => {
            *self_duckdb = other_duckdb;
        }
        (DbConfig::Redshift(self_redshift), DbConfig::Redshift(other_redshift)) => {
            self_redshift.merge(*other_redshift);
        }
        _ => unimplemented!("database config override for non-BigQuery/DuckDB/Redshift adapters"),
    }
}
