//! End-to-end `dbt deps` tests for agent-skill installation.
//!
//! These drive the real `get_or_install_packages` entry point against `local:`
//! packages, so they exercise lock computation, package install and the skill
//! pass together without touching the network. They also stand as a regression
//! test for the constraint that `dbt deps` needs no profile and builds no
//! manifest: nothing here supplies either.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::{FsCommand, IoArgs};
use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
use fs_deps::get_or_install_packages;
use fs_deps::private_package::LocalPrivatePackageResolver;
use tempfile::TempDir;

const AGENTS_DIR: &str = ".agents/skills";
const CLAUDE_DIR: &str = ".claude/skills";

/// A throwaway project tree with `local:` packages, ready for `dbt deps`.
struct TestProject {
    _tmp: TempDir,
    root: PathBuf,
}

impl TestProject {
    fn new(project_yml: &str) -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("dbt_project.yml"), project_yml).unwrap();
        Self { _tmp: tmp, root }
    }

    /// Create a sibling package directory and declare it in `packages.yml`.
    fn with_local_package(&self, name: &str, project_yml: &str) -> PathBuf {
        let package_root = self.root.parent().unwrap().join(name);
        fs::create_dir_all(&package_root).unwrap();
        fs::write(package_root.join("dbt_project.yml"), project_yml).unwrap();

        let packages_yml = self.root.join("packages.yml");
        let existing =
            fs::read_to_string(&packages_yml).unwrap_or_else(|_| "packages:\n".to_string());
        fs::write(
            &packages_yml,
            format!("{existing}  - local: \"../{name}\"\n"),
        )
        .unwrap();

        package_root
    }

    fn write_skill(&self, rel: &str, name: &str, body: &str) {
        write_skill_at(&self.root.join(rel), name, body);
    }

    /// Path to a skill installed under its own name: `<dir>/<name>`. dbt does
    /// not namespace or rename skills, so every provider lays them out this way.
    fn installed(&self, dir: &str, skill: &str) -> PathBuf {
        self.root.join(dir).join(skill)
    }

    async fn deps(&self, ai_provider: Option<&[String]>) {
        self.try_deps(ai_provider)
            .await
            .expect("dbt deps should succeed");
    }

    async fn try_deps(
        &self,
        ai_provider: Option<&[String]>,
    ) -> Result<(), Box<dbt_common::FsError>> {
        let io = IoArgs {
            in_dir: self.root.clone(),
            out_dir: self.root.join("target"),
            ..Default::default()
        };
        // A profile-free Jinja environment: exactly what `dbt deps` uses.
        let env = initialize_load_profile_jinja_environment();

        get_or_install_packages(
            &io,
            FsCommand::Deps,
            &env,
            &self.root.join("dbt_packages"),
            true,  // install_deps
            None,  // add_package
            false, // upgrade
            false, // lock
            Default::default(),
            false, // version_check
            false, // skip_private_deps
            None,  // replay_mode
            &CancellationToken::never_cancels(),
            false, // use_v2_compatible_package_downloads
            false, // require_hub_verified_downloads
            Arc::new(LocalPrivatePackageResolver),
            None, // cloud_config
            ai_provider,
        )
        .await
        .map(|_| ())
    }
}

fn write_skill_at(dir: &Path, name: &str, body: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("SKILL.md"),
        format!("---\nname: {name}\ndescription: A test skill.\n---\n{body}"),
    )
    .unwrap();
}

#[tokio::test]
async fn installs_a_skill_shipped_by_a_package() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    write_skill_at(&package.join("skills/from-package"), "from-package", "body");

    project.deps(Some(&["claude".to_string()])).await;

    let installed = project.installed(CLAUDE_DIR, "from-package");
    assert!(installed.join("SKILL.md").is_file());
}

