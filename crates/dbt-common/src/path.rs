use std::{
    borrow::{Borrow, Cow},
    ffi::{OsStr, OsString},
    fmt::Debug,
    hash::Hash,
    ops::Deref,
    path::{Component, Display, Path, PathBuf},
};

/// Compute the `target/compiled` or `target/run` output path for a snapshot node.
///
/// Snapshots always use the many-to-one nested layout: `out_dir/package/original_file_path/name.sql`.
/// A single .sql file can contain multiple snapshot blocks, so the basename heuristic in
/// `get_target_write_path` would create both a file and a directory at the same path (EISDIR)
/// when one snapshot's name matches the source filename. The same collision also bites when a
/// `target/` directory populated by dbt-core v1 is reused by Fusion (dbt-core#15692), so both
/// output subdirectories must use this layout — `out_dir` selects which one.
///
/// Mirrors dbt-core `SnapshotNode.get_target_write_path` (dbt-core#12693), which likewise applies
/// to both the "compiled" and "run" subdirectories.
pub fn get_snapshot_write_path(
    out_dir: &Path,
    package_name: &str,
    original_file_path: &Path,
    name: &str,
) -> PathBuf {
    out_dir
        .join(package_name)
        .join(original_file_path)
        .join(format!("{name}.sql"))
}

/// Compute the absolute write path for a node inside `target/compiled` or `target/run`,
/// matching dbt-core's `ParsedNode.get_target_write_path` behavior.
///
/// Returns `{out_dir}/{package_name}/{path_segment}` where `path_segment` is:
/// - `original_file_path` when its filename matches `path`'s filename (one-to-one, e.g. models)
/// - `original_file_path/{path}` otherwise (many-to-one, e.g. generic tests from schema YAML)
///
/// Exception: if `in_dir.join(original_file_path)` resolves outside `in_dir`, `path` is used
/// directly instead.
///
/// The escape case arises when `--target-path` points outside the project root (which dbt-core
/// permits). Fusion's generic test nodes store `original_file_path` as the path to the generated
/// SQL file; when that file lives in an out-of-project target directory, the path computed
/// relative to the project root contains `..` segments. Embedding those segments into the
/// compiled/run path produces nonsense, so we fall back to `path` (e.g. `generic_tests/foo.sql`)
/// which is always relative to the project root and gives a clean result.
///
/// `in_dir` must be an absolute path (the project root). `out_dir` is the specific output
/// subdirectory — e.g. `io_args.out_dir.join("compiled")` or `io_args.out_dir.join("run")`.
///
/// dbt-core reference: `contracts/graph/nodes.py` `ParsedNode.get_target_write_path`
pub fn get_target_write_path(
    in_dir: &Path,
    out_dir: &Path,
    package_name: &str,
    path: &Path,
    original_file_path: &Path,
) -> PathBuf {
    let abs_ofp = DbtPath::normalize(&in_dir.join(original_file_path)).to_path_buf();
    let escapes_root = !abs_ofp.starts_with(in_dir);
    let path_segment = if escapes_root {
        path.to_path_buf()
    } else if path.file_name() == original_file_path.file_name() {
        original_file_path.to_path_buf()
    } else {
        original_file_path.join(path)
    };
    out_dir.join(package_name).join(path_segment)
}

use serde::{Deserialize, Serialize};

/// Template suffixes that may trail a resource file's real extension
/// (`model.sql.j2`). Mirrors dbt-core's `JINJA_FILE_EXTENSIONS`.
pub const JINJA_FILE_EXTENSIONS: &[&str] = &["j2", "jinja2", "jinja"];

fn is_jinja_extension(ext: &str) -> bool {
    JINJA_FILE_EXTENSIONS
        .iter()
        .any(|jinja_ext| ext.eq_ignore_ascii_case(jinja_ext))
}

