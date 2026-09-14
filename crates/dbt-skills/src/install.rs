//! Copying selected skills into each provider directory, and pruning the ones
//! dbt previously installed that are no longer wanted.
//!
//! Every install is a plain copy — no symlinks — with dbt's bookkeeping injected
//! into the copy's `metadata` frontmatter. That record is what lets dbt
//! recognize its own installs later, so it can reclaim its own space
//! unconditionally while never touching a skill the user wrote.
//!
//! Each skill installs under its own name (`<destination>/<name>/`), exactly as
//! the package shipped it — dbt does not rename or namespace it. Ownership is
//! recorded in the copy's metadata, not encoded in its path: on every run dbt
//! brings its own installs up to date and removes the ones it installed before
//! that are no longer wanted, while a skill it does not manage is left untouched.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use chrono::Utc;
use dbt_common::tracing::dbt_emit::{emit_info_progress_message, emit_warn_log_message};
use dbt_common::{ErrorCode, FsResult, stdfs};
use dbt_telemetry::ProgressMessage;
use walkdir::WalkDir;

use crate::discover::{DiscoveredSkill, SkillOrigin};
use crate::hash::{SkillDigests, skill_digests};
use crate::metadata;
use crate::validate::SKILL_FILE;

/// What happened to one skill in one destination directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Freshly copied in.
    Installed,
    /// Already present and identical to what dbt would write; left alone.
    Unchanged,
    /// A dbt-installed copy that had drifted from its source (or been edited);
    /// reclaimed and rewritten.
    Updated,
    /// The source resolves to the destination, so there is nothing to copy.
    SourceIsDestination,
    /// A skill dbt does not manage already occupies the target; left untouched.
    SkippedNotOurs,
    /// A dbt-installed copy that is no longer wanted; removed.
    Pruned,
}

impl InstallOutcome {
    /// The action word dbt prints for this outcome, or `None` when it is a
    /// no-op worth staying quiet about.
    const fn action(self) -> Option<&'static str> {
        match self {
            InstallOutcome::Installed => Some("Installing"),
            InstallOutcome::Updated => Some("Updating"),
            InstallOutcome::Pruned => Some("Removing"),
            InstallOutcome::Unchanged
            | InstallOutcome::SourceIsDestination
            | InstallOutcome::SkippedNotOurs => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    /// Destination directory, relative to the project root.
    pub destination: PathBuf,
    /// On-disk identity of the skill within the destination.
    pub skill_name: String,
    pub outcome: InstallOutcome,
}

/// Announce what happened to one skill, in the same shape as the rest of dbt:
/// `Installing  <skill> (<package>) -> <destination>`.
fn report_progress(report: &InstallReport, origin: Option<&SkillOrigin>) {
    let Some(action) = report.outcome.action() else {
        return;
    };

    let package = match origin {
        Some(SkillOrigin::Package { name, version }) => match version {
            Some(version) => format!(" ({name} {version})"),
            None => format!(" ({name})"),
        },
        Some(SkillOrigin::Project) => " (this project)".to_string(),
        None => String::new(),
    };

    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        action.to_string(),
        format!(
            "{}{package} -> {}",
            report.skill_name,
            report.destination.display()
        ),
    ));
}

/// Install `selected` into every destination, then prune what dbt no longer
/// wants there.
///
/// `project_root` anchors the (relative) destination directories. Returns one
/// report per skill per destination, plus one per pruned directory. Callers are
/// expected to have already rejected same-named skills (see
/// [`crate::install_package_skills`]); here a duplicate name would simply have
/// the later skill overwrite the earlier one in each destination.
pub fn install_skills(
    project_root: &Path,
    destinations: &[PathBuf],
    selected: &[DiscoveredSkill],
) -> FsResult<Vec<InstallReport>> {
    // The source directories don't change across destinations, so hash each
    // skill once here rather than re-walking it for every destination.
    let digests: Vec<SkillDigests> = selected
        .iter()
        .map(|skill| skill_digests(&skill.dir))
        .collect::<FsResult<_>>()?;

    // The set of names dbt wants installed is the same in every destination, so
    // build it once. It doubles as the prune keep-set below.
    let wanted: BTreeSet<String> = selected.iter().map(|skill| skill.name.clone()).collect();

    let mut reports = Vec::new();

    for destination in destinations {
        let absolute = project_root.join(destination);

        for (skill, digest) in selected.iter().zip(&digests) {
            let report = InstallReport {
                destination: destination.clone(),
                skill_name: skill.name.clone(),
                outcome: install_one(&absolute, skill, digest)?,
            };
            report_progress(&report, Some(&skill.origin));
            reports.push(report);
        }

        reports.extend(prune(&absolute, destination, &wanted)?);
    }

    Ok(reports)
}

