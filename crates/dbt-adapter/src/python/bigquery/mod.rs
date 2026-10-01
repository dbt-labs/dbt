use crate::AdapterResponse;
use crate::adapter::adapter_impl::AdapterImpl;
use crate::engine::Options;
use adbc_core::options::OptionValue;

use dbt_adbc::bigquery::{
    CREATE_BATCH_REQ_BATCH_ID, CREATE_BATCH_REQ_BATCH_YML, CREATE_BATCH_REQ_PARENT,
    CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_BUCKET, CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_PATH,
    CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_FILE_NAME, CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_NAME,
    CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PARENT, CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PROJECT,
    CREATE_NOTEBOOK_EXECUTE_JOB_REQ_REGION, CREATE_NOTEBOOK_EXECUTE_JOB_REQ_TEMPLATE_ID,
    DATAPROC_POOLING_TIMEOUT, DATAPROC_PROJECT, DATAPROC_REGION,
    DATAPROC_SUBMIT_JOB_REQ_CLUSTER_NAME, DATAPROC_SUBMIT_JOB_REQ_GCS_PATH, WRITE_GCS_BUCKET,
    WRITE_GCS_CONTENT, WRITE_GCS_OBJECT_NAME,
};
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_yaml::Value::Mapping;
use dbt_yaml::{Error, Value as YmlValue};
use minijinja::{State, Value};
use uuid::Uuid;

const DEFAULT_JAR_FILE_URI: &str =
    "gs://spark-lib/bigquery/spark-bigquery-with-dependencies_2.13-0.34.0.jar";

#[derive(Debug, Clone, PartialEq)]
enum SubmissionMethod {
    Cluster,
    Serverless,
    BigFrames,
}

pub struct JobContext<'a> {
    pub adapter: &'a AdapterImpl,
    pub ctx: &'a QueryCtx,
    pub conn: &'a mut dyn Connection,
    pub config: &'a Value,
    pub token: CancellationToken,
}

pub struct ClusterJobParams<'a> {
    pub project: &'a str,
    pub region: &'a str,
    pub gcs_path: &'a str,
    pub timeout: i64,
}

pub struct ServerlessJobParams<'a> {
    pub project: &'a str,
    pub region: &'a str,
    pub gcs_path: &'a str,
    pub batch_id: &'a str,
    pub timeout: i64,
}

pub struct BigframesJobParams<'a> {
    pub project: &'a str,
    pub region: &'a str,
    pub gcs_path: &'a str,
    pub model_file_name: &'a str,
    pub model_name: &'a str,
    pub gsc_bucket: &'a str,
    pub batch_id: &'a str,
    pub timeout: i64,
}

impl SubmissionMethod {
    fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "cluster" => Some(Self::Cluster),
            "serverless" => Some(Self::Serverless),
            "bigframes" => Some(Self::BigFrames),
            _ => None,
        }
    }
}

