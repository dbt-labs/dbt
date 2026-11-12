use std::borrow::Cow;
use std::env;
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

/// Normalize a path, removing `~`, `.` and `..`
///
/// This will resolve the home directory and relative directories.
///
/// Examples:
/// - ~/a/b       -> /home/user/a/b
/// - ./a/b       -> a/b
/// - ../a/b      -> ../a/b
/// - a/b/../../c -> c
/// - a/..        -> a
/// - a/./b       -> a/b
pub fn normalize(path: &Path) -> Cow<'_, Path> {
    let mut unchanged = !path.as_os_str().is_empty();
    let mut only_dotdots = true;
    let mut needs_separator = false;
    let mut len = 0;
    for (i, component) in path.components().enumerate() {
        unchanged &= match component {
            Component::CurDir => path == Path::new("."),
            Component::ParentDir => only_dotdots,
            Component::Normal(c) => i > 0 || c != OsStr::new("~"),
            Component::Prefix(..) | Component::RootDir => true,
        };
        if !unchanged {
            break;
        }
        only_dotdots &= component == Component::ParentDir;
        len += usize::from(needs_separator) + component.as_os_str().len();
        needs_separator = !matches!(component, Component::Prefix(..) | Component::RootDir);
    }
    if unchanged && len == path.as_os_str().len() {
        return Cow::Borrowed(path);
    }

    let mut components = path.components().peekable();
    let mut cleaned = PathBuf::new();
    let mut component_count = 0;
    match components.peek() {
        Some(c @ Component::Prefix(..)) => {
            cleaned.push(c.as_os_str());
            components.next();
        }
        Some(Component::Normal(c)) if *c == OsStr::new("~") => {
            if let Some(home) = env::home_dir() {
                components.next();
                component_count = home
                    .components()
                    .filter(|c| !matches!(c, Component::Prefix(..)))
                    .count();
                cleaned = home;
            }
        }
        _ => {}
    }
    let mut dotdots = 0;
    for component in components {
        match component {
            Component::Prefix(..) => {}
            Component::RootDir => {
                cleaned.push(component.as_os_str());
                component_count += 1;
            }
            Component::CurDir => {}
            Component::ParentDir if component_count == 1 && cleaned.is_absolute() => {}
            Component::ParentDir if component_count == dotdots => {
                cleaned.push("..");
                dotdots += 1;
                component_count += 1;
            }
            Component::ParentDir => {
                cleaned.pop();
                component_count -= 1;
            }
            Component::Normal(c) => {
                cleaned.push(c);
                component_count += 1;
            }
        }
    }
    if component_count == 0 {
        cleaned.push(".");
    }
    Cow::Owned(cleaned)
}

#[cfg(all(test, feature = "compat-tests"))]
mod compat_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(path: &str) -> Cow<'_, Path> {
        normalize(Path::new(path))
    }

    fn is_borrowed(path: &str) -> bool {
        matches!(norm(path), Cow::Borrowed(_))
    }

    fn home() -> PathBuf {
        env::home_dir().unwrap()
    }

    #[test]
    fn empty_becomes_cur_dir() {
        assert_eq!(norm(""), PathBuf::from("."));
        assert_eq!(norm("."), PathBuf::from("."));
        assert_eq!(norm("./."), PathBuf::from("."));
    }

    #[test]
    fn removes_cur_dir_components() {
        assert_eq!(norm("./models/./foo.sql"), PathBuf::from("models/foo.sql"));
    }

    #[test]
    fn resolves_parent_dir_components() {
        assert_eq!(
            norm("models/staging/../foo.sql"),
            PathBuf::from("models/foo.sql")
        );
        assert_eq!(norm("a/b/../../c"), PathBuf::from("c"));
        assert_eq!(norm("a/.."), PathBuf::from("."));
    }

    #[test]
    fn keeps_leading_parent_dirs_of_relative_paths() {
        assert_eq!(norm(".."), PathBuf::from(".."));
        assert_eq!(norm("../../a"), PathBuf::from("../../a"));
        assert_eq!(norm("foo/../../bar"), PathBuf::from("../bar"));
        assert_eq!(norm("../a/../../b"), PathBuf::from("../../b"));
    }

    #[test]
    fn drops_parent_dirs_above_root() {
        assert_eq!(norm("/.."), PathBuf::from("/"));
        assert_eq!(norm("/../a/b/../c"), PathBuf::from("/a/c"));
        assert_eq!(
            norm("/repo/datahub/../common"),
            PathBuf::from("/repo/common")
        );
    }

    #[test]
    fn collapses_repeated_separators() {
        assert_eq!(norm("a//b///c/"), PathBuf::from("a/b/c"));
    }

    #[test]
    #[cfg(windows)]
    fn keeps_windows_prefix() {
        assert_eq!(norm(r"C:\a\..\b"), PathBuf::from(r"C:\b"));
        assert_eq!(norm(r"C:\..\b"), PathBuf::from(r"C:\b"));
        assert_eq!(
            norm(r"\\server\share\a\..\b"),
            PathBuf::from(r"\\server\share\b")
        );
    }

    #[test]
    fn expands_bare_tilde() {
        assert_eq!(norm("~"), home());
        assert_eq!(norm("~/"), home());
    }

    #[test]
    fn expands_tilde_slash_prefix() {
        assert_eq!(norm("~/keys/sa.json"), home().join("keys/sa.json"));
    }

    #[test]
    fn cleans_after_expanding_tilde() {
        assert_eq!(norm("~/./keys/../sa.json"), home().join("sa.json"));
        assert_eq!(norm("./~/keys"), PathBuf::from("~/keys"));
        assert_eq!(norm("~/.."), home().parent().unwrap());
    }

    #[test]
    #[cfg(windows)]
    fn expands_tilde_backslash_prefix() {
        assert_eq!(norm(r"~\keys\sa.json"), home().join(r"keys\sa.json"));
    }

    #[test]
    #[cfg(not(windows))]
    fn leaves_tilde_backslash_prefix() {
        assert_eq!(norm(r"~\keys"), PathBuf::from(r"~\keys"));
    }

    #[test]
    fn leaves_other_user_tilde() {
        assert_eq!(norm("~alice/keys"), PathBuf::from("~alice/keys"));
    }

    #[test]
    fn leaves_tilde_outside_first_component() {
        assert_eq!(norm("/abs/~/path"), PathBuf::from("/abs/~/path"));
        assert_eq!(norm("rel/~/path"), PathBuf::from("rel/~/path"));
    }

    #[test]
    fn borrows_paths_that_are_already_normal() {
        assert!(is_borrowed("."));
        assert!(is_borrowed("a"));
        assert!(is_borrowed("models/foo.sql"));
        assert!(is_borrowed("/abs/path"));
        assert!(is_borrowed("/"));
        assert!(is_borrowed(".."));
        assert!(is_borrowed("../../a"));
        assert!(is_borrowed("~alice/keys"));
        assert!(is_borrowed("rel/~/path"));
    }

    #[test]
    fn allocates_paths_that_change() {
        assert!(!is_borrowed(""));
        assert!(!is_borrowed("./"));
        assert!(!is_borrowed("./a"));
        assert!(!is_borrowed("a/./b"));
        assert!(!is_borrowed("a/../b"));
        assert!(!is_borrowed("/.."));
        assert!(!is_borrowed("a//b"));
        assert!(!is_borrowed("a/"));
        assert!(!is_borrowed("~"));
        assert!(!is_borrowed("~/keys"));
    }
}