/// Remove every dbt-installed skill under `destinations`, wanted or not. Used by
/// `dbt clean`.
pub fn prune_all(project_root: &Path, destinations: &[PathBuf]) -> FsResult<Vec<InstallReport>> {
    let mut reports = Vec::new();
    for destination in destinations {
        reports.extend(prune(
            &project_root.join(destination),
            destination,
            &BTreeSet::new(),
        )?);
    }
    Ok(reports)
}

fn install_one(
    destination: &Path,
    skill: &DiscoveredSkill,
    digest: &SkillDigests,
) -> FsResult<InstallOutcome> {
    let target = destination.join(&skill.name);

    // A project may author its skills directly inside a provider directory. In
    // that case there is nothing to copy, and copying would be self-destructive.
    if same_path(&skill.dir, &target) {
        return Ok(InstallOutcome::SourceIsDestination);
    }

    if target.exists() {
        let installed = stdfs::read_to_string(target.join(SKILL_FILE)).unwrap_or_default();
        let Some(existing) = metadata::read(&installed) else {
            // Something dbt does not manage occupies the target. dbt reclaims
            // only what it recorded as its own, so this is left untouched.
            emit_warn_log_message(
                ErrorCode::SkillDestinationOccupied,
                format!(
                    "Not installing skill '{}' into {}: a skill dbt does not manage is already there.",
                    skill.name,
                    target.display()
                ),
            );
            return Ok(InstallOutcome::SkippedNotOurs);
        };

        // Already dbt's. If it still matches what dbt would write, leave the
        // bytes alone so a re-run does not churn the working tree; otherwise it
        // has drifted (a changed source, or a hand edit) and dbt reclaims it.
        //
        // Regenerate the expected `SKILL.md` from the *stored* hash and
        // timestamp, not this run's freshly computed `digest.full`: those are
        // recorded provenance, so reproducing them is how the comparison stays
        // about the actual content. Real drift is caught by the body of the
        // comparison and by `bundled_files_match`, not by the hash value.
        let expected = render(skill, &existing.source_hash, &existing.installed_at)?;
        if installed == expected && bundled_files_match(digest, &target)? {
            return Ok(InstallOutcome::Unchanged);
        }

        stdfs::remove_dir_all(&target)?;
        write_skill(skill, &target, &digest.full)?;
        return Ok(InstallOutcome::Updated);
    }

    write_skill(skill, &target, &digest.full)?;
    Ok(InstallOutcome::Installed)
}

/// The exact `SKILL.md` bytes dbt writes for this skill: the package's own
/// `SKILL.md`, with dbt's ownership metadata injected. The frontmatter `name`
/// is left exactly as the package wrote it.
fn render(skill: &DiscoveredSkill, source_hash: &str, installed_at: &str) -> FsResult<String> {
    let source_md = stdfs::read_to_string(skill.dir.join(SKILL_FILE))?;

    let (source, package, version) = skill.origin.metadata_fields();
    metadata::inject(
        &source_md,
        &metadata::SkillMetadata {
            source: source.to_string(),
            package,
            version,
            source_path: skill.source_path.to_string_lossy().replace('\\', "/"),
            source_hash: source_hash.to_string(),
            installed_at: installed_at.to_string(),
        },
    )
}

/// Copy the skill in, then overwrite its `SKILL.md` with the metadata-bearing
/// version. The source tree is never modified.
fn write_skill(skill: &DiscoveredSkill, target: &Path, source_hash: &str) -> FsResult<()> {
    copy_skill(&skill.dir, target)?;
    let rendered = render(skill, source_hash, &Utc::now().to_rfc3339())?;
    stdfs::write(target.join(SKILL_FILE), rendered)
}

