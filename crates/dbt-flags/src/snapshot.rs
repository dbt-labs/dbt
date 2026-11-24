use std::ffi::OsString;
use std::fmt;
use std::sync::{Arc, LazyLock, PoisonError, RwLock};

use crate::def::{FlagDef, ProjectFlags, Remote, Stage, normalize_name};
use crate::parse::{Assignment, parse_bool, parse_names};

/// Comma-separated names of beta features to turn on, e.g. `metricflow_v2,other`.
pub const BETA_FEATURES_ENV: &str = "DBT_ENGINE_BETA_FEATURES";

const BETA_FEATURES_CLI: &str = "--beta-features";
const BETA_FEATURES_PROJECT: &str = "`beta_features` in the `flags:` of dbt_project.yml";

/// Where a resolved flag value came from, highest precedence first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The flag is [`Stage::Graduated`] or [`Stage::Abandoned`]; its value is locked in.
    Retired,
    /// A remote value marked `force` on a [`Remote::Force`] flag.
    RemoteForced,
    /// The flag's environment variable, or one of its `legacy_env`.
    EnvVar,
    /// `--beta-features`.
    Cli,
    /// `DBT_ENGINE_BETA_FEATURES`.
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
    /// Normalized names from `--beta-features`.
    cli: Vec<String>,
    /// Normalized names from `DBT_ENGINE_BETA_FEATURES`.
    env: Vec<String>,
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

    /// Resolves `def`, consulting the root project's `flags:` through `project`
    /// when the flag's stage lets a project set it.
    pub fn resolve(&self, def: &FlagDef, project: &dyn ProjectFlags) -> Resolved {
        if let Some(value) = def.stage.locked() {
            return Resolved::new(value, Source::Retired);
        }
        if def.remote == Remote::Force {
            if let Some(remote) = self.remote_for(def).filter(|r| r.force) {
                return Resolved::new(remote.value, Source::RemoteForced);
            }
        }
        if let Some(value) = self.env_var_value(def) {
            return Resolved::new(value, Source::EnvVar);
        }
        match def.stage {
            Stage::Beta => {
                if let Some(resolved) = self.beta_setting(def, project) {
                    return resolved;
                }
            }
            Stage::Behavior => {
                if let Some(value) = def.names().find_map(|name| project.value(name)) {
                    return Resolved::new(value, Source::Project);
                }
                // A setting from when the flag was a beta feature still applies, so a project
                // that turned the feature on keeps it across the promotion.
                if let Some(resolved) = self.beta_setting(def, project) {
                    return resolved;
                }
            }
            Stage::Experimental | Stage::Graduated | Stage::Abandoned => {}
        }
        if matches!(def.stage, Stage::Beta | Stage::Behavior) {
            if let Some(assignment) = self
                .user_settings
                .iter()
                .rev()
                .find(|a| def.matches(&a.name))
            {
                return Resolved::new(assignment.value, Source::UserSettings);
            }
        }
        if def.remote != Remote::Never {
            if let Some(remote) = self.remote_for(def) {
                return Resolved::new(remote.value, Source::Remote);
            }
        }
        Resolved::new(def.default, Source::Default)
    }

    /// Warnings to show the user for this snapshot against `registry`: entries of
    /// `--beta-features` and `DBT_ENGINE_BETA_FEATURES` that are malformed, unknown, or name a
    /// flag that is not a beta feature, and environment variables of flags that are malformed
    /// or have no effect.
    ///
    /// Unknown names in `user_settings.yml` and in remote payloads are not
    /// reported: the former also holds settings this registry does not own, and
    /// the latter may carry flags for newer releases.
    pub fn warnings(&self, registry: &[&FlagDef]) -> Vec<String> {
        let mut warnings = self.parse_warnings.clone();
        for (names, origin) in [
            (&self.cli, BETA_FEATURES_CLI),
            (&self.env, BETA_FEATURES_ENV),
        ] {
            warnings.extend(
                names
                    .iter()
                    .filter_map(|name| beta_list_warning(registry, name, origin)),
            );
        }
        for def in registry {
            for var in def.env_vars() {
                let Some(value) = (self.env_lookup)(&var) else {
                    continue;
                };
                if let Some(locked) = def.stage.locked() {
                    warnings.push(format!("{}; unset {var}", retired(def, locked)));
                } else if value.to_str().and_then(parse_bool).is_none() {
                    warnings.push(format!(
                        "Ignoring {var}='{}': the value must be a boolean (1/0, true/false, \
                         yes/no, on/off)",
                        value.to_string_lossy()
                    ));
                }
            }
        }
        warnings
    }

    /// Warnings to show the user for the root project's `flags:` against `registry`: entries
    /// of `beta_features` that are not booleans, are unknown, or name a flag that is not a beta
    /// feature, and a beta feature that is set outside `beta_features`, which has no effect.
    pub fn project_warnings(registry: &[&FlagDef], project: &dyn ProjectFlags) -> Vec<String> {
        let mut warnings = Vec::new();
        project.beta_features(&mut |name, value| {
            if value.is_none() {
                warnings.push(format!(
                    "Ignoring '{name}' in {BETA_FEATURES_PROJECT}: set it as `{name}: true`"
                ));
                return;
            }
            warnings.extend(beta_list_warning(
                registry,
                &normalize_name(name),
                BETA_FEATURES_PROJECT,
            ));
        });
        for def in registry.iter().filter(|def| def.stage == Stage::Beta) {
            if let Some(name) = def.names().find(|name| project.value(name).is_some()) {
                warnings.push(format!(
                    "`{name}` in the `flags:` of dbt_project.yml has no effect: '{}' is a beta \
                     feature, so set it as `{}: true` under `beta_features` instead",
                    def.name, def.name
                ));
            }
        }
        warnings
    }

    /// What the surfaces of beta features set `def` to: `--beta-features`, then
    /// `DBT_ENGINE_BETA_FEATURES`, then the project's `beta_features`.
    fn beta_setting(&self, def: &FlagDef, project: &dyn ProjectFlags) -> Option<Resolved> {
        if listed(&self.cli, def) {
            return Some(Resolved::new(true, Source::Cli));
        }
        if listed(&self.env, def) {
            return Some(Resolved::new(true, Source::Env));
        }
        let mut in_project = None;
        project.beta_features(&mut |name, value| {
            if def.matches(name) {
                in_project = in_project.or(value);
            }
        });
        in_project.map(|value| Resolved::new(value, Source::Project))
    }

    /// The flag's own environment variable (under its name, then its aliases), then its
    /// `legacy_env`. An unparseable value is skipped.
    fn env_var_value(&self, def: &FlagDef) -> Option<bool> {
        def.env_vars()
            .chain(def.legacy_env.iter().map(|var| (*var).to_string()))
            .find_map(|var| {
                (self.env_lookup)(&var)
                    .as_deref()
                    .and_then(|value| value.to_str())
                    .and_then(parse_bool)
            })
    }

    fn remote_for(&self, def: &FlagDef) -> Option<&RemoteValue> {
        self.remote.iter().rev().find(|r| def.matches(&r.name))
    }
}

