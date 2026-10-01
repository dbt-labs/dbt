use adbc_core::error::{Error, Result, Status};

use std::env;

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
    match env::var_os(var_name) {
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn set_test_var(name: &str, value: &str) {
        #[allow(
            clippy::disallowed_methods,
            reason = "env-var parser tests serialize mutations with ENV_LOCK"
        )]
        unsafe {
            env::set_var(name, value);
        }
    }

    fn remove_test_var(name: &str) {
        unsafe {
            env::remove_var(name);
        }
    }

    #[test]
    fn env_var_bool_accepts_missing_true_and_false_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let name = "DBT_ADBC_TEST_BOOL_VALUES";

        remove_test_var(name);
        assert!(!env_var_bool(name).unwrap().unwrap_or(false));

        for value in ["1", "TRUE", "yes", "On"] {
            set_test_var(name, value);
            assert!(
                env_var_bool(name).unwrap().unwrap_or(false),
                "{value:?} should parse as true"
            );
        }

        for value in ["0", "FALSE", "no", "Off", ""] {
            set_test_var(name, value);
            assert!(
                !env_var_bool(name).unwrap().unwrap_or(false),
                "{value:?} should parse as false"
            );
        }

        remove_test_var(name);
    }

    #[test]
    fn env_var_bool_rejects_unknown_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let name = "DBT_ADBC_TEST_BOOL_INVALID";

        set_test_var(name, "maybe");
        let error = env_var_bool(name).unwrap_err();
        remove_test_var(name);

        assert_eq!(error.status, Status::InvalidArguments);
        assert!(error.message.contains(name));
        assert!(error.message.contains("maybe"));
        assert!(error.message.contains("1, true, yes, on"));
        assert!(error.message.contains("0, false, no, off, "));
    }
}