/// Compare every file except `SKILL.md` between the (already-hashed) source and
/// the installed copy. `SKILL.md` carries injected metadata and is compared
/// separately against regenerated bytes.
fn bundled_files_match(source: &SkillDigests, target: &Path) -> FsResult<bool> {
    Ok(source.excluding_skill_md == skill_digests(target)?.excluding_skill_md)
}

/// Remove dbt-installed skills in `destination` whose name is not in `wanted`.
///
/// Each dbt install is one top-level `<name>/` directory carrying dbt metadata,
/// so pruning only has to look one directory deep. Skills dbt does not manage
/// carry no such metadata and are never touched.
fn prune(
    destination: &Path,
    relative_destination: &Path,
    wanted: &BTreeSet<String>,
) -> FsResult<Vec<InstallReport>> {
    let mut reports = Vec::new();
    for path in dbt_owned_children(destination)? {
        let name = leaf_name(&path);
        if wanted.contains(&name) {
            continue;
        }
        stdfs::remove_dir_all(&path)?;
        reports.push(pruned(relative_destination, &name));
    }
    Ok(reports)
}

fn pruned(relative_destination: &Path, name: &str) -> InstallReport {
    let report = InstallReport {
        destination: relative_destination.to_path_buf(),
        skill_name: name.to_string(),
        outcome: InstallOutcome::Pruned,
    };
    report_progress(&report, None);
    report
}

/// Immediate subdirectories of `dir` whose `SKILL.md` carries dbt metadata.
fn dbt_owned_children(dir: &Path) -> FsResult<Vec<PathBuf>> {
    let mut owned = Vec::new();
    for path in child_dirs(dir)? {
        let installed = stdfs::read_to_string(path.join(SKILL_FILE)).unwrap_or_default();
        if metadata::read(&installed).is_some() {
            owned.push(path);
        }
    }
    Ok(owned)
}

/// Immediate subdirectories of `dir`, sorted for reproducibility.
fn child_dirs(dir: &Path) -> FsResult<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut dirs: Vec<PathBuf> = stdfs::read_dir(dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    Ok(dirs)
}

fn leaf_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Recursively copy a skill directory.
fn copy_skill(source: &Path, target: &Path) -> FsResult<()> {
    stdfs::create_dir_all(target)?;

    let entries = WalkDir::new(source)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok);

    for entry in entries {
        let relative = stdfs::diff_paths(entry.path(), source)?;
        if relative.as_os_str().is_empty() {
            continue;
        }
        copy_entry(entry.path(), &target.join(&relative), entry.file_type())?;
    }

    Ok(())
}

/// Recreate one walked entry at `destination`. Anything that is neither a file
/// nor a directory (a symlink inside the source tree, say) is skipped.
fn copy_entry(source: &Path, destination: &Path, file_type: std::fs::FileType) -> FsResult<()> {
    if file_type.is_dir() {
        return stdfs::create_dir_all(destination);
    }
    if !file_type.is_file() {
        return Ok(());
    }

    if let Some(parent) = destination.parent() {
        stdfs::create_dir_all(parent)?;
    }
    stdfs::copy(source, destination).map(|_| ())
}

