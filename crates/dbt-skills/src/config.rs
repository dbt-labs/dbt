//! Resolving `enabled` for a skill.
//!
//! This is the shared helper the spec's risk R1 calls for: the deps-time install
//! pass and (in a later phase) the parser must resolve `enabled` identically, so
//! both go through here. It reuses the same FQN config machinery every other
//! resource type uses, with the same precedence — a package's own `skills:`
//! block is the base and the root project's `skills:` block overrides it.

use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_common::{FsResult, fs_err};
use dbt_schemas::schemas::project::{
    DbtProjectConfig, ProjectConfigResolver, ResolvedConfig, SkillConfig, init_project_config,
};

use crate::discover::{DiscoveredSkill, SkillSourceProject};

/// The adapter handed to the shared config resolver, which requires one.
///
/// Resolving skill config does not depend on the adapter. `SkillConfig` has no
/// adapter-specific keys, so it inherits the no-op
/// `ResolvableConfig::canonicalize_adapter_aliases` — the only path by which
/// this value reaches config-tree resolution. The one other consumer,
/// `authored_quoting_per_adapter`, is reached solely through
/// `build_root_project_configs`, which this pass never calls.
///
/// So the value is arbitrary, and it has to be *something*: `AdapterType` has no
/// neutral variant. It cannot be the project's real adapter either, because this
/// pass runs during `dbt deps`, before any profile has been read. `Snowflake` is
/// the value the rest of the codebase already reaches for when it needs an
/// arbitrary default adapter, so this follows that convention rather than
/// implying skills mean anything Snowflake-specific.
///
/// `skill_config_resolution_ignores_the_adapter` keeps this assumption honest,
/// checking every `AdapterType` variant so a newly added adapter cannot quietly
/// break the assumption.
const CONFIG_RESOLVER_ADAPTER: AdapterType = AdapterType::Snowflake;

