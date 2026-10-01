use crate::adapter::adapter_impl::AdapterImpl;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use dbt_common::tracing::dbt_emit::{emit_warn_log_message, print_err};
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult, ErrorCode};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::cell::RefCell;
use std::io::Read;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use ureq::http;
use ureq::{self, Agent, Body, Error as UreqError, RequestBuilder};

const DEFAULT_SHARED_NOTEBOOK_ROOT: &str = "/Shared/dbt_python_model";
const USER_AGENT: &str = "dbt-fs";
const REQUEST_TIMEOUT_SECS: u64 = 60;
const POLL_INTERVAL_SECS: u64 = 10;

pub(crate) struct DatabricksApiClient {
    agent: Agent,
    base_url: String,
    auth_header: String,
    use_user_folder: bool,
    cached_user: RefCell<Option<String>>,
}

impl DatabricksApiClient {
    pub(crate) fn new(adapter: &AdapterImpl, use_user_folder: bool) -> AdapterResult<Self> {
        let host = adapter.get_db_config("host").ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks profile is missing 'host'.",
            )
        })?;
        let token = adapter.get_db_config("token").ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks profile is missing 'token'. A personal access token is required to submit Python models.",
            )
        })?;

        let base_url = Self::normalize_host(host.as_ref());
        let agent: Agent = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(REQUEST_TIMEOUT_SECS)))
            .build()
            .into();

        Ok(Self {
            agent,
            base_url,
            auth_header: format!("Bearer {}", token),
            use_user_folder,
            cached_user: RefCell::new(None),
        })
    }

    pub(crate) fn notebook_path(
        &self,
        catalog: &str,
        schema: &str,
        identifier: &str,
    ) -> AdapterResult<String> {
        let base_folder = if self.use_user_folder {
            let user = self.current_user()?;
            format!("/Users/{}/dbt_python_models/{}/{}", user, catalog, schema)
        } else {
            format!("{}/{schema}", DEFAULT_SHARED_NOTEBOOK_ROOT)
        };

        Ok(format!("{}/{}", base_folder, identifier))
    }

    pub(crate) fn ensure_directory(&self, path: &str) -> AdapterResult<()> {
        let body = json!({ "path": path });
        self.post_json_noop("/api/2.0/workspace/mkdirs", body)
    }

    pub(crate) fn import_notebook(&self, path: &str, compiled_code: &str) -> AdapterResult<()> {
        let encoded = BASE64_STANDARD.encode(compiled_code);
        let body = json!({
            "path": path,
            "language": "PYTHON",
            "format": "SOURCE",
            "overwrite": true,
            "content": encoded,
        });
        self.post_json_noop("/api/2.0/workspace/import", body)
    }

    pub(crate) fn submit_job_run(
        &self,
        run_name: &str,
        job_spec: &serde_json::Value,
        timeout_seconds: u64,
        additional_job_config: &serde_json::Value,
    ) -> AdapterResult<String> {
        let payload = super::build_job_run_payload(
            run_name,
            job_spec.clone(),
            timeout_seconds,
            additional_job_config.clone(),
        );

        let response: SubmitRunResponse = self.post_json("/api/2.1/jobs/runs/submit", payload)?;
        Ok(response.run_id.to_string())
    }

    pub(crate) fn poll_job_completion(
        &self,
        run_id: &str,
        timeout_seconds: u64,
    ) -> AdapterResult<()> {
        let start = Instant::now();
        let timeout_duration = if timeout_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(timeout_seconds))
        };

        loop {
            let state = self.run_state(run_id)?;
            if let Some(life_cycle_state) = state.life_cycle_state.as_deref()
                && matches!(
                    life_cycle_state,
                    "TERMINATED" | "SKIPPED" | "INTERNAL_ERROR"
                )
            {
                return Self::handle_terminal_state(life_cycle_state, &state);
            }

            if let Some(limit) = timeout_duration
                && start.elapsed() >= limit
            {
                return Err(AdapterError::new(
                    AdapterErrorKind::Driver,
                    format!("Databricks job {run_id} timed out after {timeout_seconds} seconds"),
                ));
            }

            thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }
    }

    fn run_state(&self, run_id: &str) -> AdapterResult<RunStatePayload> {
        let response: GetRunResponse =
            self.get("/api/2.1/jobs/runs/get", Some(&[("run_id", run_id)]))?;
        Ok(response.state)
    }

    fn handle_terminal_state(life_cycle_state: &str, state: &RunStatePayload) -> AdapterResult<()> {
        if life_cycle_state == "TERMINATED"
            && state
                .result_state
                .as_deref()
                .is_some_and(|result| result == "SUCCESS")
        {
            return Ok(());
        }

        let result_state = state.result_state.as_deref().unwrap_or("UNKNOWN");
        let message = state
            .state_message
            .as_deref()
            .unwrap_or("Databricks reported a failure without additional context");

        Err(AdapterError::new(
            AdapterErrorKind::Driver,
            format!(
                "Databricks job failed (life_cycle_state={life_cycle_state}, result_state={result_state}): {message}"
            ),
        ))
    }

    fn current_user(&self) -> AdapterResult<String> {
        if !self.use_user_folder {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                "current_user() should not be called when user folders are disabled",
            ));
        }

        if let Some(user) = self.cached_user.borrow().clone() {
            return Ok(user);
        }

        let response: CurrentUserResponse = self.get("/api/2.0/preview/scim/v2/Me", None)?;
        if response.user_name.trim().is_empty() {
            return Err(AdapterError::new(
                AdapterErrorKind::Driver,
                "Unable to determine Databricks user name for notebook uploads",
            ));
        }

        self.cached_user
            .borrow_mut()
            .replace(response.user_name.clone());
        Ok(response.user_name)
    }

    fn post_json<T: DeserializeOwned>(
        &self,
        path: &str,
        body: serde_json::Value,
    ) -> AdapterResult<T> {
        let url = self.full_url(path);
        let request = self.configure_request(self.agent.post(&url), true);
        let response = match request.send_json(body) {
            Ok(resp) => resp,
            Err(err) => return Err(self.map_ureq_error(err)),
        };
        self.parse_json_response(response)
    }

    fn post_json_noop(&self, path: &str, body: serde_json::Value) -> AdapterResult<()> {
        let url = self.full_url(path);
        let request = self.configure_request(self.agent.post(&url), true);
        let response = match request.send_json(body) {
            Ok(resp) => resp,
            Err(err) => return Err(self.map_ureq_error(err)),
        };
        self.consume_response(response)
    }

    fn get<T: DeserializeOwned>(
        &self,
        path: &str,
        query: Option<&[(&str, &str)]>,
    ) -> AdapterResult<T> {
        let url = self.full_url(path);
        let mut request = self.configure_request(self.agent.get(&url), false);
        if let Some(params) = query {
            for (key, value) in params {
                request = request.query(key, value);
            }
        }
        let response = match request.call() {
            Ok(resp) => resp,
            Err(err) => return Err(self.map_ureq_error(err)),
        };
        self.parse_json_response(response)
    }

    fn full_url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    fn configure_request<B>(
        &self,
        mut request: RequestBuilder<B>,
        include_content_type: bool,
    ) -> RequestBuilder<B> {
        request = request
            .header("Authorization", &self.auth_header)
            .header("User-Agent", USER_AGENT);
        if include_content_type {
            request = request.header("Content-Type", "application/json");
        }
        request
    }

    fn parse_json_response<T: DeserializeOwned>(
        &self,
        response: http::Response<Body>,
    ) -> AdapterResult<T> {
        let mut response = self.ensure_success(response)?;
        response.body_mut().read_json::<T>().map_err(|err| {
            AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Failed to parse Databricks API response: {err}"),
            )
        })
    }

    fn consume_response(&self, response: http::Response<Body>) -> AdapterResult<()> {
        let response = self.ensure_success(response)?;
        Self::body_to_string(response)?;
        Ok(())
    }

    fn ensure_success(
        &self,
        response: http::Response<Body>,
    ) -> AdapterResult<http::Response<Body>> {
        let status = response.status().as_u16();
        if status >= 400 {
            let body = Self::body_to_string(response)?;
            Err(Self::http_error(status, body))
        } else {
            Ok(response)
        }
    }

    fn body_to_string(response: http::Response<Body>) -> AdapterResult<String> {
        let mut reader = response.into_body().into_reader();
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer).map_err(|err| {
            AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Failed to read Databricks API response: {err}"),
            )
        })?;
        Ok(String::from_utf8_lossy(&buffer).to_string())
    }

    fn map_ureq_error(&self, err: UreqError) -> AdapterError {
        match err {
            UreqError::StatusCode(status) => Self::http_error(status, String::new()),
            UreqError::Timeout(_) => AdapterError::new(
                AdapterErrorKind::Driver,
                "Databricks API request timed out".to_string(),
            ),
            UreqError::Io(io_err) => AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Databricks API I/O error: {io_err}"),
            ),
            other => AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Databricks API error: {other:?}"),
            ),
        }
    }

    fn http_error(status: u16, body: String) -> AdapterError {
        if let Ok(err) = serde_json::from_str::<DatabricksErrorResponse>(&body) {
            let code = err.error_code.unwrap_or_else(|| "Unknown".to_string());
            let message = err
                .message
                .unwrap_or_else(|| "Databricks API request failed".to_string());
            AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Databricks API (status {status}, code {code}): {message}"),
            )
        } else {
            let trimmed = body.trim();
            let rendered = if trimmed.is_empty() {
                "<empty response>"
            } else {
                trimmed
            };
            AdapterError::new(
                AdapterErrorKind::Driver,
                format!("Databricks API (status {status}) returned an error: {rendered}"),
            )
        }
    }

    fn normalize_host(host: &str) -> String {
        let trimmed = host.trim().trim_end_matches('/');
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            trimmed.to_string()
        } else {
            format!("https://{trimmed}")
        }
    }

    /// https://github.com/databricks/dbt-databricks/blob/3fa9099d7decb194cac1325c859b860919cb6476/dbt/adapters/databricks/api_client.py#L154
    pub(crate) fn create_context(&self, cluster_id: &str) -> AdapterResult<String> {
        self.ensure_cluster_ready(cluster_id)?;
        self.create_execution_context_with_retry(cluster_id)
    }

    fn ensure_cluster_ready(&self, cluster_id: &str) -> AdapterResult<()> {
        let current_status = self.get_cluster_status(cluster_id)?;

        if matches!(current_status.as_str(), "TERMINATED" | "TERMINATING") {
            self.start_cluster(cluster_id)?;
        } else if current_status != "RUNNING" {
            self.wait_for_cluster(cluster_id)?;
        }

        Ok(())
    }

    fn get_cluster_status(&self, cluster_id: &str) -> AdapterResult<String> {
        let response: ClusterStatusResponse =
            self.get("/api/2.0/clusters/get", Some(&[("cluster_id", cluster_id)]))?;

        Ok(response.state)
    }

    fn start_cluster(&self, cluster_id: &str) -> AdapterResult<()> {
        let payload = json!({
            "cluster_id": cluster_id
        });

        self.post_json_noop("/api/2.0/clusters/start", payload)
    }

    fn wait_for_cluster(&self, cluster_id: &str) -> AdapterResult<()> {
        let max_wait_time = Duration::from_secs(900); // 15 minutes
        let start = Instant::now();

        loop {
            // DBX uses get_cluster_libraries_status instead
            let status = self.get_cluster_status(cluster_id)?;

            if status == "RUNNING" {
                return Ok(());
            }

            if start.elapsed() >= max_wait_time {
                return Err(AdapterError::new(
                    AdapterErrorKind::Driver,
                    format!(
                        "Cluster {} did not reach RUNNING state within {} seconds",
                        cluster_id,
                        max_wait_time.as_secs()
                    ),
                ));
            }

            thread::sleep(Duration::from_secs(5));
        }
    }

    fn create_execution_context_with_retry(&self, cluster_id: &str) -> AdapterResult<String> {
        const MAX_RETRIES: u32 = 5;
        let mut last_error = None;

        for attempt in 0..MAX_RETRIES {
            let result = self.try_create_context(cluster_id);

            match result {
                Ok(context_id) => return Ok(context_id),
                Err(e) => {
                    let error_msg = e.to_string().to_lowercase();

                    let is_retryable = error_msg.contains("contextstatus.error")
                        || error_msg.contains("failed to reach running")
                        || (error_msg.contains("context") && error_msg.contains("error"));

                    if is_retryable && attempt < MAX_RETRIES - 1 {
                        // TODO: Track context_id from failed attempt and destroy it
                        // The Python version extracts context_id from the exception and destroys it
                        // For now, we'll skip this cleanup step as it requires parsing error messages

                        // Adapted core backoff logic
                        let base_wait = 2_u64.pow(attempt);
                        let jitter = (SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .subsec_micros()
                            % 1_000_000) as f64
                            / 1_000_000.0;
                        let wait_time = Duration::from_secs_f64(base_wait as f64 + jitter);

                        print_err(
                            ErrorCode::RemoteError,
                            format!(
                                "Execution context creation failed (attempt {}/{}), retrying in {:.1}s: {}",
                                attempt + 1,
                                MAX_RETRIES,
                                wait_time.as_secs_f64(),
                                e
                            ),
                        );

                        thread::sleep(wait_time);
                        last_error = Some(e);
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Driver,
                format!(
                    "Failed to create execution context after {} retries",
                    MAX_RETRIES
                ),
            )
        }))
    }

    fn try_create_context(&self, cluster_id: &str) -> AdapterResult<String> {
        let payload = json!({
            "clusterId": cluster_id,
            "language": "python"
        });

        let response: CreateContextResponse =
            self.post_json("/api/1.2/contexts/create", payload)?;
        Ok(response.id)
    }

    /// https://docs.databricks.com/api/workspace/contexts/destroy
    pub(crate) fn destroy_context(&self, cluster_id: &str, context_id: &str) -> AdapterResult<()> {
        let payload = json!({
            "clusterId": cluster_id,
            "contextId": context_id
        });

        self.post_json_noop("/api/1.2/contexts/destroy", payload)
    }

    /// https://docs.databricks.com/api/workspace/commands/execute
    pub(crate) fn execute_command(
        &self,
        cluster_id: &str,
        context_id: &str,
        command: &str,
    ) -> AdapterResult<String> {
        let payload = json!({
            "clusterId": cluster_id,
            "contextId": context_id,
            "language": "python",
            "command": command
        });

        let response: ExecuteCommandResponse =
            self.post_json("/api/1.2/commands/execute", payload)?;
        Ok(response.id)
    }

    /// https://docs.databricks.com/api/workspace/commands/status
    pub(crate) fn poll_command_completion(
        &self,
        cluster_id: &str,
        context_id: &str,
        command_id: &str,
        timeout_seconds: u64,
    ) -> AdapterResult<()> {
        let start = Instant::now();
        let timeout_duration = if timeout_seconds == 0 {
            None
        } else {
            Some(Duration::from_secs(timeout_seconds))
        };

        loop {
            let response: CommandStatusResponse = self.get(
                "/api/1.2/commands/status",
                Some(&[
                    ("clusterId", cluster_id),
                    ("contextId", context_id),
                    ("commandId", command_id),
                ]),
            )?;

            match response.status.as_str() {
                "Finished" => {
                    return Ok(());
                }
                "Error" | "Cancelled" => {
                    let cause = response
                        .results
                        .and_then(|r| r.cause)
                        .unwrap_or_else(|| "Unknown error".to_string());
                    return Err(AdapterError::new(
                        AdapterErrorKind::Driver,
                        format!(
                            "Command execution failed with status '{}': {}",
                            response.status, cause
                        ),
                    ));
                }
                "Running" | "Queued" => {}
                _ => {
                    emit_warn_log_message(
                        ErrorCode::UnexpectedApiResponse,
                        format!("Unknown command status: {}", response.status),
                    );
                }
            }

            if let Some(limit) = timeout_duration
                && start.elapsed() >= limit
            {
                return Err(AdapterError::new(
                    AdapterErrorKind::Driver,
                    format!(
                        "Command execution timed out after {} seconds",
                        timeout_seconds
                    ),
                ));
            }

            thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }
    }

    /// https://docs.databricks.com/api/workspace/jobs/list
    pub(crate) fn search_workflows_by_name(
        &self,
        workflow_name: &str,
    ) -> AdapterResult<WorkflowSearchResponse> {
        self.get(
            "/api/2.1/jobs/list",
            Some(&[("name", workflow_name), ("expand_tasks", "false")]),
        )
    }

    /// https://docs.databricks.com/api/workspace/jobs/create
    pub(crate) fn create_workflow(&self, workflow_spec: serde_json::Value) -> AdapterResult<u64> {
        let response: CreateWorkflowResponse =
            self.post_json("/api/2.1/jobs/create", workflow_spec)?;
        Ok(response.job_id)
    }

    /// https://docs.databricks.com/api/workspace/jobs/update
    pub(crate) fn update_workflow(
        &self,
        job_id: u64,
        workflow_spec: serde_json::Value,
    ) -> AdapterResult<()> {
        let mut payload = workflow_spec;
        if let serde_json::Value::Object(ref mut map) = payload {
            map.insert("job_id".to_string(), json!(job_id));
        }

        self.post_json::<serde_json::Value>("/api/2.1/jobs/update", payload)?;
        Ok(())
    }

    /// https://docs.databricks.com/api/workspace/jobs/run-now
    pub(crate) fn run_workflow(&self, job_id: u64, enable_queueing: bool) -> AdapterResult<u64> {
        let payload = json!({
            "job_id": job_id,
            "queue": {
                "enabled": enable_queueing
            }
        });

        let response: RunWorkflowResponse = self.post_json("/api/2.1/jobs/run-now", payload)?;
        Ok(response.run_id)
    }
}