/// The resource extension of a file, looking through at most one template suffix:
/// `a.sql.j2` -> `sql`, `a.sql` -> `sql`, `a.j2` -> `j2` (no real extension underneath).
pub fn resource_extension(path: &Path) -> Option<&str> {
    let ext = path.extension()?.to_str()?;
    if !is_jinja_extension(ext) {
        return Some(ext);
    }
    // `a.sql.j2` has file_stem `a.sql`; only look through the suffix when a real
    // extension remains underneath, so a bare `a.jinja` keeps its own extension.
    match path.file_stem()?.to_str()?.rsplit_once('.') {
        Some((base, inner)) if !base.is_empty() && !inner.is_empty() => Some(inner),
        _ => Some(ext),
    }
}

/// The node name derived from a file path, stripping a template suffix along with
/// the resource extension: `a.sql` -> `a`, `a.sql.j2` -> `a`.
pub fn node_name_from_path(path: &Path) -> Option<&str> {
    let stem = path.file_stem()?.to_str()?;
    if path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(is_jinja_extension)
        && let Some((base, _)) = stem.rsplit_once('.')
        && !base.is_empty()
    {
        return Some(base);
    }
    Some(stem)
}

/// Strip the first matching resource-root prefix (e.g. `models`) from a
/// package-relative path, yielding the path dbt exposes to Jinja as
/// `model.path` (relative to the resource directory).
///
/// dbt-core stores a node's `path` relative to the configured resource root
/// (e.g. `staging/orders.sql`), distinct from `original_file_path` which stays
/// package-relative (e.g. `models/staging/orders.sql`). Fusion keeps the
/// package-relative form internally (it is joined with package roots for file
/// I/O and parse caching), so the resource-relative form is derived on demand
/// for the Jinja `model.path` value.
///
/// Returns the input unchanged when no configured root matches or when the path
/// is exactly a resource root (empty remainder).
///
/// dbt-core reference: `FilePath.relative_path`.
pub fn strip_resource_paths(path: &Path, resource_paths: &[String]) -> PathBuf {
    for resource_path in resource_paths {
        let resource_path = Path::new(resource_path);
        if let Ok(stripped) = path.strip_prefix(resource_path)
            && !stripped.as_os_str().is_empty()
        {
            return stripped.to_path_buf();
        }
    }
    path.to_path_buf()
}

/// Self-normalizing path. Wrapper around [PathBuf].
/// Case-sensitivity for equality ([PartialEq] and [Eq]) is determined by the OS.
/// Comparisons ([Ord] and [PartialOrd]) are always case-sensitive.
/// Back-slashes '\' are replaced with forward-slashes '/'.
/// Use this instead of [PathBuf].
#[derive(Clone, Debug, Default, Ord, PartialOrd, Serialize, Deserialize)]
pub struct DbtPath(PathBuf);

enum PrefixDoubleSlash {
    None,
    Colon,
    StartsWith,
}

#[allow(unused)]
impl DbtPath {
    fn normalize(value: &Path) -> DbtPath {
        let mut collapse_parent_depth = 0;
        let mut ret = PathBuf::new();

        // Replace slashes first before we get components because it will incorrectly
        // separate the components if we do not.
        // Collapses parents except if the parents are first.
        let value = PathBuf::from(value.to_string_lossy().replace("\\", "/"));

        let value_string = value.to_string_lossy();
        let prefix_double_slash = if value_string.rmatches("://").count() == 1
            && value_string.rmatches(":/").count() == 1
        {
            PrefixDoubleSlash::Colon
        } else if value_string.starts_with("//") {
            PrefixDoubleSlash::StartsWith
        } else {
            PrefixDoubleSlash::None
        };

        for component in value.components() {
            match component {
                Component::Prefix(prefix_component) => {
                    collapse_parent_depth += 1;
                    ret.push(prefix_component.as_os_str())
                }
                Component::RootDir => {
                    collapse_parent_depth += 1;
                    ret.push(component.as_os_str());
                }
                Component::CurDir => {
                    collapse_parent_depth += 1;
                    ret.push(".");
                }
                Component::ParentDir => {
                    if collapse_parent_depth > 0 {
                        ret.pop();
                        collapse_parent_depth -= 1;
                    } else {
                        ret.push("..");
                    }
                }
                Component::Normal(c) => {
                    collapse_parent_depth += 1;
                    ret.push(c);
                }
            }
        }

        let ret = PathBuf::from(ret.to_string_lossy().replace("\\", "/"));
        let ret = match prefix_double_slash {
            PrefixDoubleSlash::None => ret,
            PrefixDoubleSlash::Colon => {
                PathBuf::from(ret.to_string_lossy().replacen(":/", "://", 1))
            }
            PrefixDoubleSlash::StartsWith => {
                PathBuf::from("/".to_string() + &ret.to_string_lossy())
            }
        };

        Self(ret)
    }

