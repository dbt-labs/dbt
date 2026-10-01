use std::{collections::BTreeMap, env};

use dbt_common::io_args::EvalArgs;
use dbt_common::io_args::LogLevel;
use dbt_common::io_args::ReplayMode;
use dbt_common::warn_error_options::WarnErrorOptions;
use itertools::Itertools;
use minijinja::Value;

/// Invocation args is the dictionary of arguments passed into the jinja environment.
// TODO: this is not complete, we will add more as we go.
#[derive(Debug, Clone)]
pub struct InvocationArgs {
    /// command
    pub invocation_command: String,
    /// vars
    pub vars: BTreeMap<String, Value>,
    /// select
    pub select: Option<String>,
    /// exclude
    pub exclude: Option<String>,
    /// profiles_dir
    pub profiles_dir: Option<String>,
    /// packages_install_path
    pub packages_install_path: Option<String>,
    /// target
    pub target: Option<String>,
    /// num_threads
    pub num_threads: Option<usize>,
    /// invocation_id
    pub invocation_id: uuid::Uuid,

    // and here are all flags
    /// Flags
    pub warn_error: bool,
    /// Warning error options
    pub warn_error_options: WarnErrorOptions,
    /// Version check
    pub version_check: bool,
    /// Introspect
    pub introspect: bool,
    /// Defer
    pub defer: bool,
    /// Defer state
    pub defer_state: String,
    /// Debug
    pub debug: bool,
    /// Log format file
    pub log_format_file: String,
    /// Log format
    pub log_format: String,
    /// Log level file
    pub log_level_file: String,
    /// Log level
    pub log_level: String,
    /// Log path
    pub log_path: String,
    /// Profile
    pub profile: String,
    /// Project dir
    pub project_dir: String,
    /// Quiet
    pub quiet: bool,
    /// Resource type
    pub resource_type: Vec<String>,
    /// Send anonymous usage stats
    pub send_anonymous_usage_stats: bool,
    /// Write json
    pub write_json: bool,
    /// Full refresh
    pub full_refresh: bool,
    /// Store test failures in the database
    pub store_failures: bool,
    /// Favor state over current environment for deferral
    pub favor_state: bool,
    /// Empty flag
    pub empty: bool,

    /// Replay mode (when running against a recording)
    pub replay: Option<ReplayMode>,

    /// Use v2 compatible package downloads when installing from Package Hub
    pub use_v2_compatible_package_downloads: bool,
}

impl Default for InvocationArgs {
    fn default() -> Self {
        Self {
            invocation_command: String::new(),
            vars: BTreeMap::new(),
            select: None,
            exclude: None,
            profiles_dir: None,
            packages_install_path: None,
            target: None,
            num_threads: None,
            invocation_id: uuid::Uuid::nil(),
            warn_error: false,
            warn_error_options: WarnErrorOptions::default(),
            version_check: false,
            introspect: true,
            defer: false,
            defer_state: String::new(),
            debug: false,
            log_format_file: String::new(),
            log_format: String::new(),
            log_level_file: String::new(),
            log_level: String::new(),
            log_path: String::new(),
            profile: String::new(),
            project_dir: String::new(),
            quiet: false,
            resource_type: Vec::new(),
            send_anonymous_usage_stats: false,
            write_json: false,
            full_refresh: false,
            store_failures: false,
            favor_state: false,
            empty: false,
            replay: None,
            use_v2_compatible_package_downloads: false,
        }
    }
}

impl InvocationArgs {
    /// Create an InvocationArgs from an EvalArgs.
    pub fn from_eval_args(arg: &EvalArgs) -> Self {
        let log_level = arg.log_level.unwrap_or(LogLevel::Info);

        let log_level_file = arg.log_level_file.unwrap_or(log_level);

        let log_format = arg.log_format;
        let log_format_file = arg.log_format_file.unwrap_or(log_format);

        InvocationArgs {
            invocation_command: arg.command.as_str().to_string(),
            vars: arg
                .vars
                .iter()
                .map(|(k, v)| {
                    let value = Value::from_serialize(v);
                    (k.clone(), value)
                })
                .collect(),
            select: arg.select.clone().map(|select| select.to_string()),
            exclude: arg.exclude.clone().map(|exclude| exclude.to_string()),
            profiles_dir: arg
                .profiles_dir
                .clone()
                .map(|path| path.to_string_lossy().to_string()),
            packages_install_path: arg
                .packages_install_path
                .clone()
                .map(|path| path.to_string_lossy().to_string()),
            target: arg.target.clone(),
            // unrestricted multi-threading
            num_threads: arg.num_threads,
            invocation_id: arg.io.invocation_id,
            warn_error: arg.warn_error.unwrap_or_default(),
            warn_error_options: arg.warn_error_options.clone(),
            version_check: arg.version_check,
            introspect: arg.introspect,
            defer: arg.defer,
            defer_state: arg
                .defer_state
                .clone()
                .unwrap_or_default()
                .display()
                .to_string(),
            debug: arg.debug,
            log_format: log_format.to_string(),
            log_format_file: log_format_file.to_string(),
            log_level: log_level.to_string(),
            log_level_file: log_level_file.to_string(),
            log_path: arg
                .log_path
                .clone()
                .unwrap_or_default()
                .display()
                .to_string(),
            profile: arg.profile.clone().unwrap_or_default(),
            project_dir: arg
                .project_dir
                .clone()
                .unwrap_or_default()
                .display()
                .to_string(),
            quiet: arg.quiet,
            resource_type: arg.resource_types.iter().map(|rt| rt.to_string()).collect(),
            send_anonymous_usage_stats: arg.send_anonymous_usage_stats,
            write_json: arg.write_json,
            full_refresh: arg.full_refresh,
            store_failures: arg.store_failures,
            favor_state: arg.favor_state,
            empty: arg.empty,
            replay: arg.replay.clone(),
            use_v2_compatible_package_downloads: arg.io.use_v2_compatible_package_downloads,
        }
    }

