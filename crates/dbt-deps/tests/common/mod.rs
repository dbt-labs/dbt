//! A throwaway dbt project for end-to-end `dbt deps` tests. It drives the real
//! `get_or_install_packages` entry point against `local:` packages and `git:` packages
//! served from local repos, without touching the network.

// Each test crate uses only some of these helpers.
#![allow(dead_code)]

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

/// A throwaway project tree with `local:` and `git:` packages, ready for `dbt deps`.
pub struct TestProject {
    _tmp: TempDir,
    pub root: PathBuf,
}

impl TestProject {
    pub fn new() -> Self {
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
    pub fn with_local_package(&self, dir: &str, project_yml: &str) {
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
    pub fn with_git_package(&self, dir: &str, project_yml: &str) {
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

    pub fn commit_git_package(&self, dir: &str) {
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

    pub async fn try_deps(&self, upgrade: bool) -> Result<(), Box<dbt_common::FsError>> {
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
