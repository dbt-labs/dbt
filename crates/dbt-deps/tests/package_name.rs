//! End-to-end `dbt deps` tests for package names that are not a plain dir name.
//!
//! A package is installed to `dbt_packages/<name>`, and `<name>` comes from the
//! package's own `dbt_project.yml`. These drive the real
//! `get_or_install_packages` entry point against a `local:` package and a `git:` package
//! served from a local repo, without touching the network.

mod common;

use common::TestProject;

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
