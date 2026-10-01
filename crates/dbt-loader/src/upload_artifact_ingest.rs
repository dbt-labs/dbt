use crate::cloud_http_client::{
    CloudAuthScheme, build_cloud_api_client as build_shared_cloud_api_client,
    build_private_api_url, build_retry_client,
};
use dbt_cloud_config::ResolvedCloudConfig;
use dbt_common::constants::{
    DBT_CATALOG_JSON, DBT_DEFAULT_LOG_FILE_NAME, DBT_LOG_DIR_NAME, DBT_MANIFEST_JSON,
    DBT_SOURCES_JSON,
};
use dbt_common::io_args::IoArgs;
use dbt_common::tracing::dbt_emit::{
    emit_debug_log_message, emit_info_progress_message, emit_warn_log_message,
};
use dbt_common::{ErrorCode, FsResult, fs_err, tokiofs};
use dbt_telemetry::ProgressMessage;
use reqwest_middleware::ClientWithMiddleware;
use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

const RUN_RESULTS_JSON: &str = "run_results.json";
const PUBLICATION_JSON: &str = ".publication.json";
const DBT_UPLOAD_TO_ARTIFACTS_INGEST_API: &str = "DBT_UPLOAD_TO_ARTIFACTS_INGEST_API";
const DBT_CLOUD_PUBLICATION_FILE_PATH: &str = "DBT_CLOUD_PUBLICATION_FILE_PATH";
const DBT_INVOCATION_ENV: &str = "DBT_INVOCATION_ENV";

struct UploadConfig {
    tenant_hostname: String,
    account_id: String,
    cloud_token: String,
    environment_id: String,
    job_id: Option<u64>,
}

impl UploadConfig {
    fn ingest_url(&self) -> String {
        build_private_api_url(
            &self.tenant_hostname,
            &self.account_id,
            &format!("environments/{}/ingests/", self.environment_id),
        )
    }
}

struct ArtifactPaths {
    manifest_path: PathBuf,
    run_results_path: PathBuf,
    sources_path: Option<PathBuf>,
    catalog_path: Option<PathBuf>,
    publication_path: Option<PathBuf>,
    log_path: Option<PathBuf>,
}

struct IngestCreateResult {
    upload_url: String,
    ingest_id: String,
}

pub async fn upload_artifacts_ingest_if_enabled(
    dbt_cloud_config: &Option<ResolvedCloudConfig>,
    io: &IoArgs,
    write_catalog: bool,
) -> FsResult<()> {
    if !should_upload_artifacts() {
        return Ok(());
    }

    let Some(config) = resolve_upload_config(dbt_cloud_config) else {
        return Ok(());
    };
    emit_debug_log_message(format!(
        "Artifact ingest upload config: host={}, account_id={}, environment_id={}",
        config.tenant_hostname, config.account_id, config.environment_id
    ));

    let Some(artifact_paths) = resolve_artifact_paths(io, write_catalog).await else {
        return Ok(());
    };

    let zip_bytes = match build_artifact_zip(
        &artifact_paths.manifest_path,
        &artifact_paths.run_results_path,
        artifact_paths.sources_path.as_deref(),
        artifact_paths.catalog_path.as_deref(),
        artifact_paths.publication_path.as_deref(),
        artifact_paths.log_path.as_deref(),
    )
    .await
    {
        Ok(bytes) => bytes,
        Err(err) => {
            emit_skip_warning(
                ErrorCode::IoError,
                format!(
                    "Skipping artifact ingest upload: failed to build ZIP payload: {}",
                    err
                ),
            );
            return Ok(());
        }
    };

    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        "Uploading".to_string(),
        format!(
            "artifacts ingest bundle (environment {})",
            config.environment_id
        ),
    ));

    let Some(cloud_client) = build_cloud_api_client(&config, io) else {
        return Ok(());
    };
    let upload_client = build_retry_client(reqwest::Client::new());

    let Some(create_result) = create_ingest_request(&cloud_client, &config).await else {
        return Ok(());
    };

    if !upload_ingest_zip(&upload_client, &create_result.upload_url, zip_bytes).await {
        return Ok(());
    }

    if !complete_ingest_request(&cloud_client, &config, &create_result.ingest_id).await {
        return Ok(());
    }

    emit_info_progress_message(ProgressMessage::new_from_action_and_target(
        "Uploaded".to_string(),
        format!(
            "artifacts ingest bundle (environment {})",
            config.environment_id
        ),
    ));

    Ok(())
}