#[tokio::test]
async fn installs_the_projects_own_skills_too() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/mine", "mine", "body");

    project.deps(Some(&["wizard".to_string()])).await;

    assert!(
        project
            .installed(AGENTS_DIR, "mine")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn writes_into_every_provider_directory_once() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/mine", "mine", "body");

    // wizard and codex share `.agents/skills`; claude reads its own directory.
    project
        .deps(Some(&[
            "wizard".to_string(),
            "codex".to_string(),
            "claude".to_string(),
        ]))
        .await;

    assert!(
        project
            .installed(AGENTS_DIR, "mine")
            .join("SKILL.md")
            .is_file()
    );
    assert!(
        project
            .installed(CLAUDE_DIR, "mine")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn installs_nothing_when_ai_provider_is_unset() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/mine", "mine", "body");

    project.deps(None).await;

    assert!(!project.root.join(AGENTS_DIR).exists());
    assert!(!project.root.join(CLAUDE_DIR).exists());
}

#[tokio::test]
async fn an_unknown_provider_installs_nothing_but_deps_still_succeeds() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/mine", "mine", "body");

    project.deps(Some(&["not-a-harness".to_string()])).await;

    assert!(!project.root.join(AGENTS_DIR).exists());
}

#[tokio::test]
async fn ai_provider_can_be_set_in_project_flags() {
    let project =
        TestProject::new("name: root_project\nprofile: default\nflags:\n  ai_provider: claude\n");
    project.write_skill("skills/mine", "mine", "body");

    project.deps(None).await;

    assert!(
        project
            .installed(CLAUDE_DIR, "mine")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn a_disabled_skill_is_not_installed() {
    let project = TestProject::new(
        "name: root_project\nprofile: default\n\
         skills:\n  root_project:\n    mine:\n      +enabled: false\n",
    );
    project.write_skill("skills/mine", "mine", "body");
    project.write_skill("skills/yours", "yours", "body");

    project.deps(Some(&["wizard".to_string()])).await;

    assert!(!project.installed(AGENTS_DIR, "mine").exists());
    assert!(project.installed(AGENTS_DIR, "yours").is_dir());
}

#[tokio::test]
async fn a_package_skill_can_be_disabled_by_the_root_project() {
    let project = TestProject::new(
        "name: root_project\nprofile: default\n\
         skills:\n  some_pkg:\n    +enabled: false\n",
    );
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    write_skill_at(&package.join("skills/unwanted"), "unwanted", "body");

    project.deps(Some(&["wizard".to_string()])).await;

    assert!(!project.installed(AGENTS_DIR, "unwanted").exists());
}

#[tokio::test]
async fn a_package_is_read_from_its_own_skill_paths() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package(
        "some_pkg",
        "name: some_pkg\nprofile: default\nskill-paths: [\"agent-skills\"]\n",
    );
    write_skill_at(&package.join("agent-skills/custom"), "custom", "body");

    project.deps(Some(&["wizard".to_string()])).await;

    assert!(
        project
            .installed(AGENTS_DIR, "custom")
            .join("SKILL.md")
            .is_file()
    );
}

#[tokio::test]
async fn a_same_named_skill_from_two_sources_fails_deps() {
    // Two `shared` skills would install into the same directory. dbt treats this
    // like duplicate model names: `dbt deps` fails and nothing is written.
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/shared", "shared", "from the project");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    write_skill_at(&package.join("skills/shared"), "shared", "from the package");

    let err = project
        .try_deps(Some(&["wizard".to_string()]))
        .await
        .unwrap_err();
    assert_eq!(err.code, dbt_common::ErrorCode::SkillNameCollision);
    assert!(!project.installed(AGENTS_DIR, "shared").exists());
}

#[tokio::test]
async fn same_named_package_skills_fail_deps() {
    // The same conflict across two packages: neither is allowed to shadow the
    // other, so dbt fails rather than pick a winner.
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let zeta = project.with_local_package("zeta_pkg", "name: zeta_pkg\nprofile: default\n");
    let alpha = project.with_local_package("alpha_pkg", "name: alpha_pkg\nprofile: default\n");
    write_skill_at(&zeta.join("skills/shared"), "shared", "from zeta");
    write_skill_at(&alpha.join("skills/shared"), "shared", "from alpha");

    let err = project
        .try_deps(Some(&["wizard".to_string()]))
        .await
        .unwrap_err();
    assert_eq!(err.code, dbt_common::ErrorCode::SkillNameCollision);
}

#[tokio::test]
async fn a_malformed_skill_is_skipped_without_failing_deps() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/good", "good", "body");
    let bad = project.root.join("skills/bad");
    fs::create_dir_all(&bad).unwrap();
    fs::write(bad.join("SKILL.md"), "no frontmatter at all\n").unwrap();

    project.deps(Some(&["wizard".to_string()])).await;

    assert!(project.installed(AGENTS_DIR, "good").is_dir());
    assert!(!project.installed(AGENTS_DIR, "bad").exists());
}

