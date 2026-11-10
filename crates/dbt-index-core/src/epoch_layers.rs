//! Epoch-append parquet layers for dbt-index.

use std::path::{Path, PathBuf};

/// Epoch file-name prefix. `pub` because the generated epoch views glob on it
/// (`info_schema::epoch_views`), and a mismatch there would silently read no
/// files rather than fail.
pub const SCHEMA_VERSION: &str = "v1";

/// File name of the base epoch — epoch 0, the one rewritten in place by a full
/// parse or a consolidation.
pub fn base_epoch_filename() -> String {
    format!("{SCHEMA_VERSION}_0.parquet")
}

/// List epoch files matching `{SCHEMA_VERSION}_{N}.parquet` in `dir`, sorted ascending.
pub fn existing_epochs(dir: &Path) -> Vec<(u32, PathBuf)> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let prefix = format!("{SCHEMA_VERSION}_");
    let mut epochs: Vec<(u32, PathBuf)> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension()?.to_str()? != "parquet" {
                return None;
            }
            let stem = p.file_stem()?.to_str()?;
            let n_str = stem.strip_prefix(prefix.as_str())?;
            let n: u32 = n_str.parse().ok()?;
            Some((n, p))
        })
        .collect();
    epochs.sort_by_key(|(n, _)| *n);
    epochs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn existing_epochs_empty_on_missing_dir() {
        let dir = PathBuf::from("/nonexistent/path/that/does/not/exist");
        assert!(existing_epochs(&dir).is_empty());
    }

    #[test]
    fn existing_epochs_lists_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("v1_2.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_0.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_1.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("other.parquet"), b"").unwrap();
        std::fs::write(dir.path().join("v1_0.txt"), b"").unwrap();

        let epochs = existing_epochs(dir.path());
        assert_eq!(epochs.len(), 3);
        assert_eq!(epochs[0].0, 0);
        assert_eq!(epochs[1].0, 1);
        assert_eq!(epochs[2].0, 2);
    }
}