/// Compare two paths by their canonical form, falling back to a literal
/// comparison when either does not exist yet.
fn same_path(left: &Path, right: &Path) -> bool {
    match (stdfs::canonicalize(left), stdfs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn dest(dir: &str) -> PathBuf {
        PathBuf::from(dir)
    }

    fn make_source(root: &Path, name: &str, body: &str) -> DiscoveredSkill {
        let dir = root.join("skills").join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(SKILL_FILE),
            format!("---\nname: {name}\ndescription: A skill.\n---\n{body}"),
        )
        .unwrap();

        DiscoveredSkill {
            name: name.to_string(),
            dir,
            source_path: PathBuf::from(format!("skills/{name}")),
            origin: SkillOrigin::Package {
                name: "some_pkg".to_string(),
                version: Some("1.0.0".to_string()),
            },
            fqn: vec!["some_pkg".to_string(), name.to_string()],
            precedence: 1,
        }
    }

    fn outcomes(reports: &[InstallReport]) -> Vec<InstallOutcome> {
        reports.iter().map(|r| r.outcome).collect()
    }

    #[test]
    fn an_install_uses_the_skills_own_name_and_records_metadata() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let selected = vec![make_source(root, "alpha", "body")];

        let reports = install_skills(root, &[dest(".agents/skills")], &selected).unwrap();
        assert_eq!(outcomes(&reports), vec![InstallOutcome::Installed]);

        let installed = root.join(".agents/skills/alpha").join(SKILL_FILE);
        let md = fs::read_to_string(&installed).unwrap();
        let meta = metadata::read(&md).unwrap();
        assert_eq!(meta.package.as_deref(), Some("some_pkg"));
        // The skill's own name is left exactly as the package wrote it.
        assert!(md.contains("name: alpha"), "{md}");
    }

    #[test]
    fn bundled_files_travel_with_the_skill() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let skill = make_source(root, "alpha", "body");
        fs::create_dir_all(skill.dir.join("scripts")).unwrap();
        fs::write(skill.dir.join("scripts/run.sh"), "echo hi").unwrap();

        install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();
        assert!(root.join(".agents/skills/alpha/scripts/run.sh").is_file());
    }

    #[test]
    fn re_running_is_a_no_op() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let selected = vec![make_source(root, "alpha", "body")];

        install_skills(root, &[dest(".agents/skills")], &selected).unwrap();
        let path = root.join(".agents/skills/alpha").join(SKILL_FILE);
        let before = fs::read_to_string(&path).unwrap();

        let reports = install_skills(root, &[dest(".agents/skills")], &selected).unwrap();
        assert_eq!(outcomes(&reports), vec![InstallOutcome::Unchanged]);
        // Byte-identical, including the recorded timestamp — nothing was rewritten.
        assert_eq!(fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn a_changed_source_updates_the_installed_copy() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let skill = make_source(root, "alpha", "first");
        install_skills(
            root,
            &[dest(".agents/skills")],
            std::slice::from_ref(&skill),
        )
        .unwrap();

        fs::write(
            skill.dir.join(SKILL_FILE),
            "---\nname: alpha\ndescription: A skill.\n---\nsecond",
        )
        .unwrap();
        let reports = install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();

        assert_eq!(outcomes(&reports), vec![InstallOutcome::Updated]);
        let md = fs::read_to_string(root.join(".agents/skills/alpha").join(SKILL_FILE)).unwrap();
        assert!(md.ends_with("second"), "{md}");
    }

    #[test]
    fn a_user_edited_copy_of_dbts_own_skill_is_reclaimed() {
        // An edit to a copy dbt installed is overwritten on the next run, because
        // dbt still recognizes its own metadata — unlike a skill dbt does not
        // manage, which carries no such metadata and is left alone.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let skill = make_source(root, "alpha", "body");
        install_skills(
            root,
            &[dest(".agents/skills")],
            std::slice::from_ref(&skill),
        )
        .unwrap();

        // The user edits the body but leaves dbt's metadata frontmatter intact,
        // so dbt still recognizes the copy as its own.
        let installed = root.join(".agents/skills/alpha").join(SKILL_FILE);
        let edited = format!(
            "{}\nthe user appended this\n",
            fs::read_to_string(&installed).unwrap()
        );
        fs::write(&installed, &edited).unwrap();

        let reports = install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();
        assert_eq!(outcomes(&reports), vec![InstallOutcome::Updated]);
        let after = fs::read_to_string(&installed).unwrap();
        assert!(metadata::read(&after).is_some());
        assert!(!after.contains("the user appended this"), "{after}");
    }

    #[test]
    fn a_skill_dbt_does_not_manage_is_never_touched() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let skill = make_source(root, "alpha", "body");

        let occupied = root.join(".agents/skills/alpha");
        fs::create_dir_all(&occupied).unwrap();
        fs::write(occupied.join(SKILL_FILE), "hand written").unwrap();

        let reports = install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();
        assert_eq!(outcomes(&reports), vec![InstallOutcome::SkippedNotOurs]);
        assert_eq!(
            fs::read_to_string(occupied.join(SKILL_FILE)).unwrap(),
            "hand written"
        );
    }

    #[test]
    fn a_no_longer_wanted_skill_is_pruned() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let alpha = make_source(root, "alpha", "body");
        let beta = make_source(root, "beta", "body");

        install_skills(root, &[dest(".agents/skills")], &[alpha.clone(), beta]).unwrap();
        // Drop beta, leaving only alpha wanted.
        let reports = install_skills(root, &[dest(".agents/skills")], &[alpha]).unwrap();

        assert!(
            reports
                .iter()
                .any(|r| r.outcome == InstallOutcome::Pruned && r.skill_name == "beta")
        );
        assert!(!root.join(".agents/skills/beta").exists());
        assert!(root.join(".agents/skills/alpha").is_dir());
    }

    #[test]
    fn pruning_removes_dbt_dirs_but_not_user_dirs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        install_skills(
            root,
            &[dest(".agents/skills")],
            &[make_source(root, "alpha", "body")],
        )
        .unwrap();

        // A skill the user authored in the same directory, with no dbt metadata.
        let mine = root.join(".agents/skills/hand-written");
        fs::create_dir_all(&mine).unwrap();
        fs::write(mine.join(SKILL_FILE), "mine").unwrap();

        install_skills(root, &[dest(".agents/skills")], &[]).unwrap();
        assert!(!root.join(".agents/skills/alpha").exists());
        assert!(mine.join(SKILL_FILE).is_file());
    }

    #[test]
    fn prune_all_removes_dbt_skills_across_destinations() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let destinations = [dest(".agents/skills"), dest(".claude/skills")];
        install_skills(root, &destinations, &[make_source(root, "alpha", "body")]).unwrap();

        prune_all(root, &destinations).unwrap();
        assert!(!root.join(".agents/skills/alpha").exists());
        assert!(!root.join(".claude/skills/alpha").exists());
    }

    #[test]
    fn a_skill_authored_in_the_provider_dir_is_not_copied_onto_itself() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();

        // A project skill whose source already sits at its install target.
        let dir = root.join(".agents/skills/alpha");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join(SKILL_FILE),
            "---\nname: alpha\ndescription: A skill.\n---\n",
        )
        .unwrap();

        let skill = DiscoveredSkill {
            name: "alpha".to_string(),
            dir: dir.clone(),
            source_path: PathBuf::from(".agents/skills/alpha"),
            origin: SkillOrigin::Project,
            fqn: vec!["root_project".to_string(), "alpha".to_string()],
            precedence: 0,
        };

        let reports = install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();
        assert_eq!(
            outcomes(&reports),
            vec![InstallOutcome::SourceIsDestination]
        );
        // Left exactly as authored; no metadata injected.
        assert!(metadata::read(&fs::read_to_string(dir.join(SKILL_FILE)).unwrap()).is_none());
    }

    #[test]
    fn writes_into_every_destination() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let selected = vec![make_source(root, "alpha", "body")];
        let destinations = [dest(".agents/skills"), dest(".claude/skills")];

        install_skills(root, &destinations, &selected).unwrap();
        assert!(root.join(".agents/skills/alpha").join(SKILL_FILE).is_file());
        assert!(root.join(".claude/skills/alpha").join(SKILL_FILE).is_file());
    }

    #[test]
    #[cfg(unix)]
    fn symlinks_inside_a_skill_are_not_followed_out_of_the_tree() {
        // A package can ship whatever it likes inside its own skill directory,
        // including a symlink to somewhere outside the project. Copying such a
        // link — or worse, its target — would leak files into the install.
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        fs::write(root.join("secret.txt"), "TOP SECRET").unwrap();

        let skill = make_source(root, "leaky", "body");
        std::os::unix::fs::symlink(root.join("secret.txt"), skill.dir.join("stolen.txt")).unwrap();
        std::os::unix::fs::symlink(root, skill.dir.join("everything")).unwrap();

        install_skills(root, &[dest(".agents/skills")], &[skill]).unwrap();

        let installed = root.join(".agents/skills/leaky");
        assert!(installed.join(SKILL_FILE).is_file());
        assert!(!installed.join("stolen.txt").exists());
        assert!(!installed.join("everything").exists());
    }
}
