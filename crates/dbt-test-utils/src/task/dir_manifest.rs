use std::{ffi::OsStr, path::Path};

use async_trait::async_trait;
use dbt_common::stdfs;
use dbt_test_primitives::is_update_golden_files_mode;
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use super::{ProjectEnv, Task, TestEnv, TestError, TestResult};
use crate::task::goldie::{OutputNormalizer, diff_goldie};

// TODO: make this configurable per call site (extend via builder) instead of a
// hardcoded list. The baked-in `.git` entry is rarely what callers want
// uniformly — some tests need it filtered out entirely via a normalizer.
const SKIPPED_DIR_NAMES: &[&str] = &[".git"];

fn should_skip(file_name: &OsStr) -> bool {
    SKIPPED_DIR_NAMES
        .iter()
        .any(|name| file_name == OsStr::new(name))
}

/// Walk dir recursively, compute SHA256 per file, return sorted manifest string.
/// Skipped entries are recorded as `<path>: skipped`.
/// Symlinks are recorded as `<path>  symlink:<target>`.
pub fn compute_dir_manifest(dir: &Path) -> TestResult<String> {
    let mut entries = Vec::new();

    let mut walker = WalkDir::new(dir).sort_by_file_name().into_iter();
    while let Some(next) = walker.next() {
        let entry = next.map_err(|e| {
            TestError::new(format!(
                "Failed to walk directory '{}' while computing manifest: {e}",
                dir.display()
            ))
        })?;
        let path = entry.path();

        // Normalize to forward slashes for cross-platform golden file compatibility
        let relative = path
            .strip_prefix(dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");

        if should_skip(entry.file_name()) {
            entries.push(format!("{relative}: skipped"));
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }

        if entry.file_type().is_symlink() {
            let target = std::fs::read_link(path).map_err(|e| {
                TestError::new(format!(
                    "Failed to read symlink '{}' while computing manifest: {e}",
                    path.display()
                ))
            })?;
            let target_str = target.to_string_lossy().replace('\\', "/");
            entries.push(format!("{relative}  symlink:{target_str}"));
        } else if entry.file_type().is_file() {
            let contents = std::fs::read(path).map_err(|e| {
                TestError::new(format!(
                    "Failed to read file '{}' while computing manifest: {e}",
                    path.display()
                ))
            })?;
            let hash = Sha256::digest(&contents);
            entries.push(format!("{relative}  sha256:{hash:x}"));
        }
        // Skip directories — they're implicit from file paths
    }

    Ok(entries.join("\n"))
}

/// Compare manifest against golden file. Update if GOLDIE_UPDATE=1.
pub fn assert_golden_manifest(
    test_name: &str,
    manifest: &str,
    golden_dir: &Path,
) -> TestResult<()> {
    stdfs::create_dir_all(golden_dir)
        .map_err(|e| TestError::new(format!("Failed to create golden dir: {e}")))?;

    let golden_path = golden_dir.join(format!("{test_name}.manifest"));

    if is_update_golden_files_mode() {
        stdfs::write(&golden_path, manifest)
            .map_err(|e| TestError::new(format!("Failed to write golden manifest: {e}")))?;
        return Ok(());
    }

    if let Some(patch) = diff_goldie("manifest", manifest.to_string(), false, &golden_path, |g| g) {
        return Err(TestError::GoldieMismatch(vec![patch]));
    }

    Ok(())
}

/// Task that computes a directory checksum manifest and compares against golden file.
/// Plugs into TaskSeq like ExecuteAndCompareTelemetry does for OTEL tests.
/// Generic: works for any directory (dbt_packages, target/compiled, generated assets, etc.)
pub struct CompareDirManifest {
    /// Name for the golden manifest, must be explicit.
    pub name: String,
    /// Relative path under project dir to compare (e.g., "dbt_packages", "target/compiled")
    pub dir: String,
    /// Extra normalizers applied in order to the manifest string before golden diff.
    pub normalizers: Vec<OutputNormalizer>,
}

impl CompareDirManifest {
    pub fn new(name: impl Into<String>, dir: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            dir: dir.into(),
            normalizers: vec![],
        }
    }

    /// Set extra output normalizers applied before golden comparison.
    pub fn with_normalizers(mut self, normalizers: Vec<OutputNormalizer>) -> Self {
        self.normalizers = normalizers;
        self
    }
}

#[async_trait]
impl Task for CompareDirManifest {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        let target_path = project_env.absolute_project_dir.join(&self.dir);
        if !target_path.exists() {
            return Err(TestError::new(format!(
                "Directory '{}' does not exist at '{}'",
                self.dir,
                target_path.display()
            )));
        }
        let mut manifest = compute_dir_manifest(&target_path)?;
        for normalizer in &self.normalizers {
            manifest = normalizer(manifest);
        }
        let task_suffix = if task_index > 0 {
            format!("_{task_index}")
        } else {
            String::new()
        };
        assert_golden_manifest(
            &format!("{}{}", self.name, task_suffix),
            &manifest,
            &test_env.golden_dir,
        )
    }

    fn is_counted(&self) -> bool {
        false
    }
}
