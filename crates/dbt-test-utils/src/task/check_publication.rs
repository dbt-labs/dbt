use async_trait::async_trait;
use dbt_common::stdfs;
use dbt_test_primitives::is_update_golden_files_mode;
use serde_json::Value;
use std::path::PathBuf;

use super::{ProjectEnv, Task, TestEnv, TestResult};

/// JSON paths (dot-notation, `*` as wildcard) to strip before comparing publication artifacts.
/// These fields are volatile and would cause spurious test failures.
const VOLATILE_PATHS: &[&str] = &[
    "metadata.generated_at",
    "metadata.invocation_id",
    "metadata.dbt_version",
    "public_models.*.generated_at",
];

fn remove_json_path(value: &mut Value, path: &str) {
    let parts: Vec<&str> = path.split('.').collect();
    remove_json_path_parts(value, &parts);
}

fn remove_json_path_parts(value: &mut Value, parts: &[&str]) {
    if parts.is_empty() {
        return;
    }
    if let Value::Object(map) = value {
        if parts.len() == 1 {
            map.remove(parts[0]);
        } else if parts[0] == "*" {
            let field_to_remove = parts[1..].join(".");
            for obj in map.values_mut() {
                remove_json_path(obj, &field_to_remove);
            }
        } else if let Some(next) = map.get_mut(parts[0]) {
            remove_json_path_parts(next, &parts[1..]);
        }
    }
    if let Value::Array(arr) = value {
        if parts[0] == "*" && parts.len() > 1 {
            let field_to_remove = parts[1..].join(".");
            for item in arr.iter_mut() {
                remove_json_path(item, &field_to_remove);
            }
        }
    }
}

/// Checks the publication artifact written during a compile/parse run against a golden file.
///
/// The artifact is read from `project_env.absolute_project_dir / rel_artifact_path`, which
/// matches the path set via `DBT_CLOUD_PUBLICATION_FILE_PATH`. Volatile fields (timestamps,
/// invocation ID, dbt version) are stripped before comparison.
///
/// In update mode (`GOLDIE_UPDATE=1`), the normalized artifact is written to the golden dir.
pub struct CheckPublicationArtifact {
    /// Relative path to the publication artifact within the project dir.
    /// Should match what is passed as `DBT_CLOUD_PUBLICATION_FILE_PATH`.
    pub rel_artifact_path: PathBuf,
    /// File name for the golden file, stored in `test_env.golden_dir`.
    pub golden_name: String,
}

impl CheckPublicationArtifact {
    pub fn new(rel_artifact_path: impl Into<PathBuf>, golden_name: impl Into<String>) -> Self {
        Self {
            rel_artifact_path: rel_artifact_path.into(),
            golden_name: golden_name.into(),
        }
    }
}

#[async_trait]
impl Task for CheckPublicationArtifact {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        _task_index: usize,
    ) -> TestResult<()> {
        let actual_path = project_env
            .absolute_project_dir
            .join(&self.rel_artifact_path);
        let raw = stdfs::read_to_string(&actual_path)?;
        let mut value: Value = serde_json::from_str(&raw)?;

        for path in VOLATILE_PATHS {
            remove_json_path(&mut value, path);
        }

        let normalized = serde_json::to_string_pretty(&value)?;
        let golden_path = test_env.golden_dir.join(&self.golden_name);

        if is_update_golden_files_mode() {
            if let Some(parent) = golden_path.parent() {
                stdfs::create_dir_all(parent)?;
            }
            stdfs::write(&golden_path, normalized)?;
        } else {
            let expected = stdfs::read_to_string(&golden_path)?;
            assert_eq!(normalized, expected);
        }

        Ok(())
    }
}
