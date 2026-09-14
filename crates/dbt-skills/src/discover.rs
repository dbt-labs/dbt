//! Finding skills on disk, in the root project and in every installed package.

use std::path::{Path, PathBuf};

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{FsResult, stdfs};
use dbt_schemas::schemas::project::{DEFAULT_SKILL_PATH, DbtProject, ProjectSkillConfig};
use walkdir::WalkDir;

use crate::validate::{SKILL_FILE, validate_skill};

/// Where a skill came from. Recorded in the installed skill's metadata and shown
/// in progress output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillOrigin {
    Project,
    Package {
        name: String,
        version: Option<String>,
    },
}

impl SkillOrigin {
    pub fn package_name(&self) -> Option<&str> {
        match self {
            SkillOrigin::Project => None,
            SkillOrigin::Package { name, .. } => Some(name),
        }
    }

    /// The `(source, package, version)` triple dbt records in an installed
    /// skill's metadata: `"project"` with no package or version, or `"package"`
    /// with the package's name and version. This is the one place the origin is
    /// flattened into metadata's shape.
    pub fn metadata_fields(&self) -> (&'static str, Option<String>, Option<String>) {
        match self {
            SkillOrigin::Project => ("project", None, None),
            SkillOrigin::Package { name, version } => {
                ("package", Some(name.clone()), version.clone())
            }
        }
    }
}

/// A skill directory discovered under some project's `skill-paths`.
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Directory name, which the AgentSkills spec pins to the frontmatter `name`.
    pub name: String,
    /// Absolute path to the skill directory (the parent of its `SKILL.md`).
    pub dir: PathBuf,
    /// Path of the skill directory relative to the owning project root.
    pub source_path: PathBuf,
    pub origin: SkillOrigin,
    /// `[package, ...subpath under skill-paths, name]`, for FQN config lookup.
    pub fqn: Vec<String>,
    /// Index of the owning project in the supplied `projects` slice, used to pick
    /// that project's `enabled` resolver. The root project is always index 0.
    pub precedence: usize,
}

/// One project (root or installed package) to scan for skills.
pub struct SkillSourceProject {
    pub root: PathBuf,
    pub package_name: String,
    pub version: Option<String>,
    pub origin_is_project: bool,
    pub skill_paths: Vec<String>,
    /// The project's own `skills:` config block.
    pub skills_config: Option<ProjectSkillConfig>,
}

impl SkillSourceProject {
    /// Build a source project from its already-resolved parts.
    ///
    /// The single place a `dbt_project.yml`'s skill-relevant fields are mapped
    /// onto this view, so the root project and every package resolve
    /// `skill-paths` (defaulting to `skills/` when unset) and `skills:`
    /// identically. `skill_paths` is `None` when the file did not set it.
    ///
    /// The root arrives as a fully-parsed [`DbtProject`] (the deps run needs its
    /// `flags:` too), while a package arrives through the lenient
    /// [`crate::SkillProjectView`]; both funnel their skill fields through here.
    pub(crate) fn new(
        root: PathBuf,
        package_name: String,
        version: Option<String>,
        origin_is_project: bool,
        skill_paths: Option<Vec<String>>,
        skills_config: Option<ProjectSkillConfig>,
    ) -> Self {
        Self {
            root,
            package_name,
            version,
            origin_is_project,
            skill_paths: skill_paths.unwrap_or_else(|| vec![DEFAULT_SKILL_PATH.to_string()]),
            skills_config,
        }
    }

    /// Build a source project from an already-parsed `dbt_project.yml`.
    pub fn from_dbt_project(root: &Path, project: &DbtProject, origin_is_project: bool) -> Self {
        Self::new(
            root.to_path_buf(),
            project.name.clone(),
            project.version.as_ref().map(|v| v.to_string()),
            origin_is_project,
            project.skill_paths.clone(),
            project.skills.clone(),
        )
    }
}

