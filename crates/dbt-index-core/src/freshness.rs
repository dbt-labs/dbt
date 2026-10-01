// Artifact freshness tracking — write side.
//
// After ingestion we store lightweight file fingerprints (mtime + size) so
// subsequent commands can detect when artifacts have changed on disk and
// automatically re-ingest before serving stale data.

use std::collections::HashMap;
use std::path::Path;

pub const META_FILE: &str = ".artifact_meta.json";

pub const ARTIFACT_FILES: &[&str] = &[
    "manifest.json",
    "catalog.json",
    "run_results.json",
    "sources.json",
    "semantic_manifest.json",
];

#[derive(Debug, thiserror::Error)]
pub enum ArtifactMetaError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

/// How the index was produced -- determines whether auto-refresh is safe.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub enum WriteSource {
    /// Index produced by `--write-metadata` (rich: grain, lineage, inferred types).
    /// Auto-reingest from JSON would destroy this enriched metadata.
    DirectWrite,
    /// Index produced by JSON artifact ingestion (`dbt-index ingest` / auto-refresh).
    #[default]
    JsonIngest,
}

/// Lightweight fingerprint of a file on disk (stat-only, no content read).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct FileFingerprint {
    pub size: u64,
    /// Seconds since Unix epoch (from file mtime).
    pub modified_secs: u64,
}

/// Metadata stored alongside the index to track artifact freshness.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
pub struct ArtifactMeta {
    pub target_dir: String,
    pub fingerprints: HashMap<String, FileFingerprint>,
    #[serde(default)]
    pub write_source: WriteSource,
    #[serde(default)]
    pub context_dir: Option<String>,
    #[serde(default)]
    pub context_fingerprints: HashMap<String, FileFingerprint>,
    /// Stable identity of the source that produced the configured index.
    /// Older metadata omits this field and continues to use the inferred
    /// source location.
    #[serde(default)]
    pub source_identity: Option<String>,
}

pub fn fingerprint_file(path: &Path) -> Option<FileFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    let modified_secs = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();
    Some(FileFingerprint {
        size: meta.len(),
        modified_secs,
    })
}

pub fn compute_fingerprints(target_dir: &Path) -> HashMap<String, FileFingerprint> {
    let mut fps = HashMap::new();
    for name in ARTIFACT_FILES {
        if let Some(fp) = fingerprint_file(&target_dir.join(name)) {
            fps.insert((*name).to_string(), fp);
        }
    }
    fps
}

pub fn compute_context_fingerprints(context_dir: &Path) -> HashMap<String, FileFingerprint> {
    let mut fps = HashMap::new();
    if context_dir.is_dir() {
        collect_md_fingerprints(context_dir, context_dir, &mut fps);
    }
    fps
}

pub fn collect_md_fingerprints(
    base: &Path,
    dir: &Path,
    out: &mut HashMap<String, FileFingerprint>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_md_fingerprints(base, &path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Some(fp) = fingerprint_file(&path) {
                let rel = path
                    .strip_prefix(base)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .to_string();
                out.insert(rel, fp);
            }
        }
    }
}

/// Save artifact fingerprints to the index directory after ingestion.
///
/// `target_dir` should be canonicalized (or at least absolute) so that
/// later freshness checks can locate the artifacts regardless of cwd.
///
/// `write_source` records how the index was produced so that
/// `check_freshness` (in dbt-index) can decide whether auto-reingest is safe.
pub fn save_artifact_meta(
    index_dir: &Path,
    target_dir: &Path,
    write_source: WriteSource,
    context_dir: Option<&Path>,
) -> Result<(), ArtifactMetaError> {
    save_artifact_meta_with_source(index_dir, target_dir, write_source, context_dir, None)
}

/// Save artifact fingerprints and, when provided, the stable source identity
/// used to produce the configured index.
pub fn save_artifact_meta_with_source(
    index_dir: &Path,
    target_dir: &Path,
    write_source: WriteSource,
    context_dir: Option<&Path>,
    source_identity: Option<&Path>,
) -> Result<(), ArtifactMetaError> {
    let fingerprints = compute_fingerprints(target_dir);
    let (ctx_dir_str, ctx_fps) = match context_dir.filter(|p| p.is_dir()) {
        Some(d) => (
            Some(d.to_string_lossy().into_owned()),
            compute_context_fingerprints(d),
        ),
        None => (None, HashMap::new()),
    };
    let meta = ArtifactMeta {
        target_dir: target_dir.to_string_lossy().into_owned(),
        fingerprints,
        write_source,
        context_dir: ctx_dir_str,
        context_fingerprints: ctx_fps,
        source_identity: source_identity.map(|path| path.to_string_lossy().into_owned()),
    };
    let json = serde_json::to_string_pretty(&meta)?;
    std::fs::write(index_dir.join(META_FILE), json)?;
    Ok(())
}