pub fn submit_python_job(
    adapter: &AdapterImpl,
    ctx: &QueryCtx,
    conn: &'_ mut dyn Connection,
    _state: &State,
    model: &Value,
    compiled_code: &str,
    token: CancellationToken,
) -> AdapterResult<AdapterResponse> {
    let config = model
        .get_attr("config")
        .map_err(|e| AdapterError::new(AdapterErrorKind::Internal, e.to_string()))?;

    let schema = model
        .get_attr("schema")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| AdapterError::new(AdapterErrorKind::Internal, "schema is required"))?;

    let alias = model
        .get_attr("alias")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .ok_or_else(|| AdapterError::new(AdapterErrorKind::Internal, "alias is required"))?;

    let database = adapter
        .get_db_config("database")
        .map(|v| v.to_string())
        .unwrap_or_default();

    let execution_project = adapter
        .get_db_config("execution_project")
        .map(|v| v.to_string())
        .unwrap_or(database);

    let compute_region = adapter
        .get_db_config("compute_region")
        .map(|v| v.to_string())
        .unwrap_or_default();

    let gcs_bucket = adapter
        .get_db_config("gcs_bucket")
        .map(|v| v.to_string())
        .unwrap_or_default();

    if compute_region.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "Need to supply compute_region in profile to submit python job",
        ));
    }

    if gcs_bucket.is_empty() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "Need to supply gcs_bucket in profile to submit python job",
        ));
    }

    let submission_method = SubmissionMethod::from_str(
        &config
            .get_attr("submission_method")
            .ok()
            .and_then(|v| v.as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| {
                adapter
                    .get_db_config("submission_method")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "serverless".to_string())
            }),
    )
    .ok_or_else(|| AdapterError::new(AdapterErrorKind::Internal, "Invalid submission method"))?;

    let job_execution_timeout = adapter
        .get_db_config("job_execution_timeout_seconds")
        .map(|v| v.to_string())
        .map(|v| {
            v.parse::<i64>()
                .expect("job_execution_timeout_seconds must be a valid integer")
        });

    let default_timeout = if submission_method == SubmissionMethod::BigFrames {
        60 * 60
    } else {
        24 * 60 * 60
    };

    let timeout = config
        .get_attr("timeout")
        .ok()
        .and_then(|v| v.as_i64())
        .or(job_execution_timeout)
        .unwrap_or(default_timeout);

    let packages: Vec<String> = config
        .get_attr("packages")
        .ok()
        .and_then(|v| v.try_iter().ok())
        .map(|iter| {
            iter.filter_map(|item| item.as_str().map(|s| s.to_string()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    let batch_id = config
        .get_attr("batch_id")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let compiled_code = preprocess_compiled_code(&submission_method, compiled_code, &packages)?;

    let model_file_name = format!("{}/{}.py", schema, alias);
    let gcs_path = format!("gs://{}/{}", &gcs_bucket, model_file_name);

    let options: Options = vec![
        (
            WRITE_GCS_BUCKET.to_string(),
            OptionValue::String(gcs_bucket.clone()),
        ),
        (
            WRITE_GCS_OBJECT_NAME.to_string(),
            OptionValue::String(model_file_name.to_string()),
        ),
        (
            WRITE_GCS_CONTENT.to_string(),
            OptionValue::String(compiled_code),
        ),
    ];

    adapter.execute(
        None,
        conn,
        Some(ctx),
        "SELECT 1",
        false,
        false,
        None,
        Some(options),
        token.clone(),
    )?;

    let job_ctx = JobContext {
        adapter,
        ctx,
        conn,
        config: &config,
        token,
    };

    match submission_method {
        SubmissionMethod::Cluster => {
            let params = ClusterJobParams {
                project: execution_project.as_str(),
                region: compute_region.as_str(),
                gcs_path: gcs_path.as_str(),
                timeout,
            };
            submit_cluster_job(job_ctx, params)
        }
        SubmissionMethod::Serverless => {
            let params = ServerlessJobParams {
                project: execution_project.as_str(),
                region: compute_region.as_str(),
                gcs_path: gcs_path.as_str(),
                batch_id: batch_id.as_str(),
                timeout,
            };
            submit_serverless_job(job_ctx, params)
        }
        SubmissionMethod::BigFrames => {
            let params = BigframesJobParams {
                project: execution_project.as_str(),
                region: compute_region.as_str(),
                gcs_path: gcs_path.as_str(),
                model_file_name: &model_file_name,
                model_name: &alias,
                gsc_bucket: &gcs_bucket,
                batch_id: batch_id.as_str(),
                timeout,
            };
            submit_bigframes_job(job_ctx, params)
        }
    }
}

fn preprocess_compiled_code(
    submission_method: &SubmissionMethod,
    compiled_code: &str,
    packages: &[String],
) -> AdapterResult<String> {
    match submission_method {
        SubmissionMethod::Cluster => Ok(compiled_code.to_string()),
        SubmissionMethod::Serverless => Ok(compiled_code.to_string()),
        SubmissionMethod::BigFrames => convert_py_to_ipynb(compiled_code, packages),
    }
}

fn submit_cluster_job(
    job_ctx: JobContext,
    params: ClusterJobParams,
) -> AdapterResult<AdapterResponse> {
    let proj_dataproc_cluster_name = job_ctx
        .adapter
        .get_db_config("dataproc_cluster_name")
        .map(|v| v.to_string());

    let dataproc_cluster_name = job_ctx.config
        .get_attr("dataproc_cluster_name")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .or(proj_dataproc_cluster_name)
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Need to supply dataproc_cluster_name in profile or config to submit python job with cluster submission method",
            )
        })?;

    let options: Options = vec![
        (
            DATAPROC_REGION.to_string(),
            OptionValue::String(params.region.to_string()),
        ),
        (
            DATAPROC_PROJECT.to_string(),
            OptionValue::String(params.project.to_string()),
        ),
        (
            DATAPROC_POOLING_TIMEOUT.to_string(),
            OptionValue::String(params.timeout.to_string()),
        ),
        (
            DATAPROC_SUBMIT_JOB_REQ_CLUSTER_NAME.to_string(),
            OptionValue::String(dataproc_cluster_name),
        ),
        (
            DATAPROC_SUBMIT_JOB_REQ_GCS_PATH.to_string(),
            OptionValue::String(params.gcs_path.to_string()),
        ),
    ];

    let response = job_ctx.adapter.execute(
        None,
        job_ctx.conn,
        Some(job_ctx.ctx),
        "SELECT 1",
        false,
        false,
        None,
        Some(options),
        job_ctx.token.clone(),
    )?;

    Ok(response.0)
}

