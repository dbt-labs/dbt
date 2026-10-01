// Allow disallowed methods for this module because RustEmbed generates calls to Path::canonicalize
#![allow(clippy::disallowed_methods)]

use crate::profile_setup::{ProfileAction, ProfileSetup};
use dbt_common::pretty_string::{GREEN, YELLOW};
use dbt_common::tracing::dbt_emit::emit_info_log_message;
use dbt_common::{ErrorCode, FsResult, fs_err};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub mod assets {
    #![allow(clippy::disallowed_methods)] // RustEmbed generates calls to std::path::Path::canonicalize

    use rust_embed::{EmbeddedFile, RustEmbed};

    #[derive(RustEmbed)]
    #[folder = "assets/jaffle_shop/"]
    struct JaffleShopProjectTemplate;

    #[derive(RustEmbed)]
    #[folder = "assets/moms_flower_shop/"]
    struct MomsFlowerShopProjectTemplate;

    pub enum ProjectTemplateAsset {
        JaffleShop,
        MomsFlowerShop,
    }

    impl ProjectTemplateAsset {
        /// Return the name of the project in assets/{project_name}.
        pub fn default_project_name(&self) -> &'static str {
            match self {
                ProjectTemplateAsset::JaffleShop => "jaffle_shop",
                ProjectTemplateAsset::MomsFlowerShop => "moms_flower_shop",
            }
        }

        pub fn get(&self, file_path: &str) -> Option<EmbeddedFile> {
            match self {
                ProjectTemplateAsset::JaffleShop => JaffleShopProjectTemplate::get(file_path),
                ProjectTemplateAsset::MomsFlowerShop => {
                    MomsFlowerShopProjectTemplate::get(file_path)
                }
            }
        }

        pub fn iter(&self) -> Box<dyn Iterator<Item = ::std::borrow::Cow<'static, str>>> {
            match self {
                ProjectTemplateAsset::JaffleShop => Box::new(JaffleShopProjectTemplate::iter()),
                ProjectTemplateAsset::MomsFlowerShop => {
                    Box::new(MomsFlowerShopProjectTemplate::iter())
                }
            }
        }
    }
}

/// Create or update .vscode/extensions.json file with dbt extension recommendation
fn create_or_update_vscode_extensions(target_dir: &Path) -> FsResult<()> {
    let vscode_dir = target_dir.join(".vscode");
    let extensions_file = vscode_dir.join("extensions.json");

    // Create .vscode directory if it doesn't exist
    fs::create_dir_all(&vscode_dir)?;

    let dbt_extension = "dbtLabsInc.dbt";

    if extensions_file.exists() {
        // File exists, read and check if our extension is already there
        let content = fs::read_to_string(&extensions_file)?;

        // Parse the JSON to check if our extension is already present
        let mut json: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to parse extensions.json: {}", e))?;

        // Ensure we have a recommendations array
        if !json.is_object() {
            json = serde_json::json!({});
        }

        let mut recommendations = json
            .get("recommendations")
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_else(Vec::new);

        // Check if our extension is already in the list
        let already_exists = recommendations
            .iter()
            .any(|item| item.as_str() == Some(dbt_extension));

        if !already_exists {
            recommendations.push(serde_json::Value::String(dbt_extension.to_string()));
            json["recommendations"] = serde_json::Value::Array(recommendations);

            // Write back the updated content with pretty formatting
            let updated_content = serde_json::to_string_pretty(&json).map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "Failed to serialize extensions.json: {}",
                    e
                )
            })?;
            fs::write(&extensions_file, updated_content)?;

            emit_info_log_message(format!(
                "{} Added dbt extension recommendation to existing .vscode/extensions.json",
                GREEN.apply_to("Info")
            ));
        } else {
            emit_info_log_message(format!(
                "{} dbt extension already recommended in .vscode/extensions.json, skipping",
                YELLOW.apply_to("Info")
            ));
        }
    } else {
        // File doesn't exist, create it with our extension
        let extensions_json = serde_json::json!({
            "recommendations": [
                dbt_extension
            ]
        });
        let extensions_content = serde_json::to_string_pretty(&extensions_json).map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to serialize extensions.json: {}",
                e
            )
        })?;
        fs::write(&extensions_file, extensions_content)?;

        emit_info_log_message(format!(
            "{} Created .vscode/extensions.json with dbt extension recommendation",
            GREEN.apply_to("Info")
        ));
    }

    Ok(())
}

