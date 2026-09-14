//! Installing agent skills (AgentSkills-format `SKILL.md` directories) that
//! ship with dbt packages.
//!
//! Skills are read from the root project's `skill-paths` and from every
//! installed package's own `skill-paths`, then copied flat into the directory
//! each configured `ai_provider` reads from. This runs at package-install time,
//! before any manifest exists, so everything here works from files on disk
//! alone — no profile, no warehouse connection, no parsed nodes.

pub mod config;
pub mod discover;
pub mod hash;
pub mod install;
pub mod metadata;
pub mod providers;
pub mod validate;
pub mod yaml;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult, fs_err, stdfs};
use dbt_schemas::schemas::project::{
    DbtProject, ProjectSkillConfig, disallow_plus_prefix_from_flags,
};

use crate::config::filter_enabled;
use crate::discover::{DiscoveredSkill, SkillSourceProject, discover_skills};
#[cfg(test)]
use crate::install::InstallOutcome;
use crate::install::{InstallReport, install_skills, prune_all};
use crate::providers::{AiProvider, parse_providers, resolve_ai_provider, resolve_destinations};

pub use crate::providers::{CLAUDE_SKILLS_DIR, DEFAULT_SKILLS_DIR};

/// The subset of a `dbt_project.yml` the skill pass needs.
///
/// Deliberately minimal and tolerant: package `dbt_project.yml` files are read
/// straight off disk here, without the profile, Jinja context or validation the
/// full project loader applies. Unknown keys are collected and ignored.
#[derive(Debug, Default, serde::Deserialize)]
struct SkillProjectView {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "skill-paths")]
    skill_paths: Option<Vec<String>>,
    #[serde(default)]
    skills: Option<ProjectSkillConfig>,
}

/// A package to scan, in collision-precedence order.
pub struct InstalledPackage {
    /// Directory the package was installed into.
    pub root: PathBuf,
    /// Package name as it appears in `package-lock.yml`.
    pub name: String,
    pub version: Option<String>,
}

/// Read the root project and each installed package, and install every enabled
/// skill into every directory the configured providers read from, pruning any
/// skill dbt previously installed there that is no longer wanted.
///
/// Returns `Ok(None)` when there is nothing to install to: no `ai_provider` at
/// all (with a warning, if there were skills to install), or none of the
/// configured providers is recognized. Otherwise the pass runs even with nothing
/// to install, so orphaned dbt installs are still pruned.
///
/// This owns the whole "what fails `dbt deps`" policy so callers don't have to:
/// the only error it returns is an unresolved name collision
/// ([`ErrorCode::SkillNameCollision`]), which the user must resolve the way they
/// resolve duplicate model names. Every other problem is a warning and is
/// skipped — an invalid `SKILL.md`, and any I/O failure writing or pruning
/// skills, must not break `dbt deps`.
pub fn install_package_skills(
    project_root: &Path,
    root_project: &DbtProject,
    packages: &[InstalledPackage],
    ai_provider: Option<&[String]>,
) -> FsResult<Option<Vec<InstallReport>>> {
    let mut projects = vec![SkillSourceProject::from_dbt_project(
        project_root,
        root_project,
        true,
    )];
    for package in packages {
        projects.push(read_package_project(package));
    }

    let discovered = discover_skills(&projects);
    let resolved_providers = resolve_ai_provider(ai_provider, root_project.flags.as_ref());

    let Some(raw_providers) = resolved_providers else {
        // Only nag about a missing provider when there was something to install;
        // a project with no skills at all has nothing to say here.
        if !discovered.is_empty() {
            emit_warn_log_message(
                ErrorCode::AiProviderUnset,
                format!(
                    "Found {} agent skill(s) in this project and its packages, but 'ai_provider' \
                     is not set, so none were installed. Set it in dbt_project.yml (flags: \
                     {{ai_provider: claude}}), via --ai-provider, or with DBT_AI_PROVIDER. Known \
                     providers: {}.",
                    discovered.len(),
                    AiProvider::all_names()
                ),
            );
        }
        return Ok(None);
    };

    // Unknown provider names have already warned on their own behalf; if none
    // survive there is nowhere to install to.
    let destinations = resolve_destinations(&parse_providers(&raw_providers));
    if destinations.is_empty() {
        return Ok(None);
    }

    // Run the pass even with nothing discovered: dbt prunes what it recorded
    // metadata for, so a skill that has disappeared from its source must be
    // removed from the destinations on this run. `filter_enabled` on an empty
    // set is a no-op. A malformed `skills:` config is a content problem, not a
    // collision, so it warns and skips rather than failing `dbt deps`.
    let enabled = match filter_enabled(
        discovered,
        &projects,
        disallow_plus_prefix_from_flags(root_project.flags.as_ref()),
    ) {
        Ok(enabled) => enabled,
        Err(e) => {
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!("Skipping agent skills: could not resolve `skills:` config: {e}"),
            );
            return Ok(None);
        }
    };

    // Skills install under their own name, so two enabled skills that share a
    // name would land in the same directory. dbt does not silently pick a
    // winner: like duplicate model names, this is the user's to resolve, so it
    // is a hard error before anything is written — the one problem this pass
    // propagates rather than warning about.
    reject_name_collisions(&enabled)?;

    // Writing and pruning are best-effort: an I/O failure here must not break
    // `dbt deps`, so it warns and the pass reports nothing installed.
    match install_skills(project_root, &destinations, &enabled) {
        Ok(reports) => Ok(Some(reports)),
        Err(e) => {
            emit_warn_log_message(
                ErrorCode::IoError,
                format!("Could not install agent skills: {e}"),
            );
            Ok(None)
        }
    }
}