fn submit_serverless_job(
    job_ctx: JobContext,
    params: ServerlessJobParams,
) -> AdapterResult<AdapterResponse> {
    let jar_file_uri = job_ctx
        .config
        .get_attr("jar_file_uri")
        .ok()
        .and_then(|v| v.as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| DEFAULT_JAR_FILE_URI.to_string());

    let dataproc_batch_config = job_ctx.adapter.get_db_config_value("dataproc_batch");
    let batch = batch_to_str(
        params.gcs_path.to_string(),
        jar_file_uri,
        dataproc_batch_config,
    )?;

    let options: Options = vec![
        (
            DATAPROC_REGION.to_string(),
            OptionValue::String(params.region.to_string()),
        ),
        (
            CREATE_BATCH_REQ_PARENT.to_string(),
            OptionValue::String(format!(
                "projects/{}/locations/{}",
                params.project, params.region
            )),
        ),
        (
            CREATE_BATCH_REQ_BATCH_YML.to_string(),
            OptionValue::String(batch),
        ),
        (
            CREATE_BATCH_REQ_BATCH_ID.to_string(),
            OptionValue::String(params.batch_id.to_string()),
        ),
        (
            DATAPROC_POOLING_TIMEOUT.to_string(),
            OptionValue::String(params.timeout.to_string()),
        ),
    ];

    let response = job_ctx.adapter.execute(
        None,
        job_ctx.conn,
        Some(job_ctx.ctx),
        "SELECT 1",
        false,
        false,
        None,
        Some(options),
        job_ctx.token.clone(),
    )?;

    Ok(response.0)
}

pub fn batch_to_str(
    gcs_path: String,
    jar_file_uri: String,
    overrides: Option<&YmlValue>,
) -> Result<String, Error> {
    let mut batch = default_batch_value(gcs_path, jar_file_uri);

    if let Some(ovr) = overrides {
        merge_yaml(&mut batch, ovr);
    }

    let yaml_string = dbt_yaml::to_string(&batch)?;

    Ok(yaml_string)
}

fn default_batch_value(gcs_path: String, jar_file_uri: String) -> YmlValue {
    let yaml_str = format!(
        r#"
runtime_config:
  version: "1.1"
  properties:
    spark.executor.instances: "2"
pyspark_batch:
  main_python_file_uri: "{}"
  jar_file_uris:
    - "{}"
"#,
        gcs_path, jar_file_uri
    );

    dbt_yaml::from_str(yaml_str.as_str()).unwrap()
}

fn merge_yaml(base: &mut YmlValue, overrides: &YmlValue) {
    if overrides.is_null() {
        return;
    }

    match (base, overrides) {
        (Mapping(base_map, ..), Mapping(override_map, ..)) => {
            for (k, v) in override_map {
                if let Some(existing) = base_map.get_mut(k) {
                    merge_yaml(existing, v);
                } else {
                    base_map.insert(k.clone(), v.clone());
                }
            }
        }
        (base_slot, new_val) => {
            *base_slot = new_val.clone();
        }
    }
}

fn submit_bigframes_job(
    job_ctx: JobContext,
    params: BigframesJobParams,
) -> AdapterResult<AdapterResponse> {
    let notebook_template_id = job_ctx
        .config
        .get_attr("notebook_template_id")
        .ok()
        .and_then(|v| v.as_i64().map(|u| u.to_string()));

    let mut options: Options = vec![
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_PATH.to_string(),
            OptionValue::String(params.gcs_path.to_string()),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_FILE_NAME.to_string(),
            OptionValue::String(params.model_file_name.to_string()),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_NAME.to_string(),
            OptionValue::String(params.model_name.to_string()),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_BUCKET.to_string(),
            OptionValue::String(params.gsc_bucket.to_string()),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PARENT.to_string(),
            OptionValue::String(format!(
                "projects/{}/locations/{}",
                params.project, params.region
            )),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PROJECT.to_string(),
            OptionValue::String(params.project.to_string()),
        ),
        (
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_REGION.to_string(),
            OptionValue::String(params.region.to_string()),
        ),
        (
            CREATE_BATCH_REQ_BATCH_ID.to_string(),
            OptionValue::String(params.batch_id.to_string()),
        ),
        (
            DATAPROC_POOLING_TIMEOUT.to_string(),
            OptionValue::String(params.timeout.to_string()),
        ),
    ];

    if let Some(notebook_template_id) = notebook_template_id {
        options.push((
            CREATE_NOTEBOOK_EXECUTE_JOB_REQ_TEMPLATE_ID.to_string(),
            OptionValue::String(notebook_template_id),
        ));
    }

    let response = job_ctx.adapter.execute(
        None,
        job_ctx.conn,
        Some(job_ctx.ctx),
        "SELECT 1",
        false,
        true,
        None,
        Some(options),
        job_ctx.token.clone(),
    )?;

    Ok(response.0)
}