fn should_upload_artifacts() -> bool {
    if !is_truthy_env_var(DBT_UPLOAD_TO_ARTIFACTS_INGEST_API) {
        return false;
    }

    std::env::var(DBT_INVOCATION_ENV)
        .ok()
        .is_none_or(|env| env.as_str() == "manual")
}

fn is_truthy_value(value: &str) -> bool {
    !value.is_empty()
        && !value.eq_ignore_ascii_case("0")
        && !value.eq_ignore_ascii_case("false")
        && !value.eq_ignore_ascii_case("f")
}

// Mirrors dbt-core's env_set_truthy behavior:
// variable is set and value is not empty and not one of "0", "false", or "f"
// (case-insensitive).
fn is_truthy_env_var(var_name: &str) -> bool {
    std::env::var(var_name)
        .ok()
        .is_some_and(|value| is_truthy_value(&value))
}

fn resolve_upload_config(dbt_cloud_config: &Option<ResolvedCloudConfig>) -> Option<UploadConfig> {
    let creds = dbt_cloud_config
        .as_ref()
        .and_then(|c| c.credentials.as_ref());

    let tenant_hostname = required_value(
        creds.map(|c| c.host.clone()).filter(|h| !h.is_empty()),
        ErrorCode::InvalidConfig,
        "Skipping artifact ingest upload: no dbt Cloud host configured",
    )?;

    let account_id = required_value(
        creds
            .map(|c| c.account_id.clone())
            .filter(|id| !id.is_empty()),
        ErrorCode::CredentialMissing,
        "Skipping artifact ingest upload: no dbt Cloud account ID configured",
    )?;

    let cloud_token = required_value(
        creds.map(|c| c.token.clone()).filter(|t| !t.is_empty()),
        ErrorCode::CredentialMissing,
        "Skipping artifact ingest upload: no dbt Cloud token configured",
    )?;

    let environment_id = required_value(
        dbt_cloud_config
            .as_ref()
            .and_then(|c| c.environment_id.clone()),
        ErrorCode::CredentialMissing,
        "Skipping artifact ingest upload: no dbt Cloud environment ID configured",
    )?;

    let job_id = dbt_cloud_config
        .as_ref()
        .and_then(|c| c.job_id.as_deref())
        .and_then(|id| match id.parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => {
                emit_skip_warning(
                    ErrorCode::InvalidConfig,
                    format!("DBT_CLOUD_JOB_ID '{}' is not a valid integer, ignoring", id),
                );
                None
            }
        });

    Some(UploadConfig {
        tenant_hostname,
        account_id,
        cloud_token,
        environment_id,
        job_id,
    })
}

fn required_value(
    value: Option<String>,
    error_code: ErrorCode,
    message: impl Into<String>,
) -> Option<String> {
    if value.is_none() {
        emit_skip_warning(error_code, message);
    }
    value
}

async fn resolve_artifact_paths(io: &IoArgs, write_catalog: bool) -> Option<ArtifactPaths> {
    let manifest_path = io.out_dir.join(DBT_MANIFEST_JSON);
    let run_results_path = io.out_dir.join(RUN_RESULTS_JSON);
    if tokiofs::metadata(&manifest_path).await.is_err()
        || tokiofs::metadata(&run_results_path).await.is_err()
    {
        emit_skip_warning(
            ErrorCode::FileNotFound,
            format!(
                "Skipping artifact ingest upload: required artifacts are missing ({} and/or {})",
                manifest_path.display(),
                run_results_path.display()
            ),
        );
        return None;
    }

    Some(ArtifactPaths {
        manifest_path,
        run_results_path,
        sources_path: resolve_optional_artifact_path(io, DBT_SOURCES_JSON).await,
        // Only include catalog.json if the write-catalog arg was set
        catalog_path: if write_catalog {
            resolve_optional_artifact_path(io, DBT_CATALOG_JSON).await
        } else {
            None
        },
        publication_path: resolve_publication_path(io).await,
        log_path: resolve_log_path(io).await,
    })
}