    pub fn new() -> Self {
        Self(PathBuf::new())
    }

    /// See [std::path::absolute] for documentation.
    pub fn absolute<P: AsRef<Path>>(path: P) -> std::io::Result<Self> {
        Ok(Self::normalize(&std::path::absolute(path)?))
    }

    /// See [PathBuf::as_path] for documentation.
    pub fn as_path(&self) -> &Path {
        self.0.as_path()
    }

    pub fn to_path_buf(&self) -> PathBuf {
        self.0.clone()
    }

    /// Resolves this (relative) path against `base`, returning an absolute [`PathBuf`].
    pub fn to_absolute(&self, base: &Path) -> PathBuf {
        base.join(self.0.as_path())
    }

    /// See [Path::file_name] for documentation.
    pub fn file_name(&self) -> Option<&OsStr> {
        self.0.file_name()
    }

    /// See [Path::is_relative] for documentation.
    pub fn is_relative(&self) -> bool {
        self.0.is_relative()
    }

    /// See [Path::is_absolute] for documentation.
    pub fn is_absolute(&self) -> bool {
        self.0.is_absolute()
    }

    /// See [Path::has_root] for documentation.
    pub fn has_root(&self) -> bool {
        self.0.has_root()
    }

    /// See [Path::extension] for documentation.
    pub fn extension(&self) -> Option<&OsStr> {
        self.0.extension()
    }

    /// See [Path::join] for documentation.
    pub fn join<P: AsRef<Path>>(&self, path: P) -> Self {
        Self::normalize(&self.0.join(path))
    }

    /// Case-sensitivity based on the OS.
    pub fn has_extension(&self, ext: &str) -> bool {
        self.extension().is_some_and(|x| {
            if cfg!(target_os = "linux") {
                x.eq(ext)
            } else {
                x.eq_ignore_ascii_case(ext)
            }
        })
    }

    /// Case-sensitivity based on the OS.
    pub fn get_relative_path<P: AsRef<Path>>(&self, base_path: P) -> Option<Self> {
        Some(Self::normalize(&diff_paths_os_ascii_case(
            &self.0,
            &Self::normalize(base_path.as_ref()).0,
        )?))
    }

    /// See [Path::to_str] for documentation.
    pub fn to_str(&self) -> Option<&str> {
        self.0.to_str()
    }

    /// See [Path::to_string_lossy] for documentation.
    pub fn to_string_lossy(&self) -> Cow<'_, str> {
        self.0.to_string_lossy()
    }

    /// See [Path::display] for documentation.
    pub fn display(&self) -> Display<'_> {
        self.0.display()
    }

    pub fn is_empty(&self) -> bool {
        self.as_path().as_os_str().is_empty()
    }
}

impl From<String> for DbtPath {
    fn from(value: String) -> Self {
        Self::normalize(&PathBuf::from(value))
    }
}

impl From<&String> for DbtPath {
    fn from(value: &String) -> Self {
        Self::normalize(&PathBuf::from(value))
    }
}

impl From<&str> for DbtPath {
    fn from(value: &str) -> Self {
        Self::normalize(&PathBuf::from(value))
    }
}

impl From<PathBuf> for DbtPath {
    fn from(value: PathBuf) -> Self {
        Self::normalize(&value)
    }
}

impl From<&PathBuf> for DbtPath {
    fn from(value: &PathBuf) -> Self {
        Self::normalize(value)
    }
}

impl From<OsString> for DbtPath {
    fn from(value: OsString) -> Self {
        Self::normalize(&PathBuf::from(value))
    }
}

