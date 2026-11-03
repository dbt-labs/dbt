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
use dbt_profile_schemas::profile::ProfileTarget;
use dbt_schemas::schemas::profiles::DbConfig;
use dbt_schemas::schemas::project::DbtProjectSimplified;

use dialoguer::{Confirm, Select};

use std::collections::{BTreeMap, HashMap};
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
    // A list of outputs (from the Lake Compute setup): its default is the
    // target's adapter.
    let output = match output.as_array() {
        Some(list) if dbt_flags::LAKECOMPUTE_PROFILE_SETUP.enabled() => {
            list.get(default_output(list))?
        }
        _ => output,
    };
    output
        .get("type")
        .and_then(|t| t.as_str())
        .map(|s| s.to_string())
}

/// The outputs of `profile_name`'s `target` in a parsed profiles.yml, when
/// it's a list of outputs.
fn target_outputs(
    parsed: &serde_json::Value,
    profile_name: &str,
    target: &str,
) -> Vec<serde_json::Value> {
    parsed
        .get(profile_name)
        .and_then(|p| p.get("outputs"))
        .and_then(|o| o.get(target))
        .and_then(|t| t.as_array())
        .cloned()
        .unwrap_or_default()
}

/// Which of a target's list of outputs is its default: the one flagged
/// `default: true` (also as a string, as dbt reads it rendered), else the first.
fn default_output(list: &[serde_json::Value]) -> usize {
    list.iter()
        .position(|o| match o.get("default") {
            Some(serde_json::Value::Bool(flag)) => *flag,
            Some(serde_json::Value::String(flag)) => flag.trim().eq_ignore_ascii_case("true"),
            _ => false,
        })
        .unwrap_or(0)
}

fn is_lakecompute(output: &serde_json::Value) -> bool {
    output_type(output) == Some(AdapterType::LakeCompute)
}

/// An output's adapter, as dbt reads its `type` (case doesn't matter).
fn output_type(output: &serde_json::Value) -> Option<AdapterType> {
    output.get("type")?.as_str()?.parse().ok()
}

/// `profile` with a lakecompute output (when `lakecompute`) beside its
/// target's warehouse output, which stays the default: the target becomes a
/// list of outputs, as dbt reads one where models can run on more than one
/// adapter. `existing` is the target's list as it was: its lakecompute output
/// and any other outputs are kept as they were, and its default's name too.
fn with_lakecompute(
    profile: &ProfileTarget,
    existing: &[serde_json::Value],
    lakecompute: bool,
) -> FsResult<dbt_yaml::Value> {
    use dbt_yaml::{Mapping, Span, Value};
    let yaml = |e: dbt_yaml::Error| fs_err!(ErrorCode::IoError, "Failed to serialize profile: {e}");
    let warehouse = profile.outputs.get(&profile.target).ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "target '{}' has no output to add lakecompute beside",
            profile.target
        )
    })?;
    // The warehouse output this replaces: the default, when it's not the
    // lakecompute one; else the first output on the warehouse's adapter.
    let default_at = default_output(existing);
    let replaced = existing
        .get(default_at)
        .is_some_and(|o| !is_lakecompute(o))
        .then_some(default_at)
        .or_else(|| {
            existing
                .iter()
                .position(|o| output_type(o) == Some(warehouse.adapter_type()))
        });
    let name = replaced
        .and_then(|i| existing[i].get("name").and_then(|n| n.as_str()))
        .map_or_else(|| warehouse.adapter_type().to_string(), str::to_string);
    // An output named `name`: `config`'s fields after the name (and flag).
    let named = |name: &str, default: bool, config: &DbConfig| -> FsResult<Value> {
        let mut output = Mapping::new();
        output.insert(Value::from("name"), Value::from(name));
        if default {
            output.insert(Value::from("default"), Value::from(true));
        }
        if let Value::Mapping(fields, _) = dbt_yaml::to_value(config).map_err(yaml)? {
            output.extend(fields);
        }
        Ok(Value::Mapping(output, Span::default()))
    };
    let mut outputs = vec![named(&name, true, warehouse)?];
    for (i, output) in existing.iter().enumerate() {
        if Some(i) == replaced {
            continue;
        }
        let mut output = output.clone();
        // The warehouse output is the default now.
        if let Some(fields) = output.as_object_mut() {
            fields.remove("default");
        }
        outputs.push(dbt_yaml::to_value(&output).map_err(yaml)?);
    }
    if lakecompute && !existing.iter().any(is_lakecompute) {
        let lakecompute =
            dbt_profile_schemas::ProfileSetup::new(AdapterType::LakeCompute).setup(None)?;
        outputs.push(named(
            AdapterType::LakeCompute.as_ref(),
            false,
            &lakecompute,
        )?);
    }
    let mut value = dbt_yaml::to_value(profile).map_err(yaml)?;
    if let Some(Value::Mapping(targets, _)) = value.get_mut("outputs") {
        targets.insert(
            Value::from(profile.target.as_str()),
            Value::Sequence(outputs, Span::default()),
        );
    }
    Ok(value)
}

