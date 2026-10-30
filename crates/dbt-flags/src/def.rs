use crate::snapshot::{Resolved, current};

/// Lifecycle stage of a flag. Controls where it can be set and how it is surfaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Rollout, kill switch, or dark launch. Not settable in `dbt_project.yml`
    /// and not documented for users.
    Internal,
    /// Documented opt-in feature with no stability promise: it may change or be
    /// removed in any release.
    Preview,
    /// Public behavior change flag, settable in the root project's `dbt_project.yml` `flags:`.
    Behavior,
    /// Retired. Still accepted so existing configuration keeps working, but
    /// always resolves to the flag's default.
    Removed,
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
    /// Canonical `snake_case` name, used on the CLI, in env lists, in
    /// `flags:` blocks, and in remote payloads.
    pub name: &'static str,
    pub description: &'static str,
    pub stage: Stage,
    pub default: bool,
    pub remote: Remote,
    /// Names this flag was previously known by. Every setting that matches an
    /// alias resolves to this flag, so a flag can be renamed as it graduates.
    pub aliases: &'static [&'static str],
    /// Environment variables that predate this registry and still set the
    /// flag. Unparseable values are ignored, as they were before.
    pub legacy_env: &'static [&'static str],
    /// Issue tracking the flag's rollout and eventual removal.
    pub tracking: Option<&'static str>,
    /// Release (`MAJOR.MINOR.PATCH`) by which the flag must be removed or
    /// graduated. A registry test fails once the crate version reaches it.
    pub remove_by: Option<&'static str>,
    pub docs_url: Option<&'static str>,
}

impl FlagDef {
    /// The canonical name followed by every alias.
    pub fn names(&self) -> impl Iterator<Item = &'static str> {
        std::iter::once(self.name).chain(self.aliases.iter().copied())
    }

    /// Whether `name` refers to this flag. Matching ignores ASCII case and
    /// treats `-` as `_`, so `--feature git-deps-fast-path` works.
    pub fn matches(&self, name: &str) -> bool {
        self.names().any(|candidate| names_equal(candidate, name))
    }

    /// Whether users may set this flag in the root project's `dbt_project.yml` `flags:`.
    pub fn settable_in_project(&self) -> bool {
        matches!(self.stage, Stage::Preview | Stage::Behavior)
    }

    /// The flag's value for the current invocation, ignoring `dbt_project.yml`.
    ///
    /// Use [`FlagDef::resolve_with_project`] for flags users can set in a project.
    pub fn enabled(&'static self) -> bool {
        current().resolve(self, &|_| None).value
    }

    /// Resolves the flag for the current invocation, consulting the root project's
    /// `flags:` through `project` (called with each of the flag's names). Project flags are
    /// global: never pass a package's `flags:` here.
    pub fn resolve_with_project(&'static self, project: &dyn Fn(&str) -> Option<bool>) -> Resolved {
        current().resolve(self, project)
    }
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