fn build_cloud_api_client(config: &UploadConfig, io: &IoArgs) -> Option<ClientWithMiddleware> {
    let invocation_id = io.invocation_id.to_string();
    match build_shared_cloud_api_client(
        &config.cloud_token,
        CloudAuthScheme::Token,
        Some(invocation_id.as_str()),
    ) {
        Ok(client) => Some(client),
        Err(err) => {
            emit_skip_warning(
                ErrorCode::NetworkError,
                format!(
                    "Skipping artifact ingest upload: failed to build cloud HTTP client: {}",
                    err
                ),
            );
            None
        }
    }
}

async fn create_ingest_request(
    client: &ClientWithMiddleware,
    config: &UploadConfig,
) -> Option<IngestCreateResult> {
    let mut body = serde_json::Map::new();
    if let Some(job_id) = config.job_id {
        body.insert(
            "job_id".to_string(),
            serde_json::Value::Number(job_id.into()),
        );
    }

    let create_response = match client.post(config.ingest_url()).json(&body).send().await {
        Ok(response) => response,
        Err(err) => {
            emit_upload_failure_warning(
                ErrorCode::NetworkError,
                format!("create step failed: {}", err),
            );
            return None;
        }
    };

    if create_response.status().as_u16() != 200 {
        emit_upload_failure_warning(
            error_code_from_status(create_response.status()),
            format!("create step returned HTTP {}", create_response.status()),
        );
        return None;
    }

    let create_response_json: serde_json::Value = match create_response.json().await {
        Ok(value) => value,
        Err(err) => {
            emit_upload_failure_warning(
                ErrorCode::SerializationError,
                format!("failed to parse create response: {}", err),
            );
            return None;
        }
    };

    let (upload_url, ingest_id) = match parse_create_response(&create_response_json) {
        Some(values) => values,
        None => {
            emit_upload_failure_warning(
                ErrorCode::SerializationError,
                "create response did not include upload_url and id",
            );
            return None;
        }
    };

    Some(IngestCreateResult {
        upload_url,
        ingest_id,
    })
}

async fn upload_ingest_zip(
    client: &ClientWithMiddleware,
    upload_url: &str,
    zip_bytes: Vec<u8>,
) -> bool {
    let upload_response = match client.put(upload_url).body(zip_bytes).send().await {
        Ok(response) => response,
        Err(err) => {
            emit_upload_failure_warning(
                ErrorCode::NetworkError,
                format!("upload step failed: {}", err),
            );
            return false;
        }
    };

    if upload_response.status().as_u16() == 200 || upload_response.status().as_u16() == 204 {
        return true;
    }

    emit_upload_failure_warning(
        error_code_from_status(upload_response.status()),
        format!("upload step returned HTTP {}", upload_response.status()),
    );
    false
}

async fn complete_ingest_request(
    client: &ClientWithMiddleware,
    config: &UploadConfig,
    ingest_id: &str,
) -> bool {
    let complete_response = match client
        .patch(format!("{}{}/", config.ingest_url(), ingest_id))
        .json(&serde_json::json!({ "upload_status": "SUCCESS" }))
        .send()
        .await
    {
        Ok(response) => response,
        Err(err) => {
            emit_upload_failure_warning(
                ErrorCode::NetworkError,
                format!("complete step failed: {}", err),
            );
            return false;
        }
    };

    if complete_response.status().as_u16() == 204 {
        return true;
    }

    emit_upload_failure_warning(
        error_code_from_status(complete_response.status()),
        format!("complete step returned HTTP {}", complete_response.status()),
    );
    false
}