impl From<&Path> for DbtPath {
    fn from(value: &Path) -> Self {
        Self::normalize(&PathBuf::from(value))
    }
}

impl AsRef<Path> for DbtPath {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

impl AsRef<OsStr> for DbtPath {
    fn as_ref(&self) -> &OsStr {
        self.0.as_ref()
    }
}

impl Borrow<Path> for DbtPath {
    fn borrow(&self) -> &Path {
        self.0.deref()
    }
}

impl Deref for DbtPath {
    type Target = Path;
    fn deref(&self) -> &Path {
        self.0.deref()
    }
}

impl PartialEq for DbtPath {
    /// Case-sensitivity based on the OS.
    fn eq(&self, other: &Self) -> bool {
        let str1 = self.0.to_string_lossy();
        let str2 = other.0.to_string_lossy();
        if cfg!(target_os = "linux") {
            str1.eq(&str2)
        } else {
            str1.eq_ignore_ascii_case(&str2)
        }
    }
}
impl Eq for DbtPath {}

impl Hash for DbtPath {
    /// Case-sensitivity based on the OS.
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        let str = self.0.to_string_lossy();
        if cfg!(target_os = "linux") {
            str.hash(state);
        } else {
            for c in str.chars() {
                c.to_ascii_lowercase().hash(state);
            }
            state.write_u8(0xff);
        }
    }
}

impl std::fmt::Display for DbtPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0.display(), f)
    }
}

/// Is the given character a file-system reserved character?
/// We are pessimistic and consider any character is a
/// file-system reserved character if it is reserved on Windows.
pub fn is_file_system_reserved_character(c: char) -> bool {
    matches!(c, '<' | '>' | ':' | '\"' | '/' | '\\' | '|' | '?' | '*')
}

/// Equality is determined by the OS.
fn component_eq_os_ascii_case(component1: &Component<'_>, component2: &Component<'_>) -> bool {
    match (component1, component2) {
        (Component::CurDir, Component::CurDir)
        | (Component::ParentDir, Component::ParentDir)
        | (Component::RootDir, Component::RootDir) => true,
        (Component::Normal(str1), Component::Normal(str2)) => {
            if cfg!(target_os = "linux") {
                str1.eq(str2)
            } else {
                str1.eq_ignore_ascii_case(str2)
            }
        }
        (Component::Prefix(prefix1), Component::Prefix(prefix2)) => {
            if cfg!(target_os = "linux") {
                prefix1.as_os_str().eq(prefix2.as_os_str())
            } else {
                prefix1
                    .as_os_str()
                    .eq_ignore_ascii_case(prefix2.as_os_str())
            }
        }
        _ => false,
    }
}

