use std::ffi::OsString;
use std::fmt;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use crate::def::{FlagDef, Remote, Stage, names_equal, normalize_name};
use crate::parse::{Assignment, parse_assignments, parse_bool};

/// Comma-separated `NAME[=VALUE]` settings, e.g. `git_deps_fast_path,other=false`.
pub const FEATURES_ENV: &str = "DBT_ENGINE_FEATURES";

/// Where a resolved flag value came from, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The flag is [`Stage::Removed`]; its default is locked in.
    Removed,
    /// A remote value marked `force` on a [`Remote::Force`] flag.
    RemoteForced,
    Cli,
    Env,
    Project,
    UserSettings,
    Remote,
    Default,
}

/// A flag's value together with the layer that decided it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resolved {
    pub value: bool,
    pub source: Source,
}

impl Resolved {
    fn new(value: bool, source: Source) -> Self {
        Self { value, source }
    }
}

type EnvLookup = dyn Fn(&str) -> Option<OsString> + Send + Sync;

#[derive(Debug, Clone)]
struct RemoteValue {
    name: String,
    value: bool,
    force: bool,
}

/// The invocation-scoped flag layers: everything except `dbt_project.yml`,
/// which call sites pass in when they resolve a project-settable flag.
pub struct FlagSnapshot {
    cli: Vec<Assignment>,
    env: Vec<Assignment>,
    user_settings: Vec<Assignment>,
    remote: Vec<RemoteValue>,
    env_lookup: Box<EnvLookup>,
    parse_warnings: Vec<String>,
}

impl fmt::Debug for FlagSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlagSnapshot")
            .field("cli", &self.cli)
            .field("env", &self.env)
            .field("user_settings", &self.user_settings)
            .field("remote", &self.remote)
            .finish_non_exhaustive()
    }
}

impl FlagSnapshot {
    /// A builder that reads the process environment.
    pub fn builder() -> FlagSnapshotBuilder {
        FlagSnapshotBuilder {
            cli: Vec::new(),
            user_settings: Vec::new(),
            remote: Vec::new(),
            env_lookup: Box::new(|name| std::env::var_os(name)),
        }
    }

    /// Env layers only: what applies when no snapshot has been installed.
    pub fn from_process_env() -> Self {
        Self::builder().build()
    }

    /// Resolves `def`, consulting the project's `flags:` through `project`
    /// when the flag is settable there.
    pub fn resolve(&self, def: &FlagDef, project: &dyn Fn(&str) -> Option<bool>) -> Resolved {
        if def.stage == Stage::Removed {
            return Resolved::new(def.default, Source::Removed);
        }
        if def.remote == Remote::Force {
            if let Some(remote) = self.remote_for(def).filter(|r| r.force) {
                return Resolved::new(remote.value, Source::RemoteForced);
            }
        }
        if let Some(value) = last_match(&self.cli, def) {
            return Resolved::new(value, Source::Cli);
        }
        if let Some(value) = last_match(&self.env, def) {
            return Resolved::new(value, Source::Env);
        }
        for var in def.legacy_env {
            if let Some(value) = (self.env_lookup)(var)
                .as_deref()
                .and_then(|v| v.to_str())
                .and_then(parse_bool)
            {
                return Resolved::new(value, Source::Env);
            }
        }
        if def.settable_in_project() {
            if let Some(value) = def.names().find_map(project) {
                return Resolved::new(value, Source::Project);
            }
        }
        if let Some(value) = last_match(&self.user_settings, def) {
            return Resolved::new(value, Source::UserSettings);
        }
        if def.remote != Remote::Never {
            if let Some(remote) = self.remote_for(def) {
                return Resolved::new(remote.value, Source::Remote);
            }
        }
        Resolved::new(def.default, Source::Default)
    }

    /// Warnings to show the user for this snapshot against `registry`:
    /// unparseable settings, unknown flag names, and settings of removed flags.
    ///
    /// Unknown names in `user_settings.yml` and in remote payloads are not
    /// reported: the former also holds settings this registry does not own, and
    /// the latter may carry flags for newer releases.
    pub fn warnings(&self, registry: &[&FlagDef]) -> Vec<String> {
        let mut warnings = self.parse_warnings.clone();
        for (assignments, origin) in [(&self.cli, "--feature"), (&self.env, FEATURES_ENV)] {
            for assignment in assignments {
                match registry.iter().find(|def| def.matches(&assignment.name)) {
                    None => warnings.push(format!(
                        "Unknown feature flag '{}' in {origin}; it has no effect",
                        assignment.name
                    )),
                    Some(def) if def.stage == Stage::Removed => warnings.push(format!(
                        "Feature flag '{}' has been removed and is now always {}; \
                         remove it from {origin}",
                        def.name,
                        if def.default { "on" } else { "off" }
                    )),
                    Some(_) => {}
                }
            }
        }
        warnings
    }

    fn remote_for(&self, def: &FlagDef) -> Option<&RemoteValue> {
        self.remote.iter().rev().find(|r| def.matches(&r.name))
    }
}