async fn is_non_empty_file(path: &Path) -> bool {
    tokiofs::metadata(path)
        .await
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

/// Returns the path to `file_name` in `io.out_dir` if it exists and is non-empty.
async fn resolve_optional_artifact_path(io: &IoArgs, file_name: &str) -> Option<PathBuf> {
    let path = io.out_dir.join(file_name);
    is_non_empty_file(&path).await.then_some(path)
}

async fn resolve_log_path(io: &IoArgs) -> Option<PathBuf> {
    // main.rs writes the resolved absolute log dir into io.log_path before tracing
    // init, so it is the canonical resolver. Fall back to {in_dir}/logs only for
    // callers that never run through main (e.g. tests).
    let log_dir = io
        .log_path
        .clone()
        .unwrap_or_else(|| io.in_dir.join(DBT_LOG_DIR_NAME));

    // Prefer OTEL/telemetry file (mirrors dbt-orc's preference for telemetry.jsonl)
    if let Some(file_name) = &io.otel_file_name {
        let otel_path = log_dir.join(file_name);
        if is_non_empty_file(&otel_path).await {
            return Some(otel_path);
        }
    }

    // Fall back to dbt.log
    let dbt_log = log_dir.join(DBT_DEFAULT_LOG_FILE_NAME);
    if is_non_empty_file(&dbt_log).await {
        Some(dbt_log)
    } else {
        None
    }
}

async fn resolve_publication_path(io: &IoArgs) -> Option<PathBuf> {
    let env_path = std::env::var(DBT_CLOUD_PUBLICATION_FILE_PATH)
        .ok()
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let fallback = resolve_optional_artifact_path(io, PUBLICATION_JSON).await;

    match env_path {
        Some(path) if is_non_empty_file(&path).await => Some(path),
        Some(path) => {
            emit_warn_log_message(
                ErrorCode::FileNotFound,
                format!(
                    "Artifact ingest upload will continue without publication artifact; file not found at {}",
                    path.display()
                ),
            );
            fallback
        }
        None => fallback,
    }
}

async fn build_artifact_zip(
    manifest_path: &Path,
    run_results_path: &Path,
    sources_path: Option<&Path>,
    catalog_path: Option<&Path>,
    publication_path: Option<&Path>,
    log_path: Option<&Path>,
) -> FsResult<Vec<u8>> {
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .unix_permissions(0o644);

    let cursor = Cursor::new(Vec::<u8>::new());
    let mut zip = ZipWriter::new(cursor);

    add_file_to_zip(&mut zip, manifest_path, DBT_MANIFEST_JSON, options).await?;
    add_file_to_zip(&mut zip, run_results_path, RUN_RESULTS_JSON, options).await?;
    if let Some(sources_path) = sources_path {
        add_file_to_zip(&mut zip, sources_path, DBT_SOURCES_JSON, options).await?;
    }
    if let Some(catalog_path) = catalog_path {
        add_file_to_zip(&mut zip, catalog_path, DBT_CATALOG_JSON, options).await?;
    }
    if let Some(publication_path) = publication_path {
        add_file_to_zip(&mut zip, publication_path, PUBLICATION_JSON, options).await?;
    }
    if let Some(log_path) = log_path {
        let archive_name = log_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(DBT_DEFAULT_LOG_FILE_NAME);
        add_file_to_zip(&mut zip, log_path, archive_name, options).await?;
    }

    let cursor = zip.finish().map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to finalize artifacts ingest ZIP: {}",
            err
        )
    })?;
    Ok(cursor.into_inner())
}

async fn add_file_to_zip(
    zip: &mut ZipWriter<Cursor<Vec<u8>>>,
    file_path: &Path,
    archive_path: &str,
    options: SimpleFileOptions,
) -> FsResult<()> {
    emit_debug_log_message(format!(
        "Adding {} to ingest ZIP (source: {})",
        archive_path,
        file_path.display()
    ));
    let contents = tokiofs::read(file_path).await?;

    zip.start_file(archive_path, options).map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to add {} to artifacts ingest ZIP: {}",
            archive_path,
            err
        )
    })?;

    zip.write_all(&contents).map_err(|err| {
        fs_err!(
            ErrorCode::IoError,
            "Failed to write {} to artifacts ingest ZIP: {}",
            archive_path,
            err
        )
    })?;

    Ok(())
}

