use std::fmt::Debug;
use std::sync::Arc;
use std::{collections::BTreeMap, rc::Rc};

use minijinja::{
    Error as MinijinjaError, ErrorKind as MinijinjaErrorKind, State,
    arg_utils::ArgParser,
    listener::RenderingEventListener,
    value::{Object, ObjectRepr, Value},
};

use crate::invocation_args::InvocationArgs;

/// Minijinja Value representing the dbt flags collection
#[derive(Debug, Clone)]
pub struct Flags {
    flags: BTreeMap<String, Value>,
}

impl Object for Flags {
    fn repr(self: &Arc<Self>) -> ObjectRepr {
        ObjectRepr::Plain
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        key.as_str().and_then(|s| self.lookup(s))
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        name: &str,
        args: &[Value],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, MinijinjaError> {
        match name {
            "get" => {
                let mut args = ArgParser::new(args, None);
                let key: String = args.get("name")?;
                let default = args
                    .get_optional::<Value>("default")
                    .unwrap_or_else(|| Value::from(""));
                Ok(self
                    .lookup(&key)
                    .filter(|v| !v.is_none())
                    .unwrap_or(default))
            }
            _ => Err(MinijinjaError::new(
                MinijinjaErrorKind::UnknownMethod,
                format!("Unknown method on flags: {name}"),
            )),
        }
    }
}

impl Flags {
    /// Look up a flag by name, case-insensitively.
    ///
    /// dbt Core normalizes flag names when resolving them, so a project flag
    /// authored as `list_relations_per_page` is reachable whether a macro reads
    /// it lower-case or upper-case. We match that by trying the key verbatim
    /// first (the common, cheap path) and falling back to a case-insensitive
    /// scan. Keys are stored as authored, so consumers that read the flag map
    /// directly (e.g. behavior-flag overrides) keep seeing the original names.
    fn lookup(&self, key: &str) -> Option<Value> {
        if let Some(value) = self.flags.get(key) {
            return Some(value.clone());
        }
        self.flags
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(key))
            .map(|(_, value)| value.clone())
    }

    /// Create a new flags object with default values filled in.
    pub fn new() -> Flags {
        let mut flags = Flags {
            flags: BTreeMap::new(),
        };
        flags.set_defaults();
        flags
    }

    /// Create a new flags object including project-level flags.
    ///
    /// TODO: this is an incomplete support to unblock this use case (see snowflake__list_relations_without_caching macro)
    /// the actual dbt flags needs to encompass not only project flags, but also env vars, and cli options
    /// https://docs.getdbt.com/reference/global-configs/about-global-configs
    pub fn from_project_flags(project_flags: BTreeMap<String, Value>) -> Flags {
        let mut flags = Flags {
            flags: project_flags,
        };
        flags.set_defaults();
        flags
    }

    fn set_defaults(&mut self) {
        self.flags
            .insert("INDIRECT_SELECTION".to_string(), Value::from("eager"));
        self.flags
            .insert("TARGET_PATH".to_string(), Value::UNDEFINED);
        self.flags
            .insert("DEFER_STATE".to_string(), Value::UNDEFINED);
        self.flags
            .insert("WARN_ERROR".to_string(), Value::UNDEFINED);
        self.flags
            .insert("FULL_REFRESH".to_string(), Value::from(false));
        self.flags
            .insert("STRICT_MODE".to_string(), Value::from(false));
        self.flags
            .insert("STORE_FAILURES".to_string(), Value::from(false));
        self.flags
            .insert("FAVOR_STATE".to_string(), Value::from(false));
        self.flags
            .insert("INTROSPECT".to_string(), Value::from(true));
        self.flags.insert("EMPTY".to_string(), Value::from(false));
        self.flags.insert(
            "STATE_MODIFIED_COMPARE_VARS".to_string(),
            Value::from(false),
        );
        // FIXME(@serramatutu): this is just a stub value so that macros that rely on this don't
        // fail. Once we integrate PRINTER_WIDTH into our logs and clap so that everything actually
        // respects it, we should use the real value
        self.flags
            .insert("PRINTER_WIDTH".to_string(), Value::from(80));
    }

    /// Create a new flags object from the invocation args
    pub fn from_invocation_args(invocation_args: BTreeMap<String, Value>) -> Flags {
        Flags {
            flags: invocation_args,
        }
    }