/// Fail if two enabled skills resolve to the same install name.
///
/// Without namespacing, a name is a skill's identity in the destination
/// directory, so a duplicate would have one skill overwrite another. dbt treats
/// this the way it treats duplicate model names: it refuses to proceed and asks
/// the user (or a package maintainer) to make the names unique. The
/// `skills:`-config `+enabled: false` toggle is how a user disables all but one.
fn reject_name_collisions(skills: &[DiscoveredSkill]) -> FsResult<()> {
    let mut by_name: BTreeMap<&str, Vec<&DiscoveredSkill>> = BTreeMap::new();
    for skill in skills {
        by_name.entry(&skill.name).or_default().push(skill);
    }

    for (name, colliding) in &by_name {
        if colliding.len() < 2 {
            continue;
        }
        let sources = colliding
            .iter()
            .map(|skill| match skill.origin.package_name() {
                Some(package) => format!("package '{package}'"),
                None => "this project".to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(fs_err!(
            ErrorCode::SkillNameCollision,
            "Two or more agent skills are named '{name}' ({sources}), so they would install into \
             the same directory. Skill names must be unique across your project and its packages. \
             Disable all but one in `skills:` config, keyed by the package (or project) that ships \
             it — for example `<source>: {{ {name}: {{ +enabled: false }} }}` — or ask the package \
             maintainer to rename it."
        ));
    }
    Ok(())
}

/// Remove every skill dbt previously installed, across the destinations the
/// configured providers read from. Used by `dbt clean`.
///
/// Reads only files on disk — no packages need be installed. Returns
/// `Ok(None)` when no `ai_provider` is configured (nothing dbt could have
/// installed, so nothing to prune). Skills dbt does not manage are never
/// touched.
pub fn prune_installed_skills(
    project_root: &Path,
    root_project: &DbtProject,
    ai_provider: Option<&[String]>,
) -> FsResult<Option<Vec<InstallReport>>> {
    let Some(raw_providers) = resolve_ai_provider(ai_provider, root_project.flags.as_ref()) else {
        return Ok(None);
    };
    let destinations = resolve_destinations(&parse_providers(&raw_providers));
    if destinations.is_empty() {
        return Ok(None);
    }
    Ok(Some(prune_all(project_root, &destinations)?))
}

fn read_package_project(package: &InstalledPackage) -> SkillSourceProject {
    let view = stdfs::read_to_string(package.root.join("dbt_project.yml"))
        .ok()
        .and_then(|contents| yaml::from_str::<SkillProjectView>(&contents).ok())
        .unwrap_or_default();

    // Prefer the project's declared name — that's what `skills:` config and
    // FQNs key off — falling back to the lock entry's name.
    let package_name = if view.name.is_empty() {
        package.name.clone()
    } else {
        view.name
    };

    SkillSourceProject::new(
        package.root.clone(),
        package_name,
        package.version.clone(),
        false,
        view.skill_paths,
        view.skills,
    )
}

#[cfg(test)]
fn count(reports: &[InstallReport], outcome: InstallOutcome) -> usize {
    reports.iter().filter(|r| r.outcome == outcome).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::validate::SKILL_FILE;
    use std::fs;
    use tempfile::TempDir;

    fn write_skill(root: &Path, rel: &str, name: &str) {
        let dir = root.join(rel);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: A skill.\n---\n"),
        )
        .unwrap();
    }

    fn root_project(name: &str) -> DbtProject {
        DbtProject {
            name: name.to_string(),
            ..Default::default()
        }
    }

    fn write_package(root: &Path, name: &str, project_yml: &str) -> InstalledPackage {
        let package_root = root.join("dbt_packages").join(name);
        fs::create_dir_all(&package_root).unwrap();
        fs::write(package_root.join("dbt_project.yml"), project_yml).unwrap();
        InstalledPackage {
            root: package_root,
            name: name.to_string(),
            version: Some("1.0.0".to_string()),
        }
    }

    #[test]
    fn installs_project_and_package_skills_into_the_provider_dir() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_skill(root, "skills/from-project", "from-project");

        let package = write_package(root, "some_pkg", "name: some_pkg\n");
        write_skill(&package.root, "skills/from-package", "from-package");

        let reports = install_package_skills(
            root,
            &root_project("root_project"),
            &[package],
            Some(&["claude".to_string()]),
        )
        .unwrap()
        .unwrap();

        // Each skill installs under its own name, in the provider's directory.
        assert_eq!(count(&reports, InstallOutcome::Installed), 2);
        assert!(
            root.join(".claude/skills/from-project")
                .join(SKILL_FILE)
                .is_file()
        );
        assert!(
            root.join(".claude/skills/from-package")
                .join(SKILL_FILE)
                .is_file()
        );
    }

    #[test]
    fn skills_present_but_no_provider_installs_nothing() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_skill(root, "skills/alpha", "alpha");

        let result =
            install_package_skills(root, &root_project("root_project"), &[], None).unwrap();
        assert!(result.is_none());
        assert!(!root.join(DEFAULT_SKILLS_DIR).exists());
    }

    #[test]
    fn no_skills_and_no_provider_does_nothing() {
        let tmp = TempDir::new().unwrap();
        let result =
            install_package_skills(tmp.path(), &root_project("root_project"), &[], None).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn a_package_honors_its_own_skill_paths() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let package = write_package(
            root,
            "some_pkg",
            "name: some_pkg\nskill-paths: [\"agent-skills\"]\n",
        );
        write_skill(&package.root, "agent-skills/custom", "custom");

        install_package_skills(
            root,
            &root_project("root_project"),
            &[package],
            Some(&["wizard".to_string()]),
        )
        .unwrap()
        .unwrap();

        assert!(
            root.join(DEFAULT_SKILLS_DIR)
                .join("custom")
                .join(SKILL_FILE)
                .is_file()
        );
    }

    #[test]
    fn ai_provider_can_come_from_project_flags() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_skill(root, "skills/alpha", "alpha");

        let mut project = root_project("root_project");
        project.flags = Some(yaml::from_str("ai_provider: claude").unwrap());

        install_package_skills(root, &project, &[], None)
            .unwrap()
            .unwrap();
        assert!(root.join(".claude/skills/alpha").join(SKILL_FILE).is_file());
    }

    #[test]
    fn a_disabled_skill_is_not_installed() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_skill(root, "skills/alpha", "alpha");
        write_skill(root, "skills/beta", "beta");

        let mut project = root_project("root_project");
        project.skills =
            Some(yaml::from_str("root_project:\n  alpha:\n    +enabled: false\n").unwrap());

        install_package_skills(root, &project, &[], Some(&["wizard".to_string()]))
            .unwrap()
            .unwrap();

        assert!(!root.join(DEFAULT_SKILLS_DIR).join("alpha").exists());
        assert!(root.join(DEFAULT_SKILLS_DIR).join("beta").is_dir());
    }

    #[test]
    fn a_same_named_skill_from_two_sources_is_a_hard_error() {
        // Two skills named `shared` would install into the same directory. dbt
        // refuses rather than picking a winner, and writes nothing.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        write_skill(root, "skills/shared", "shared");

        let package = write_package(root, "some_pkg", "name: some_pkg\n");
        write_skill(&package.root, "skills/shared", "shared");

        let err = install_package_skills(
            root,
            &root_project("root_project"),
            &[package],
            Some(&["wizard".to_string()]),
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::SkillNameCollision);
        assert!(!root.join(DEFAULT_SKILLS_DIR).join("shared").exists());
    }

    #[test]
    fn a_transitive_package_can_be_disabled_by_name() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        let package = write_package(root, "transitive_pkg", "name: transitive_pkg\n");
        write_skill(&package.root, "skills/bloat", "bloat");

        let mut project = root_project("root_project");
        project.skills = Some(yaml::from_str("transitive_pkg:\n  +enabled: false\n").unwrap());

        install_package_skills(root, &project, &[package], Some(&["wizard".to_string()]))
            .unwrap()
            .unwrap();

        assert!(!root.join(DEFAULT_SKILLS_DIR).join("bloat").exists());
    }

    #[test]
    fn a_package_cannot_escape_its_directory_with_a_relative_skill_path() {
        // A compromised package pointing skill-paths above itself must not make
        // dbt copy files from outside the package into the user's project.
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path();
        write_skill(outside, "private/exfiltrated", "exfiltrated");

        let root = outside.join("proj");
        fs::create_dir_all(&root).unwrap();
        let package = write_package(
            &root,
            "evil",
            "name: evil\nskill-paths: [\"../../../private\"]\n",
        );

        install_package_skills(
            &root,
            &root_project("root_project"),
            &[package],
            Some(&["wizard".to_string()]),
        )
        .unwrap();

        assert!(!root.join(DEFAULT_SKILLS_DIR).join("exfiltrated").exists());
    }

    #[test]
    #[cfg(unix)]
    fn a_package_cannot_escape_its_directory_with_a_symlinked_skill_path() {
        // Same attack without a `..` anywhere: skill-paths names a plain
        // directory that happens to be a symlink out of the package.
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path();
        write_skill(outside, "private/via-symlink", "via-symlink");

        let root = outside.join("proj");
        fs::create_dir_all(&root).unwrap();
        let package = write_package(&root, "evil", "name: evil\nskill-paths: [\"skills\"]\n");
        std::os::unix::fs::symlink(outside.join("private"), package.root.join("skills")).unwrap();

        install_package_skills(
            &root,
            &root_project("root_project"),
            &[package],
            Some(&["wizard".to_string()]),
        )
        .unwrap();

        assert!(!root.join(DEFAULT_SKILLS_DIR).join("via-symlink").exists());
    }

    #[test]
    fn an_escaping_skill_path_does_not_stop_a_well_behaved_one() {
        let tmp = TempDir::new().unwrap();
        let outside = tmp.path();
        write_skill(outside, "private/exfiltrated", "exfiltrated");

        let root = outside.join("proj");
        fs::create_dir_all(&root).unwrap();
        let package = write_package(
            &root,
            "mixed",
            "name: mixed\nskill-paths: [\"../../../private\", \"skills\"]\n",
        );
        write_skill(&package.root, "skills/legitimate", "legitimate");

        install_package_skills(
            &root,
            &root_project("root_project"),
            &[package],
            Some(&["wizard".to_string()]),
        )
        .unwrap();

        assert!(!root.join(DEFAULT_SKILLS_DIR).join("exfiltrated").exists());
        assert!(root.join(DEFAULT_SKILLS_DIR).join("legitimate").is_dir());
    }
}