fn parse_create_response(response: &serde_json::Value) -> Option<(String, String)> {
    let data = response.get("data").unwrap_or(response);
    let signed_url = data
        .get("signed_url")
        .or_else(|| data.get("signedUrl"))
        .or_else(|| data.get("upload_url"))
        .or_else(|| data.get("url"))
        .and_then(|value| value.as_str())?
        .to_string();

    let ingest_id = data
        .get("id")
        .or_else(|| data.get("ingest_id"))
        .or_else(|| data.get("ingestId"))
        .and_then(|value| {
            value
                .as_str()
                .map(ToString::to_string)
                .or_else(|| value.as_i64().map(|value| value.to_string()))
                .or_else(|| value.as_u64().map(|value| value.to_string()))
        })?;

    Some((signed_url, ingest_id))
}

fn error_code_from_status(status: reqwest::StatusCode) -> ErrorCode {
    if status.as_u16() == 429 {
        ErrorCode::RateLimited
    } else {
        ErrorCode::HttpError
    }
}

fn emit_skip_warning(error_code: ErrorCode, message: impl Into<String>) {
    emit_warn_log_message(error_code, message.into());
}

fn emit_upload_failure_warning(error_code: ErrorCode, message: impl Into<String>) {
    emit_warn_log_message(
        error_code,
        format!(
            "Artifact ingest upload failed: {}. Continuing without upload.",
            message.into()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_cloud_config::CloudCredentials;
    use tempfile::tempdir;
    use zip::ZipArchive;

    fn sample_upload_cloud_config() -> Option<ResolvedCloudConfig> {
        Some(ResolvedCloudConfig {
            credentials: Some(CloudCredentials {
                account_id: "123".to_string(),
                host: "cloud.getdbt.com".to_string(),
                token: "config-token".to_string(),
            }),
            project_id: Some("157".to_string()),
            account_id: Some("123".to_string()),
            account_identifier: None,
            environment_id: Some("216".to_string()),
            defer_env_id: None,
            defer_job_id: None,
            state_org_id: None,
            job_id: None,
        })
    }

    #[test]
    fn test_parse_create_response_with_data_object() {
        let response = serde_json::json!({
            "data": {
                "upload_url": "https://example.com/upload",
                "id": "123"
            }
        });

        let parsed = parse_create_response(&response);
        assert_eq!(
            parsed,
            Some(("https://example.com/upload".to_string(), "123".to_string()))
        );
    }

    #[dbt_runtime::test]
    async fn test_build_artifact_zip() {
        let temp_dir = tempdir().unwrap();
        let manifest_path = temp_dir.path().join(DBT_MANIFEST_JSON);
        let run_results_path = temp_dir.path().join(RUN_RESULTS_JSON);
        let sources_path = temp_dir.path().join(DBT_SOURCES_JSON);
        let catalog_path = temp_dir.path().join(DBT_CATALOG_JSON);
        let publication_path = temp_dir.path().join(PUBLICATION_JSON);
        let log_path = temp_dir.path().join(DBT_DEFAULT_LOG_FILE_NAME);

        std::fs::write(&manifest_path, "{\"manifest\":true}").unwrap();
        std::fs::write(&run_results_path, "{\"run_results\":true}").unwrap();
        std::fs::write(&sources_path, "{\"sources\":true}").unwrap();
        std::fs::write(&catalog_path, "{\"catalog\":true}").unwrap();
        std::fs::write(&publication_path, "{\"publication\":true}").unwrap();
        std::fs::write(&log_path, "some log content").unwrap();

        let zip_bytes = build_artifact_zip(
            &manifest_path,
            &run_results_path,
            Some(sources_path.as_path()),
            Some(catalog_path.as_path()),
            Some(publication_path.as_path()),
            Some(log_path.as_path()),
        )
        .await
        .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        assert!(archive.by_name(DBT_MANIFEST_JSON).is_ok());
        assert!(archive.by_name(RUN_RESULTS_JSON).is_ok());
        assert!(archive.by_name(DBT_SOURCES_JSON).is_ok());
        assert!(archive.by_name(DBT_CATALOG_JSON).is_ok());
        assert!(archive.by_name(PUBLICATION_JSON).is_ok());
        assert!(archive.by_name(DBT_DEFAULT_LOG_FILE_NAME).is_ok());
    }

    #[dbt_runtime::test]
    async fn test_build_artifact_zip_includes_log_file() {
        let temp_dir = tempdir().unwrap();
        let manifest_path = temp_dir.path().join(DBT_MANIFEST_JSON);
        let run_results_path = temp_dir.path().join(RUN_RESULTS_JSON);
        let log_path = temp_dir.path().join(DBT_DEFAULT_LOG_FILE_NAME);

        std::fs::write(&manifest_path, "{\"manifest\":true}").unwrap();
        std::fs::write(&run_results_path, "{\"run_results\":true}").unwrap();
        std::fs::write(&log_path, "some log content").unwrap();

        let zip_bytes = build_artifact_zip(
            &manifest_path,
            &run_results_path,
            None,
            None,
            None,
            Some(log_path.as_path()),
        )
        .await
        .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        assert!(archive.by_name(DBT_MANIFEST_JSON).is_ok());
        assert!(archive.by_name(RUN_RESULTS_JSON).is_ok());
        assert!(archive.by_name(DBT_DEFAULT_LOG_FILE_NAME).is_ok());
    }

    #[dbt_runtime::test]
    async fn test_build_artifact_zip_succeeds_without_log_file() {
        let temp_dir = tempdir().unwrap();
        let manifest_path = temp_dir.path().join(DBT_MANIFEST_JSON);
        let run_results_path = temp_dir.path().join(RUN_RESULTS_JSON);

        std::fs::write(&manifest_path, "{\"manifest\":true}").unwrap();
        std::fs::write(&run_results_path, "{\"run_results\":true}").unwrap();

        let zip_bytes =
            build_artifact_zip(&manifest_path, &run_results_path, None, None, None, None)
                .await
                .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        assert!(archive.by_name(DBT_MANIFEST_JSON).is_ok());
        assert!(archive.by_name(RUN_RESULTS_JSON).is_ok());
        assert!(archive.by_name(DBT_DEFAULT_LOG_FILE_NAME).is_err());
    }

    #[dbt_runtime::test]
    async fn test_resolve_optional_artifact_path_returns_some_when_file_exists() {
        let temp_dir = tempdir().unwrap();
        let io = IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let sources_path = temp_dir.path().join(DBT_SOURCES_JSON);
        std::fs::write(&sources_path, "{\"sources\":true}").unwrap();

        let result = resolve_optional_artifact_path(&io, DBT_SOURCES_JSON).await;
        assert_eq!(result, Some(sources_path));
    }

    #[dbt_runtime::test]
    async fn test_resolve_optional_artifact_path_returns_none_when_missing() {
        let temp_dir = tempdir().unwrap();
        let io = IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let result = resolve_optional_artifact_path(&io, DBT_SOURCES_JSON).await;
        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_resolve_optional_artifact_path_returns_none_for_empty_file() {
        let temp_dir = tempdir().unwrap();
        let io = IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let sources_path = temp_dir.path().join(DBT_SOURCES_JSON);
        std::fs::write(&sources_path, "").unwrap();

        let result = resolve_optional_artifact_path(&io, DBT_SOURCES_JSON).await;
        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_resolve_artifact_paths_excludes_catalog_when_write_catalog_false() {
        let temp_dir = tempdir().unwrap();
        let io = IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        // Write all artifacts to disk
        std::fs::write(temp_dir.path().join(DBT_MANIFEST_JSON), "{}").unwrap();
        std::fs::write(temp_dir.path().join(RUN_RESULTS_JSON), "{}").unwrap();
        std::fs::write(temp_dir.path().join(DBT_CATALOG_JSON), "{\"catalog\":true}").unwrap();
        std::fs::write(temp_dir.path().join(DBT_SOURCES_JSON), "{\"sources\":true}").unwrap();

        // With write_catalog=false, catalog should be excluded even though the file exists
        let paths = resolve_artifact_paths(&io, false).await.unwrap();
        assert!(paths.catalog_path.is_none());
        assert!(paths.sources_path.is_some());

        // With write_catalog=true, catalog should be included
        let paths = resolve_artifact_paths(&io, true).await.unwrap();
        assert!(paths.catalog_path.is_some());
    }

    #[dbt_runtime::test]
    async fn test_build_artifact_zip_with_sources_but_no_catalog() {
        let temp_dir = tempdir().unwrap();
        let manifest_path = temp_dir.path().join(DBT_MANIFEST_JSON);
        let run_results_path = temp_dir.path().join(RUN_RESULTS_JSON);
        let sources_path = temp_dir.path().join(DBT_SOURCES_JSON);

        std::fs::write(&manifest_path, "{\"manifest\":true}").unwrap();
        std::fs::write(&run_results_path, "{\"run_results\":true}").unwrap();
        std::fs::write(&sources_path, "{\"sources\":true}").unwrap();

        let zip_bytes = build_artifact_zip(
            &manifest_path,
            &run_results_path,
            Some(sources_path.as_path()),
            None,
            None,
            None,
        )
        .await
        .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        assert!(archive.by_name(DBT_MANIFEST_JSON).is_ok());
        assert!(archive.by_name(RUN_RESULTS_JSON).is_ok());
        assert!(archive.by_name(DBT_SOURCES_JSON).is_ok());
        assert!(archive.by_name(DBT_CATALOG_JSON).is_err());
    }

    #[dbt_runtime::test]
    async fn test_resolve_log_path_prefers_otel_file() {
        let temp_dir = tempdir().unwrap();
        let log_dir = temp_dir.path();

        let otel_file = log_dir.join("telemetry.jsonl");
        let dbt_log = log_dir.join(DBT_DEFAULT_LOG_FILE_NAME);
        std::fs::write(&otel_file, "otel content").unwrap();
        std::fs::write(&dbt_log, "log content").unwrap();

        let io = IoArgs {
            log_path: Some(log_dir.to_path_buf()),
            otel_file_name: Some("telemetry.jsonl".to_string()),
            ..Default::default()
        };

        let result = resolve_log_path(&io).await;
        assert_eq!(result, Some(otel_file));
    }

    #[dbt_runtime::test]
    async fn test_resolve_log_path_falls_back_to_dbt_log() {
        let temp_dir = tempdir().unwrap();
        let log_dir = temp_dir.path();

        let dbt_log = log_dir.join(DBT_DEFAULT_LOG_FILE_NAME);
        std::fs::write(&dbt_log, "log content").unwrap();

        let io = IoArgs {
            log_path: Some(log_dir.to_path_buf()),
            ..Default::default()
        };

        let result = resolve_log_path(&io).await;
        assert_eq!(result, Some(dbt_log));
    }

    #[dbt_runtime::test]
    async fn test_resolve_log_path_returns_none_when_no_files() {
        let temp_dir = tempdir().unwrap();
        let log_dir = temp_dir.path();

        let io = IoArgs {
            log_path: Some(log_dir.to_path_buf()),
            ..Default::default()
        };

        let result = resolve_log_path(&io).await;
        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_resolve_log_path_skips_empty_files() {
        let temp_dir = tempdir().unwrap();
        let log_dir = temp_dir.path();

        // Write empty otel and dbt.log files
        let otel_file = log_dir.join("telemetry.jsonl");
        let dbt_log = log_dir.join(DBT_DEFAULT_LOG_FILE_NAME);
        std::fs::write(&otel_file, "").unwrap();
        std::fs::write(&dbt_log, "").unwrap();

        let io = IoArgs {
            log_path: Some(log_dir.to_path_buf()),
            otel_file_name: Some("telemetry.jsonl".to_string()),
            ..Default::default()
        };

        let result = resolve_log_path(&io).await;
        assert!(result.is_none());
    }

    #[dbt_runtime::test]
    async fn test_build_artifact_zip_uses_source_filename_as_archive_name() {
        let temp_dir = tempdir().unwrap();
        let log_dir = temp_dir.path();

        let otel_file = log_dir.join("telemetry.jsonl");
        std::fs::write(&otel_file, "otel content").unwrap();

        let manifest_path = temp_dir.path().join(DBT_MANIFEST_JSON);
        let run_results_path = temp_dir.path().join(RUN_RESULTS_JSON);
        std::fs::write(&manifest_path, "{\"manifest\":true}").unwrap();
        std::fs::write(&run_results_path, "{\"run_results\":true}").unwrap();

        let zip_bytes = build_artifact_zip(
            &manifest_path,
            &run_results_path,
            None,
            None,
            None,
            Some(otel_file.as_path()),
        )
        .await
        .unwrap();

        let mut archive = ZipArchive::new(Cursor::new(zip_bytes)).unwrap();
        // Should be stored under the source filename, not dbt.log
        assert!(archive.by_name("telemetry.jsonl").is_ok());
        assert!(archive.by_name(DBT_DEFAULT_LOG_FILE_NAME).is_err());
    }

    #[test]
    fn test_resolve_upload_config_with_full_config() {
        let cloud_config = sample_upload_cloud_config();
        let config = resolve_upload_config(&cloud_config).unwrap();

        assert_eq!(config.account_id, "123");
        assert_eq!(config.cloud_token, "config-token");
        assert_eq!(config.environment_id, "216");
        assert_eq!(config.tenant_hostname, "cloud.getdbt.com");
    }

    #[test]
    fn test_resolve_upload_config_requires_environment_id() {
        let cloud_config = Some(ResolvedCloudConfig {
            credentials: Some(CloudCredentials {
                account_id: "123".to_string(),
                host: "cloud.getdbt.com".to_string(),
                token: "config-token".to_string(),
            }),
            environment_id: None,
            ..Default::default()
        });
        let config = resolve_upload_config(&cloud_config);
        assert!(config.is_none());
    }

    #[test]
    fn test_resolve_upload_config_requires_credentials() {
        let cloud_config = Some(ResolvedCloudConfig {
            credentials: None,
            environment_id: Some("216".to_string()),
            ..Default::default()
        });
        let config = resolve_upload_config(&cloud_config);
        assert!(config.is_none());
    }

    #[test]
    fn test_resolve_upload_config_none_config() {
        let config = resolve_upload_config(&None);
        assert!(config.is_none());
    }

    #[test]
    fn test_resolve_upload_config_with_job_id() {
        let cloud_config = Some(ResolvedCloudConfig {
            job_id: Some("42".to_string()),
            ..sample_upload_cloud_config().unwrap()
        });
        let config = resolve_upload_config(&cloud_config).unwrap();
        assert_eq!(config.job_id, Some(42u64));
    }

    #[test]
    fn test_resolve_upload_config_ignores_invalid_job_id() {
        let cloud_config = Some(ResolvedCloudConfig {
            job_id: Some("not_a_number".to_string()),
            ..sample_upload_cloud_config().unwrap()
        });
        let config = resolve_upload_config(&cloud_config).unwrap();
        assert_eq!(config.job_id, None);
    }

    #[test]
    fn test_resolve_upload_config_without_job_id() {
        let cloud_config = sample_upload_cloud_config();
        let config = resolve_upload_config(&cloud_config).unwrap();
        assert_eq!(config.job_id, None);
    }

    #[test]
    fn test_is_truthy_value_matches_existing_env_set_truthy_semantics() {
        assert!(is_truthy_value("1"));
        assert!(is_truthy_value("true"));
        assert!(is_truthy_value("yes"));
        assert!(!is_truthy_value(""));
        assert!(!is_truthy_value("0"));
        assert!(!is_truthy_value("false"));
        assert!(!is_truthy_value("f"));
        assert!(!is_truthy_value("F"));
    }
}
