//! The registry of flags. Add new flags here and to [`ALL`].

use crate::def::{FlagDef, Remote, Stage};

/// Host-specific fast paths (GitHub archive download + GraphQL ref resolution)
/// for git packages, falling back to `git clone` when they fail.
pub static GIT_DEPS_FAST_PATH: FlagDef = FlagDef {
    name: "git_deps_fast_path",
    description: "Use host-specific fast paths (GitHub archive download and GraphQL ref \
                  resolution) for git packages, falling back to git clone.",
    stage: Stage::Internal,
    default: false,
    remote: Remote::Default,
    aliases: &[],
    legacy_env: &["DBT_DEPS_GIT_FAST_PATH"],
    tracking: None,
    remove_by: None,
    docs_url: None,
};

/// Whether an unqualified `ref()` / `source()` / `function()` from a node inside
/// an installed package searches that package before the root project.
pub static REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT: FlagDef = FlagDef {
    name: "require_ref_searches_node_package_before_root",
    description: "Resolve unqualified ref(), source(), and function() calls from a package \
                  node in that package before the root project.",
    stage: Stage::Behavior,
    default: true,
    remote: Remote::Never,
    aliases: &[],
    legacy_env: &["DBT_ENGINE_REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT"],
    tracking: None,
    remove_by: None,
    docs_url: None,
};

/// Whether an unpinned `ref()` to a versioned model resolves to its latest version.
pub static LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT: FlagDef = FlagDef {
    name: "latest_version_pointer_enabled_by_default",
    description: "Resolve unpinned ref() calls to a versioned model through its latest \
                  version pointer.",
    stage: Stage::Behavior,
    default: true,
    remote: Remote::Never,
    aliases: &[],
    legacy_env: &["DBT_LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT"],
    tracking: None,
    remove_by: None,
    docs_url: None,
};

/// Load the SQL frontend from a dynamic library instead of the built-in one.
pub static FRONTEND_LIBRARY: FlagDef = FlagDef {
    name: "frontend_library",
    description: "Load the SQL frontend from a dynamic library instead of the built-in one.",
    stage: Stage::Internal,
    default: false,
    remote: Remote::Never,
    aliases: &[],
    legacy_env: &[],
    tracking: None,
    remove_by: None,
    docs_url: None,
};

/// Every flag declared in this crate.
pub static ALL: &[&FlagDef] = &[
    &GIT_DEPS_FAST_PATH,
    &REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT,
    &LATEST_VERSION_POINTER_ENABLED_BY_DEFAULT,
    &FRONTEND_LIBRARY,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_version(version: &str) -> (u64, u64, u64) {
        let core = version.split(['-', '+']).next().unwrap_or(version);
        let mut parts = core.split('.').map(|p| {
            p.parse::<u64>()
                .unwrap_or_else(|_| panic!("invalid version '{version}'"))
        });
        let mut next = || parts.next().unwrap_or(0);
        (next(), next(), next())
    }

    #[test]
    fn no_flag_is_past_its_remove_by_release() {
        let current = parse_version(env!("CARGO_PKG_VERSION"));
        for def in ALL {
            if let Some(remove_by) = def.remove_by {
                assert!(
                    current < parse_version(remove_by),
                    "flag '{}' was due to be removed or graduated by {remove_by} \
                     (current version {})",
                    def.name,
                    env!("CARGO_PKG_VERSION")
                );
            }
        }
    }

    #[test]
    fn names_are_snake_case_and_unique() {
        let mut seen = Vec::new();
        for def in ALL {
            assert!(
                !def.description.is_empty(),
                "'{}' needs a description",
                def.name
            );
            for name in def.names() {
                assert!(
                    !name.is_empty()
                        && name
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                    "flag name '{name}' must be snake_case"
                );
                assert!(
                    !seen
                        .iter()
                        .any(|other: &&str| crate::def::names_equal(other, name)),
                    "flag name '{name}' is declared twice"
                );
                seen.push(name);
            }
        }
    }

    #[test]
    fn parse_version_ignores_prerelease() {
        assert_eq!(parse_version("2.0.7"), (2, 0, 7));
        assert_eq!(parse_version("2.1.0-beta.3"), (2, 1, 0));
        assert!(parse_version("2.0.7") < parse_version("2.1.0"));
    }
}
