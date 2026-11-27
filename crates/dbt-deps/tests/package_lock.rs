//! End-to-end `dbt deps` tests for a `package-lock.yml` that can't be written.
//!
//! A failed `dbt deps --upgrade` must leave both the lockfile and the installed packages
//! as they were, so later commands keep using the locked package code.

mod common;

use std::fs;

use common::TestProject;

fn installed_entries(project: &TestProject) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(project.root.join("dbt_packages"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn a_read_only_lockfile_preserves_the_installed_packages() {
    let project = TestProject::new();
    let first_revision = "name: git_pkg\nversion: '1.0'\n";
    let second_revision = "name: git_pkg\nversion: '2.0'\n";
    project.with_git_package("git_pkg", first_revision);
    project.try_deps(false).await.unwrap();

    let lock = project.root.join("package-lock.yml");
    let old_lock = fs::read(&lock).unwrap();
    let installed = project.root.join("dbt_packages/git_pkg/dbt_project.yml");
    assert_eq!(fs::read_to_string(&installed).unwrap(), first_revision);

    // Upgrade `git_pkg` to a new revision and add a package that wasn't installed.
    let upstream = project.root.join("../git_pkg/dbt_project.yml");
    fs::write(upstream, second_revision).unwrap();
    project.commit_git_package("git_pkg");
    project.with_git_package("extra_pkg", "name: extra_pkg\nversion: '1.0'\n");

    let writable = fs::metadata(&lock).unwrap().permissions();
    let mut read_only = writable.clone();
    read_only.set_readonly(true);
    fs::set_permissions(&lock, read_only).unwrap();
    let result = project.try_deps(true).await;
    fs::set_permissions(&lock, writable).unwrap();

    let error = result.expect_err("the lockfile write should fail");
    assert!(
        error
            .to_string()
            .contains("Failed to write package-lock.yml"),
        "unexpected error: {error}"
    );
    assert_eq!(fs::read(&lock).unwrap(), old_lock);
    assert_eq!(fs::read_to_string(&installed).unwrap(), first_revision);
    // No new package, backup or scratch dir is left behind.
    assert_eq!(installed_entries(&project), ["git_pkg"]);

    project.try_deps(true).await.unwrap();
    assert_ne!(fs::read(&lock).unwrap(), old_lock);
    assert_eq!(fs::read_to_string(&installed).unwrap(), second_revision);
    assert_eq!(installed_entries(&project), ["extra_pkg", "git_pkg"]);
}
