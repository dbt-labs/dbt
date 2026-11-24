//! End-to-end `dbt deps` tests for package installation failures.
//!
//! A package is installed to `dbt_packages/<name>`, and `<name>` comes from the
//! package's own `dbt_project.yml`. These drive the real
//! `get_or_install_packages` entry point against a `local:` package and a `git:` package
//! served from a local repo, without touching the network.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use dbt_common::cancellation::CancellationToken;
use dbt_common::io_args::{FsCommand, IoArgs};
use dbt_jinja_utils::phases::load::init::initialize_load_profile_jinja_environment;
use fs_deps::get_or_install_packages;
use fs_deps::private_package::LocalPrivatePackageResolver;
use tempfile::TempDir;
use url::Url;

/// A throwaway project tree with `local:` packages, ready for `dbt deps`.
struct TestProject {
    _tmp: TempDir,
    root: PathBuf,
}

impl TestProject {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("dbt_project.yml"),
            "name: root_project\nprofile: default\n",
        )
        .unwrap();
        Self { _tmp: tmp, root }
    }

    /// Create a sibling package directory `dir` and declare it in `packages.yml`.
    fn with_local_package(&self, dir: &str, project_yml: &str) {
        let package_root = self.root.parent().unwrap().join(dir);
        fs::create_dir_all(&package_root).unwrap();
        fs::write(package_root.join("dbt_project.yml"), project_yml).unwrap();

        let packages_yml = self.root.join("packages.yml");
        let existing =
            fs::read_to_string(&packages_yml).unwrap_or_else(|_| "packages:\n".to_string());
        fs::write(
            &packages_yml,
            format!("{existing}  - local: \"../{dir}\"\n"),
        )
        .unwrap();
    }

    /// Create a sibling git repo `dir` and declare it in `packages.yml`.
    fn with_git_package(&self, dir: &str, project_yml: &str) {
        let repo = self.root.parent().unwrap().join(dir);
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join("dbt_project.yml"), project_yml).unwrap();
        let status = Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .current_dir(&repo)
            .status()
            .unwrap();
        assert!(status.success());
        self.commit_git_package(dir);

        // A file URL has no backslashes, which a Windows path would put into the quoted YAML.
        let repo_url = Url::from_file_path(&repo).unwrap();
        let packages_yml = self.root.join("packages.yml");
        let existing =
            fs::read_to_string(&packages_yml).unwrap_or_else(|_| "packages:\n".to_string());
        fs::write(
            &packages_yml,
            format!("{existing}  - git: \"{repo_url}\"\n    revision: main\n"),
        )
        .unwrap();
    }

    fn commit_git_package(&self, dir: &str) {
        let repo = self.root.parent().unwrap().join(dir);
        for args in [
            &["add", "."][..],
            &[
                "-c",
                "user.name=test",
                "-c",
                "user.email=test@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "fixture revision",
            ],
        ] {
            let status = Command::new("git")
                .args(args)
                .current_dir(&repo)
                .status()
                .unwrap();
            assert!(status.success());
        }
    }

    async fn try_deps(&self, upgrade: bool) -> Result<(), Box<dbt_common::FsError>> {
        let io = IoArgs {
            in_dir: self.root.clone(),
            out_dir: self.root.join("target"),
            ..Default::default()
        };
        let env = initialize_load_profile_jinja_environment();

        get_or_install_packages(
            &io,
            FsCommand::Deps,
            &env,
            &self.root.join("dbt_packages"),
            true, // install_deps
            None, // add_package
            upgrade,
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
            None, // ai_provider
        )
        .await
        .map(|_| ())
    }
}

/// Installs `good_pkg`, then adds a package named `bad_name` and runs `deps` again.
/// `deps` must fail, and the project and `good_pkg` must still be there.
async fn assert_name_is_rejected(bad_name: &str) {
    let project = TestProject::new();
    project.with_local_package("good_pkg", "name: good_pkg\nprofile: default\n");
    project
        .try_deps(false)
        .await
        .expect("dbt deps should succeed");
    let good_pkg = project.root.join("dbt_packages/good_pkg/dbt_project.yml");
    assert!(good_pkg.is_file());

    project.with_git_package(
        "bad_pkg",
        &format!("name: \"{bad_name}\"\nprofile: default\n"),
    );
    let err = project
        .try_deps(false)
        .await
        .expect_err("dbt deps should reject the package name");

    assert!(
        err.to_string().contains("is not a valid directory name"),
        "unexpected error: {err}"
    );
    assert!(project.root.join("dbt_project.yml").is_file());
    assert!(good_pkg.is_file());
}

#[tokio::test]
async fn a_parent_dir_package_name_fails_deps_and_deletes_nothing() {
    assert_name_is_rejected("..").await;
}

#[tokio::test]
async fn an_empty_package_name_fails_deps_and_deletes_nothing() {
    assert_name_is_rejected("").await;
}

#[tokio::test]
async fn a_current_dir_package_name_fails_deps_and_deletes_nothing() {
    assert_name_is_rejected(".").await;
}

#[tokio::test]
async fn a_nested_package_name_fails_deps_and_deletes_nothing() {
    assert_name_is_rejected("nested/pkg").await;
}

#[tokio::test]
async fn a_read_only_lockfile_preserves_the_installed_revision() {
    let project = TestProject::new();
    let first_revision = "name: git_pkg\nversion: '1.0'\n";
    let second_revision = "name: git_pkg\nversion: '2.0'\n";
    project.with_git_package("git_pkg", first_revision);
    project.try_deps(false).await.unwrap();

    let lock = project.root.join("package-lock.yml");
    let old_lock = fs::read(&lock).unwrap();
    let installed = project.root.join("dbt_packages/git_pkg/dbt_project.yml");
    assert_eq!(fs::read_to_string(&installed).unwrap(), first_revision);

    let upstream = project.root.join("../git_pkg/dbt_project.yml");
    fs::write(upstream, second_revision).unwrap();
    project.commit_git_package("git_pkg");

    let original_permissions = fs::metadata(&lock).unwrap().permissions();
    let mut read_only = original_permissions.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&lock, read_only).unwrap();
    let result = project.try_deps(true).await;
    fs::set_permissions(&lock, original_permissions).unwrap();

    let error = result.expect_err("the lockfile write should fail");
    assert!(
        error
            .to_string()
            .contains("Failed to write package-lock.yml")
    );
    assert_eq!(fs::read(&lock).unwrap(), old_lock);
    assert_eq!(fs::read_to_string(&installed).unwrap(), first_revision);

    project.try_deps(true).await.unwrap();
    assert_ne!(fs::read(&lock).unwrap(), old_lock);
    assert_eq!(fs::read_to_string(&installed).unwrap(), second_revision);
}