fn last_match(assignments: &[Assignment], def: &FlagDef) -> Option<bool> {
    assignments
        .iter()
        .rev()
        .find(|a| def.names().any(|name| names_equal(name, &a.name)))
        .map(|a| a.value)
}

/// Collects the invocation-scoped layers for a [`FlagSnapshot`].
pub struct FlagSnapshotBuilder {
    cli: Vec<String>,
    user_settings: Vec<Assignment>,
    remote: Vec<RemoteValue>,
    env_lookup: Box<EnvLookup>,
}

impl FlagSnapshotBuilder {
    /// `--feature NAME[=VALUE]` values, in command-line order.
    pub fn cli<I, S>(mut self, specs: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.cli
            .extend(specs.into_iter().map(|s| s.as_ref().to_string()));
        self
    }

    /// Boolean entries from `user_settings.yml` `flags:`.
    pub fn user_settings<I, S>(mut self, values: I) -> Self
    where
        I: IntoIterator<Item = (S, bool)>,
        S: AsRef<str>,
    {
        self.user_settings
            .extend(values.into_iter().map(|(name, value)| Assignment {
                name: normalize_name(name.as_ref()),
                value,
            }));
        self
    }

    /// A platform-evaluated value. `force` only takes effect on flags declared
    /// [`Remote::Force`].
    pub fn remote(mut self, name: &str, value: bool, force: bool) -> Self {
        self.remote.push(RemoteValue {
            name: normalize_name(name),
            value,
            force,
        });
        self
    }

    /// Replaces the process environment as the source of env layers.
    pub fn env_lookup(
        mut self,
        lookup: impl Fn(&str) -> Option<OsString> + Send + Sync + 'static,
    ) -> Self {
        self.env_lookup = Box::new(lookup);
        self
    }

    pub fn build(self) -> FlagSnapshot {
        let (cli, mut parse_warnings) =
            parse_assignments(self.cli.iter().map(String::as_str), "--feature");
        let env_value = (self.env_lookup)(FEATURES_ENV)
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (env, env_warnings) = parse_assignments(env_value.split(','), FEATURES_ENV);
        parse_warnings.extend(env_warnings);
        FlagSnapshot {
            cli,
            env,
            user_settings: self.user_settings,
            remote: self.remote,
            env_lookup: self.env_lookup,
            parse_warnings,
        }
    }
}

static INSTALLED: RwLock<Option<Arc<FlagSnapshot>>> = RwLock::new(None);
static FALLBACK: LazyLock<Arc<FlagSnapshot>> =
    LazyLock::new(|| Arc::new(FlagSnapshot::from_process_env()));

/// Makes `snapshot` the current invocation's flags. Called once per invocation
/// at startup; a later call replaces the earlier snapshot.
pub fn install(snapshot: FlagSnapshot) {
    *INSTALLED.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(snapshot));
}

