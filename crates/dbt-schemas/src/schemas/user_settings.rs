use std::path::{Path, PathBuf};

use dbt_yaml::Value as YmlValue;
use serde::Deserialize;

#[derive(Deserialize, Default)]
pub struct UserSettings {
    #[serde(default)]
    pub flags: Option<YmlValue>,
}

impl UserSettings {
    /// Path to `~/.dbt/user_settings.yml`, or `None` if the home directory can't be determined.
    pub fn path() -> Option<PathBuf> {
        dirs::home_dir().map(|h| h.join(".dbt").join("user_settings.yml"))
    }

    /// Load from `~/.dbt/user_settings.yml`. Returns default (empty flags) on any error.
    pub fn load() -> Self {
        Self::path()
            .and_then(|p| Self::load_from(&p))
            .unwrap_or_default()
    }

    /// Load from an arbitrary path. Returns `None` if the file can't be read or parsed.
    pub fn load_from(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        dbt_yaml::from_str(&content).ok()
    }

    /// Returns `true` if `flags.manage_state` is explicitly `true`.
    pub fn manage_state(&self) -> bool {
        matches!(self.get_flag("manage_state"), Some(YmlValue::Bool(true, _)))
    }

    /// Returns `true` if `flags.manage_state` is present (regardless of value).
    pub fn manage_state_is_set(&self) -> bool {
        self.get_flag("manage_state").is_some()
    }

    fn get_flag(&self, name: &str) -> Option<&YmlValue> {
        let YmlValue::Mapping(ref map, _) = *self.flags.as_ref()? else {
            return None;
        };
        for (k, v) in map {
            if let YmlValue::String(key, _) = k {
                if key == name {
                    return Some(v);
                }
            }
        }
        None
    }
}
