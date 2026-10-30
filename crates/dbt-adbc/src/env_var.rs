use adbc_core::error::{Error, Result, Status};

use std::env;
use std::ffi::OsStr;

const TRUE_VALUES: [&str; 4] = ["1", "true", "yes", "on"];
const FALSE_VALUES: [&str; 5] = ["0", "false", "no", "off", ""];

/// [`env_var_bool`] for opt-in flags: unset or unparseable is `false`, with a
/// warning. For flags read deep in the loader, where a typo shouldn't abort a
/// run.
pub fn env_var_bool_or_warn(var_name: &str) -> bool {
    match env_var_bool(var_name) {
        Ok(enabled) => enabled.unwrap_or(false),
        Err(e) => {
            tracing::warn!("Ignoring {var_name}: {e}");
            false
        }
    }
}

pub fn env_var_bool(var_name: &str) -> Result<Option<bool>> {
    env_var_bool_inner(var_name, env::var_os(var_name).as_deref())
}

fn env_var_bool_inner(var_name: &str, val: Option<&OsStr>) -> Result<Option<bool>> {
    match val {
        Some(val) => {
            if TRUE_VALUES.iter().any(|s| val.eq_ignore_ascii_case(s)) {
                Ok(Some(true))
            } else if FALSE_VALUES.iter().any(|s| val.eq_ignore_ascii_case(s)) {
                Ok(Some(false))
            } else {
                let err = Error::with_message_and_status(
                    format!(
                        "Invalid value for environment variable {var_name:?}: {:?}. Expected one of: {} (true) or {} (false).",
                        val,
                        TRUE_VALUES.join(", "),
                        FALSE_VALUES.join(", ")
                    ),
                    Status::InvalidArguments,
                );
                Err(err)
            }
        }
        None => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adbc_core::error::Status;

    #[test]
    fn env_var_bool_accepts_missing_true_and_false_values() {
        let name = "DBT_ADBC_TEST_BOOL_VALUES";

        assert!(!env_var_bool_inner(name, None).unwrap().unwrap_or(false));

        for value in ["1", "TRUE", "yes", "On"] {
            assert!(
                env_var_bool_inner(name, Some(OsStr::new(value)))
                    .unwrap()
                    .unwrap_or(false),
                "{value:?} should parse as true"
            );
        }

        for value in ["0", "FALSE", "no", "Off", ""] {
            assert!(
                !env_var_bool_inner(name, Some(OsStr::new(value)))
                    .unwrap()
                    .unwrap_or(false),
                "{value:?} should parse as false"
            );
        }
    }

    #[test]
    fn env_var_bool_rejects_unknown_values() {
        let name = "DBT_ADBC_TEST_BOOL_INVALID";

        let error = env_var_bool_inner(name, Some(OsStr::new("maybe"))).unwrap_err();

        assert_eq!(error.status, Status::InvalidArguments);
        assert!(error.message.contains(name));
        assert!(error.message.contains("maybe"));
        assert!(error.message.contains("1, true, yes, on"));
        assert!(error.message.contains("0, false, no, off, "));
    }
}