#[tokio::test]
async fn re_running_deps_is_a_no_op() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    project.write_skill("skills/mine", "mine", "body");
    let provider = ["wizard".to_string()];

    project.deps(Some(&provider)).await;
    let installed = project.installed(AGENTS_DIR, "mine");
    let first = fs::read_to_string(installed.join("SKILL.md")).unwrap();

    project.deps(Some(&provider)).await;

    assert_eq!(
        fs::read_to_string(installed.join("SKILL.md")).unwrap(),
        first
    );
    // The destination holds exactly one skill, with no duplicate re-install.
    let dir = project.root.join(AGENTS_DIR);
    let entries: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(entries, vec!["mine".to_string()], "no duplicate install");
}

#[tokio::test]
async fn a_changed_package_skill_is_updated_in_place() {
    // dbt recognizes its own install by the metadata it recorded, so a re-run
    // with changed source content overwrites its own installed copy.
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    let source = package.join("skills/evolving");
    write_skill_at(&source, "evolving", "version one");
    let provider = ["wizard".to_string()];

    project.deps(Some(&provider)).await;
    write_skill_at(&source, "evolving", "version two");
    project.deps(Some(&provider)).await;

    let installed = project.installed(AGENTS_DIR, "evolving");
    let contents = fs::read_to_string(installed.join("SKILL.md")).unwrap();
    assert!(contents.ends_with("version two"), "{contents}");
}

#[tokio::test]
async fn a_skill_whose_metadata_was_stripped_is_left_alone() {
    // Once the user rewrites an installed skill so dbt's ownership metadata is
    // gone, dbt no longer recognizes the copy as its own and refuses to touch it.
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    let source = package.join("skills/evolving");
    write_skill_at(&source, "evolving", "version one");
    let provider = ["wizard".to_string()];

    project.deps(Some(&provider)).await;

    let installed = project.installed(AGENTS_DIR, "evolving");
    fs::write(installed.join("SKILL.md"), "the user rewrote this").unwrap();

    write_skill_at(&source, "evolving", "version two");
    project.deps(Some(&provider)).await;

    assert_eq!(
        fs::read_to_string(installed.join("SKILL.md")).unwrap(),
        "the user rewrote this"
    );
}

#[tokio::test]
async fn a_removed_skill_is_pruned_and_user_skills_survive() {
    // A skill dbt installed (and recorded metadata for) is pruned on the next
    // run once it disappears from its source. A directory the user wrote
    // themselves carries no dbt metadata, so dbt does not manage it and it
    // survives.
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    write_skill_at(&package.join("skills/temporary"), "temporary", "body");
    let provider = ["wizard".to_string()];

    project.deps(Some(&provider)).await;
    assert!(project.installed(AGENTS_DIR, "temporary").is_dir());

    // A skill the user hand-wrote in the destination, with no dbt metadata.
    let hand_written = project.root.join(AGENTS_DIR).join("hand-written");
    write_skill_at(&hand_written, "hand-written", "mine");

    fs::remove_dir_all(package.join("skills/temporary")).unwrap();
    project.deps(Some(&provider)).await;

    assert!(!project.installed(AGENTS_DIR, "temporary").exists());
    assert!(hand_written.join("SKILL.md").is_file());
}

#[tokio::test]
async fn bundled_files_are_copied_alongside_the_skill() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    let source = package.join("skills/with-scripts");
    write_skill_at(&source, "with-scripts", "body");
    fs::create_dir_all(source.join("scripts")).unwrap();
    fs::write(source.join("scripts/run.sh"), "echo hi").unwrap();

    project.deps(Some(&["wizard".to_string()])).await;

    let installed = project.installed(AGENTS_DIR, "with-scripts");
    assert_eq!(
        fs::read_to_string(installed.join("scripts/run.sh")).unwrap(),
        "echo hi"
    );
}

#[tokio::test]
async fn the_source_skill_is_never_modified() {
    let project = TestProject::new("name: root_project\nprofile: default\n");
    let package = project.with_local_package("some_pkg", "name: some_pkg\nprofile: default\n");
    let source = package.join("skills/pristine");
    write_skill_at(&source, "pristine", "body");
    let before = fs::read_to_string(source.join("SKILL.md")).unwrap();

    project.deps(Some(&["claude".to_string()])).await;

    assert_eq!(fs::read_to_string(source.join("SKILL.md")).unwrap(), before);
}
