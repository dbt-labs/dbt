use crate::adapter_config::{
    setup_bigquery_profile, setup_clickhouse_profile, setup_databricks_profile,
    setup_exasol_profile, setup_fabric_profile, setup_postgres_profile, setup_redshift_profile,
    setup_snowflake_profile,
};
// Re-exported so `crate::profile_setup::{ProfileTarget, Profiles}` keeps resolving.
pub use crate::adapter_config::{ProfileTarget, Profiles};
use crate::dbt_cloud_client::{CloudProject, DbtCloudClient, DbtCloudYml};
use crate::yaml_utils::{
    has_top_level_key_parsed_file, list_top_level_keys_from_file, remove_top_level_key_from_str,
};
use dbt_adapter_core::AdapterType;
use dbt_common::constants::DBT_PACKAGES_DIR_NAME;
use dbt_common::pretty_string::GREEN;
use dbt_common::tracing::dbt_emit::{emit_info_log_message, emit_warn_log_message};
use dbt_common::{ErrorCode, FsResult, fs_err, io_args::IoArgs};
use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
use dbt_jinja_utils::serde::{into_typed_with_jinja, value_from_file};
use dbt_loader::{args::LoadArgs, load_profiles};
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::project::DbtProjectSimplified;

use dialoguer::{Confirm, Select};

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Load profile using the standard dbt-loader infrastructure
fn load_profile_with_loader(
    profiles_dir: Option<&Path>,
    profile_name: &str,
    target: Option<&str>,
) -> FsResult<DbConfig> {
    let current_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let io_args = IoArgs {
        in_dir: current_dir,
        ..Default::default()
    };

    let load_args = LoadArgs {
        io: io_args,
        profiles_dir: profiles_dir.map(PathBuf::from),
        profile: Some(profile_name.to_string()),
        target: target.map(String::from),
        ..Default::default()
    };

    let dbt_project = DbtProjectSimplified {
        packages_install_path: Some(DBT_PACKAGES_DIR_NAME.to_string()),
        profile: Some(profile_name.to_string()).into(),
        dbt_cloud: None,
        flags: None,
        source_paths: Default::default(),
        log_path: Default::default(),
        target_path: Default::default(),
        __ignored__: Default::default(),
    };

    let dbt_profile = load_profiles(&load_args, &dbt_project)?;
    Ok(dbt_profile.default_db_config().clone())
}

#[derive(Debug, Clone)]
pub struct ProjectStore {
    config: DbtCloudYml,
}

impl ProjectStore {
    pub fn from_dbt_cloud_yml() -> FsResult<Option<Self>> {
        let home_dir = match dirs::home_dir() {
            Some(dir) => dir,
            None => return Ok(None),
        };

        let dbt_cloud_config_path = home_dir.join(".dbt").join("dbt_cloud.yml");
        if !dbt_cloud_config_path.exists() {
            return Ok(None);
        }

        let yaml_value = value_from_file(&dbt_cloud_config_path, true, None)?;

        let env = initialize_load_profile_jinja_environment();
        let empty_context = HashMap::<String, String>::new();

        let config: DbtCloudYml =
            into_typed_with_jinja(yaml_value, false, &env, &empty_context, &[], None, true)?;

        Ok(Some(Self { config }))
    }

    pub fn get_active_project(&self) -> Option<&CloudProject> {
        let active_project_id = &self.config.context.active_project;
        self.config
            .projects
            .iter()
            .find(|project| project.project_id == *active_project_id)
    }

    pub fn get_all_projects(&self) -> &Vec<CloudProject> {
        &self.config.projects
    }

    pub fn get_active_project_id(&self) -> &str {
        &self.config.context.active_project
    }

    pub fn get_base_url(&self, project_id: Option<&str>) -> String {
        if let Some(project_id) = project_id
            && let Some(project) = self
                .config
                .projects
                .iter()
                .find(|p| p.project_id == project_id)
        {
            return format!("https://{}", project.account_host);
        }

        format!("https://{}", self.config.context.active_host)
    }