/// Construct a relative path from a provided base directory path to the provided path.
/// Modified version of https://docs.rs/pathdiff/0.2.3/src/pathdiff/lib.rs.html#43 to support case sensitivity based on the OS.
fn diff_paths_os_ascii_case<P, B>(path: P, base: B) -> Option<PathBuf>
where
    P: AsRef<Path>,
    B: AsRef<Path>,
{
    let path = path.as_ref();
    let base = base.as_ref();

    if path.is_absolute() != base.is_absolute() {
        if path.is_absolute() {
            Some(PathBuf::from(path))
        } else {
            None
        }
    } else {
        let mut ita = path.components();
        let mut itb = base.components();
        let mut comps: Vec<Component> = vec![];
        loop {
            match (ita.next(), itb.next()) {
                (None, None) => break,
                (Some(a), None) => {
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
                (None, _) => comps.push(Component::ParentDir),
                (Some(a), Some(b)) if comps.is_empty() && component_eq_os_ascii_case(&a, &b) => (),
                (Some(a), Some(Component::CurDir)) => comps.push(a),
                (Some(_), Some(Component::ParentDir)) => return None,
                (Some(a), Some(_)) => {
                    comps.push(Component::ParentDir);
                    for _ in itb {
                        comps.push(Component::ParentDir);
                    }
                    comps.push(a);
                    comps.extend(ita.by_ref());
                    break;
                }
            }
        }
        Some(comps.iter().map(|c| c.as_os_str()).collect())
    }
}

/// Compare two strings treating `/` and `\` as equivalent.
/// Zero-allocation alternative to `a.replace('\\', "/") == b.replace('\\', "/")`.
pub fn path_separator_eq(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.chars()
            .zip(b.chars())
            .all(|(ca, cb)| ca == cb || (ca == '\\' && cb == '/') || (ca == '/' && cb == '\\'))
}

/// Based off of https://docs.rs/pathdiff/0.2.3/src/pathdiff/lib.rs.html#171
#[cfg(test)]
mod tests {
    use std::hash::{DefaultHasher, Hasher};

    use super::*;

    #[test]
    fn test_absolute() {
        fn abs(path: &str) -> String {
            // Absolute paths look different on Windows vs Unix.
            if cfg!(target_os = "windows") {
                format!("C:\\{}", path)
            } else {
                format!("/{}", path)
            }
        }

        assert_diff_paths(&abs("foo"), &abs("bar"), Some("../foo"));
        assert_diff_paths(&abs("foo"), "bar", Some(&abs("foo")));
        assert_diff_paths("foo", &abs("bar"), None);
        assert_diff_paths("foo", "bar", Some("../foo"));
    }

    #[test]
    fn test_identity() {
        assert_diff_paths(".", ".", Some(""));
        assert_diff_paths("../foo", "../foo", Some(""));
        assert_diff_paths("./foo", "./foo", Some(""));
        assert_diff_paths("/foo", "/foo", Some(""));
        assert_diff_paths("foo", "foo", Some(""));

        assert_diff_paths("../foo/bar/baz", "../foo/bar/baz", Some(""));
        assert_diff_paths("foo/bar/baz", "foo/bar/baz", Some(""));
    }

    #[test]
    fn test_subset() {
        assert_diff_paths("foo", "fo", Some("../foo"));
        assert_diff_paths("fo", "foo", Some("../fo"));
    }

    #[test]
    fn test_empty() {
        assert_diff_paths("", "", Some(""));
        assert_diff_paths("foo", "", Some("foo"));
        assert_diff_paths("", "foo", Some(".."));
    }

    #[test]
    fn test_relative() {
        assert_diff_paths("../foo", "../bar", Some("../foo"));
        assert_diff_paths("../foo", "../foo/bar/baz", Some("../.."));
        assert_diff_paths("../foo/bar/baz", "../foo", Some("bar/baz"));

        assert_diff_paths("foo/bar/baz", "foo", Some("bar/baz"));
        assert_diff_paths("foo/bar/baz", "foo/bar", Some("baz"));
        assert_diff_paths("foo/bar/baz", "foo/bar/baz", Some(""));
        assert_diff_paths("foo/bar/baz", "foo/bar/baz/", Some(""));

        assert_diff_paths("foo/bar/baz/", "foo", Some("bar/baz"));
        assert_diff_paths("foo/bar/baz/", "foo/bar", Some("baz"));
        assert_diff_paths("foo/bar/baz/", "foo/bar/baz", Some(""));
        assert_diff_paths("foo/bar/baz/", "foo/bar/baz/", Some(""));

        assert_diff_paths("foo/bar/baz", "foo/", Some("bar/baz"));
        assert_diff_paths("foo/bar/baz", "foo/bar/", Some("baz"));
        assert_diff_paths("foo/bar/baz", "foo/bar/baz", Some(""));
    }

    #[test]
    fn test_current_directory() {
        assert_diff_paths(".", "foo", Some("../."));
        assert_diff_paths("foo", ".", Some("foo"));
        assert_diff_paths("/foo", "/.", Some("foo"));
    }

    #[test]
    fn test_relative_case_sensitivity() {
        if cfg!(target_os = "linux") {
            assert_diff_paths("FOO/bar/baz", "foo/bar", Some("../../FOO/bar/baz"));
            assert_diff_paths("FOO/bar/BAZ", "foo/bar", Some("../../FOO/bar/BAZ"));
        } else {
            assert_diff_paths("FOO/bar/baz", "foo/bar", Some("baz"));
            assert_diff_paths("FOO/bar/BAZ", "foo/bar", Some("BAZ"));
        }
    }

    #[test]
    fn test_case_sensitivity_eq_for_dbt_path() {
        if cfg!(target_os = "linux") {
            assert!(DbtPath::from("chicken") == DbtPath::from("chicken"));
            assert!(DbtPath::from("CHICKEN") != DbtPath::from("chicken"));
        } else {
            assert!(DbtPath::from("chicken") == DbtPath::from("chicken"));
            assert!(DbtPath::from("CHICKEN") == DbtPath::from("chicken"));
        }
    }

    #[test]
    fn test_case_sensitivity_hash_for_dbt_path() {
        let lowercase_path = DbtPath::from("chicken");
        let uppercase_path = DbtPath::from("CHICKEN");

        let mut lowercase_hasher = DefaultHasher::new();
        let mut uppercase_hasher = DefaultHasher::new();

        lowercase_path.hash(&mut lowercase_hasher);
        uppercase_path.hash(&mut uppercase_hasher);

        if cfg!(target_os = "linux") {
            assert_ne!(lowercase_hasher.finish(), uppercase_hasher.finish());
        } else {
            assert_eq!(lowercase_hasher.finish(), uppercase_hasher.finish());
        }

        assert_ne!(lowercase_hasher.finish(), 0);
        assert_ne!(uppercase_hasher.finish(), 0);
    }

    #[test]
    fn test_case_sensitivity_with_non_utf8_characters() {
        assert!(DbtPath::from("chicken/ñ") != DbtPath::from("chicken/À"));

        if cfg!(target_os = "linux") {
            assert!(DbtPath::from("chicken/À") == DbtPath::from("chicken/À"));
            assert!(DbtPath::from("CHICKEN/À") != DbtPath::from("chicken/À"));
        } else {
            assert!(DbtPath::from("chicken/À") == DbtPath::from("chicken/À"));
            assert!(DbtPath::from("CHICKEN/À") == DbtPath::from("chicken/À"));
        }
    }

    fn assert_diff_paths(path: &str, base: &str, expected: Option<&str>) {
        assert_eq!(
            diff_paths_os_ascii_case(path, base),
            expected.map(|s| s.into())
        );
    }

    #[test]
    fn test_resource_extension() {
        fn ext(p: &str) -> Option<&str> {
            resource_extension(Path::new(p))
        }
        assert_eq!(ext("models/a.sql"), Some("sql"));
        assert_eq!(ext("models/a.sql.j2"), Some("sql"));
        assert_eq!(ext("models/a.sql.jinja2"), Some("sql"));
        assert_eq!(ext("models/a.sql.jinja"), Some("sql"));
        assert_eq!(ext("models/a.SQL.J2"), Some("SQL"));
        assert_eq!(ext("models/a.b.sql.j2"), Some("sql"));
        assert_eq!(ext("docs/a.md.j2"), Some("md"));
        assert_eq!(ext("models/a.py"), Some("py"));
        assert_eq!(ext("seeds/a.csv"), Some("csv"));
        // No real extension underneath the template suffix.
        assert_eq!(ext("models/a.j2"), Some("j2"));
        assert_eq!(ext("macros/a.jinja"), Some("jinja"));
        assert_eq!(ext("models/a"), None);
        assert_eq!(ext("models/.hidden"), None);
        assert_eq!(ext("models/.hidden.j2"), Some("j2"));
    }

    #[test]
    fn test_node_name_from_path() {
        fn name(p: &str) -> Option<&str> {
            node_name_from_path(Path::new(p))
        }
        assert_eq!(name("models/a.sql"), Some("a"));
        assert_eq!(name("models/a.sql.j2"), Some("a"));
        assert_eq!(name("models/a.sql.jinja2"), Some("a"));
        assert_eq!(name("models/a.sql.jinja"), Some("a"));
        assert_eq!(name("models/a.SQL.J2"), Some("a"));
        assert_eq!(name("models/a.b.sql.j2"), Some("a.b"));
        assert_eq!(name("docs/a.md.j2"), Some("a"));
        assert_eq!(name("models/a.py"), Some("a"));
        assert_eq!(name("models/a.j2"), Some("a"));
        assert_eq!(name("macros/a.jinja"), Some("a"));
        assert_eq!(name("models/a"), Some("a"));
        assert_eq!(name("models/.hidden"), Some(".hidden"));
        assert_eq!(name("models/.hidden.j2"), Some(".hidden"));
    }

    #[test]
    fn test_path_separator_eq() {
        assert!(path_separator_eq("foo/bar", "foo/bar"));
        assert!(path_separator_eq("foo/bar", "foo\\bar"));
        assert!(path_separator_eq("foo\\bar", "foo/bar"));
        assert!(!path_separator_eq("foo/bar", "foo/baz"));
        assert!(!path_separator_eq("foo/bar", "foo/bar/baz"));
    }

    #[test]
    fn test_to_absolute() {
        let relative = DbtPath::from("models/my_model.sql");
        let base = Path::new("/projects/my_project");
        let absolute = relative.to_absolute(base);
        assert_eq!(
            absolute,
            PathBuf::from("/projects/my_project/models/my_model.sql")
        );
    }

    #[test]
    fn test_is_empty() {
        let empty = DbtPath::default();
        let relative = DbtPath::from("models/my_model.sql");
        assert!(empty.is_empty());
        assert!(!relative.is_empty());
    }

    #[test]
    fn normalize_back_slash_to_forward_slash() {
        let path = DbtPath::from("models\\my_model.sql");
        assert_eq!("models/my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn normalize_back_slash_to_forward_slash_2() {
        let path = DbtPath::from("C:\\models\\my_model.sql");
        assert_eq!("C:/models/my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn normalize_back_slash_to_forward_slash_3() {
        let path = DbtPath::from("C:\\models\\..\\my_model.sql");
        assert_eq!("C:/my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn normalize_back_slash_to_forward_slash_4() {
        let path = DbtPath::from("C:/models\\../my_model.sql");
        assert_eq!("C:/my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first() {
        let path = DbtPath::from("../my_model.sql");
        assert_eq!("../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_2() {
        let path = DbtPath::from("../../my_model.sql");
        assert_eq!("../../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_3() {
        let path = DbtPath::from("../../folder/../my_model.sql");
        assert_eq!("../../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_4() {
        let path = DbtPath::from("test/../../my_model.sql");
        assert_eq!("../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_5() {
        let path = DbtPath::from("test/test2/../../../my_model.sql");
        assert_eq!("../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_6() {
        let path = DbtPath::from("test/test2/../../my_model.sql");
        assert_eq!("my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_7() {
        let path = DbtPath::from("../../folder/../../my_model.sql");
        assert_eq!("../../../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_8() {
        let path = DbtPath::from("/../my_model.sql");
        assert_eq!("/my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_9() {
        let path = DbtPath::from("/../../my_model.sql");
        assert_eq!("/../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_10() {
        let path = DbtPath::from("./../my_model.sql");
        assert_eq!("my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn should_not_collapse_parent_if_it_is_first_11() {
        let path = DbtPath::from("./../../my_model.sql");
        assert_eq!("../my_model.sql", path.to_string_lossy());
    }

    #[test]
    fn custom_prefix_should_have_expected_forward_slahses() {
        let path = DbtPath::from("jaffle_shop://models//schema.yml");
        assert_eq!("jaffle_shop://models/schema.yml", path.to_string_lossy());
    }

    #[test]
    fn custom_prefix_should_have_expected_forward_slahses_2() {
        let path = DbtPath::from("jaffle_shop:\\\\models\\\\schema.yml");
        assert_eq!("jaffle_shop://models/schema.yml", path.to_string_lossy());
    }

    #[test]
    fn custom_prefix_should_have_expected_forward_slahses_3() {
        let path = DbtPath::from("//models//schema.yml");
        assert_eq!("//models/schema.yml", path.to_string_lossy());
    }
}