/// The installed snapshot, or one built from the process environment when
/// nothing has been installed (e.g. library and test callers).
pub fn current() -> Arc<FlagSnapshot> {
    INSTALLED
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .unwrap_or_else(|| Arc::clone(&FALLBACK))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    const INTERNAL: FlagDef = FlagDef {
        name: "internal_flag",
        description: "",
        stage: Stage::Internal,
        default: false,
        remote: Remote::Default,
        aliases: &["old_internal_flag"],
        legacy_env: &["DBT_LEGACY_INTERNAL"],
        tracking: None,
        remove_by: None,
        docs_url: None,
    };

    const BEHAVIOR: FlagDef = FlagDef {
        name: "behavior_flag",
        stage: Stage::Behavior,
        default: true,
        remote: Remote::Never,
        aliases: &[],
        legacy_env: &["DBT_ENGINE_BEHAVIOR_FLAG"],
        ..INTERNAL
    };

    const KILL_SWITCH: FlagDef = FlagDef {
        name: "kill_switch",
        default: true,
        remote: Remote::Force,
        aliases: &[],
        legacy_env: &[],
        ..INTERNAL
    };

    const REMOVED: FlagDef = FlagDef {
        name: "removed_flag",
        stage: Stage::Removed,
        default: true,
        aliases: &[],
        legacy_env: &["DBT_REMOVED"],
        ..INTERNAL
    };

    fn builder(env: &[(&str, &str)]) -> FlagSnapshotBuilder {
        let env: HashMap<String, OsString> = env
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        FlagSnapshot::builder().env_lookup(move |name| env.get(name).cloned())
    }

    fn no_project(_: &str) -> Option<bool> {
        None
    }

    #[test]
    fn default_applies_when_nothing_is_set() {
        let snapshot = builder(&[]).build();
        assert_eq!(
            snapshot.resolve(&INTERNAL, &no_project),
            Resolved::new(false, Source::Default)
        );
    }

    #[test]
    fn layers_apply_in_precedence_order() {
        let project_false = |name: &str| (name == "behavior_flag").then_some(false);
        let flag = BEHAVIOR;

        let snapshot = builder(&[])
            .user_settings([("behavior_flag", false)])
            .build();
        assert_eq!(
            snapshot.resolve(&flag, &no_project),
            Resolved::new(false, Source::UserSettings)
        );
        assert_eq!(
            snapshot.resolve(&flag, &project_false),
            Resolved::new(false, Source::Project)
        );

        let snapshot = builder(&[("DBT_ENGINE_BEHAVIOR_FLAG", "true")]).build();
        assert_eq!(
            snapshot.resolve(&flag, &project_false),
            Resolved::new(true, Source::Env)
        );

        let snapshot = builder(&[
            ("DBT_ENGINE_BEHAVIOR_FLAG", "true"),
            (FEATURES_ENV, "behavior_flag=false"),
        ])
        .build();
        assert_eq!(
            snapshot.resolve(&flag, &project_false),
            Resolved::new(false, Source::Env)
        );

        let snapshot = builder(&[(FEATURES_ENV, "behavior_flag=false")])
            .cli(["behavior-flag"])
            .build();
        assert_eq!(
            snapshot.resolve(&flag, &project_false),
            Resolved::new(true, Source::Cli)
        );
    }

    #[test]
    fn malformed_legacy_env_falls_through() {
        let snapshot = builder(&[("DBT_LEGACY_INTERNAL", "maybe")]).build();
        assert_eq!(
            snapshot.resolve(&INTERNAL, &no_project),
            Resolved::new(false, Source::Default)
        );
    }

    #[test]
    fn internal_flags_ignore_the_project_layer() {
        let snapshot = builder(&[]).build();
        let project = |_: &str| Some(true);
        assert_eq!(
            snapshot.resolve(&INTERNAL, &project).source,
            Source::Default
        );
    }

    #[test]
    fn aliases_resolve_to_the_flag_everywhere() {
        let snapshot = builder(&[]).cli(["OLD-INTERNAL-FLAG"]).build();
        assert_eq!(
            snapshot.resolve(&INTERNAL, &no_project),
            Resolved::new(true, Source::Cli)
        );
        assert!(snapshot.warnings(&[&INTERNAL]).is_empty());
    }

    #[test]
    fn last_setting_in_a_layer_wins() {
        let snapshot = builder(&[])
            .cli(["internal_flag", "internal_flag=off"])
            .build();
        assert!(!snapshot.resolve(&INTERNAL, &no_project).value);
    }

    #[test]
    fn remote_overrides_only_the_default() {
        let snapshot = builder(&[]).remote("internal_flag", true, false).build();
        assert_eq!(
            snapshot.resolve(&INTERNAL, &no_project),
            Resolved::new(true, Source::Remote)
        );

        let snapshot = builder(&[])
            .remote("internal_flag", true, true)
            .cli(["internal_flag=false"])
            .build();
        assert_eq!(
            snapshot.resolve(&INTERNAL, &no_project),
            Resolved::new(false, Source::Cli),
            "force is ignored on flags not declared Remote::Force"
        );

        let snapshot = builder(&[]).remote("behavior_flag", false, false).build();
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &no_project).source,
            Source::Default,
            "Remote::Never flags ignore remote values"
        );
    }

    #[test]
    fn forced_remote_wins_over_local_settings() {
        let snapshot = builder(&[(FEATURES_ENV, "kill_switch")])
            .cli(["kill_switch=true"])
            .remote("kill_switch", false, true)
            .build();
        assert_eq!(
            snapshot.resolve(&KILL_SWITCH, &no_project),
            Resolved::new(false, Source::RemoteForced)
        );
    }

    #[test]
    fn removed_flags_are_locked_to_their_default() {
        let snapshot = builder(&[("DBT_REMOVED", "0")])
            .cli(["removed_flag=false"])
            .build();
        assert_eq!(
            snapshot.resolve(&REMOVED, &|_| Some(false)),
            Resolved::new(true, Source::Removed)
        );
        let warnings = snapshot.warnings(&[&REMOVED]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("now always on"), "{warnings:?}");
    }

    #[test]
    fn warnings_report_unknown_and_malformed_settings() {
        let snapshot = builder(&[(FEATURES_ENV, "nope,internal_flag=sometimes")])
            .cli(["also_nope"])
            .user_settings([("not_a_registry_flag", true)])
            .remote("future_flag", true, false)
            .build();
        let warnings = snapshot.warnings(&[&INTERNAL]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("'internal_flag=sometimes'"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("'also_nope' in --feature"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(&format!("'nope' in {FEATURES_ENV}")))
        );
    }

    #[test]
    fn installed_snapshot_is_current() {
        static FLAG: FlagDef = FlagDef {
            name: "installed_test_flag",
            legacy_env: &[],
            aliases: &[],
            ..INTERNAL
        };
        install(builder(&[]).cli(["installed_test_flag"]).build());
        assert!(FLAG.enabled());
        install(builder(&[]).build());
        assert!(!FLAG.enabled());
    }
}
