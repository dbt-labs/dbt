use std::path::Path;

use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_platform_auth::Credential;
use dbt_schemas::schemas::UserSettings;

const MANAGE_STATE_ENV: &str = "DBT_ENGINE_MANAGE_STATE";

// ── State config detection ────────────────────────────────────────────────────

/// Where `manage_state: true` was found in the local environment.
#[derive(Debug, Clone, Copy)]
pub enum StateConfigSource {
    EnvVar,
    DbtProjectYml,
    UserSettingsYml,
}

impl StateConfigSource {
    fn inline_description(self) -> &'static str {
        match self {
            Self::EnvVar => "set via `DBT_ENGINE_MANAGE_STATE`",
            Self::DbtProjectYml => "set in `./dbt_project.yml`",
            Self::UserSettingsYml => "set in `~/.dbt/user_settings.yml`",
        }
    }
}

/// Checks whether dbt State is locally configured via env var or YAML files.
/// Returns the first matching source (env > project > user settings), or `None`.
pub fn check_state_configured() -> Option<StateConfigSource> {
    if std::env::var(MANAGE_STATE_ENV)
        .map(|v| v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        return Some(StateConfigSource::EnvVar);
    }

    if UserSettings::load_from(Path::new("dbt_project.yml"))
        .map(|s| s.manage_state())
        .unwrap_or(false)
    {
        return Some(StateConfigSource::DbtProjectYml);
    }

    if UserSettings::load().manage_state() {
        return Some(StateConfigSource::UserSettingsYml);
    }

    None
}

/// Hits the dbt platform features endpoint and returns whether `dbt-state` is enabled.
/// Any failure (network, parse, auth) returns `false`.
pub async fn is_state_feature_enabled(cred: &Credential, http: &reqwest::Client) -> bool {
    match fetch_state_feature_flag(cred, http).await {
        Ok(enabled) => enabled,
        Err(e) => {
            tracing::debug!("dbt-state feature check failed: {e}");
            false
        }
    }
}

async fn fetch_state_feature_flag(
    cred: &Credential,
    http: &reqwest::Client,
) -> Result<bool, String> {
    let url = format!(
        "https://{}/api/private/accounts/{}/features/",
        cred.account_host(),
        cred.account_id()
    );
    let resp = http
        .get(&url)
        .header("Authorization", format!("Bearer {}", cred.token()))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("status {}", resp.status()));
    }

    let json: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(json
        .get("data")
        .and_then(|d| d.get("dbt-state"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

/// Runs the post-login state guidance flow based on the resolved credential.
pub async fn run_state_guidance(cred: &Credential, http: &reqwest::Client) -> FsResult<()> {
    let state_enabled = is_state_feature_enabled(cred, http).await;
    let state_configured = check_state_configured();

    match (state_enabled, state_configured) {
        (true, None) => prompt_to_set_state().await?,
        (false, Some(source)) => print_info_enable_state(source),
        _ => {}
    }

    Ok(())
}

/// Runs post-login state guidance when dbt State auth has just succeeded (so
/// state is known to be enabled — no platform credential needed to verify it).
/// Creates or updates `~/.dbt/user_settings.yml` with `manage_state: true`
/// unless the user has already explicitly set it to `false`.
pub fn run_state_guidance_after_state_login() -> FsResult<()> {
    if !UserSettings::load().manage_state_is_set() {
        write_manage_state_to_user_settings()?;
    }
    Ok(())
}

// ── User-facing messages and prompt ──────────────────────────────────────────

async fn prompt_to_set_state() -> FsResult<()> {
    let prompt = format!(
        "Looks like dbt State is enabled for your dbt platform account. Enable state on this \
        machine by default, for faster and cheaper builds? You can always change this \
        configuration in {}",
        console::style("~/.dbt/user_settings.yml").bold()
    );

    let confirmed = tokio::task::spawn_blocking(move || {
        dialoguer::Confirm::new()
            .with_prompt(prompt)
            .default(true)
            .interact()
    })
    .await
    .map_err(|e| fs_err!(ErrorCode::Unknown, "prompt task panicked: {e}"))?
    .map_err(|e| fs_err!(ErrorCode::Unknown, "prompt failed: {e}"))?;

    if confirmed {
        write_manage_state_to_user_settings()?;
    } else {
        println!(
            "If you want to enable dbt State in the future, you can add the following to \
            ~/.dbt/user_settings.yml\n  flags:\n    manage_state: true\n\
            Or set the environment variable: DBT_ENGINE_MANAGE_STATE=true"
        );
    }

    Ok(())
}

fn print_info_enable_state(source: StateConfigSource) {
    let desc = source.inline_description();
    println!(
        "Looks like dbt State is enabled on this machine ({desc}) but not in your dbt platform \
        account. To enable State in your platform account, see docs: \
        {}.",
        console::style("https://docs.getdbt.com/docs/deploy/dbt-state-setup").bold()
    );
}

// ── YAML write helper ─────────────────────────────────────────────────────────

fn write_manage_state_to_user_settings() -> FsResult<()> {
    let path = UserSettings::path().ok_or_else(|| {
        fs_err!(
            ErrorCode::Unknown,
            "cannot determine home directory to write ~/.dbt/user_settings.yml"
        )
    })?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| fs_err!(ErrorCode::Unknown, "failed to create ~/.dbt: {e}"))?;
    }

    let no_span = || dbt_yaml::Span::default();

    // Read and merge with existing content if present, otherwise start fresh.
    let mut root: dbt_yaml::Value = if path.exists() {
        let content = std::fs::read_to_string(&path)
            .map_err(|e| fs_err!(ErrorCode::Unknown, "failed to read user_settings.yml: {e}"))?;
        dbt_yaml::from_str(&content)
            .unwrap_or_else(|_| dbt_yaml::Value::Mapping(dbt_yaml::Mapping::new(), no_span()))
    } else {
        dbt_yaml::Value::Mapping(dbt_yaml::Mapping::new(), no_span())
    };

    // Ensure root is a mapping.
    if !root.is_mapping() {
        root = dbt_yaml::Value::Mapping(dbt_yaml::Mapping::new(), no_span());
    }

    // Set root.flags.manage_state = true, creating the flags block if absent.
    let flags_key = dbt_yaml::Value::String("flags".to_string(), no_span());
    let manage_state_key = dbt_yaml::Value::String("manage_state".to_string(), no_span());

    let mapping = root.as_mapping_mut().unwrap();
    match mapping.get_mut(&flags_key) {
        Some(flags_val) if flags_val.is_mapping() => {
            flags_val
                .as_mapping_mut()
                .unwrap()
                .insert(manage_state_key, dbt_yaml::Value::Bool(true, no_span()));
        }
        _ => {
            let mut flags = dbt_yaml::Mapping::new();
            flags.insert(manage_state_key, dbt_yaml::Value::Bool(true, no_span()));
            mapping.insert(flags_key, dbt_yaml::Value::Mapping(flags, no_span()));
        }
    }

    let yaml = dbt_yaml::to_string(&root).map_err(|e| {
        fs_err!(
            ErrorCode::Unknown,
            "failed to serialize user_settings.yml: {e}"
        )
    })?;

    std::fs::write(&path, yaml)
        .map_err(|e| fs_err!(ErrorCode::Unknown, "failed to write user_settings.yml: {e}"))?;

    println!("dbt State enabled. Configuration written to ~/.dbt/user_settings.yml.");

    Ok(())
}
