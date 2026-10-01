//! Simple golden file testing for Rust.
//!
//! A fork of `goldie` 0.5.0 that emits mismatches as unified diffs wrapped in
//! `<<<<<<<< BEGIN PATCH` / `>>>>>>>> END PATCH` markers, matching the
//! integration-test goldie mechanism in `dbt-test-utils` (see
//! `fs/sa/crates/dbt-test-utils/src/task/goldie.rs` and `task_seq.rs`). This
//! makes failures consumable by `apply_golden_patches.py` /
//! `cargo xtask apply-ci-goldies` without re-running tests locally.
//!
//! ```text
//! dbt_goldie::assert!(text);
//! ```
//!
//! The golden filename is automatically determined based on the test file and
//! test function name (identical to upstream `goldie`, so existing golden files
//! keep working). Run tests with `GOLDIE_UPDATE=true` to update golden files.

use std::collections::BTreeMap;
use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::Arc;
use std::sync::Mutex;

use anyhow::{Context, Result};
use once_cell::sync::Lazy;
use serde::Deserialize;

/// Assert the golden file matches.
#[macro_export]
macro_rules! assert {
    ($actual:expr) => {{
        let g = $crate::_new_goldie!();
        if let Err(err) = g.assert($actual) {
            ::std::panic!("{}", err);
        }
    }};
}

/// Assert the golden file matches the debug output.
#[macro_export]
macro_rules! assert_debug {
    ($actual:expr) => {{
        let g = $crate::_new_goldie!();
        if let Err(err) = g.assert_debug($actual) {
            ::std::panic!("{}", err);
        }
    }};
}

/// Constructs a new goldie instance.
///
/// Not public API.
#[doc(hidden)]
#[macro_export]
macro_rules! _new_goldie {
    () => {{
        let source_file = $crate::cargo_workspace_dir(env!("CARGO_MANIFEST_DIR")).join(file!());
        let function_path = $crate::_function_path!();
        $crate::Goldie::new(source_file, function_path)
    }};
}

/// Returns the fully qualified path to the current item.
///
/// Goldie uses this to get the name of the test function.
///
/// Not public API.
#[doc(hidden)]
#[macro_export]
macro_rules! _function_path {
    () => {{
        fn f() {}
        fn type_name_of_val<T>(_: T) -> &'static str {
            ::std::any::type_name::<T>()
        }
        let mut name = type_name_of_val(f).strip_suffix("::f").unwrap_or("");
        while let Some(rest) = name.strip_suffix("::{{closure}}") {
            name = rest;
        }
        name
    }};
}

#[derive(Debug)]
pub struct Goldie {
    /// The path to the golden file.
    golden_file: PathBuf,
    /// Whether to update the golden file if it doesn't match.
    update: bool,
}

impl Goldie {
    /// Construct a new golden file tester.
    ///
    /// Where
    /// - `source_file` is path to the source file that the test resides in.
    /// - `function_path` is the full path to the function. e.g.
    ///   `crate::module::tests::function_name`.
    pub fn new(source_file: impl AsRef<Path>, function_path: impl AsRef<str>) -> Self {
        Self::new_impl(source_file.as_ref(), function_path.as_ref())
    }

    fn new_impl(source_file: &Path, function_path: &str) -> Self {
        let (_, name) = function_path.rsplit_once("::").unwrap();

        let golden_file = {
            let mut p = source_file.parent().unwrap().to_owned();
            p.push("testdata");
            p.push(name);
            p.set_extension("golden");
            p
        };

        let update = matches!(
            env::var("GOLDIE_UPDATE").ok().as_deref(),
            Some("1" | "true")
        );

        Self {
            golden_file,
            update,
        }
    }