    /// Get dictionary that represents this set of flags
    pub fn to_dict(&self) -> BTreeMap<String, Value> {
        // Reference: https://github.com/dbt-labs/dbt-core/blob/62757f198761ca3a8b8700535bc8c28f84d5c5d5/core/dbt/flags.py#L46
        static FLAG_ATTR: &[&str] = &[
            "use_experimental_parser",
            "static_parser",
            "warn_error",
            "warn_error_options",
            "write_json",
            "partial_parse",
            "use_colors",
            "profiles_dir",
            "debug",
            "log_format",
            "version_check",
            "fail_fast",
            "send_anonymous_usage_stats",
            "printer_width",
            "indirect_selection",
            "log_cache_events",
            "quiet",
            "no_print",
            "cache_selected_only",
            "introspect",
            "target_path",
            "log_path",
            "invocation_command",
            "empty",
            "use_v2_compatible_package_downloads",
        ];
        FLAG_ATTR
            .iter()
            .filter_map(|&key| {
                self.flags
                    .get(&key.to_uppercase())
                    .map(|value| (key.to_string(), value.clone()))
            })
            .collect()
    }

    /// Get dictionary that represents only project flags (excludes CLI flags)
    /// Returns flags from dbt_project.yml that are not standard CLI flags
    pub fn project_flags(&self) -> BTreeMap<String, Value> {
        // Reference: https://github.com/dbt-labs/dbt-core/blob/62757f198761ca3a8b8700535bc8c28f84d5c5d5/core/dbt/flags.py#L46
        static CLI_FLAG_NAMES: &[&str] = &[
            "USE_EXPERIMENTAL_PARSER",
            "STATIC_PARSER",
            "WARN_ERROR",
            "WARN_ERROR_OPTIONS",
            "WRITE_JSON",
            "PARTIAL_PARSE",
            "USE_COLORS",
            "PROFILES_DIR",
            "DEBUG",
            "LOG_FORMAT",
            "VERSION_CHECK",
            "FAIL_FAST",
            "SEND_ANONYMOUS_USAGE_STATS",
            "PRINTER_WIDTH",
            "INDIRECT_SELECTION",
            "LOG_CACHE_EVENTS",
            "QUIET",
            "NO_PRINT",
            "CACHE_SELECTED_ONLY",
            "INTROSPECT",
            "TARGET_PATH",
            "LOG_PATH",
            "INVOCATION_COMMAND",
            "EMPTY",
            // Flags set by set_cli_flags()
            "DEFER",
            "DEFER_STATE",
            "LOG_FORMAT_FILE",
            "LOG_LEVEL_FILE",
            "LOG_LEVEL",
            "PROFILE",
            "PROJECT_DIR",
            "RESOURCE_TYPE",
            "STORE_FAILURES",
            "FAVOR_STATE",
            // Default flags set by set_defaults()
            "FULL_REFRESH",
            "STRICT_MODE",
            "STATE_MODIFIED_COMPARE_VARS",
            // v2 downloads
            "USE_V2_COMPATIBLE_PACKAGE_DOWNLOADS",
        ];

        self.flags
            .iter()
            .filter_map(|(key, value)| {
                if CLI_FLAG_NAMES.contains(&key.as_str()) {
                    None
                } else {
                    Some((key.clone(), value.clone()))
                }
            })
            .collect()
    }