pub fn init_project(
    project_name: &str,
    profile_name: &str,
    target_dir: &Path,
    project_template: &assets::ProjectTemplateAsset,
) -> FsResult<()> {
    fs::create_dir_all(target_dir)?;

    // Extract all embedded files
    for file_path in project_template.iter() {
        let file_content = project_template.get(&file_path).ok_or_else(|| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to read embedded file: {}",
                file_path
            )
        })?;

        let target_file_path = target_dir.join(file_path.as_ref());

        // Create parent directories if they don't exist
        if let Some(parent) = target_file_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Replace template placeholders
        let content = String::from_utf8_lossy(&file_content.data);
        let content = content.replace(project_template.default_project_name(), project_name);
        let content = content.replace("__PROFILE_NAME__", profile_name);

        // Write the file
        fs::write(&target_file_path, content)?;
    }

    Ok(())
}

pub fn get_profiles_dir() -> PathBuf {
    // Try environment variable first, then fall back to default
    env::var("DBT_PROFILES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| env::home_dir().unwrap_or_else(|| ".".into()).join(".dbt"))
}

/// Check if we're currently in a dbt project directory
pub fn is_in_dbt_project() -> bool {
    Path::new("dbt_project.yml").exists()
}

/// Get the profile name from dbt_project.yml if we're in a project
pub fn get_profile_name_from_project() -> FsResult<String> {
    let content = fs::read_to_string("dbt_project.yml")?;
    let project: serde_json::Value = dbt_yaml::from_str(&content)
        .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to parse dbt_project.yml: {}", e))?;

    if let Some(profile_value) = project.get("profile")
        && let Some(profile_str) = profile_value.as_str()
    {
        return Ok(profile_str.to_string());
    }

    // No profile found in dbt_project.yml, ask user to provide one
    use dialoguer::Input;

    let default_profile = project
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("my_profile");

    let profile_name: String = Input::new()
        .with_prompt(
            "No profile found in dbt_project.yml. What profile name would you like to use?",
        )
        .default(default_profile.to_string())
        .interact_text()
        .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to get profile name: {}", e))?;

    // Now update the dbt_project.yml file to include the profile
    update_dbt_project_profile(&profile_name)?;

    Ok(profile_name)
}

/// Point dbt_project.yml at `profile_name`.
///
/// If a top-level `profile:` field already exists (the sample templates ship one), it is
/// replaced in place so we never end up with two `profile:` entries. Otherwise a new field is
/// inserted right after `name:`, or before the first other top-level field, or appended.
fn update_dbt_project_profile(profile_name: &str) -> FsResult<()> {
    let content = fs::read_to_string("dbt_project.yml")?;
    let updated_content = set_profile_in_project_yaml(&content, profile_name);

    if !updated_content.ends_with('\n') {
        fs::write("dbt_project.yml", updated_content + "\n")?;
    } else {
        fs::write("dbt_project.yml", updated_content)?;
    }

    emit_info_log_message(format!(
        "{} Set profile '{}' in dbt_project.yml",
        GREEN.apply_to("Success"),
        profile_name
    ));

    Ok(())
}

/// Return `content` with its top-level `profile:` field pointed at `profile_name`.
///
/// If a top-level `profile:` field already exists (the sample templates ship one), it is
/// replaced in place so we never end up with two `profile:` entries. Otherwise a new field is
/// inserted before the first other top-level field, or appended when there is none.
fn set_profile_in_project_yaml(content: &str, profile_name: &str) -> String {
    // A zero-indent, non-blank, non-comment `key: ...` line.
    let is_top_level_field = |line: &str| {
        !line.starts_with(' ')
            && !line.starts_with('\t')
            && !line.trim().is_empty()
            && !line.trim_start().starts_with('#')
            && line.contains(':')
    };

    let lines: Vec<&str> = content.lines().collect();
    let mut new_lines: Vec<String> = Vec::with_capacity(lines.len() + 1);
    let mut profile_set = false;

    for line in lines.iter() {
        // Replace an existing top-level `profile:` field in place.
        if !profile_set && is_top_level_field(line) && line.trim_start().starts_with("profile:") {
            new_lines.push(format!("profile: {profile_name}"));
            profile_set = true;
            continue;
        }

        // Otherwise insert before the first top-level field that is not `name:`.
        if !profile_set && is_top_level_field(line) && !line.trim_start().starts_with("name:") {
            new_lines.push(format!("profile: {profile_name}"));
            profile_set = true;
        }

        new_lines.push((*line).to_string());
    }

    // No suitable anchor found (e.g. only a `name:` field): append at the end.
    if !profile_set {
        new_lines.push(format!("profile: {profile_name}"));
    }

    new_lines.join("\n")
}

/// Check if a profile exists in profiles.yml
pub fn check_if_profile_exists(profile_name: &str, profiles_dir: &Path) -> FsResult<bool> {
    let profiles_file = profiles_dir.join("profiles.yml");
    if !profiles_file.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(profiles_file)?;
    let profiles: serde_json::Value = dbt_yaml::from_str(&content)
        .map_err(|e| fs_err!(ErrorCode::IoError, "Failed to parse profiles.yml: {}", e))?;

    Ok(profiles.get(profile_name).is_some())
}

/// Main init workflow that handles both project creation and profile setup
pub async fn run_init_workflow(
    project_name: Option<String>,
    skip_profile_setup: bool,
    existing_profile: Option<String>,
    project_template: &assets::ProjectTemplateAsset,
) -> FsResult<()> {
    let profiles_dir = get_profiles_dir();
    let profile_setup = ProfileSetup::new(profiles_dir.clone());

    let inside_existing_project = is_in_dbt_project();

    // Determine whether the user explicitly provided a project name.
    let (mut project_name, user_specified_project_name) = match project_name {
        Some(name) => (name, true),
        None => (project_template.default_project_name().to_string(), false),
    };

    // CASE 1: Inside an existing project **and** the user did NOT provide a new project name →
    // behave like dbt-core: only set up (or update) a profile.
    if inside_existing_project && !user_specified_project_name {
        if existing_profile.is_some() {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "Cannot init existing project with specified profile, edit dbt_project.yml instead"
            ));
        }

        emit_info_log_message(format!(
            "{} A dbt_project.yml already exists in this directory; skipping sample project creation.",
            YELLOW.apply_to("Warning")
        ));

        // Create or update .vscode/extensions.json even when skipping project creation
        create_or_update_vscode_extensions(Path::new("."))?;

        if !skip_profile_setup {
            let profile_name = get_profile_name_from_project()?;
            match profile_setup.prompt_profile_action()? {
                ProfileAction::CreateNew => {
                    profile_setup.setup_profile(&profile_name, false).await?;
                }
                ProfileAction::CreateFromCloud => {
                    profile_setup.setup_profile(&profile_name, true).await?;
                }
                ProfileAction::UseExisting(chosen_name) => {
                    emit_info_log_message(format!(
                        "{} Using existing profile '{chosen_name}'",
                        GREEN.apply_to("Info")
                    ));
                    update_dbt_project_profile(&chosen_name)?;
                }
                ProfileAction::Skip => {
                    emit_info_log_message(format!(
                        "{} Skipping profile setup.",
                        GREEN.apply_to("Info")
                    ));
                }
            }
        }

        return Ok(());
    }

    // CASE 2: Either we're not in a project, **or** the user asked for a new project explicitly –
    // proceed to create the sample project directory.

    {
        // If the chosen project directory already exists, find the next available
        if Path::new(&project_name).exists() {
            let unique_name = next_available_dir_name(&project_name);
            emit_info_log_message(format!(
                "{} Directory '{}' already exists, using '{}' instead",
                YELLOW.apply_to("Warning"),
                project_name,
                YELLOW.apply_to(&unique_name)
            ));
            project_name = unique_name;
        }

        // Validate profile if specified
        if let Some(ref profile_name) = existing_profile
            && !check_if_profile_exists(profile_name, &profiles_dir)?
        {
            return Err(fs_err!(
                ErrorCode::InvalidArgument,
                "Could not find profile named '{}'",
                profile_name
            ));
        }

        // Create the project
        let project_dir = Path::new(&project_name);
        let profile_name = existing_profile
            .clone()
            .unwrap_or_else(|| project_name.clone());
        init_project(&project_name, &profile_name, project_dir, project_template)?;

        // Create or update .vscode/extensions.json in the new project
        create_or_update_vscode_extensions(project_dir)?;

        // Change to project directory
        env::set_current_dir(&project_name)?;

        emit_info_log_message(format!(
            "{} Project created successfully!",
            GREEN.apply_to("Success")
        ));
        emit_info_log_message(format!(
            "{} Project name: {}",
            GREEN.apply_to("Info"),
            project_name
        ));
        emit_info_log_message(format!(
            "{} Project directory: {}",
            GREEN.apply_to("Info"),
            project_dir.display()
        ));

        // Setup profile if not skipped
        if !skip_profile_setup {
            match existing_profile {
                // --profile=<name> was explicitly supplied on the CLI: already validated above,
                // just point dbt_project.yml at it.
                Some(ref profile_name) => {
                    update_dbt_project_profile(profile_name)?;
                }
                // No explicit profile supplied — ask the user what they'd like to do.
                None => match profile_setup.prompt_profile_action()? {
                    ProfileAction::CreateNew => {
                        profile_setup.setup_profile(&project_name, false).await?;
                    }
                    ProfileAction::CreateFromCloud => {
                        profile_setup.setup_profile(&project_name, true).await?;
                    }
                    ProfileAction::UseExisting(chosen_name) => {
                        emit_info_log_message(format!(
                            "{} Using existing profile '{chosen_name}'",
                            GREEN.apply_to("Info")
                        ));
                        update_dbt_project_profile(&chosen_name)?;
                    }
                    ProfileAction::Skip => {
                        emit_info_log_message(format!(
                            "{} Skipping profile setup.",
                            GREEN.apply_to("Info")
                        ));
                    }
                },
            }
        }
    }

    Ok(())
}