/// Scan every source project's `skill-paths` for directories containing a
/// `SKILL.md`.
///
/// `projects` must start with the root project (index 0), so its `enabled`
/// config overrides the packages that follow; the order among packages does not
/// matter, since each skill resolves `enabled` against its own package's config
/// merged with the root project's override.
///
/// Discovery never fails the caller. A `SKILL.md` that doesn't validate is
/// reported as a warning and skipped, so one malformed skill in a third-party
/// package can't break `dbt deps` for everything else.
pub fn discover_skills(projects: &[SkillSourceProject]) -> Vec<DiscoveredSkill> {
    let discovered = projects
        .iter()
        .enumerate()
        .flat_map(|(precedence, project)| {
            project
                .skill_paths
                .iter()
                .flat_map(move |skill_path| discover_under(project, precedence, skill_path))
        });

    // Overlapping `skill-paths` within one project (e.g. `skills` and
    // `skills/team`) can reach the same skill directory more than once. That is
    // one skill, not several, so keep only the first sighting of each directory
    // — otherwise a later name-collision check would see a phantom duplicate.
    let mut seen = std::collections::HashSet::new();
    discovered
        .filter(|skill| seen.insert(skill.dir.clone()))
        .collect()
}

/// Skills under a single one of a project's `skill-paths`.
fn discover_under(
    project: &SkillSourceProject,
    precedence: usize,
    skill_path: &str,
) -> Vec<DiscoveredSkill> {
    let Some(search_root) = contained_search_root(project, skill_path) else {
        return Vec::new();
    };

    find_skill_dirs(&search_root)
        .iter()
        .filter_map(|skill_dir| {
            build_skill(project, precedence, skill_path, skill_dir)
                .inspect_err(|e| {
                    emit_warn_log_message(ErrorCode::InvalidSkill, format!("Skipping skill: {e}"))
                })
                .ok()
        })
        .collect()
}

/// Resolve one `skill-paths` entry, refusing anything that escapes the project
/// that declared it.
///
/// `skill-paths` in an installed package's `dbt_project.yml` is untrusted
/// input: a compromised package could set it to `../../..`, or ship its
/// `skills/` directory as a symlink pointing anywhere on the filesystem. Either
/// would make dbt walk outside the package and copy whatever directories it
/// found containing a `SKILL.md` into the user's project. The same rule is
/// applied to the root project, so there is one rule rather than two.
///
/// Containment is checked on the **canonical** paths, so it holds for symlinks
/// as well as `..`. Returns the path to walk, or `None` if it must not be
/// walked. A path that simply doesn't exist is not an error — an unused
/// `skill-paths` default is normal.
fn contained_search_root(project: &SkillSourceProject, skill_path: &str) -> Option<PathBuf> {
    let search_root = project.root.join(skill_path);
    if !search_root.is_dir() {
        return None;
    }

    let (Ok(canonical_root), Ok(canonical_search_root)) = (
        stdfs::canonicalize(&project.root),
        stdfs::canonicalize(&search_root),
    ) else {
        emit_warn_log_message(
            ErrorCode::SkillPathEscapesProject,
            format!(
                "Skipping skill-path '{skill_path}' in '{}': dbt could not resolve it to confirm \
                 it stays inside the project.",
                project.package_name
            ),
        );
        return None;
    };

    if !canonical_search_root.starts_with(&canonical_root) {
        emit_warn_log_message(
            ErrorCode::SkillPathEscapesProject,
            format!(
                "Skipping skill-path '{skill_path}' in '{}': it resolves to {}, which is outside \
                 that project. skill-paths must stay within the project that declares them.",
                project.package_name,
                canonical_search_root.display()
            ),
        );
        return None;
    }

    // Walk the original path, not the canonical one, so paths recorded on each
    // skill stay relative to the project root as configured.
    Some(search_root)
}

/// Directories under `search_root` that directly contain a `SKILL.md`.
///
/// The search recurses, so packages may group their skills into subdirectories;
/// the install itself is always flat (most harnesses don't recurse).
fn find_skill_dirs(search_root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = WalkDir::new(search_root)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file() && entry.file_name() == SKILL_FILE)
        .filter_map(|entry| entry.path().parent().map(Path::to_path_buf))
        .collect();

    // WalkDir order is filesystem-dependent; sort so discovery is reproducible.
    dirs.sort();
    dirs.dedup();
    dirs
}

