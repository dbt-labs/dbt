use std::path::Path;

use normalize_path::NormalizePath;

use super::normalize;

#[track_caller]
fn assert_shellexpand_compatibility(path: &str) {
    let expanded = shellexpand::tilde(path);
    assert_eq!(
        normalize(Path::new(path)),
        Path::new(expanded.as_ref()),
        "path: {path:?}, shellexpand: {expanded:?}"
    );
}

#[track_caller]
fn assert_normalize_path_compatibility(path: &str) {
    assert_eq!(
        normalize(Path::new(path)).as_os_str(),
        Path::new(path).normalize().as_os_str(),
        "path: {path:?}"
    );
}

#[track_caller]
fn assert_normalize_path_divergence(path: &str, ours: &str, theirs: &str) {
    assert_ne!(ours, theirs, "path: {path:?}");
    assert_eq!(
        normalize(Path::new(path)).as_os_str(),
        ours,
        "path: {path:?}"
    );
    assert_eq!(
        Path::new(path).normalize().as_os_str(),
        theirs,
        "path: {path:?}"
    );
}

#[test]
fn shellexpand_bare_tilde() {
    assert_shellexpand_compatibility("~");
}

#[test]
fn shellexpand_tilde_slash() {
    assert_shellexpand_compatibility("~/");
}

#[test]
fn shellexpand_tilde_prefix() {
    assert_shellexpand_compatibility("~/keys/sa.json");
}

#[test]
fn shellexpand_tilde_backslash() {
    assert_shellexpand_compatibility(r"~\keys\sa.json");
}

#[test]
fn shellexpand_other_user() {
    assert_shellexpand_compatibility("~alice/keys");
}

#[test]
fn shellexpand_tilde_in_middle() {
    assert_shellexpand_compatibility("/abs/~/path");
    assert_shellexpand_compatibility("rel/~/path");
}

#[test]
fn shellexpand_relative_with_dots() {
    assert_shellexpand_compatibility("../my/./path/");
}

#[test]
fn shellexpand_absolute() {
    assert_shellexpand_compatibility("/abs/path");
}

#[test]
fn normalize_path_cur_dir_components() {
    assert_normalize_path_compatibility("./models/./foo.sql");
}

#[test]
fn normalize_path_parent_dir_components() {
    assert_normalize_path_compatibility("models/staging/../foo.sql");
    assert_normalize_path_compatibility("a/b/../../c");
}

#[test]
fn normalize_path_parent_above_root() {
    assert_normalize_path_compatibility("/..");
    assert_normalize_path_compatibility("/../a/b/../c");
    assert_normalize_path_compatibility("/repo/datahub/../common");
}

#[test]
fn normalize_path_repeated_separators() {
    assert_normalize_path_compatibility("a//b///c/");
}

#[test]
fn normalize_path_trailing_separator() {
    assert_normalize_path_compatibility("my/./path/");
}

#[test]
fn normalize_path_already_normal() {
    assert_normalize_path_compatibility("a");
    assert_normalize_path_compatibility("/");
    assert_normalize_path_compatibility("/abs/path");
    assert_normalize_path_compatibility("~alice/keys");
    assert_normalize_path_compatibility("rel/~/path");
}

#[test]
#[cfg(windows)]
fn normalize_path_windows_prefix() {
    assert_normalize_path_compatibility(r"C:\a\..\b");
    assert_normalize_path_compatibility(r"C:\..\b");
    assert_normalize_path_compatibility(r"\\server\share\a\..\b");
}

#[test]
fn normalize_path_diverges_on_empty_result() {
    assert_normalize_path_divergence("", ".", "");
    assert_normalize_path_divergence(".", ".", "");
    assert_normalize_path_divergence("a/..", ".", "");
}

#[test]
fn normalize_path_diverges_on_leading_parent_dirs() {
    assert_normalize_path_divergence("..", "..", "");
    assert_normalize_path_divergence("../my/./path/", "../my/path", "my/path");
    assert_normalize_path_divergence("foo/../../bar", "../bar", "bar");
}

#[test]
fn normalize_path_diverges_on_tilde() {
    let home = std::env::home_dir().unwrap();
    assert_normalize_path_divergence("~/keys", home.join("keys").to_str().unwrap(), "~/keys");
}