/// Asks whether to add a lakecompute output beside Snowflake; yes by default
/// when the target has one (setting it up again keeps it).
fn ask_for_lakecompute(has: bool) -> FsResult<bool> {
    let prompt = "Also run models on Lake Compute? (adds a lakecompute output beside Snowflake)";
    Confirm::new()
        .with_prompt(prompt)
        .default(has)
        .interact()
        .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get confirmation: {}", e))
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
            AdapterType::DuckDB,
            AdapterType::Exasol,
            AdapterType::Postgres,
            AdapterType::Redshift,
            AdapterType::Fabric,
            AdapterType::SingleStore,
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
        let profile_setup = dbt_profile_schemas::ProfileSetup::new(adapter);
        let db_config = profile_setup.setup(existing_config)?;

        let mut outputs = HashMap::new();
        outputs.insert("dev".to_string(), db_config);

        Ok(ProfileTarget {
            target: "dev".to_string(),
            outputs,
        })
    }

    /// Write or update a single profile block in the appropriate profiles.yml,
    /// preserving existing content, order, and comments.
    pub fn write_profile(
        &self,
        profile_name: &str,
        profile: &impl serde::Serialize,
    ) -> FsResult<()> {
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

        let mut top = BTreeMap::new();
        top.insert(profile_name.to_string(), profile);
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
        if adapter != AdapterType::Snowflake || !dbt_flags::LAKECOMPUTE_PROFILE_SETUP.enabled() {
            return self.write_profile(profile_name, &profile);
        }
        let mut existing = fs::read_to_string(self.active_profiles_path())
            .ok()
            .and_then(|text| dbt_yaml::from_str::<serde_json::Value>(&text).ok())
            .map(|parsed| target_outputs(&parsed, profile_name, &profile.target))
            .unwrap_or_default();
        if !ask_for_lakecompute(existing.iter().any(is_lakecompute))? {
            // The target's other outputs stay; a lakecompute one goes.
            existing.retain(|o| !is_lakecompute(o));
            if existing.len() <= 1 {
                return self.write_profile(profile_name, &profile);
            }
            return self
                .write_profile(profile_name, &with_lakecompute(&profile, &existing, false)?);
        }
        self.write_profile(profile_name, &with_lakecompute(&profile, &existing, true)?)?;
        let next = format!(
            "{} Added a lakecompute output: models with adapter='lakecompute' run on Lake Compute. \
             It reads its Fivetran token from {}; `dbt debug` checks the connection.",
            GREEN.apply_to("Info"),
            dbt_profile_schemas::LAKE_COMPUTE_TOKEN_ENV,
        );
        emit_info_log_message(next);
        Ok(())
    }

    pub fn cloud_client() -> &'static DbtCloudClient {
        &DbtCloudClient
    }
}

#[cfg(test)]
mod tests {
    use super::adapter_type_for_profile;
    use dbt_profile_schemas::profile::ProfileTarget;
    use dbt_schemas::schemas::profiles::{DbConfig, SnowflakeDbConfig};

    fn parse(yaml: &str) -> serde_json::Value {
        dbt_yaml::from_str(yaml).unwrap()
    }

    /// A profile whose `dev` target is one Snowflake output.
    fn snowflake_profile(config: SnowflakeDbConfig) -> ProfileTarget {
        ProfileTarget {
            target: "dev".to_string(),
            outputs: [("dev".to_string(), DbConfig::Snowflake(Box::new(config)))].into(),
        }
    }

    #[test]
    fn a_lakecompute_output_goes_beside_the_warehouse_one() {
        use super::{is_lakecompute, target_outputs, with_lakecompute};
        let profile = snowflake_profile(SnowflakeDbConfig {
            account: Some("acme".to_string()),
            ..Default::default()
        });
        let value = with_lakecompute(&profile, &[], true).unwrap();
        let text = dbt_yaml::to_string(
            &[("p", &value)]
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>(),
        )
        .unwrap();
        let parsed = parse(&text);
        let outputs = parsed["p"]["outputs"]["dev"]
            .as_array()
            .expect("a list of outputs");
        assert_eq!(outputs.len(), 2, "{text}");
        assert_eq!(
            (
                outputs[0]["name"].as_str(),
                outputs[0]["default"].as_bool(),
                outputs[0]["type"].as_str()
            ),
            (Some("snowflake"), Some(true), Some("snowflake")),
            "{text}"
        );
        assert_eq!(outputs[0]["account"], "acme");
        assert_eq!(outputs[1]["type"], "lakecompute");
        assert_eq!(outputs[1]["method"], "fivetran");
        assert_eq!(
            outputs[1]["fivetran_credential"],
            "{{ env_var('DBT_LAKE_COMPUTE_TOKEN', '') }}"
        );
        assert!(
            target_outputs(&parsed, "p", "dev")
                .iter()
                .any(is_lakecompute)
        );
        let single = parse("p:\n  target: dev\n  outputs:\n    dev:\n      type: snowflake\n");
        assert!(
            !target_outputs(&single, "p", "dev")
                .iter()
                .any(is_lakecompute)
        );
    }