    pub fn try_load_profile(&self, profiles_dir: &Path, profile_name: &str) -> Option<DbConfig> {
        load_profile_with_loader(Some(profiles_dir), profile_name, None).ok()
    }
}

/// Extract the adapter type for `profile_name` from a parsed profiles.yml document.
///
/// Reads the `type` of the profile's selected `target` output (falling back to the first
/// output when no `target` is set). Returns `None` when any part is missing.
fn adapter_type_for_profile(parsed: &serde_json::Value, profile_name: &str) -> Option<String> {
    let outputs = parsed.get(profile_name)?.get("outputs")?;
    let output = match parsed
        .get(profile_name)?
        .get("target")
        .and_then(|t| t.as_str())
    {
        Some(target) => outputs.get(target)?,
        None => outputs.as_object()?.values().next()?,
    };
    output
        .get("type")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// Outcome of the initial profile-action prompt shown to the user during `dbt init`.
pub enum ProfileAction {
    /// Create a brand-new profile from scratch using the interactive wizard,
    /// without pulling any credentials from dbt_cloud.yml.
    CreateNew,
    /// Create a new profile pre-populated from dbt_cloud.yml credentials.
    CreateFromCloud,
    /// Re-use an existing profile already stored in profiles.yml.
    UseExisting(String),
    /// Skip profile setup entirely.
    Skip,
}

pub struct ProfileSetup {
    pub profiles_dir: PathBuf,
    pub project_store: Option<ProjectStore>,
}

impl ProfileSetup {
    pub fn new(profiles_dir: PathBuf) -> Self {
        let project_store = ProjectStore::from_dbt_cloud_yml().unwrap_or(None);
        Self {
            profiles_dir,
            project_store,
        }
    }

    pub fn get_available_adapters() -> &'static [AdapterType] {
        &[
            AdapterType::Snowflake,
            AdapterType::Databricks,
            AdapterType::Bigquery,
            AdapterType::ClickHouse,
            AdapterType::Exasol,
            AdapterType::Postgres,
            AdapterType::Redshift,
            AdapterType::Fabric,
        ]
    }

    pub fn ask_for_adapter_choice(default_adapter: Option<AdapterType>) -> FsResult<AdapterType> {
        let adapters = Self::get_available_adapters();
        let default_index = default_adapter
            .and_then(|d| adapters.iter().position(|a| *a == d))
            .unwrap_or(0);

        let selection = Select::new()
            .with_prompt("Which adapter would you like to use?")
            .items(adapters)
            .default(default_index)
            .interact()
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get adapter selection: {}", e))?;

        Ok(adapters[selection])
    }

    /// Resolve the active profiles.yml path.
    ///
    /// Mirrors the path-resolution logic used by `write_profile`: a local `./profiles.yml`
    /// takes precedence over `<profiles_dir>/profiles.yml`.
    fn active_profiles_path(&self) -> PathBuf {
        let local = PathBuf::from("profiles.yml");
        if local.exists() {
            local
        } else {
            self.profiles_dir.join("profiles.yml")
        }
    }

    /// Return the names of all profiles already stored in the active profiles.yml.
    pub fn list_existing_profiles(&self) -> FsResult<Vec<String>> {
        list_top_level_keys_from_file(&self.active_profiles_path())
    }

    /// Return `(profile_name, adapter_type)` for every profile in the active profiles.yml,
    /// preserving document order. `adapter_type` is `None` when the type cannot be
    /// determined (e.g. malformed profile or missing `outputs`).
    pub fn list_existing_profiles_with_types(&self) -> FsResult<Vec<(String, Option<String>)>> {
        let target = self.active_profiles_path();
        let names = list_top_level_keys_from_file(&target)?;
        if names.is_empty() {
            return Ok(vec![]);
        }

        // Best-effort parse to read each profile's adapter type; fall back to `None` on any
        // parse failure so a malformed file still lists the names.
        let parsed = fs::read_to_string(&target)
            .ok()
            .and_then(|content| dbt_yaml::from_str::<serde_json::Value>(&content).ok());

        Ok(names
            .into_iter()
            .map(|name| {
                let adapter_type = parsed
                    .as_ref()
                    .and_then(|p| adapter_type_for_profile(p, &name));
                (name, adapter_type)
            })
            .collect())
    }

    /// Ask the user how they would like to handle profiles when initialising a new project.
    ///
    /// Options are built dynamically: "Use an existing profile" only appears when profiles.yml
    /// contains at least one profile, and "Create a new profile based on dbt_cloud.yml
    /// credentials" only appears when a dbt_cloud.yml was found. When neither is available the
    /// prompt is skipped and `ProfileAction::CreateNew` is returned.
    pub fn prompt_profile_action(&self) -> FsResult<ProfileAction> {
        let existing = self.list_existing_profiles_with_types()?;
        let has_cloud = self.project_store.is_some();

        // Nothing special to offer: behave exactly as before and go straight to the wizard.
        if existing.is_empty() && !has_cloud {
            return Ok(ProfileAction::CreateNew);
        }

        // Build the option list and a parallel list of the action each entry maps to.
        let mut labels: Vec<String> = Vec::new();
        let mut actions: Vec<ProfileAction> = Vec::new();

        if !existing.is_empty() {
            labels.push("Use an existing profile from profiles.yml".to_string());
            actions.push(ProfileAction::UseExisting(String::new()));
        }
        if has_cloud {
            labels.push("Create a new profile based on dbt_cloud.yml credentials".to_string());
            actions.push(ProfileAction::CreateFromCloud);
        }
        labels.push("Set up a new profile from scratch".to_string());
        actions.push(ProfileAction::CreateNew);
        labels.push("Skip profile setup".to_string());
        actions.push(ProfileAction::Skip);

        let choice = Select::new()
            .with_prompt("Which would you like to do?")
            .items(&labels)
            .default(0)
            .interact()
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get selection: {}", e))?;

        match &actions[choice] {
            ProfileAction::UseExisting(_) => {
                let profile_labels: Vec<String> = existing
                    .iter()
                    .map(|(name, adapter_type)| match adapter_type {
                        Some(t) => format!("{name} ({t})"),
                        None => name.clone(),
                    })
                    .collect();

                let selection = Select::new()
                    .with_prompt("Select a profile to use")
                    .items(&profile_labels)
                    .default(0)
                    .interact()
                    .map_err(|e| {
                        fs_err!(ErrorCode::IoError, "Failed to get profile selection: {}", e)
                    })?;
                Ok(ProfileAction::UseExisting(existing[selection].0.clone()))
            }
            ProfileAction::CreateFromCloud => Ok(ProfileAction::CreateFromCloud),
            ProfileAction::CreateNew => Ok(ProfileAction::CreateNew),
            ProfileAction::Skip => Ok(ProfileAction::Skip),
        }
    }

    #[allow(unreachable_patterns)]
    pub fn create_profile_for_adapter(
        &self,
        adapter: AdapterType,
        _profile_name: &str,
        existing_config: Option<&DbConfig>,
    ) -> FsResult<ProfileTarget> {
        let db_config = match adapter {
            AdapterType::Snowflake => {
                let snowflake_config = match existing_config {
                    Some(DbConfig::Snowflake(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Snowflake(setup_snowflake_profile(snowflake_config.map(Box::as_ref))?)
            }
            AdapterType::Bigquery => {
                let bigquery_config = match existing_config {
                    Some(DbConfig::Bigquery(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Bigquery(setup_bigquery_profile(bigquery_config.map(Box::as_ref))?)
            }
            AdapterType::Databricks => {
                let databricks_config = match existing_config {
                    Some(DbConfig::Databricks(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Databricks(setup_databricks_profile(
                    databricks_config.map(Box::as_ref),
                )?)
            }
            AdapterType::Postgres => {
                let postgres_config = match existing_config {
                    Some(DbConfig::Postgres(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Postgres(setup_postgres_profile(postgres_config.map(Box::as_ref))?)
            }
            AdapterType::Redshift => {
                let redshift_config = match existing_config {
                    Some(DbConfig::Redshift(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Redshift(setup_redshift_profile(redshift_config.map(Box::as_ref))?)
            }
            AdapterType::Spark => {
                let _salesforce_config = match existing_config {
                    Some(DbConfig::Spark(config)) => Some(config),
                    _ => None,
                };
                todo!("setup_spark_profile")
            }
            AdapterType::Salesforce => {
                let _salesforce_config = match existing_config {
                    Some(DbConfig::Salesforce(config)) => Some(config),
                    _ => None,
                };
                todo!("setup_salesforce_profile")
            }
            AdapterType::Fabric => {
                let fabric_config = match existing_config {
                    Some(DbConfig::Fabric(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Fabric(setup_fabric_profile(fabric_config.map(Box::as_ref))?)
            }

            AdapterType::DuckDB => {
                // DuckDB doesn't require credentials for local file-based operations
                // TODO: Create proper DuckDB profile setup
                return Err(fs_err!(
                    ErrorCode::Generic,
                    "DuckDB profile setup not yet implemented. DuckDB runs locally without credentials."
                ));
            }
            AdapterType::LakeCompute => {
                // TODO: Create proper lake compute profile setup
                return Err(fs_err!(
                    ErrorCode::Generic,
                    "lake_compute profile setup not yet implemented."
                ));
            }
            AdapterType::ClickHouse => {
                let clickhouse_config = match existing_config {
                    Some(DbConfig::ClickHouse(config)) => Some(config),
                    _ => None,
                };
                DbConfig::ClickHouse(setup_clickhouse_profile(
                    clickhouse_config.map(Box::as_ref),
                )?)
            }
            AdapterType::Exasol => {
                let exasol_config = match existing_config {
                    Some(DbConfig::Exasol(config)) => Some(config),
                    _ => None,
                };
                DbConfig::Exasol(setup_exasol_profile(exasol_config.map(Box::as_ref))?)
            }
            AdapterType::Starburst => todo!("Starburst"),
            AdapterType::Athena => todo!("Athena"),
            AdapterType::Trino => todo!("Trino"),
            AdapterType::Datafusion => todo!("Datafusion"),
            AdapterType::Dremio => todo!("Dremio"),
            AdapterType::Oracle => todo!("Oracle"),
        };

        let mut outputs = HashMap::new();
        outputs.insert("dev".to_string(), db_config);

        Ok(ProfileTarget {
            target: "dev".to_string(),
            outputs,
        })
    }

    /// Write or update a single profile block in the appropriate profiles.yml,
    /// preserving existing content, order, and comments.
    pub fn write_profile(&self, profile_name: &str, profile: &ProfileTarget) -> FsResult<()> {
        // Determine target profiles.yml path:
        // 1) If ./profiles.yml exists, prefer writing there
        // 2) Else write to self.profiles_dir/profiles.yml (creating directory if needed)
        let local_profiles = PathBuf::from("profiles.yml");
        let target_file: PathBuf = if local_profiles.exists() {
            local_profiles
        } else {
            let profiles_dir = Path::new(&self.profiles_dir);
            if !profiles_dir.exists() {
                fs::create_dir_all(profiles_dir)?;
            }
            profiles_dir.join("profiles.yml")
        };

        let mut existing = if target_file.exists() {
            fs::read_to_string(&target_file)?
        } else {
            String::new()
        };

        if has_top_level_key_parsed_file(&target_file, profile_name)? {
            let overwrite = Confirm::new()
                .with_prompt(format!(
                    "The profile '{}' already exists in {}. Continue and overwrite it?",
                    profile_name,
                    target_file.display()
                ))
                .default(false)
                .interact()
                .map_err(|e| {
                    fs_err!(
                        ErrorCode::IoError,
                        "Failed to get overwrite confirmation: {}",
                        e
                    )
                })?;

            if !overwrite {
                return Err(fs_err!(ErrorCode::IoError, "Profile setup cancelled"));
            }

            if target_file.exists() && !existing.is_empty() {
                let backup_file = if target_file.file_name().is_some() {
                    target_file.with_extension("yml.bkp")
                } else {
                    target_file.with_extension("bkp")
                };
                fs::write(&backup_file, &existing)?;
                emit_info_log_message(format!(
                    "{} Backup created at {}",
                    GREEN.apply_to("Info"),
                    backup_file.display()
                ));
            }
        }

        existing = remove_top_level_key_from_str(existing, profile_name);

        if !existing.is_empty() && !existing.ends_with('\n') {
            existing.push('\n');
        }

        let mut top: HashMap<String, ProfileTarget> = HashMap::new();
        top.insert(profile_name.to_string(), profile.clone());
        let new_block = dbt_yaml::to_string(&top).map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to serialize profile block: {}",
                e
            )
        })?;

        existing.push_str(&new_block);

        if !existing.ends_with('\n') {
            existing.push('\n');
        }

        fs::write(&target_file, existing)?;

        emit_info_log_message(format!(
            "{} Profile written to {}",
            GREEN.apply_to("Success"),
            target_file.display()
        ));
        Ok(())
    }

    async fn handle_cloud_project_selection(project_store: &ProjectStore) -> FsResult<String> {
        let active_project = project_store.get_active_project();
        let all_projects = project_store.get_all_projects();

        if let Some(active) = active_project {
            emit_info_log_message(format!(
                "Found active project: {} (ID: {})",
                active.project_name, active.project_id
            ));

            let use_active = Confirm::new()
                .with_prompt(format!(
                    "Use active project '{}' from dbt_cloud.yml?",
                    active.project_name
                ))
                .default(true)
                .interact()
                .map_err(|e| {
                    fs_err!(ErrorCode::IoError, "Failed to get project selection: {}", e)
                })?;

            if use_active {
                return Ok(active.project_id.clone());
            }
        }

        if all_projects.is_empty() {
            return Err(fs_err!(
                ErrorCode::IoError,
                "No projects found in dbt_cloud.yml"
            ));
        }

        let project_names: Vec<String> = all_projects
            .iter()
            .map(|p| format!("{} (ID: {})", p.project_name, p.project_id))
            .collect();

        let selection = Select::new()
            .with_prompt("Select a project from dbt platform:")
            .items(&project_names)
            .interact()
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get project selection: {}", e))?;

        Ok(all_projects[selection].project_id.clone())
    }

    async fn fetch_cloud_config(
        project_store: &ProjectStore,
        project_id: &str,
        adapter: AdapterType,
    ) -> FsResult<Option<DbConfig>> {
        let base_url = project_store.get_base_url(Some(project_id));

        match DbtCloudClient::get_credential_db_config(&base_url, Some(project_id), Some(adapter))
            .await
        {
            Ok(db_config) => Ok(db_config),
            Err(e) => {
                emit_warn_log_message(
                    ErrorCode::DbtPlatformApiError,
                    format!("Failed to fetch cloud config: {e}"),
                );
                Ok(None)
            }
        }
    }

    /// Run the interactive profile wizard for `profile_name`.
    ///
    /// When `use_cloud` is true and a dbt_cloud.yml was found, a brand-new profile is
    /// pre-populated from the selected cloud project's credentials. When false the wizard
    /// never reaches out to dbt_cloud.yml (the "from scratch" path).
    pub async fn setup_profile(&self, profile_name: &str, use_cloud: bool) -> FsResult<()> {
        emit_info_log_message(format!(
            "{} Setting up your profile...",
            GREEN.apply_to("Info")
        ));

        // Load the profile once at the beginning and cache the result
        let existing_config = if let Some(store) = &self.project_store {
            store.try_load_profile(&self.profiles_dir, profile_name)
        } else {
            load_profile_with_loader(Some(&self.profiles_dir), profile_name, None).ok()
        };

        let profile_exists = existing_config.is_some();

        let profile_action = if profile_exists {
            emit_info_log_message(format!(
                "Profile '{profile_name}' already exists. You can choose how to proceed."
            ));

            use dialoguer::Select;
            let options = vec![
                "Edit existing profile (update fields interactively)",
                "Overwrite completely (start fresh)",
                "Cancel (keep existing profile as-is)",
            ];

            Select::new()
                .with_prompt("What would you like to do?")
                .items(&options)
                .default(0)
                .interact()
                .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get selection: {}", e))?
        } else {
            1 // New profile, equivalent to "overwrite"
        };

        match profile_action {
            0 => {
                emit_info_log_message(format!(
                    "{} Editing existing profile...",
                    GREEN.apply_to("Info")
                ));
            }
            1 => {
                emit_info_log_message(format!(
                    "{} Creating new profile...",
                    GREEN.apply_to("Info")
                ));
            }
            2 => {
                emit_info_log_message(format!(
                    "{} Profile setup cancelled.",
                    GREEN.apply_to("Info")
                ));
                return Ok(());
            }
            _ => unreachable!(),
        }

        let adapter_type = existing_config.as_ref().map(|d| d.adapter_type());
        let adapter = Self::ask_for_adapter_choice(adapter_type)?;

        let cloud_config = if profile_action == 1 && use_cloud {
            if let Some(project_store) = &self.project_store {
                emit_info_log_message("Found dbt_cloud.yml configuration");
                let project_id = Self::handle_cloud_project_selection(project_store).await?;

                match Self::fetch_cloud_config(project_store, &project_id, adapter).await? {
                    Some(config) => Some(config),
                    None => {
                        emit_info_log_message("No cloud config found for this adapter/project");
                        None
                    }
                }
            } else {
                emit_info_log_message(
                    "No dbt_cloud.yml found - proceeding without cloud pre-population",
                );
                None
            }
        } else if profile_action == 1 {
            // "Set up a new profile from scratch": intentionally do not read dbt_cloud.yml.
            None
        } else {
            emit_info_log_message("Editing existing profile - skipping cloud config fetch");
            None
        };

        let should_use_existing_config = existing_config
            .as_ref()
            .map(|d| d.adapter_type() == adapter)
            .unwrap_or(false);

        let final_existing_config = if should_use_existing_config {
            existing_config.as_ref()
        } else {
            if let Some(existing_config) = existing_config.as_ref() {
                emit_info_log_message(format!(
                    "Adapter type changed from '{}' to '{}' - starting with fresh configuration",
                    existing_config.adapter_type(),
                    adapter
                ));
            }
            None
        };

        let merged_config = cloud_config.or_else(|| final_existing_config.cloned());

        let profile =
            self.create_profile_for_adapter(adapter, profile_name, merged_config.as_ref())?;
        self.write_profile(profile_name, &profile)?;

        Ok(())
    }

    pub fn cloud_client() -> &'static DbtCloudClient {
        &DbtCloudClient
    }
}

#[cfg(test)]
mod tests {
    use super::adapter_type_for_profile;

    fn parse(yaml: &str) -> serde_json::Value {
        dbt_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn reads_type_from_selected_target() {
        let parsed = parse(
            "my_profile:\n  target: prod\n  outputs:\n    dev:\n      type: postgres\n    prod:\n      type: snowflake\n",
        );
        assert_eq!(
            adapter_type_for_profile(&parsed, "my_profile").as_deref(),
            Some("snowflake")
        );
    }

    #[test]
    fn falls_back_to_first_output_without_target() {
        let parsed = parse("my_profile:\n  outputs:\n    dev:\n      type: bigquery\n");
        assert_eq!(
            adapter_type_for_profile(&parsed, "my_profile").as_deref(),
            Some("bigquery")
        );
    }

    #[test]
    fn returns_none_when_type_missing() {
        let parsed = parse("my_profile:\n  outputs:\n    dev:\n      host: localhost\n");
        assert_eq!(adapter_type_for_profile(&parsed, "my_profile"), None);
        assert_eq!(adapter_type_for_profile(&parsed, "unknown_profile"), None);
    }
}