    /// Convert the InvocationArgs to a dictionary.
    pub fn to_dict(&self) -> BTreeMap<String, Value> {
        let mut dict = BTreeMap::new();
        dict.insert(
            "WHICH".to_string(),
            Value::from(self.invocation_command.clone()),
        );

        dict.insert(
            "INVOCATION_COMMAND".to_string(),
            Value::from(env::args().join(" ")),
        );

        dict.insert("vars".to_string(), Value::from_serialize(&self.vars));
        dict.insert("SELECT".to_string(), Value::from(self.select.clone()));
        dict.insert("EXCLUDE".to_string(), Value::from(self.exclude.clone()));
        dict.insert(
            "PROFILES_DIR".to_string(),
            Value::from(self.profiles_dir.clone()),
        );
        dict.insert(
            "PACKAGES_INSTALL_PATH".to_string(),
            Value::from(self.packages_install_path.clone()),
        );
        dict.insert("TARGET".to_string(), Value::from(self.target.clone()));
        dict.insert("NUM_THREADS".to_string(), Value::from(self.num_threads));
        dict.insert(
            "INVOCATION_ID".to_string(),
            Value::from(self.invocation_id.to_string()),
        );
        dict.insert("warn_error".to_string(), Value::from(self.warn_error));
        dict.insert(
            "warn_error_options".to_string(),
            Value::from_serialize(&self.warn_error_options),
        );
        dict.insert("VERSION_CHECK".to_string(), Value::from(self.version_check));
        dict.insert("INTROSPECT".to_string(), Value::from(self.introspect));
        dict.insert("DEFER".to_string(), Value::from(self.defer));
        dict.insert(
            "DEFER_STATE".to_string(),
            Value::from(self.defer_state.clone()),
        );
        dict.insert("DEBUG".to_string(), Value::from(self.debug));
        dict.insert(
            "LOG_FORMAT_FILE".to_string(),
            Value::from(self.log_format_file.clone()),
        );
        dict.insert(
            "LOG_FORMAT".to_string(),
            Value::from(self.log_format.clone()),
        );
        dict.insert(
            "LOG_LEVEL_FILE".to_string(),
            Value::from(self.log_level_file.clone()),
        );
        dict.insert("LOG_LEVEL".to_string(), Value::from(self.log_level.clone()));
        dict.insert("LOG_PATH".to_string(), Value::from(self.log_path.clone()));
        dict.insert("PROFILE".to_string(), Value::from(self.profile.clone()));
        dict.insert(
            "PROJECT_DIR".to_string(),
            Value::from(self.project_dir.clone()),
        );
        dict.insert("QUIET".to_string(), Value::from(self.quiet));
        dict.insert(
            "RESOURCE_TYPE".to_string(),
            Value::from(self.resource_type.clone()),
        );
        dict.insert(
            "SEND_ANONYMOUS_USAGE_STATS".to_string(),
            Value::from(self.send_anonymous_usage_stats),
        );
        dict.insert("WRITE_JSON".to_string(), Value::from(self.write_json));
        dict.insert("FULL_REFRESH".to_string(), Value::from(self.full_refresh));
        dict.insert(
            "STORE_FAILURES".to_string(),
            Value::from(self.store_failures),
        );
        dict.insert("FAVOR_STATE".to_string(), Value::from(self.favor_state));
        dict.insert("EMPTY".to_string(), Value::from(self.empty));
        dict.insert("REPLAY".to_string(), Value::from(self.replay.is_some()));
        dict.insert(
            "USE_V2_COMPATIBLE_PACKAGE_DOWNLOADS".to_string(),
            Value::from(self.use_v2_compatible_package_downloads),
        );

        // !!HACK!!: Inject a lower case version of the upper-case keys, for use
        // in `invocation_args_dict` -- we do this because this method is
        // currently been used both to create `flags` and `invocation_args_dict`.
        //
        // FIXME: we should separate `flags` and `invocation_args_dict` creation
        // codepaths.
        dict.into_iter()
            .flat_map(|(k, v)| {
                if k == k.to_uppercase() {
                    let lower_value = v.clone();
                    vec![(k.clone(), v), (k.to_lowercase(), lower_value)]
                } else {
                    vec![(k, v)]
                }
            })
            .collect()
    }

    /// Set the number of threads to use.
    pub fn set_num_threads(&self, final_threads: Option<usize>) -> Self {
        Self {
            num_threads: final_threads,
            ..self.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::io_args::ReplayMode;

    #[test]
    // Replay mode is detected during macro
    // registration (parse phase) via `invocation_args_dict.REPLAY`. If this key stops being
    // surfaced by `InvocationArgs::to_dict()`, replay-only macro overrides (e.g. to suppress
    // non-semantic package behavior) will silently stop applying.
    fn to_dict_includes_replay_flag() {
        let args = InvocationArgs {
            replay: Some(ReplayMode::FsReplay("some/path".into())),
            ..InvocationArgs::default()
        };
        let d = args.to_dict();
        let replay = d.get("REPLAY").expect("REPLAY should be present");
        assert!(
            replay.is_true(),
            "REPLAY should be present and truthy, got: {replay:?}"
        );

        let args2 = InvocationArgs {
            replay: None,
            ..InvocationArgs::default()
        };
        let d2 = args2.to_dict();
        let replay2 = d2.get("REPLAY").expect("REPLAY should be present");
        assert!(
            !replay2.is_true(),
            "REPLAY should be present and falsy, got: {replay2:?}"
        );
    }
}