fn short_uuid() -> String {
    let full = Uuid::new_v4().simple().to_string();
    full[..8].to_string()
}

fn convert_py_to_ipynb(compiled_code: &str, packages: &[String]) -> AdapterResult<String> {
    let full_code = if !packages.is_empty() {
        let install_packages_source = get_install_packages_function_source();
        let packages_list = format!(
            "[{}]",
            packages
                .iter()
                .map(|p| format!("'{}'", p))
                .collect::<Vec<_>>()
                .join(", ")
        );
        format!(
            "{}\n_install_packages({})\n{}",
            install_packages_source, packages_list, compiled_code
        )
    } else {
        compiled_code.to_string()
    };

    let notebook = serde_json::json!({
        "cells": [
            {
                "cell_type": "code",
                "execution_count": null,
                "id": short_uuid(),
                "metadata": {},
                "outputs": [],
                "source": full_code
            }
        ],
        "metadata": {},
        "nbformat": 4,
        "nbformat_minor": 5
    });

    serde_json::to_string_pretty(&notebook).map_err(|e| {
        AdapterError::new(
            AdapterErrorKind::Internal,
            format!("Failed to serialize notebook: {}", e),
        )
    })
}

fn get_install_packages_function_source() -> &'static str {
    r#"def _install_packages(packages: list[str]) -> None:
    """Checks and installs packages via pip in a separate environment.

    Parses requirement strings (e.g., 'pandas>=1.0', 'numpy==2.1.1') using the
    'packaging' library to check against installed versions. It only installs
    packages that are not already present in the environment. Existing packages
    will not be updated, even if a different version is requested.

    NOTE: This function is not intended for direct invocation. Instead, its
    source code is extracted using inspect.getsource for execution in a separate
    environment.
    """
    import sys
    import subprocess
    import importlib.metadata
    from packaging.requirements import Requirement
    from packaging.version import Version
    from typing import Optional, Tuple

    def _is_package_installed(
        requirement: Requirement,
    ) -> Tuple[bool, Optional[Version]]:
        try:
            version = importlib.metadata.version(requirement.name)
            return True, Version(version)
        except Exception:
            # Unable to determine the version.
            return False, None

    # Check the installation of individual packages first.
    packages_to_install = []
    for package in packages:
        requirement = Requirement(package)
        installed, version = _is_package_installed(requirement)

        if installed:
            if (
                version
                and requirement.specifier
                and version not in requirement.specifier
            ):
                print(
                    f"Package '{requirement.name}' is already installed and cannot be updated. Skipping."
                    f"The installed version: {version}."
                )
            else:
                print(
                    f"Package '{requirement.name}' is already installed. Skipping."
                    f"The installed version: {version}."
                )
        else:
            packages_to_install.append(package)

    # All packages have been installed with certain version.
    if not packages_to_install:
        return

    # Try to pip install the uninstalled packages.
    pip_command = [sys.executable, "-m", "pip", "install"] + packages_to_install
    print(
        f"Attempting to install the following packages: {', '.join(packages_to_install)}"
    )

    try:
        result = subprocess.run(
            pip_command,
            capture_output=True,
            text=True,
            check=True,
            encoding="utf-8",
        )
        if result.stdout:
            print(f"pip output:\n{result.stdout.strip()}")
        if result.stderr:
            print(f"pip warnings/errors:\n{result.stderr.strip()}")
        print(
            f"Successfully installed the following packages: {', '.join(packages_to_install)}"
        )

    except Exception as e:
        raise RuntimeError(
            f"An unexpected error occurred during package installation: {e}"
        )
"#
}