/// Resolve `enabled` for every discovered skill and drop the disabled ones.
///
/// `projects` must be the same slice, in the same order, that produced
/// `skills`; `precedence` indexes into it. `disallow_plus_prefix` comes from the
/// root project's behavior flags, the same way the parser sources it, so a
/// `+`-prefixed resource path is treated identically here and at parse time.
pub fn filter_enabled(
    skills: Vec<DiscoveredSkill>,
    projects: &[SkillSourceProject],
    disallow_plus_prefix: bool,
) -> FsResult<Vec<DiscoveredSkill>> {
    let root_project = projects.first().ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "Cannot resolve skill config without a root project"
        )
    })?;
    let root_config: DbtProjectConfig<SkillConfig> = init_project_config(
        &root_project.skills_config,
        (),
        None,
        disallow_plus_prefix,
        CONFIG_RESOLVER_ADAPTER,
    )?;

    // One resolver per source project, built once and reused across its skills.
    // Indexed positionally by `precedence`, which is the project's slice index.
    let mut resolvers: Vec<ProjectConfigResolver<SkillConfig>> = Vec::with_capacity(projects.len());
    for (index, project) in projects.iter().enumerate() {
        let resolver = if index == 0 {
            ProjectConfigResolver::for_root(root_config.clone(), CONFIG_RESOLVER_ADAPTER)
        } else {
            let local = init_project_config(
                &project.skills_config,
                (),
                Some(project.package_name.as_str()),
                disallow_plus_prefix,
                CONFIG_RESOLVER_ADAPTER,
            )?;
            ProjectConfigResolver::for_dependency(
                local,
                root_config.clone(),
                CONFIG_RESOLVER_ADAPTER,
            )
        };
        resolvers.push(resolver);
    }

    let mut enabled = Vec::new();
    for skill in skills {
        let Some(resolver) = resolvers.get(skill.precedence) else {
            continue;
        };
        if resolver
            .resolve_with_configs(&skill.fqn, &skill.fqn, &[])
            .enabled()
        {
            enabled.push(skill);
        }
    }

    Ok(enabled)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::SkillOrigin;
    use dbt_schemas::schemas::project::ProjectSkillConfig;
    use std::path::PathBuf;

    fn skill(name: &str, precedence: usize, origin: SkillOrigin) -> DiscoveredSkill {
        DiscoveredSkill {
            name: name.to_string(),
            dir: PathBuf::from(format!("/tmp/{name}")),
            source_path: PathBuf::from(format!("skills/{name}")),
            fqn: vec![
                origin.package_name().unwrap_or("root_project").to_string(),
                name.to_string(),
            ],
            origin,
            precedence,
        }
    }

    fn package(name: &str) -> SkillOrigin {
        SkillOrigin::Package {
            name: name.to_string(),
            version: None,
        }
    }

    fn source_project(name: &str, skills_yaml: Option<&str>) -> SkillSourceProject {
        SkillSourceProject {
            root: PathBuf::from("/tmp"),
            package_name: name.to_string(),
            version: None,
            origin_is_project: name == "root_project",
            skill_paths: vec!["skills".to_string()],
            skills_config: skills_yaml.map(|yaml| crate::yaml::from_str(yaml).unwrap()),
        }
    }

    #[test]
    fn skills_are_enabled_by_default() {
        let projects = [source_project("root_project", None)];
        let skills = vec![skill("alpha", 0, SkillOrigin::Project)];
        assert_eq!(filter_enabled(skills, &projects, false).unwrap().len(), 1);
    }

    #[test]
    fn the_root_project_can_disable_its_own_skill() {
        let projects = [source_project(
            "root_project",
            Some("root_project:\n  alpha:\n    +enabled: false\n"),
        )];
        let skills = vec![
            skill("alpha", 0, SkillOrigin::Project),
            skill("beta", 0, SkillOrigin::Project),
        ];
        let enabled = filter_enabled(skills, &projects, false).unwrap();
        assert_eq!(enabled.len(), 1);
        assert_eq!(enabled[0].name, "beta");
    }

    #[test]
    fn the_root_project_can_disable_a_whole_package() {
        let projects = [
            source_project("root_project", Some("transitive_pkg:\n  +enabled: false\n")),
            source_project("transitive_pkg", None),
        ];
        let skills = vec![skill("bloat", 1, package("transitive_pkg"))];
        assert!(filter_enabled(skills, &projects, false).unwrap().is_empty());
    }

    #[test]
    fn the_root_project_overrides_a_packages_own_config() {
        // The package disables its skill; the consuming project turns it back on.
        let projects = [
            source_project(
                "root_project",
                Some("some_pkg:\n  useful:\n    +enabled: true\n"),
            ),
            source_project(
                "some_pkg",
                Some("some_pkg:\n  useful:\n    +enabled: false\n"),
            ),
        ];
        let skills = vec![skill("useful", 1, package("some_pkg"))];
        assert_eq!(filter_enabled(skills, &projects, false).unwrap().len(), 1);
    }

    #[test]
    fn a_package_can_disable_its_own_skill() {
        let projects = [
            source_project("root_project", None),
            source_project(
                "some_pkg",
                Some("some_pkg:\n  internal:\n    +enabled: false\n"),
            ),
        ];
        let skills = vec![skill("internal", 1, package("some_pkg"))];
        assert!(filter_enabled(skills, &projects, false).unwrap().is_empty());
    }

    /// `filter_enabled` has to hand the shared resolver an adapter it does not
    /// need (see `CONFIG_RESOLVER_ADAPTER`). That is only sound while
    /// `SkillConfig` stays adapter-independent. If someone gives it a
    /// dialect-aliased key -- or implements `canonicalize_adapter_aliases` for
    /// it -- skills would start resolving differently depending on an arbitrary
    /// constant, silently. This fails instead.
    #[test]
    fn skill_config_resolution_ignores_the_adapter() {
        let yaml = "root_project:\n  alpha:\n    +enabled: false\n  beta:\n    +enabled: true\n";
        let project_config: Option<ProjectSkillConfig> = Some(crate::yaml::from_str(yaml).unwrap());

        let resolve = |adapter: AdapterType| {
            let root: DbtProjectConfig<SkillConfig> =
                init_project_config(&project_config, (), None, false, adapter).unwrap();
            let resolver = ProjectConfigResolver::for_root(root, adapter);
            ["alpha", "beta"].map(|name| {
                let fqn = vec!["root_project".to_string(), name.to_string()];
                resolver.resolve_with_configs(&fqn, &fqn, &[]).enabled()
            })
        };

        // Sanity check that the fixture actually distinguishes the two skills,
        // so the comparison below cannot pass by resolving nothing.
        assert_eq!(resolve(CONFIG_RESOLVER_ADAPTER), [false, true]);

        // Every adapter, not a hand-picked few: a new variant is covered
        // automatically, so the arbitrary constant stays provably safe.
        for (adapter, name) in AdapterType::iter_with_names() {
            assert_eq!(
                resolve(adapter),
                resolve(CONFIG_RESOLVER_ADAPTER),
                "skill config resolved differently for {name}"
            );
        }
    }
}
