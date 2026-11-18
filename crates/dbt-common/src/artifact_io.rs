use std::{io::Write, path::Path};

use dbt_telemetry::{ArtifactType, ArtifactWritten};
use serde::Serialize;

use crate::error::LiftableResult;
use crate::{ErrorCode, FsError, FsResult, create_info_span, ectx, stdfs};

/// Writes a JSON artifact file and emits a standard `ArtifactWritten` telemetry span.
///
/// Artifacts are streamed to avoid materializing a large intermediate YAML tree and
/// JSON string in memory. `manifest.json` serializes itself; other artifacts go
/// through `dbt_yaml::AsValue`, which writes them as `dbt_yaml::to_value` would.
pub fn write_artifact_to_file<T>(
    artifact: &T,
    artifact_type: ArtifactType,
    out_dir: &Path,
    filename: &str,
    in_dir: &Path,
) -> FsResult<()>
where
    T: Serialize,
{
    let artifact_path = out_dir.join(filename);
    let rel_path = pathdiff::diff_paths(&artifact_path, in_dir)
        .unwrap_or_else(|| artifact_path.clone())
        .to_string_lossy()
        .into_owned();

    let _sp = create_info_span(ArtifactWritten {
        artifact_type: artifact_type as i32,
        relative_path: rel_path,
    })
    .entered();

    stdfs::create_dir_all(artifact_path.parent().unwrap())?;

    let f = stdfs::File::create(&artifact_path)?;
    let mut w = std::io::BufWriter::new(f);
    if artifact_type == ArtifactType::Manifest {
        serde_json::to_writer(&mut w, artifact)?;
        w.flush()?;
    } else {
        let write_ctx = ectx!("Failed to write file: {}", artifact_path.display());
        serde_json::to_writer(&mut w, &dbt_yaml::AsValue(artifact)).or_else(|e| {
            if e.is_io() {
                Err(std::io::Error::from(e)).lift(write_ctx)
            } else {
                Err(FsError::new(
                    ErrorCode::SerializationError,
                    format!("Failed to convert artifact to YAML value: {e}"),
                )
                .into())
            }
        })?;
        w.flush().lift(write_ctx)?;
    }

    Ok(())
}