    #[track_caller]
    pub fn assert(&self, actual: impl AsRef<str>) -> Result<()> {
        let actual = actual.as_ref();
        if self.update {
            let dir = self.golden_file.parent().unwrap();
            fs::create_dir_all(dir)?;
            fs::write(&self.golden_file, actual)?;
            return Ok(());
        }

        let exists = self.golden_file.exists();
        let golden = if exists {
            fs::read_to_string(&self.golden_file)
                .with_context(|| self.error("failed to read golden file"))?
        } else {
            String::new()
        };

        if exists && golden == actual {
            return Ok(());
        }

        // Emit a unified diff wrapped in BEGIN/END PATCH markers, mirroring
        // `dbt-test-utils::task::goldie::diff_goldie` + `task_seq`. Paths are
        // made workspace-relative and prefixed `i/` (original) / `w/`
        // (modified) so `git apply -p1` (and `apply_golden_patches.py`) can
        // apply them directly. A missing golden uses `/dev/null` so the patch
        // creates the file.
        let rel =
            relative_to_git_root(&self.golden_file).unwrap_or_else(|| self.golden_file.clone());
        let original_filename = if exists {
            PathBuf::from("i").join(&rel).to_string_lossy().to_string()
        } else {
            "/dev/null".to_string()
        };
        let modified_filename = PathBuf::from("w").join(&rel).to_string_lossy().to_string();

        let patch = diffy::DiffOptions::new()
            .set_original_filename(original_filename)
            .set_modified_filename(modified_filename)
            .create_patch(&golden, actual);

        eprintln!("<<<<<<<< BEGIN PATCH");
        eprintln!("{patch}");
        eprintln!(">>>>>>>> END PATCH");
        panic!(
            "golden file `{}` does not match; see BEGIN PATCH block above",
            rel.display()
        );
    }

    #[track_caller]
    pub fn assert_debug(&self, actual: impl std::fmt::Debug) -> Result<()> {
        self.assert(format!("{actual:#?}"))
    }

    fn error(&self, msg: &str) -> String {
        format!(
            "\n\n{msg}: {}\nrun with `GOLDIE_UPDATE=1` to regenerate the golden file\n\n",
            self.golden_file.display(),
        )
    }
}

/// Return `path` relative to the nearest enclosing `.git` directory, or `None`
/// if no such ancestor exists. Vendored from
/// `dbt-test-utils::task::utils::relative_to_git_root` to avoid pulling the
/// heavier `dbt-test-utils` dep tree into test-only consumers.
fn relative_to_git_root(path: &Path) -> Option<PathBuf> {
    const GIT_DIR: &str = ".git";
    let mut current = path;
    while let Some(parent) = current.parent() {
        if parent.join(GIT_DIR).is_dir() {
            return path.strip_prefix(parent).ok().map(|p| p.to_path_buf());
        }
        current = parent;
    }
    None
}

/// Returns the Cargo workspace dir for the given manifest dir.
///
/// Not public API.
#[doc(hidden)]
pub fn cargo_workspace_dir(manifest_dir: &str) -> PathBuf {
    static DIRS: Lazy<Mutex<BTreeMap<String, Arc<Path>>>> =
        Lazy::new(|| Mutex::new(BTreeMap::new()));

    let mut dirs = DIRS.lock().unwrap();

    if let Some(dir) = dirs.get(manifest_dir) {
        return dir.to_path_buf();
    }

    let dir = env::var("CARGO_WORKSPACE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            #[derive(Deserialize)]
            struct Manifest {
                workspace_root: PathBuf,
            }
            let cargo = env::var_os("CARGO");
            let cargo = cargo.as_deref().unwrap_or_else(|| OsStr::new("cargo"));
            let output = process::Command::new(cargo)
                .args(["metadata", "--format-version=1", "--no-deps"])
                .current_dir(manifest_dir)
                .output()
                .unwrap();
            let manifest: Manifest = serde_json::from_slice(&output.stdout).unwrap();
            manifest.workspace_root
        });
    dirs.insert(
        String::from(manifest_dir),
        dir.clone().into_boxed_path().into(),
    );

    dir
}