    #[test]
    fn setting_it_up_again_keeps_the_lakecompute_output_and_the_others() {
        use super::{is_lakecompute, target_outputs, with_lakecompute};
        let before = parse(
            "p:\n  target: dev\n  outputs:\n    dev:\n      - name: wh\n        default: true\n        type: snowflake\n      - name: lc\n        type: lakecompute\n        method: fivetran\n        fivetran_credential: \"{{ env_var('MY_TOKEN') }}\"\n      - name: other\n        type: snowflake\n        account: b\n",
        );
        let existing = target_outputs(&before, "p", "dev");
        assert!(existing.iter().any(is_lakecompute));
        let profile = snowflake_profile(SnowflakeDbConfig::default());
        let value = with_lakecompute(&profile, &existing, true).unwrap();
        let text = dbt_yaml::to_string(&value).unwrap();
        let after = parse(&text);
        let outputs = after["outputs"]["dev"].as_array().unwrap();
        let names: Vec<&str> = outputs.iter().filter_map(|o| o["name"].as_str()).collect();
        assert_eq!(names, ["wh", "lc", "other"], "{text}");
        assert_eq!(
            outputs[1]["fivetran_credential"],
            "{{ env_var('MY_TOKEN') }}"
        );
        // Declined: the others stay, without the lakecompute one.
        let others: Vec<_> = existing
            .into_iter()
            .filter(|o| !is_lakecompute(o))
            .collect();
        let value = with_lakecompute(&profile, &others, false).unwrap();
        let after = parse(&dbt_yaml::to_string(&value).unwrap());
        let names: Vec<&str> = after["outputs"]["dev"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|o| o["name"].as_str())
            .collect();
        assert_eq!(names, ["wh", "other"]);
    }

    #[test]
    fn the_first_output_is_the_default_without_a_flag() {
        use super::{target_outputs, with_lakecompute};
        let profile = snowflake_profile(SnowflakeDbConfig::default());
        let written = |before: &str| {
            let existing = target_outputs(&parse(before), "p", "dev");
            let text =
                dbt_yaml::to_string(&with_lakecompute(&profile, &existing, true).unwrap()).unwrap();
            let after = parse(&text);
            let outputs = after["outputs"]["dev"].as_array().unwrap().clone();
            outputs
                .iter()
                .map(|o| {
                    (
                        o["name"].as_str().unwrap_or_default().to_string(),
                        o["type"].as_str().unwrap_or_default().to_string(),
                        o["default"].as_bool() == Some(true),
                    )
                })
                .collect::<Vec<_>>()
        };
        let s = |n: &str, t: &str, d: bool| (n.to_string(), t.to_string(), d);
        // Unflagged: the first is the default, replaced rather than kept beside.
        assert_eq!(
            written(
                "p:\n  target: dev\n  outputs:\n    dev:\n      - name: wh\n        type: snowflake\n      - name: lc\n        type: lakecompute\n"
            ),
            [s("wh", "snowflake", true), s("lc", "lakecompute", false)]
        );
        // A lakecompute default is kept, no longer the default.
        assert_eq!(
            written(
                "p:\n  target: dev\n  outputs:\n    dev:\n      - name: lc\n        type: lakecompute\n        default: true\n      - name: wh\n        type: snowflake\n"
            ),
            [s("wh", "snowflake", true), s("lc", "lakecompute", false)]
        );
    }

    #[test]
    fn dbt_loads_the_profile_it_writes_with_lakecompute() {
        use super::{load_profile_with_loader, with_lakecompute};
        let profile = snowflake_profile(SnowflakeDbConfig {
            account: Some("acme".to_string()),
            user: Some("me".to_string()),
            password: Some("{{ env_var('NO_SUCH_PASSWORD', 'x') }}".to_string()),
            database: Some("analytics".to_string()),
            schema: Some("dev".to_string()),
            warehouse: Some("transforming".to_string()),
            role: Some("transformer".to_string()),
            ..Default::default()
        });
        let value = with_lakecompute(&profile, &[], true).unwrap();
        let dir = std::env::temp_dir().join(format!("dbt-init-lakecompute-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let block: std::collections::BTreeMap<&str, &dbt_yaml::Value> = [("p", &value)].into();
        std::fs::write(
            dir.join("profiles.yml"),
            dbt_yaml::to_string(&block).unwrap(),
        )
        .unwrap();
        let loaded = load_profile_with_loader(Some(&dir), "p", None);
        let _ = std::fs::remove_dir_all(&dir);
        // Snowflake stays the target's default output.
        assert!(matches!(loaded, Ok(DbConfig::Snowflake(_))), "{loaded:?}");
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
    fn a_list_target_has_no_type_without_the_flag() {
        // As before the Lake Compute setup (`lakecompute_profile_setup` is off).
        let parsed = parse(
            "p:\n  target: dev\n  outputs:\n    dev:\n      - name: lc\n        type: lakecompute\n      - name: wh\n        default: true\n        type: snowflake\n",
        );
        assert_eq!(adapter_type_for_profile(&parsed, "p"), None);
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