fn listed(names: &[String], def: &FlagDef) -> bool {
    names.iter().any(|name| def.matches(name))
}

/// What to tell the user about `name`, set as a beta feature in `origin`, if it is not one:
/// that it has no effect, or that it still applies but belongs elsewhere.
fn beta_list_warning(registry: &[&FlagDef], name: &str, origin: &str) -> Option<String> {
    let Some(def) = registry.iter().find(|def| def.matches(name)) else {
        return Some(format!(
            "Unknown beta feature '{name}' in {origin}; it has no effect"
        ));
    };
    match def.stage {
        Stage::Beta => None,
        Stage::Experimental => Some(format!(
            "'{}' in {origin} has no effect: it is not a beta feature",
            def.name
        )),
        Stage::Behavior => Some(format!(
            "'{name}' in {origin} still applies, but '{}' is now a behavior flag: set it as \
             `{}: <true|false>` in the `flags:` of dbt_project.yml instead",
            def.name, def.name
        )),
        Stage::Graduated => Some(format!("{}; remove it from {origin}", retired(def, true))),
        Stage::Abandoned => Some(format!("{}; remove it from {origin}", retired(def, false))),
    }
}

/// What became of a flag that is locked to `value`.
fn retired(def: &FlagDef, value: bool) -> String {
    if value {
        format!("Feature '{}' has graduated and is always on", def.name)
    } else {
        format!("Feature '{}' was abandoned and is always off", def.name)
    }
}