/// Given a base directory name, return the first `{base}_{n}` (n starting at 1) that does not
/// already exist. If none of the suffixed names exist it returns the base name itself.
fn next_available_dir_name(base: &str) -> String {
    let mut counter = 1;
    loop {
        let candidate = format!("{base}_{counter}");
        if !Path::new(&candidate).exists() {
            return candidate;
        }
        counter += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::set_profile_in_project_yaml;

    #[test]
    fn replaces_existing_profile_without_duplicating() {
        // Mirrors the sample templates, which ship a `profile:` line substituted with the
        // project name. Choosing an existing profile must replace it, not add a second one.
        let content = "name: jaffle_shop_4\nprofile: jaffle_shop_4\n\nseed-paths: [\"seeds\"]\n";
        let updated = set_profile_in_project_yaml(content, "snowflake_sandbox");

        assert_eq!(updated.matches("profile:").count(), 1);
        assert!(updated.contains("profile: snowflake_sandbox"));
        assert!(!updated.contains("profile: jaffle_shop_4"));
        // Other fields are left untouched.
        assert!(updated.contains("name: jaffle_shop_4"));
        assert!(updated.contains("seed-paths: [\"seeds\"]"));
    }

    #[test]
    fn inserts_profile_when_absent() {
        let content = "name: my_project\n\nseed-paths: [\"seeds\"]\n";
        let updated = set_profile_in_project_yaml(content, "my_profile");

        assert_eq!(updated.matches("profile:").count(), 1);
        assert!(updated.contains("profile: my_profile"));
        // Inserted before the first non-name top-level field, after `name:`.
        let name_idx = updated.find("name: my_project").unwrap();
        let profile_idx = updated.find("profile: my_profile").unwrap();
        let seed_idx = updated.find("seed-paths").unwrap();
        assert!(name_idx < profile_idx && profile_idx < seed_idx);
    }

    #[test]
    fn appends_profile_when_only_name_present() {
        let content = "name: my_project\n";
        let updated = set_profile_in_project_yaml(content, "my_profile");

        assert_eq!(updated.matches("profile:").count(), 1);
        assert!(updated.contains("name: my_project"));
        assert!(updated.contains("profile: my_profile"));
    }

    #[test]
    fn ignores_indented_profile_key() {
        // A nested `profile:` (e.g. under some other mapping) must not be treated as the
        // top-level project profile; a real top-level field is inserted instead.
        let content = "name: my_project\nmodels:\n  profile: not_this\n";
        let updated = set_profile_in_project_yaml(content, "my_profile");

        assert!(updated.contains("profile: my_profile"));
        assert!(updated.contains("  profile: not_this"));
        assert_eq!(updated.matches("profile: my_profile").count(), 1);
    }
}