    /// Set the flag's according to https://github.com/dbt-labs/dbt-core/blob/HEAD/core/dbt/flags.py
    pub fn set_cli_flags(&mut self, invocation_args: &InvocationArgs) {
        self.flags.insert(
            "WARN_ERROR".to_string(),
            Value::from(invocation_args.warn_error),
        );
        self.flags.insert(
            "WARN_ERROR_OPTIONS".to_string(),
            Value::from_serialize(&invocation_args.warn_error_options),
        );
        self.flags.insert(
            "VERSION_CHECK".to_string(),
            Value::from(invocation_args.version_check),
        );
        self.flags.insert(
            "INTROSPECT".to_string(),
            Value::from(invocation_args.introspect),
        );
        self.flags
            .insert("DEFER".to_string(), Value::from(invocation_args.defer));
        self.flags.insert(
            "DEFER_STATE".to_string(),
            Value::from(invocation_args.defer_state.clone()),
        );
        self.flags
            .insert("DEBUG".to_string(), Value::from(invocation_args.debug));
        self.flags.insert(
            "LOG_FORMAT_FILE".to_string(),
            Value::from(invocation_args.log_format_file.clone()),
        );
        self.flags.insert(
            "LOG_FORMAT".to_string(),
            Value::from(invocation_args.log_format.clone()),
        );
        self.flags.insert(
            "LOG_LEVEL_FILE".to_string(),
            Value::from(invocation_args.log_level_file.clone()),
        );
        self.flags.insert(
            "LOG_LEVEL".to_string(),
            Value::from(invocation_args.log_level.clone()),
        );
        self.flags.insert(
            "LOG_PATH".to_string(),
            Value::from(invocation_args.log_path.clone()),
        );
        self.flags.insert(
            "PROFILE".to_string(),
            Value::from(invocation_args.profile.clone()),
        );
        self.flags.insert(
            "PROFILES_DIR".to_string(),
            Value::from(invocation_args.profiles_dir.clone().unwrap_or_default()),
        );
        self.flags.insert(
            "PROJECT_DIR".to_string(),
            Value::from(invocation_args.project_dir.clone()),
        );
        self.flags
            .insert("QUIET".to_string(), Value::from(invocation_args.quiet));
        self.flags.insert(
            "RESOURCE_TYPE".to_string(),
            Value::from(invocation_args.resource_type.clone()),
        );
        self.flags.insert(
            "SEND_ANONYMOUS_USAGE_STATS".to_string(),
            Value::from(invocation_args.send_anonymous_usage_stats),
        );
        self.flags.insert(
            "WRITE_JSON".to_string(),
            Value::from(invocation_args.write_json),
        );
        self.flags.insert(
            "STORE_FAILURES".to_string(),
            Value::from(invocation_args.store_failures),
        );
        self.flags.insert(
            "USE_V2_COMPATIBLE_PACKAGE_DOWNLOADS".to_string(),
            Value::from(invocation_args.use_v2_compatible_package_downloads),
        );
        self.flags.insert(
            "FAVOR_STATE".to_string(),
            Value::from(invocation_args.favor_state),
        );
        self.flags
            .insert("EMPTY".to_string(), Value::from(invocation_args.empty));
    }
    /// Override self with other flags
    pub fn join(&mut self, other: Flags) -> Self {
        for (key, value) in other.flags {
            self.flags.insert(key, value); // Insert or override existing keys
        }
        self.clone() // Return the updated Flags
    }
}

impl Default for Flags {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_project_flags_preserves_authored_keys() {
        let project_flags = BTreeMap::from([
            ("use_catalogs_v2".to_string(), Value::from(true)),
            ("Mixed_Case".to_string(), Value::from("v")),
        ]);
        let flags = Flags::from_project_flags(project_flags);

        // Keys are stored exactly as authored. Consumers that read the flag map
        // directly (e.g. behavior-flag overrides, which match lower-case names)
        // rely on this — normalizing the stored keys would break them.
        assert_eq!(flags.flags.get("use_catalogs_v2"), Some(&Value::from(true)));
        assert_eq!(flags.flags.get("Mixed_Case"), Some(&Value::from("v")));
    }

    #[test]
    fn lookup_is_case_insensitive_but_prefers_exact_match() {
        let flags = Flags::from_project_flags(BTreeMap::from([(
            "list_relations_page_limit".to_string(),
            Value::from(10),
        )]));

        // A project flag resolves regardless of the casing a macro reads it with.
        assert_eq!(
            flags.lookup("list_relations_page_limit"),
            Some(Value::from(10))
        );
        assert_eq!(
            flags.lookup("LIST_RELATIONS_PAGE_LIMIT"),
            Some(Value::from(10))
        );
        assert_eq!(flags.lookup("missing"), None);
    }

    #[test]
    fn lookup_prefers_exact_match_over_case_insensitive() {
        // When both casings are present (e.g. a default stored UPPERCASE plus a
        // project flag authored lower-case), an exact match wins.
        let flags = Flags::from_project_flags(BTreeMap::from([(
            "full_refresh".to_string(),
            Value::from(true),
        )]));
        assert_eq!(flags.lookup("full_refresh"), Some(Value::from(true)));
        assert_eq!(flags.lookup("FULL_REFRESH"), Some(Value::from(false)));
    }
}