#[derive(Deserialize)]
struct SubmitRunResponse {
    run_id: u64,
}

#[derive(Deserialize)]
struct GetRunResponse {
    state: RunStatePayload,
}

#[derive(Deserialize)]
struct RunStatePayload {
    #[serde(rename = "life_cycle_state")]
    life_cycle_state: Option<String>,
    #[serde(rename = "result_state")]
    result_state: Option<String>,
    #[serde(rename = "state_message")]
    state_message: Option<String>,
}

#[derive(Deserialize)]
struct CurrentUserResponse {
    #[serde(rename = "userName")]
    user_name: String,
}

#[derive(Deserialize)]
struct DatabricksErrorResponse {
    #[serde(rename = "error_code")]
    error_code: Option<String>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct CreateContextResponse {
    id: String,
}

#[derive(Deserialize)]
struct ExecuteCommandResponse {
    id: String,
}

#[derive(Deserialize)]
struct CommandStatusResponse {
    status: String,
    #[serde(flatten)]
    results: Option<CommandResults>,
}

#[derive(Deserialize)]
struct CommandResults {
    #[serde(rename = "cause")]
    cause: Option<String>,
}

#[derive(Deserialize)]
struct ClusterStatusResponse {
    state: String,
}

#[derive(Deserialize)]
pub(crate) struct WorkflowSearchResponse {
    #[serde(default)]
    pub jobs: Vec<WorkflowJob>,
}

#[derive(Deserialize)]
pub(crate) struct WorkflowJob {
    pub job_id: u64,
}

#[derive(Deserialize)]
struct CreateWorkflowResponse {
    job_id: u64,
}

#[derive(Deserialize)]
struct RunWorkflowResponse {
    run_id: u64,
}