/// Collects the invocation-scoped layers for a [`FlagSnapshot`].
pub struct FlagSnapshotBuilder {
    cli: Vec<String>,
    user_settings: Vec<Assignment>,
    remote: Vec<RemoteValue>,
    env_lookup: Box<EnvLookup>,
}

impl FlagSnapshotBuilder {
    /// `--beta-features` values, in command-line order. Each is one name or several separated
    /// by commas.
    pub fn cli<I, S>(mut self, lists: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        self.cli
            .extend(lists.into_iter().map(|s| s.as_ref().to_string()));
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
            parse_names(self.cli.iter().map(String::as_str), BETA_FEATURES_CLI);
        let env_value = (self.env_lookup)(BETA_FEATURES_ENV)
            .map(|v| v.to_string_lossy().into_owned())
            .unwrap_or_default();
        let (env, env_warnings) = parse_names([env_value.as_str()], BETA_FEATURES_ENV);
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
    use crate::def::NoProjectFlags;

    const EXPERIMENTAL: FlagDef = FlagDef {
        name: "experimental_flag",
        description: "",
        stage: Stage::Experimental,
        default: false,
        remote: Remote::Default,
        aliases: &["old_experimental_flag"],
        legacy_env: &["DBT_LEGACY_EXPERIMENTAL"],
        tracking: None,
        remove_by: None,
        docs_url: None,
    };

    const BETA: FlagDef = FlagDef {
        name: "beta_flag",
        stage: Stage::Beta,
        remote: Remote::Never,
        aliases: &["old_beta_flag"],
        legacy_env: &[],
        ..EXPERIMENTAL
    };

    const BEHAVIOR: FlagDef = FlagDef {
        name: "behavior_flag",
        stage: Stage::Behavior,
        default: true,
        remote: Remote::Never,
        aliases: &[],
        legacy_env: &["DBT_ENGINE_BEHAVIOR_FLAG"],
        ..EXPERIMENTAL
    };

    const KILL_SWITCH: FlagDef = FlagDef {
        name: "kill_switch",
        default: true,
        remote: Remote::Force,
        aliases: &[],
        legacy_env: &[],
        ..EXPERIMENTAL
    };

    const GRADUATED: FlagDef = FlagDef {
        name: "graduated_flag",
        stage: Stage::Graduated,
        default: true,
        aliases: &[],
        legacy_env: &["DBT_GRADUATED"],
        ..EXPERIMENTAL
    };

    const ABANDONED: FlagDef = FlagDef {
        name: "abandoned_flag",
        stage: Stage::Abandoned,
        aliases: &[],
        legacy_env: &[],
        ..EXPERIMENTAL
    };

    fn builder(env: &[(&str, &str)]) -> FlagSnapshotBuilder {
        let env: HashMap<String, OsString> = env
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        FlagSnapshot::builder().env_lookup(move |name| env.get(name).cloned())
    }

    /// A root project's `flags:`: its boolean keys and the entries of its `beta_features`.
    struct Project(
        &'static [(&'static str, bool)],
        &'static [(&'static str, Option<bool>)],
    );

    impl ProjectFlags for Project {
        fn value(&self, name: &str) -> Option<bool> {
            self.0
                .iter()
                .find_map(|(key, value)| (*key == name).then_some(*value))
        }

        fn beta_features(&self, visit: &mut dyn FnMut(&str, Option<bool>)) {
            self.1.iter().for_each(|(name, value)| visit(name, *value));
        }
    }

    #[test]
    fn default_applies_when_nothing_is_set() {
        let snapshot = builder(&[]).build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(false, Source::Default)
        );
    }

    #[test]
    fn an_experimental_flag_is_set_only_by_its_environment_variable() {
        let everywhere_else = Project(
            &[("experimental_flag", true)],
            &[("experimental_flag", Some(true))],
        );
        let snapshot = builder(&[(BETA_FEATURES_ENV, "experimental_flag")])
            .cli(["experimental_flag"])
            .user_settings([("experimental_flag", true)])
            .build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &everywhere_else),
            Resolved::new(false, Source::Default)
        );

        let snapshot = builder(&[("DBT_ENGINE_EXPERIMENTAL_EXPERIMENTAL_FLAG", "1")]).build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(true, Source::EnvVar)
        );
    }

    #[test]
    fn a_beta_flag_is_set_by_every_beta_layer() {
        let snapshot = builder(&[]).user_settings([("beta_flag", true)]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &NoProjectFlags),
            Resolved::new(true, Source::UserSettings)
        );
        assert_eq!(
            snapshot.resolve(&BETA, &Project(&[], &[("Beta-Flag", Some(true))])),
            Resolved::new(true, Source::Project)
        );

        let snapshot = builder(&[(BETA_FEATURES_ENV, "other,beta_flag")]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &Project(&[], &[("beta_flag", Some(true))])),
            Resolved::new(true, Source::Env)
        );

        let snapshot = builder(&[(BETA_FEATURES_ENV, "beta_flag")])
            .cli(["other,beta-flag"])
            .build();
        assert_eq!(
            snapshot.resolve(&BETA, &NoProjectFlags),
            Resolved::new(true, Source::Cli)
        );

        let snapshot = builder(&[("DBT_ENGINE_EXPERIMENTAL_BETA_FLAG", "true")]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &NoProjectFlags),
            Resolved::new(true, Source::EnvVar)
        );
    }

    #[test]
    fn a_beta_flag_is_not_set_as_a_boolean_in_the_project() {
        let snapshot = builder(&[]).build();
        let project = Project(&[("beta_flag", true)], &[]);
        assert_eq!(
            snapshot.resolve(&BETA, &project),
            Resolved::new(false, Source::Default)
        );
        let warnings = FlagSnapshot::project_warnings(&[&BETA], &project);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("set it as `beta_flag: true` under `beta_features`"));
    }

    #[test]
    fn the_environment_variable_turns_a_listed_beta_flag_off() {
        let snapshot = builder(&[
            ("DBT_ENGINE_EXPERIMENTAL_BETA_FLAG", "0"),
            (BETA_FEATURES_ENV, "beta_flag"),
        ])
        .cli(["beta_flag"])
        .user_settings([("beta_flag", true)])
        .build();
        assert_eq!(
            snapshot.resolve(&BETA, &Project(&[], &[("beta_flag", Some(true))])),
            Resolved::new(false, Source::EnvVar)
        );
    }

    #[test]
    fn a_behavior_flag_is_set_by_the_project_and_the_environment() {
        let project_false = Project(&[("behavior_flag", false)], &[]);

        let snapshot = builder(&[])
            .user_settings([("behavior_flag", false)])
            .build();
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &NoProjectFlags),
            Resolved::new(false, Source::UserSettings)
        );
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &project_false),
            Resolved::new(false, Source::Project)
        );

        let snapshot = builder(&[("DBT_ENGINE_BEHAVIOR_FLAG", "true")]).build();
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &project_false),
            Resolved::new(true, Source::EnvVar)
        );

        let snapshot = builder(&[
            ("DBT_ENGINE_BEHAVIOR_FLAG", "true"),
            ("DBT_ENGINE_EXPERIMENTAL_BEHAVIOR_FLAG", "false"),
        ])
        .build();
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &project_false),
            Resolved::new(false, Source::EnvVar),
            "the flag's own variable is read before its legacy ones"
        );
    }

    #[test]
    fn a_behavior_flag_still_reads_the_surfaces_of_beta_features() {
        let flag = FlagDef {
            default: false,
            ..BEHAVIOR
        };
        let in_beta_features = Project(&[], &[("behavior_flag", Some(true))]);

        let snapshot = builder(&[]).build();
        assert_eq!(
            snapshot.resolve(&flag, &in_beta_features),
            Resolved::new(true, Source::Project)
        );
        assert_eq!(
            FlagSnapshot::project_warnings(&[&flag], &in_beta_features),
            [
                "'behavior_flag' in `beta_features` in the `flags:` of dbt_project.yml still \
              applies, but 'behavior_flag' is now a behavior flag: set it as `behavior_flag: \
              <true|false>` in the `flags:` of dbt_project.yml instead"
            ]
        );

        let snapshot = builder(&[(BETA_FEATURES_ENV, "behavior_flag")]).build();
        assert_eq!(
            snapshot.resolve(&flag, &NoProjectFlags),
            Resolved::new(true, Source::Env)
        );
        let snapshot = builder(&[]).cli(["behavior_flag"]).build();
        assert_eq!(
            snapshot.resolve(&flag, &NoProjectFlags),
            Resolved::new(true, Source::Cli)
        );
        let warnings = snapshot.warnings(&[&flag]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("'behavior_flag' in --beta-features still applies"));
    }

    #[test]
    fn a_behavior_flags_own_setting_wins_over_the_surfaces_of_beta_features() {
        let flag = FlagDef {
            default: false,
            ..BEHAVIOR
        };
        let snapshot = builder(&[(BETA_FEATURES_ENV, "behavior_flag")])
            .cli(["behavior_flag"])
            .build();
        let project = Project(
            &[("behavior_flag", false)],
            &[("behavior_flag", Some(true))],
        );
        assert_eq!(
            snapshot.resolve(&flag, &project),
            Resolved::new(false, Source::Project)
        );
    }

    #[test]
    fn malformed_env_values_fall_through() {
        let snapshot = builder(&[
            ("DBT_LEGACY_EXPERIMENTAL", "maybe"),
            ("DBT_ENGINE_EXPERIMENTAL_EXPERIMENTAL_FLAG", "sometimes"),
        ])
        .build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(false, Source::Default)
        );
        let warnings = snapshot.warnings(&[&EXPERIMENTAL]);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].contains("DBT_ENGINE_EXPERIMENTAL_EXPERIMENTAL_FLAG='sometimes'"),
            "a legacy variable's bad value is ignored without a warning: {warnings:?}"
        );
    }

    #[test]
    fn aliases_resolve_to_the_flag_everywhere() {
        let snapshot = builder(&[]).cli(["OLD-BETA-FLAG"]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &NoProjectFlags),
            Resolved::new(true, Source::Cli)
        );
        assert!(snapshot.warnings(&[&BETA]).is_empty());

        let snapshot = builder(&[("DBT_ENGINE_EXPERIMENTAL_OLD_EXPERIMENTAL_FLAG", "1")]).build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(true, Source::EnvVar)
        );
    }

    #[test]
    fn remote_overrides_only_the_default() {
        let snapshot = builder(&[])
            .remote("experimental_flag", true, false)
            .build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(true, Source::Remote)
        );

        let snapshot = builder(&[("DBT_ENGINE_EXPERIMENTAL_EXPERIMENTAL_FLAG", "false")])
            .remote("experimental_flag", true, true)
            .build();
        assert_eq!(
            snapshot.resolve(&EXPERIMENTAL, &NoProjectFlags),
            Resolved::new(false, Source::EnvVar),
            "force is ignored on flags not declared Remote::Force"
        );

        let snapshot = builder(&[]).remote("behavior_flag", false, false).build();
        assert_eq!(
            snapshot.resolve(&BEHAVIOR, &NoProjectFlags).source,
            Source::Default,
            "Remote::Never flags ignore remote values"
        );
    }

    #[test]
    fn forced_remote_wins_over_local_settings() {
        let snapshot = builder(&[("DBT_ENGINE_EXPERIMENTAL_KILL_SWITCH", "true")])
            .remote("kill_switch", false, true)
            .build();
        assert_eq!(
            snapshot.resolve(&KILL_SWITCH, &NoProjectFlags),
            Resolved::new(false, Source::RemoteForced)
        );
    }

    #[test]
    fn retired_flags_are_locked() {
        let snapshot = builder(&[
            ("DBT_GRADUATED", "0"),
            ("DBT_ENGINE_EXPERIMENTAL_GRADUATED_FLAG", "0"),
            ("DBT_ENGINE_EXPERIMENTAL_ABANDONED_FLAG", "1"),
        ])
        .cli(["graduated_flag", "abandoned_flag"])
        .build();
        assert_eq!(
            snapshot.resolve(&GRADUATED, &Project(&[("graduated_flag", false)], &[])),
            Resolved::new(true, Source::Retired)
        );
        assert_eq!(
            snapshot.resolve(&ABANDONED, &Project(&[], &[("abandoned_flag", Some(true))])),
            Resolved::new(false, Source::Retired)
        );
        let warnings = snapshot.warnings(&[&GRADUATED, &ABANDONED]);
        assert_eq!(
            warnings,
            [
                "Feature 'graduated_flag' has graduated and is always on; remove it from \
                 --beta-features",
                "Feature 'abandoned_flag' was abandoned and is always off; remove it from \
                 --beta-features",
                "Feature 'graduated_flag' has graduated and is always on; unset \
                 DBT_ENGINE_EXPERIMENTAL_GRADUATED_FLAG",
                "Feature 'abandoned_flag' was abandoned and is always off; unset \
                 DBT_ENGINE_EXPERIMENTAL_ABANDONED_FLAG",
            ]
        );
    }

    #[test]
    fn warnings_report_unknown_and_malformed_settings() {
        let snapshot = builder(&[(BETA_FEATURES_ENV, "nope,beta_flag=false")])
            .cli(["also_nope", "experimental_flag"])
            .user_settings([("not_a_registry_flag", true)])
            .remote("future_flag", true, false)
            .build();
        let warnings = snapshot.warnings(&[&EXPERIMENTAL, &BETA]);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(&format!("'beta_flag=false' in {BETA_FEATURES_ENV}")))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("'also_nope' in --beta-features"))
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains(&format!("'nope' in {BETA_FEATURES_ENV}")))
        );
        assert!(warnings.iter().any(|w| w
            == "'experimental_flag' in --beta-features has no effect: it is not a beta feature"));
    }

    #[test]
    fn the_project_turns_a_beta_flag_off_below_the_lists() {
        let project_false = Project(&[], &[("beta_flag", Some(false))]);

        let snapshot = builder(&[]).user_settings([("beta_flag", true)]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &project_false),
            Resolved::new(false, Source::Project)
        );

        let snapshot = builder(&[]).cli(["beta_flag"]).build();
        assert_eq!(
            snapshot.resolve(&BETA, &project_false),
            Resolved::new(true, Source::Cli)
        );
    }

    #[test]
    fn project_warnings_report_entries_of_beta_features_with_no_effect() {
        let project = Project(
            &[],
            &[
                ("beta_flag", Some(true)),
                ("Nope", Some(true)),
                ("listed", None),
            ],
        );
        assert_eq!(
            snapshot_resolves_listed_as_off(&project),
            Resolved::new(false, Source::Default)
        );
        assert_eq!(
            FlagSnapshot::project_warnings(&[&BETA], &project),
            [
                "Unknown beta feature 'nope' in `beta_features` in the `flags:` of \
                 dbt_project.yml; it has no effect",
                "Ignoring 'listed' in `beta_features` in the `flags:` of dbt_project.yml: set \
                 it as `listed: true`",
            ]
        );
    }

    /// Resolves a beta flag named `listed`, which the project names without a boolean.
    fn snapshot_resolves_listed_as_off(project: &Project) -> Resolved {
        const LISTED: FlagDef = FlagDef {
            name: "listed",
            aliases: &[],
            ..BETA
        };
        builder(&[]).build().resolve(&LISTED, project)
    }

    #[test]
    fn installed_snapshot_is_current() {
        static FLAG: FlagDef = FlagDef {
            name: "installed_test_flag",
            legacy_env: &[],
            aliases: &[],
            ..BETA
        };
        install(builder(&[]).cli(["installed_test_flag"]).build());
        assert!(FLAG.enabled());
        install(builder(&[]).build());
        assert!(!FLAG.enabled());
    }
}