fn build_skill(
    project: &SkillSourceProject,
    precedence: usize,
    skill_path: &str,
    skill_dir: &Path,
) -> FsResult<DiscoveredSkill> {
    // Validate for its side effects (name/frontmatter checks); only the name is
    // kept — the directory name and skill identity are pinned to it.
    let name = validate_skill(skill_dir)?.name;
    let source_path = stdfs::diff_paths(skill_dir, &project.root)?;

    // FQN mirrors parse-time config lookup: package, then the path components
    // below `skill-paths`, then the skill name.
    let mut fqn = vec![project.package_name.clone()];
    let below_skill_path = skill_dir
        .strip_prefix(project.root.join(skill_path))
        .unwrap_or(skill_dir);
    fqn.extend(
        below_skill_path
            .components()
            .filter_map(|c| c.as_os_str().to_str().map(str::to_string)),
    );

    let origin = if project.origin_is_project {
        SkillOrigin::Project
    } else {
        SkillOrigin::Package {
            name: project.package_name.clone(),
            version: project.version.clone(),
        }
    };

    Ok(DiscoveredSkill {
        name,
        dir: skill_dir.to_path_buf(),
        source_path,
        origin,
        fqn,
        precedence,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::project::DEFAULT_SKILL_PATH;
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

    fn project(root: &Path, name: &str, origin_is_project: bool) -> SkillSourceProject {
        SkillSourceProject {
            root: root.to_path_buf(),
            package_name: name.to_string(),
            version: Some("1.0.0".to_string()),
            origin_is_project,
            skill_paths: vec![DEFAULT_SKILL_PATH.to_string()],
            skills_config: None,
        }
    }

    #[test]
    fn finds_skills_in_the_default_skill_path() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "skills/alpha", "alpha");
        write_skill(tmp.path(), "skills/beta", "beta");

        let found = discover_skills(&[project(tmp.path(), "my_project", true)]);
        let names: Vec<_> = found.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "beta"]);
        assert_eq!(found[0].origin, SkillOrigin::Project);
    }

    #[test]
    fn recurses_into_grouping_subdirectories_and_records_them_in_the_fqn() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "skills/group/nested", "nested");

        let found = discover_skills(&[project(tmp.path(), "my_project", false)]);
        assert_eq!(found.len(), 1);
        assert_eq!(
            found[0].fqn,
            vec![
                "my_project".to_string(),
                "group".to_string(),
                "nested".to_string()
            ]
        );
        assert_eq!(found[0].source_path, PathBuf::from("skills/group/nested"));
    }

    #[test]
    fn a_malformed_skill_is_skipped_without_failing_discovery() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "skills/good", "good");
        let bad = tmp.path().join("skills/bad");
        fs::create_dir_all(&bad).unwrap();
        fs::write(bad.join(SKILL_FILE), "no frontmatter here\n").unwrap();

        let found = discover_skills(&[project(tmp.path(), "my_project", true)]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "good");
    }

    #[test]
    fn overlapping_skill_paths_discover_a_shared_skill_only_once() {
        let tmp = TempDir::new().unwrap();
        write_skill(tmp.path(), "skills/team/nested", "nested");

        // `skills/team/nested` is reachable under both `skills` and
        // `skills/team`; it is one skill, so it must be discovered once.
        let mut project = project(tmp.path(), "my_project", true);
        project.skill_paths = vec!["skills".to_string(), "skills/team".to_string()];

        let found = discover_skills(&[project]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "nested");
    }

    #[test]
    fn a_missing_skill_path_is_not_an_error() {
        let tmp = TempDir::new().unwrap();
        assert!(discover_skills(&[project(tmp.path(), "my_project", true)]).is_empty());
    }

    #[test]
    fn precedence_follows_the_order_projects_are_supplied_in() {
        let root = TempDir::new().unwrap();
        let pkg = TempDir::new().unwrap();
        write_skill(root.path(), "skills/shared", "shared");
        write_skill(pkg.path(), "skills/shared", "shared");

        let found = discover_skills(&[
            project(root.path(), "root_project", true),
            project(pkg.path(), "some_package", false),
        ]);
        assert_eq!(found[0].precedence, 0);
        assert_eq!(found[1].precedence, 1);
        assert_eq!(found[1].origin.package_name(), Some("some_package"));
    }
}
