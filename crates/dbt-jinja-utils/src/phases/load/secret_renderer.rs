//! This module contains the logic for rendering secrets in the dbt project.

use std::{borrow::Cow, sync::LazyLock};

use dbt_common::{ErrorCode, FsResult, fs_err};
use minijinja::State;
use regex::Regex;

use crate::env_var;
use crate::functions::SECRET_PLACEHOLDER;
use crate::utils::SECRET_ENV_VAR_PREFIX;

/// Prefix which identifies environment variables which contains secrets.
/// A function that returns an environment variable from the environment, with special handling for secrets
pub fn secret_context_env_var(
    state: &State,
    args: &[minijinja::Value],
) -> Result<minijinja::Value, minijinja::Error> {
    let placeholder_on_secret_access = true;
    env_var(placeholder_on_secret_access, None, state, args)
}

/// Renders actual secrets that have been rendered with placeholders
///
/// Returns `None` if no placeholder was replaced, so the string stays as it is.
pub fn render_secrets(rendered_str: &str) -> FsResult<Option<String>> {
    if rendered_str.contains(SECRET_ENV_VAR_PREFIX) {
        // Create a regex that matches the entire placeholder pattern
        static RE: LazyLock<Regex> = LazyLock::new(|| {
            let pattern = SECRET_PLACEHOLDER
                .replace("{}", &format!("({SECRET_ENV_VAR_PREFIX}(.*))"))
                .replace("$", r"\$");
            Regex::new(&pattern).unwrap()
        });
        // Copy the string only once a placeholder is replaced.
        let mut result = Cow::Borrowed(rendered_str);
        // Find all matches
        for caps in RE.captures_iter(rendered_str) {
            let var_name = &caps[1]; // This captures the full env var name
            let full_match = &caps[0]; // This is the entire placeholder

            // Check if the secret exists
            match std::env::var(var_name) {
                Ok(value) => {
                    // Replace the entire placeholder with the value
                    result = Cow::Owned(result.replace(full_match, &value));
                }
                Err(_) => {
                    return Err(fs_err!(
                        ErrorCode::InvalidConfig,
                        "Environment variable '{}' not found",
                        var_name
                    ));
                }
            }
        }

        if let Cow::Owned(result) = result {
            return Ok(Some(result));
        }
    }
    Ok(None)
}
