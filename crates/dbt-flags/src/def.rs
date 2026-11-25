use crate::snapshot::{Resolved, current};

/// Lifecycle stage of a flag. Controls where it can be set and how it is surfaced.
///
/// A flag that changes what an existing project does moves `Experimental` -> `Beta` ->
/// `Behavior` -> `Graduated`. Any other flag skips `Behavior`. A flag at any stage can end
/// `Abandoned` instead. A `Graduated` or `Abandoned` flag is removed by deleting it from the
/// catalog, once users have had time to drop it from their settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// In development, or rolling out to a few users. Off by default and not documented for
    /// users. Set only with the flag's environment variable ([`FlagDef::env_var`]).
    Experimental,
    /// Ready for testing but still opt-in, with no stability promise: it may change or be
    /// abandoned in any release. Listed in `--help`. Also set with `--beta-features`,
    /// `DBT_ENGINE_BETA_FEATURES`, and `beta_features: {<name>: true}` in the root project's
    /// `flags:`.
    Beta,
    /// Public behavior change flag, set as `<name>: <bool>` in the root project's
    /// `dbt_project.yml` `flags:`. The settings of a beta feature still apply, with a warning,
    /// so a project that turned the feature on keeps it when the flag is promoted.
    Behavior,
    /// The feature is always on. The flag is still accepted, with a warning, so existing
    /// settings keep working.
    Graduated,
    /// The feature is gone and the flag is always off. The flag is still accepted, with a
    /// warning, so existing settings keep working.
    Abandoned,
}

impl Stage {
    /// The value every flag at this stage is locked to, if the stage locks it.
    pub const fn locked(self) -> Option<bool> {
        match self {
            Stage::Graduated => Some(true),
            Stage::Abandoned => Some(false),
            Stage::Experimental | Stage::Beta | Stage::Behavior => None,
        }
    }
}

/// What the root project's `dbt_project.yml` `flags:` sets. Project flags are global: never
/// read a package's `flags:` here.
pub trait ProjectFlags {
    /// The boolean at `flags: {<name>: <bool>}`.
    fn value(&self, name: &str) -> Option<bool>;

    /// Calls `visit` with each entry of `flags: {beta_features: {<name>: <bool>}}`: its name, and
    /// its value if that is a boolean.
    fn beta_features(&self, visit: &mut dyn FnMut(&str, Option<bool>));
}

/// A project that sets no flags.
pub struct NoProjectFlags;

impl ProjectFlags for NoProjectFlags {
    fn value(&self, _: &str) -> Option<bool> {
        None
    }

    fn beta_features(&self, _: &mut dyn FnMut(&str, Option<bool>)) {}
}

/// Whether a remote (platform-evaluated) value may affect a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remote {
    /// Remote values are ignored.
    Never,
    /// A remote value replaces the code default; any local setting wins over it.
    Default,
    /// Like [`Remote::Default`], and a remote value marked `force` wins over
    /// every local setting. Reserved for kill switches.
    Force,
}

/// Declaration of a single boolean feature flag.
#[derive(Debug)]
pub struct FlagDef {
    /// Canonical `snake_case` name, used in the flag's environment variable, on the CLI, in
    /// env lists, in `flags:` blocks, and in remote payloads.
    pub name: &'static str,
    pub description: &'static str,
    pub stage: Stage,
    /// The value when nothing sets the flag. `false` for an experimental or beta flag.
    pub default: bool,
    pub remote: Remote,
    /// Names this flag was previously known by. Every setting that matches an
    /// alias resolves to this flag, so a flag can be renamed as it graduates.
    pub aliases: &'static [&'static str],
    /// Environment variables that predate this registry and still set the
    /// flag, beside [`FlagDef::env_var`]. Unparseable values are ignored, as they were before.
    pub legacy_env: &'static [&'static str],
    /// Issue tracking the flag's rollout and eventual removal.
    pub tracking: Option<&'static str>,
    /// Release (`MAJOR.MINOR.PATCH`) by which the flag must be graduated,
    /// abandoned, or removed. A registry test fails once the crate version reaches it.
    pub remove_by: Option<&'static str>,
    pub docs_url: Option<&'static str>,
}

impl FlagDef {
    /// The canonical name followed by every alias.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }

    /// Whether `name` refers to this flag. Matching ignores ASCII case and
    /// treats `-` as `_`, so `--beta-features metricflow-v2` works.
    pub fn matches(&self, name: &str) -> bool {
        self.names().any(|candidate| names_equal(candidate, name))
    }

    /// The environment variable that sets this flag: `DBT_ENGINE_EXPERIMENTAL_<NAME>`. It keeps
    /// its name at every stage, so a setting survives the flag's promotion.
    pub fn env_var(&self) -> String {
        env_var_for(self.name)
    }

    /// [`FlagDef::env_var`], followed by the same variable for every alias.
    pub fn env_vars(&self) -> impl Iterator<Item = String> {
        self.names().map(env_var_for)
    }

    /// The flag's value for the current invocation, ignoring `dbt_project.yml`.
    ///
    /// Use [`FlagDef::resolve_with_project`] for flags users can set in a project.
    pub fn enabled(&'static self) -> bool {
        current().resolve(self, &NoProjectFlags).value
    }

    /// Resolves the flag for the current invocation, consulting the root project's `flags:`
    /// through `project`.
    pub fn resolve_with_project(&'static self, project: &dyn ProjectFlags) -> Resolved {
        current().resolve(self, project)
    }
}

fn env_var_for(name: &str) -> String {
    format!("DBT_ENGINE_EXPERIMENTAL_{}", name.to_ascii_uppercase())
}

pub(crate) fn names_equal(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .all(|(x, y)| normalize_byte(x) == normalize_byte(y))
}

pub(crate) fn normalize_name(name: &str) -> String {
    name.trim()
        .bytes()
        .map(|b| normalize_byte(b) as char)
        .collect()
}

fn normalize_byte(b: u8) -> u8 {
    if b == b'-' {
        b'_'
    } else {
        b.to_ascii_lowercase()
    }
}
