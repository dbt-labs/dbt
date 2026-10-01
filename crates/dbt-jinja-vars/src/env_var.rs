use minijinja::arg_utils::ArgsIter;
use minijinja::{Error, ErrorKind, State, Value};

/// The prefix for environment variables that contain secrets
pub const SECRET_ENV_VAR_PREFIX: &str = "DBT_ENV_SECRET";

/// The prefix for environment variables that are reserved for dbt
pub const DBT_INTERNAL_ENV_VAR_PREFIX: &str = "_DBT";

/// The default placeholder for environment variables when the default value is used
pub const DEFAULT_ENV_PLACEHOLDER: &str = "__dbt_placeholder__";

/// The placeholder for secret environment variables
pub const SECRET_PLACEHOLDER: &str = "$$$DBT_SECRET_START$$${}$$$DBT_SECRET_END$$$";

/// Type alias for an optional env-var override lookup function.
pub type LookupFn = dyn Fn(&str) -> Option<Value>;

/// A function that returns an environment variable from the environment
///
/// `placeholder_on_secret_access` is used to control whether to return a placeholder
/// or produce a hard error when an attempt is made to access a secret environment variable.
///
/// `overrides_fn` is an optional function that can be checked before accessing the real
/// environment variable. Effectively, this allows for mocking or overriding environment
/// variables. Mock overrides are returned unchanged, without validation, secret
/// placeholders, or tracking. Use [`env_var_with_lookup`] for scoped environment
/// values that must obey the normal access rules.
///
/// `tracker` is an optional callback invoked on each successful env-var access (or default
/// fallback) so the caller can record the resolved name/value pair.
///
/// ```python
/// def env_var(self, var: str, default: Optional[str] = None) -> str
/// ```
/// https://github.com/dbt-labs/dbt-core/blob/303c63ccc836a357505f241dbe90c3abb7b73d57/core/dbt/context/base.py#L305
#[allow(clippy::type_complexity)]
pub fn env_var(
    placeholder_on_secret_access: bool,
    overrides_fn: Option<&LookupFn>,
    tracker: Option<&dyn Fn(&str, &str)>,
    _state: &State,
    args: &[Value],
) -> Result<Value, Error> {
    let iter = ArgsIter::new("env_var", &["var"], args);
    let var = iter.next_arg::<&str>()?;
    let default = iter.next_kwarg::<Option<&Value>>("default")?;

    if let Some(value) = overrides_fn.and_then(|lookup| lookup(var)) {
        return Ok(value);
    }

    resolve_env_var(placeholder_on_secret_access, None, tracker, var, default)
}

/// Resolve an environment variable using a scoped lookup before the process environment.
///
/// Scoped values obey the same reserved-name checks, secret-access rules, and
/// tracking behavior as process values. Allowed secrets return placeholders.
#[allow(clippy::type_complexity)]
pub fn env_var_with_lookup(
    placeholder_on_secret_access: bool,
    lookup: Option<&LookupFn>,
    tracker: Option<&dyn Fn(&str, &str)>,
    _state: &State,
    args: &[Value],
) -> Result<Value, Error> {
    let iter = ArgsIter::new("env_var", &["var"], args);
    let var = iter.next_arg::<&str>()?;
    let default = iter.next_kwarg::<Option<&Value>>("default")?;

    resolve_env_var(placeholder_on_secret_access, lookup, tracker, var, default)
}

#[allow(clippy::type_complexity)]
fn resolve_env_var(
    placeholder_on_secret_access: bool,
    lookup: Option<&LookupFn>,
    tracker: Option<&dyn Fn(&str, &str)>,
    var: &str,
    default: Option<&Value>,
) -> Result<Value, Error> {
    let is_secret = var.starts_with(SECRET_ENV_VAR_PREFIX);
    if is_secret && !placeholder_on_secret_access {
        let err = Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "Secret environment variables (starting with {SECRET_ENV_VAR_PREFIX}) \
                cannot be accessed here"
            ),
        );
        return Err(err);
    }
    let is_internal = var.starts_with(DBT_INTERNAL_ENV_VAR_PREFIX);
    if is_internal {
        let err = Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "Environment variables (starting with {DBT_INTERNAL_ENV_VAR_PREFIX}) \
                cannot be accessed here"
            ),
        );
        return Err(err);
    }

    let value = lookup
        .and_then(|lookup| lookup(var))
        .or_else(|| std::env::var(var).ok().map(Value::from));
    match (value, default) {
        (Some(value), _) => {
            if is_secret {
                debug_assert!(placeholder_on_secret_access);
                let value = Value::from(SECRET_PLACEHOLDER.replace("{}", var));
                Ok(value)
            } else {
                if let Some(tracker) = tracker {
                    tracker(var, &value.to_string());
                }
                Ok(value)
            }
        }
        (None, Some(default)) => {
            if let Some(tracker) = tracker {
                tracker(var, DEFAULT_ENV_PLACEHOLDER);
            }
            Ok(default.clone())
        }
        _ => {
            let err = Error::new(
                ErrorKind::InvalidOperation,
                format!("'env_var': environment variable '{var}' not found"),
            );
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_mock(name: &str) -> String {
        let mut env = minijinja::Environment::new();
        env.add_func_func("env_var", |state: &State, args: &[Value]| {
            let lookup = |_key: &str| Some(Value::from("fixture-value"));
            let tracker = |_name: &str, _value: &str| {
                panic!("mocked values must not be tracked as environment dependencies")
            };
            env_var(true, Some(&lookup), Some(&tracker), state, args)
        });
        env.render_str(&format!("{{{{ env_var('{name}') }}}}"), (), &[])
            .unwrap()
    }

    #[test]
    fn mock_overrides_return_exact_values_without_tracking() {
        assert_eq!(render_mock("UNIT_TEST_OVERRIDE"), "fixture-value");
    }

    #[test]
    fn secret_mock_overrides_return_exact_values_without_tracking() {
        assert_eq!(
            render_mock("DBT_ENV_SECRET_UNIT_TEST_OVERRIDE"),
            "fixture-value"
        );
    }

    fn render(name: &str, allow_secrets: bool) -> Result<String, Error> {
        let mut env = minijinja::Environment::new();
        env.add_func_func("env_var", move |state: &State, args: &[Value]| {
            let lookup = |_key: &str| Some(Value::from("overlay-value"));
            env_var_with_lookup(allow_secrets, Some(&lookup), None, state, args)
        });
        env.render_str(&format!("{{{{ env_var('{name}') }}}}"), (), &[])
    }

    #[test]
    fn scoped_lookup_preserves_reserved_and_secret_access_rules() {
        assert!(render("_DBT_PRIVATE", true).is_err());
        assert!(render("DBT_ENV_SECRET_PASSWORD", false).is_err());
        assert_eq!(
            render("DBT_ENV_SECRET_PASSWORD", true).unwrap(),
            SECRET_PLACEHOLDER.replace("{}", "DBT_ENV_SECRET_PASSWORD")
        );
        assert_eq!(render("PROFILE_ACCOUNT", true).unwrap(), "overlay-value");
    }
}
