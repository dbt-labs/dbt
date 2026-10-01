use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use thiserror::Error;
use tonic::{
    Request, Response,
    metadata::MetadataValue,
    transport::{Channel, ClientTlsConfig, Endpoint},
};

use crate::auth::OAuthTokenSource;
use crate::auth::browser_flow::is_retryable_token_error;
use crate::proto::query_cache::{
    CloneRequest, CloneResponse, ConfirmExecutionRequest, ConfirmExecutionResponse,
    GetExplainMessagesRequest, GetExplainMessagesResponse, RecordExecutionsRequest,
    RecordExecutionsResponse, ResolveDeferredRelationsRequest, SelectorRequest, SelectorResponse,
    SubmitEnrichedSqlRequest, SubmitSqlResponse, SubmitSqlSpeculativeResponse,
    SubmitTelemetryBatchRequest, SubmitTelemetryBatchResponse, SubmitValuesRequest,
    ValidateClientVersionRequest, client_validation_client::ClientValidationClient,
    execution_client::ExecutionClient, explain_client::ExplainClient,
    selector_service_client::SelectorServiceClient, sql_client::SqlClient,
};
use crate::proto::query_cache::{client_telemetry_client::ClientTelemetryClient, clone_client};
use crate::service_config::{RunCacheServiceConfig, RunCacheServiceConfigError};

const AUTHORIZATION_HEADER: &str = "authorization";
const CLOUD_RUN_ID_HEADER: &str = "x-dbt-cloud-run-id";
const INVOCATION_ID_HEADER: &str = "x-dbt-invocation-id";
const ORG_ID_HEADER: &str = "x-organization-id";
const OS_NAME_HEADER: &str = "x-os-name";
const REQUEST_ID_HEADER: &str = "x-request-id";
const SESSION_ID_HEADER: &str = "x-session-id";
const SUBMITTED_AT_EPOCH_HEADER: &str = "x-submitted-at-epoch";
const SYSTEM_USER_ID_HEADER: &str = "x-system-user-id";
const HTTP2_KEEP_ALIVE_INTERVAL: Duration = Duration::from_secs(30);
const HTTP2_KEEP_ALIVE_TIMEOUT: Duration = Duration::from_secs(10);

/// gRPC caps a retry policy's `maxAttempts` at 5; higher configured values are
/// clamped to this.
const GRPC_MAX_ATTEMPTS_CAP: u32 = 5;
/// Backoff before the first retry; doubled after each attempt up to
/// [`RETRY_MAX_BACKOFF`].
const RETRY_INITIAL_BACKOFF: Duration = Duration::from_secs(1);
const RETRY_MAX_BACKOFF: Duration = Duration::from_secs(10);
const RETRY_BACKOFF_MULTIPLIER: u32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClientVersionStatus {
    Supported,
    Unsupported,
    Skipped,
}

impl ClientVersionStatus {
    pub fn allows_service_use(self) -> bool {
        matches!(self, Self::Supported)
    }
}

#[derive(Debug, Error)]
pub enum RunCacheServiceError {
    #[error("dbt State service is disabled")]
    Disabled,
    #[error(transparent)]
    Config(#[from] RunCacheServiceConfigError),
    #[error("dbt State service transport failed: {0}")]
    Transport(#[from] tonic::transport::Error),
    #[error("dbt State service authentication request failed: {0}")]
    AuthRequest(#[from] reqwest::Error),
    #[error("dbt State service authentication failed: {0}")]
    Auth(String),
    #[error("Access to organization '{org_id}' has been disabled")]
    OrgDisabled { org_id: String },
    #[error("dbt State service metadata failed: {0}")]
    Metadata(String),
    #[error("dbt State service RPC failed: {0}")]
    Rpc(#[from] tonic::Status),
    #[error("dbt State authentication was cancelled")]
    Aborted,
    #[error("dbt State authentication timed out after {0}s")]
    Timeout(u64),
    #[error(
        "dbt State authentication requires an interactive terminal, but none is available in this environment"
    )]
    NoInteractiveTerminal,
    /// No dbt platform credential is available for the account the project
    /// configures. Deliberately not an `Auth` error: like
    /// [`Self::NoInteractiveTerminal`] it is not user-actionable *auth*, so
    /// validation fails open and dbt State is bypassed for the invocation
    /// rather than failing the run.
    #[error("{}", platform_account_unavailable_message(.configured, .found.as_deref()))]
    PlatformAccountUnavailable {
        configured: String,
        found: Option<String>,
    },
}

fn platform_account_unavailable_message(configured: &str, found: Option<&str>) -> String {
    let found = match found {
        Some(found) => format!(", but the available credentials belong to account {found}"),
        None => String::new(),
    };
    format!(
        "no dbt platform credentials for account {configured}{found}. \
         Run `dbt login` with account {configured}, or update 'account_id' in the \
         'dbt-cloud' block of dbt_project.yml"
    )
}

impl RunCacheServiceError {
    /// Short, stable label for the error's variant, used as `error_type` on
    /// `ClientEnrichedSqlPrepared` telemetry. Mirrors the Python dbt State
    /// client's use of the caught exception's class name.
    pub fn error_type_label(&self) -> &'static str {
        match self {
            Self::Disabled => "Disabled",
            Self::Config(_) => "Config",
            Self::Transport(_) => "Transport",
            Self::AuthRequest(_) => "AuthRequest",
            Self::Auth(_) => "Auth",
            Self::OrgDisabled { .. } => "OrgDisabled",
            Self::Metadata(_) => "Metadata",
            Self::Rpc(_) => "Rpc",
            Self::Aborted => "Aborted",
            Self::Timeout(_) => "Timeout",
            Self::NoInteractiveTerminal => "NoInteractiveTerminal",
            Self::PlatformAccountUnavailable { .. } => "PlatformAccountUnavailable",
        }
    }

    pub fn is_transient_transport_rpc(&self) -> bool {
        match self {
            Self::Rpc(status) => {
                status.code() == tonic::Code::Unknown && status.message() == "transport error"
            }
            _ => false,
        }
    }

    /// Whether a failed RPC is a transient transport failure that is safe to
    /// retry. Retries only `UNAVAILABLE` — the status a dropped connection
    /// ("connection reset by peer", connection-refused, socket-closed, etc.)
    /// surfaces as — plus tonic's `Unknown` "transport error" (see
    /// [`Self::is_transient_transport_rpc`]), which reports a connection drop
    /// mid-request. Both are safe to retry: per gRPC semantics the server did
    /// not process the request.
    pub fn is_retryable_transport(&self) -> bool {
        self.is_transient_transport_rpc()
            || matches!(self, Self::Rpc(status) if status.code() == tonic::Code::Unavailable)
    }

    /// Returns `true` when this error should permanently disable the client
    /// for the rest of the process.
    pub fn disables_service(&self) -> bool {
        match self {
            Self::OrgDisabled { .. } => true,
            Self::Rpc(status) => matches!(
                status.code(),
                tonic::Code::PermissionDenied | tonic::Code::Unauthenticated
            ),
            Self::AuthRequest(_) => is_retryable_token_error(self),
            _ => false,
        }
    }

    pub fn is_user_actionable_auth(&self) -> bool {
        matches!(
            self,
            Self::Auth(_) | Self::AuthRequest(_) | Self::Aborted | Self::Timeout(_)
        ) && !self.disables_service()
    }

    /// Whether a failed telemetry submission is worth retrying. Mirrors the
    /// Python dbt State client's telemetry dispatcher: `Unimplemented`,
    /// `PermissionDenied`, and `InvalidArgument` RPC errors indicate the
    /// batch itself will never succeed, so those are dropped immediately.
    /// `Disabled` means the client has been permanently disabled for the rest
    /// of the process (see `GrpcRunCacheServiceClient::before_request`), so
    /// every subsequent attempt would fail identically — also not worth
    /// retrying. Everything else (including other non-RPC errors) is
    /// treated as transient.
    pub fn is_retriable_telemetry_submission(&self) -> bool {
        match self {
            Self::Disabled => false,
            Self::Rpc(status) => !matches!(
                status.code(),
                tonic::Code::Unimplemented
                    | tonic::Code::PermissionDenied
                    | tonic::Code::InvalidArgument
            ),
            _ => true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunCacheClientMetadata {
    pub session_id: String,
    pub system_user_id: String,
    pub os_name: String,
    /// dbt's per-invocation UUID for this run.
    pub dbt_invocation_id: String,
    /// dbt platform run ID, if running on the dbt platform (else empty).
    pub dbt_cloud_run_id: String,
}

impl RunCacheClientMetadata {
    pub fn new(
        session_id: impl Into<String>,
        system_user_id: impl Into<String>,
        os_name: impl Into<String>,
        dbt_invocation_id: impl Into<String>,
        dbt_cloud_run_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            system_user_id: system_user_id.into(),
            os_name: os_name.into(),
            dbt_invocation_id: dbt_invocation_id.into(),
            dbt_cloud_run_id: dbt_cloud_run_id.into(),
        }
    }

    fn attach<T>(&self, mut request: Request<T>) -> Result<Request<T>, RunCacheServiceError> {
        let metadata = request.metadata_mut();
        insert_metadata(metadata, REQUEST_ID_HEADER, &new_request_id())?;
        insert_metadata(metadata, SESSION_ID_HEADER, &self.session_id)?;
        insert_metadata(
            metadata,
            SUBMITTED_AT_EPOCH_HEADER,
            &current_epoch_millis().to_string(),
        )?;
        insert_metadata(metadata, SYSTEM_USER_ID_HEADER, &self.system_user_id)?;
        insert_metadata(metadata, OS_NAME_HEADER, &self.os_name)?;
        // Both identifiers are constant for the lifetime of a single dbt invocation.
        // The dbt_cloud_run_id is only present when running on the dbt platform, so
        // each header is omitted entirely when its value is empty.
        if !self.dbt_invocation_id.is_empty() {
            insert_metadata(metadata, INVOCATION_ID_HEADER, &self.dbt_invocation_id)?;
        }
        if !self.dbt_cloud_run_id.is_empty() {
            insert_metadata(metadata, CLOUD_RUN_ID_HEADER, &self.dbt_cloud_run_id)?;
        }
        Ok(request)
    }
}

impl Default for RunCacheClientMetadata {
    fn default() -> Self {
        Self {
            session_id: new_request_id(),
            system_user_id: String::new(),
            os_name: std::env::consts::OS.to_string(),
            dbt_invocation_id: String::new(),
            dbt_cloud_run_id: get_cloud_run_id(),
        }
    }
}

/// Return the dbt platform run ID, or an empty string when not running there.
pub fn get_cloud_run_id() -> String {
    std::env::var("DBT_CLOUD_RUN_ID").unwrap_or_default()
}

#[derive(Debug, Clone)]
enum RunCacheAuth {
    None,
    OAuth(Arc<OAuthTokenSource>),
}

impl RunCacheAuth {
    fn from_config(config: &RunCacheServiceConfig) -> Result<Self, RunCacheServiceError> {
        if !config.secure {
            return Ok(Self::None);
        }
        Ok(Self::OAuth(Arc::new(OAuthTokenSource::new(config)?)))
    }

    async fn attach<T>(&self, request: Request<T>) -> Result<Request<T>, RunCacheServiceError> {
        match self {
            Self::None => Ok(request),
            Self::OAuth(source) => {
                let token = source.token().await?;
                let org_id = source.resolve_org_id(&token)?;
                with_auth_metadata(request, &token.id_token, &org_id)
            }
        }
    }
}

fn with_auth_metadata<T>(
    mut request: Request<T>,
    id_token: &str,
    org_id: &str,
) -> Result<Request<T>, RunCacheServiceError> {
    let authorization = format!("Bearer {id_token}").parse().map_err(|err| {
        RunCacheServiceError::Auth(format!("invalid authorization metadata: {err}"))
    })?;
    let org_id = org_id
        .parse()
        .map_err(|err| RunCacheServiceError::Auth(format!("invalid org metadata: {err}")))?;

    let metadata = request.metadata_mut();
    metadata.insert(AUTHORIZATION_HEADER, authorization);
    metadata.insert(ORG_ID_HEADER, org_id);
    Ok(request)
}

pub type SharedRunCacheServiceClient = Arc<dyn RunCacheServiceClient>;

pub fn shared_run_cache_service_client<C>(client: C) -> SharedRunCacheServiceClient
where
    C: RunCacheServiceClient + 'static,
{
    Arc::new(client)
}

#[async_trait]
pub trait RunCacheServiceClient: Send + Sync {
    fn is_disabled(&self) -> bool {
        false
    }

    async fn validate_client_version(&self) -> Result<ClientVersionStatus, RunCacheServiceError>;

    async fn submit_enriched_sql(
        &self,
        request: SubmitEnrichedSqlRequest,
    ) -> Result<SubmitSqlResponse, RunCacheServiceError>;

    /// Speculative-submit RPC used while the dependency last-modified prefetch
    /// is still in flight, returning an early verdict based on cache reads only.
    async fn submit_enriched_sql_speculative(
        &self,
        _request: SubmitEnrichedSqlRequest,
    ) -> Result<SubmitSqlSpeculativeResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn submit_values(
        &self,
        request: SubmitValuesRequest,
    ) -> Result<SubmitSqlResponse, RunCacheServiceError>;

    async fn confirm_execution(
        &self,
        request: ConfirmExecutionRequest,
    ) -> Result<ConfirmExecutionResponse, RunCacheServiceError>;

    async fn record_executions(
        &self,
        _request: RecordExecutionsRequest,
    ) -> Result<RecordExecutionsResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn register_clone(
        &self,
        _request: CloneRequest,
    ) -> Result<CloneResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn submit_telemetry_batch(
        &self,
        _request: SubmitTelemetryBatchRequest,
    ) -> Result<SubmitTelemetryBatchResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn get_explain_messages(
        &self,
        _request: GetExplainMessagesRequest,
    ) -> Result<GetExplainMessagesResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }
    async fn get_state_selection(
        &self,
        _request: SelectorRequest,
    ) -> Result<SelectorResponse, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }

    async fn resolve_deferred_relations(
        &self,
        _request: ResolveDeferredRelationsRequest,
    ) -> Result<HashMap<String, String>, RunCacheServiceError> {
        Err(RunCacheServiceError::Disabled)
    }
}

#[derive(Debug, Clone)]
pub struct GrpcRunCacheServiceClient {
    sql: SqlClient<Channel>,
    clone: clone_client::CloneClient<Channel>,
    execution: ExecutionClient<Channel>,
    client_telemetry: ClientTelemetryClient<Channel>,
    client_validation: ClientValidationClient<Channel>,
    explain: ExplainClient<Channel>,
    state_selector: SelectorServiceClient<Channel>,
    auth: RunCacheAuth,
    metadata: RunCacheClientMetadata,
    disabled: Arc<AtomicBool>,
    /// Total attempts per request on transient transport failures, clamped to
    /// [`GRPC_MAX_ATTEMPTS_CAP`]. `1` disables retries.
    max_attempts: u32,
    /// Backoff before the first retry (doubled up to [`RETRY_MAX_BACKOFF`]).
    /// A field rather than a constant so tests can drive it to zero and avoid
    /// real sleeps; production always uses [`RETRY_INITIAL_BACKOFF`].
    initial_backoff: Duration,
}

impl GrpcRunCacheServiceClient {
    pub async fn connect(config: RunCacheServiceConfig) -> Result<Self, RunCacheServiceError> {
        Self::connect_with_metadata(config, RunCacheClientMetadata::default()).await
    }

    pub async fn connect_with_metadata(
        config: RunCacheServiceConfig,
        metadata: RunCacheClientMetadata,
    ) -> Result<Self, RunCacheServiceError> {
        if !config.enabled {
            return Err(RunCacheServiceError::Disabled);
        }

        let auth = RunCacheAuth::from_config(&config)?;
        let mut endpoint = Endpoint::from_shared(config.endpoint_uri())?
            .connect_timeout(config.timeout)
            .timeout(config.timeout)
            .http2_keep_alive_interval(HTTP2_KEEP_ALIVE_INTERVAL)
            .keep_alive_timeout(HTTP2_KEEP_ALIVE_TIMEOUT)
            .keep_alive_while_idle(true);
        if config.secure {
            endpoint = endpoint.tls_config(ClientTlsConfig::new().with_native_roots())?;
        }
        let channel = endpoint.connect().await?;

        Ok(Self {
            sql: SqlClient::new(channel.clone()),
            clone: clone_client::CloneClient::new(channel.clone()),
            execution: ExecutionClient::new(channel.clone()),
            client_telemetry: ClientTelemetryClient::new(channel.clone()),
            client_validation: ClientValidationClient::new(channel.clone()),
            state_selector: SelectorServiceClient::new(channel.clone()),
            explain: ExplainClient::new(channel),
            auth,
            metadata,
            disabled: Arc::new(AtomicBool::new(false)),
            max_attempts: config.max_attempts.clamp(1, GRPC_MAX_ATTEMPTS_CAP),
            initial_backoff: RETRY_INITIAL_BACKOFF,
        })
    }

    fn before_request(&self) -> Result<(), RunCacheServiceError> {
        if self.disabled.load(Ordering::Acquire) {
            Err(RunCacheServiceError::Disabled)
        } else {
            Ok(())
        }
    }

    fn after_request<T>(
        &self,
        result: Result<T, RunCacheServiceError>,
    ) -> Result<T, RunCacheServiceError> {
        match result {
            Err(err) if err.disables_service() => {
                if !self.disabled.swap(true, Ordering::AcqRel) {
                    emit_warn_log_message(
                        ErrorCode::StateServiceWarn,
                        format!(
                            "dbt State service disabled: {}; executing normally",
                            format_error_chain(&err)
                        ),
                    );
                }
                Err(RunCacheServiceError::Disabled)
            }
            other => other,
        }
    }

    async fn attach<T>(&self, request: Request<T>) -> Result<Request<T>, RunCacheServiceError> {
        self.before_request()?;
        let result = match self.auth.attach(request).await {
            Ok(request) => self.metadata.attach(request),
            Err(err) => Err(err),
        };
        self.after_request(result)
    }

    fn response<T>(
        &self,
        result: Result<Response<T>, tonic::Status>,
    ) -> Result<T, RunCacheServiceError> {
        self.after_request(
            result
                .map(|response| response.into_inner())
                .map_err(Into::into),
        )
    }

    /// Runs a single RPC operation, retrying it on transient transport failures
    /// with exponential backoff. tonic has no equivalent of gRPC's C-core
    /// service-config retry policy, so the loop reproduces it explicitly: only
    /// [`RunCacheServiceError::is_retryable_transport`] errors are retried, up to
    /// `self.max_attempts` total attempts. `attempt` is invoked afresh each time
    /// so a new `x-request-id` and auth token are attached per attempt.
    async fn with_retry<T, F, Fut>(&self, mut attempt: F) -> Result<T, RunCacheServiceError>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T, RunCacheServiceError>>,
    {
        let mut remaining = self.max_attempts.max(1);
        let mut backoff = self.initial_backoff;
        loop {
            remaining -= 1;
            match attempt().await {
                Err(err) if remaining > 0 && err.is_retryable_transport() => {
                    tokio::time::sleep(backoff).await;
                    backoff = backoff
                        .saturating_mul(RETRY_BACKOFF_MULTIPLIER)
                        .min(RETRY_MAX_BACKOFF);
                }
                other => return other,
            }
        }
    }
}

#[async_trait]
impl RunCacheServiceClient for GrpcRunCacheServiceClient {
    fn is_disabled(&self) -> bool {
        self.disabled.load(Ordering::Acquire)
    }

    async fn validate_client_version(&self) -> Result<ClientVersionStatus, RunCacheServiceError> {
        let response = self
            .with_retry(|| async {
                let request = self
                    .attach(Request::new(ValidateClientVersionRequest {
                        dbt_run_cache_version: env!("CARGO_PKG_VERSION").to_string(),
                    }))
                    .await?;
                self.response(
                    self.client_validation
                        .clone()
                        .validate_client_version(request)
                        .await,
                )
            })
            .await?;

        if response.is_supported {
            Ok(ClientVersionStatus::Supported)
        } else {
            Ok(ClientVersionStatus::Unsupported)
        }
    }

    async fn submit_enriched_sql(
        &self,
        request: SubmitEnrichedSqlRequest,
    ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.sql.clone().submit_enriched_sql(request).await)
        })
        .await
    }

    async fn submit_enriched_sql_speculative(
        &self,
        request: SubmitEnrichedSqlRequest,
    ) -> Result<SubmitSqlSpeculativeResponse, RunCacheServiceError> {
        // Deliberately not retried: the speculative verdict is fired while the
        // dependency prefetch is still in flight to shave latency, so blocking on
        // retry backoff would defeat its purpose. A transient failure here just
        // falls back to the authoritative `submit_enriched_sql`, which is retried.
        let request = self.attach(Request::new(request)).await?;
        Ok(self
            .sql
            .clone()
            .submit_enriched_sql_speculative(request)
            .await?
            .into_inner())
    }

    async fn submit_values(
        &self,
        request: SubmitValuesRequest,
    ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.sql.clone().submit_values(request).await)
        })
        .await
    }

    async fn confirm_execution(
        &self,
        request: ConfirmExecutionRequest,
    ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.execution.clone().confirm_execution(request).await)
        })
        .await
    }

    async fn record_executions(
        &self,
        request: RecordExecutionsRequest,
    ) -> Result<RecordExecutionsResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.execution.clone().record_executions(request).await)
        })
        .await
    }

    async fn register_clone(
        &self,
        request: CloneRequest,
    ) -> Result<CloneResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.clone.clone().register_clone(request).await)
        })
        .await
    }

    async fn submit_telemetry_batch(
        &self,
        request: SubmitTelemetryBatchRequest,
    ) -> Result<SubmitTelemetryBatchResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(
                self.client_telemetry
                    .clone()
                    .submit_telemetry_batch(request)
                    .await,
            )
        })
        .await
    }

    async fn get_explain_messages(
        &self,
        request: GetExplainMessagesRequest,
    ) -> Result<GetExplainMessagesResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(self.explain.clone().get_explain_messages(request).await)
        })
        .await
    }

    async fn resolve_deferred_relations(
        &self,
        request: ResolveDeferredRelationsRequest,
    ) -> Result<HashMap<String, String>, RunCacheServiceError> {
        let request = self.attach(Request::new(request)).await?;
        let response = self.response(
            self.execution
                .clone()
                .resolve_deferred_relations(request)
                .await,
        )?;

        Ok(response.fqn_by_unique_id)
    }

    async fn get_state_selection(
        &self,
        request: SelectorRequest,
    ) -> Result<SelectorResponse, RunCacheServiceError> {
        self.with_retry(|| async {
            let request = self.attach(Request::new(request.clone())).await?;
            self.response(
                self.state_selector
                    .clone()
                    .get_state_selection(request)
                    .await,
            )
        })
        .await
    }
}

/// Generate a fresh client-side request id (used both as the gRPC
/// `x-request-id` header and, for SQL submissions, as the id correlating a
/// submission attempt's `ClientEnrichedSqlPrepared` telemetry event with the
/// attempt itself, independent of whatever the server ends up deciding).
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn current_epoch_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

fn insert_metadata(
    metadata: &mut tonic::metadata::MetadataMap,
    key: &'static str,
    value: &str,
) -> Result<(), RunCacheServiceError> {
    let value = MetadataValue::try_from(value)
        .map_err(|err| RunCacheServiceError::Metadata(format!("invalid {key} metadata: {err}")))?;
    metadata.insert(key, value);
    Ok(())
}

pub async fn validate_client_version_fail_open<C>(
    client: &C,
) -> Result<ClientVersionStatus, RunCacheServiceError>
where
    C: RunCacheServiceClient + ?Sized,
{
    match client.validate_client_version().await {
        Ok(status) => Ok(status),
        Err(RunCacheServiceError::Disabled) => Ok(ClientVersionStatus::Skipped),
        Err(err) if !err.is_user_actionable_auth() => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State client validation failed: {}; executing normally",
                    format_error_chain(&err)
                ),
            );
            Ok(ClientVersionStatus::Skipped)
        }
        Err(err) => Err(err),
    }
}

/// Render an error with its full `source()` chain joined by `: `.
///
/// Several upstream error types (notably `tonic::transport::Error`) have a
/// terse `Display` that hides the actual cause. Walking the chain surfaces it.
pub fn format_error_chain(err: &(dyn std::error::Error + 'static)) -> String {
    use std::fmt::Write;
    let mut out = err.to_string();
    let mut source = err.source();
    while let Some(s) = source {
        let _ = write!(out, ": {s}");
        source = s.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicU32;
    use std::time::Duration;

    use super::*;
    use crate::service_config::{DEFAULT_API_URL, DEFAULT_OAUTH_AUTH_URL, DEFAULT_OAUTH_TOKEN_URL};

    #[test]
    fn only_supported_validation_allows_service_use() {
        assert!(ClientVersionStatus::Supported.allows_service_use());
        assert!(!ClientVersionStatus::Skipped.allows_service_use());
        assert!(!ClientVersionStatus::Unsupported.allows_service_use());
    }

    struct ValidationClient(RunCacheServiceError);

    #[async_trait]
    impl RunCacheServiceClient for ValidationClient {
        async fn validate_client_version(
            &self,
        ) -> Result<ClientVersionStatus, RunCacheServiceError> {
            Err(match &self.0 {
                RunCacheServiceError::Auth(message) => {
                    RunCacheServiceError::Auth(message.to_string())
                }
                RunCacheServiceError::Rpc(status) => {
                    RunCacheServiceError::Rpc(tonic::Status::new(status.code(), status.message()))
                }
                RunCacheServiceError::Disabled => RunCacheServiceError::Disabled,
                _ => unreachable!("validation tests only use Auth, Rpc, and Disabled"),
            })
        }

        async fn submit_enriched_sql(
            &self,
            _request: SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            unreachable!("validation test should not submit SQL")
        }

        async fn submit_values(
            &self,
            _request: SubmitValuesRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            unreachable!("validation test should not submit values")
        }

        async fn confirm_execution(
            &self,
            _request: ConfirmExecutionRequest,
        ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
            unreachable!("validation test should not confirm execution")
        }
    }

    #[dbt_runtime::test]
    async fn validation_rpc_error_fails_open_to_skipped() {
        let client = ValidationClient(RunCacheServiceError::Rpc(tonic::Status::unavailable(
            "service unavailable",
        )));

        assert_eq!(
            validate_client_version_fail_open(&client).await.unwrap(),
            ClientVersionStatus::Skipped
        );
    }

    #[dbt_runtime::test]
    async fn validation_auth_error_fails_closed() {
        let client = ValidationClient(RunCacheServiceError::Auth("bad credentials".to_string()));

        assert!(matches!(
            validate_client_version_fail_open(&client).await,
            Err(RunCacheServiceError::Auth(_))
        ));
    }

    #[dbt_runtime::test]
    async fn validation_unexpected_rpc_error_fails_open_to_skipped() {
        let client = ValidationClient(RunCacheServiceError::Rpc(tonic::Status::internal(
            "service error",
        )));

        assert_eq!(
            validate_client_version_fail_open(&client).await.unwrap(),
            ClientVersionStatus::Skipped
        );
    }

    #[dbt_runtime::test]
    async fn validation_disabled_error_fails_open_to_skipped() {
        let client = ValidationClient(RunCacheServiceError::Disabled);

        assert_eq!(
            validate_client_version_fail_open(&client).await.unwrap(),
            ClientVersionStatus::Skipped
        );
    }

    #[dbt_runtime::test]
    async fn speculative_submit_defaults_to_disabled() {
        let client = ValidationClient(RunCacheServiceError::Disabled);

        assert!(matches!(
            client
                .submit_enriched_sql_speculative(SubmitEnrichedSqlRequest::default())
                .await,
            Err(RunCacheServiceError::Disabled)
        ));
    }

    #[dbt_runtime::test]
    async fn disabled_config_does_not_connect() {
        let err = GrpcRunCacheServiceClient::connect(RunCacheServiceConfig::disabled())
            .await
            .unwrap_err();

        assert!(matches!(err, RunCacheServiceError::Disabled));
    }

    #[test]
    fn auth_metadata_contains_bearer_token_and_org_context() {
        let request = with_auth_metadata(Request::new(()), "test-token", "test-org").unwrap();

        assert_eq!(
            request
                .metadata()
                .get(AUTHORIZATION_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "Bearer test-token"
        );
        assert_eq!(
            request
                .metadata()
                .get(ORG_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "test-org"
        );
    }

    #[test]
    fn secure_auth_without_client_secret_constructs_browser_source() {
        let config = RunCacheServiceConfig {
            enabled: true,
            api_url: DEFAULT_API_URL.to_string(),
            secure: true,
            org_id: Some("test-org".to_string()),
            oauth_client_id: "client-id".to_string(),
            oauth_client_secret: None,
            oauth_token_url: DEFAULT_OAUTH_TOKEN_URL.to_string(),
            oauth_auth_url: DEFAULT_OAUTH_AUTH_URL.to_string(),
            timeout: Duration::from_secs(60),
            ..RunCacheServiceConfig::disabled()
        };

        RunCacheAuth::from_config(&config)
            .expect("construction should succeed without a client secret");
    }

    #[test]
    fn org_disabled_error_renders_with_org_id() {
        let err = RunCacheServiceError::OrgDisabled {
            org_id: "test-org".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Access to organization 'test-org' has been disabled"
        );
    }

    #[test]
    fn account_access_errors_disable_service() {
        assert!(
            RunCacheServiceError::OrgDisabled {
                org_id: "test-org".to_string()
            }
            .disables_service()
        );
        assert!(
            RunCacheServiceError::Rpc(tonic::Status::permission_denied("locked"))
                .disables_service()
        );
        assert!(
            RunCacheServiceError::Rpc(tonic::Status::unauthenticated("locked")).disables_service()
        );
        assert!(!RunCacheServiceError::Rpc(tonic::Status::unavailable("down")).disables_service());
        assert!(
            !RunCacheServiceError::Rpc(tonic::Status::unknown("transport error"))
                .disables_service()
        );
        assert!(!RunCacheServiceError::Auth("bad credentials".to_string()).disables_service());
    }

    #[dbt_runtime::test]
    async fn initial_transport_error_does_not_disable_service() {
        let err = GrpcRunCacheServiceClient::connect(RunCacheServiceConfig {
            enabled: true,
            api_url: "http://127.0.0.1:1".to_string(),
            secure: false,
            timeout: Duration::from_millis(1),
            ..RunCacheServiceConfig::disabled()
        })
        .await
        .unwrap_err();

        assert!(matches!(err, RunCacheServiceError::Transport(_)));
        assert!(!err.disables_service());
    }

    /// Build a lazily-connected client (no live endpoint) for exercising the
    /// pure request/response bookkeeping and the retry loop. Backoff is zeroed
    /// so retry tests run without real sleeps.
    fn lazy_test_client(max_attempts: u32) -> GrpcRunCacheServiceClient {
        let channel = Endpoint::from_static("http://127.0.0.1:1").connect_lazy();
        GrpcRunCacheServiceClient {
            sql: SqlClient::new(channel.clone()),
            clone: clone_client::CloneClient::new(channel.clone()),
            execution: ExecutionClient::new(channel.clone()),
            client_telemetry: ClientTelemetryClient::new(channel.clone()),
            client_validation: ClientValidationClient::new(channel.clone()),
            state_selector: SelectorServiceClient::new(channel.clone()),
            explain: ExplainClient::new(channel),
            auth: RunCacheAuth::None,
            metadata: RunCacheClientMetadata::default(),
            disabled: Arc::new(AtomicBool::new(false)),
            max_attempts,
            initial_backoff: Duration::ZERO,
        }
    }

    #[dbt_runtime::test]
    async fn unavailable_rpc_does_not_disable_service() {
        let client = lazy_test_client(1);

        assert!(matches!(
            client.after_request::<()>(Err(RunCacheServiceError::Rpc(tonic::Status::unavailable(
                "no healthy upstream"
            ),))),
            Err(RunCacheServiceError::Rpc(status))
                if status.code() == tonic::Code::Unavailable
                    && status.message() == "no healthy upstream"
        ));
        assert!(!client.is_disabled());
        assert!(client.before_request().is_ok());
    }

    #[test]
    fn transient_transport_rpc_is_identified() {
        let err = RunCacheServiceError::Rpc(tonic::Status::unknown("transport error"));
        assert!(err.is_transient_transport_rpc());

        let unavailable = RunCacheServiceError::Rpc(tonic::Status::unavailable("transport error"));
        assert!(!unavailable.is_transient_transport_rpc());

        let application_unknown =
            RunCacheServiceError::Rpc(tonic::Status::unknown("application error"));
        assert!(!application_unknown.is_transient_transport_rpc());
    }

    #[test]
    fn retryable_transport_covers_unavailable_and_transport_errors() {
        assert!(
            RunCacheServiceError::Rpc(tonic::Status::unavailable("connection reset by peer"))
                .is_retryable_transport()
        );
        assert!(
            RunCacheServiceError::Rpc(tonic::Status::unknown("transport error"))
                .is_retryable_transport()
        );
        assert!(
            !RunCacheServiceError::Rpc(tonic::Status::internal("boom")).is_retryable_transport()
        );
        assert!(!RunCacheServiceError::Disabled.is_retryable_transport());
    }

    #[dbt_runtime::test]
    async fn with_retry_retries_transient_transport_until_success() {
        let client = lazy_test_client(3);
        let calls = AtomicU32::new(0);

        let result = client
            .with_retry(|| async {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(RunCacheServiceError::Rpc(tonic::Status::unknown(
                        "transport error",
                    )))
                } else {
                    Ok(())
                }
            })
            .await;

        assert!(result.is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[dbt_runtime::test]
    async fn with_retry_exhausts_attempts_and_returns_last_error() {
        let client = lazy_test_client(3);
        let calls = AtomicU32::new(0);

        let result: Result<(), _> = client
            .with_retry(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(RunCacheServiceError::Rpc(tonic::Status::unavailable(
                    "down",
                )))
            })
            .await;

        assert!(matches!(
            result,
            Err(RunCacheServiceError::Rpc(status)) if status.code() == tonic::Code::Unavailable
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[dbt_runtime::test]
    async fn with_retry_does_not_retry_non_transient_errors() {
        let client = lazy_test_client(3);
        let calls = AtomicU32::new(0);

        let result: Result<(), _> = client
            .with_retry(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(RunCacheServiceError::Rpc(tonic::Status::permission_denied(
                    "nope",
                )))
            })
            .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[dbt_runtime::test]
    async fn with_retry_single_attempt_disables_retries() {
        let client = lazy_test_client(1);
        let calls = AtomicU32::new(0);

        let result: Result<(), _> = client
            .with_retry(|| async {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(RunCacheServiceError::Rpc(tonic::Status::unavailable(
                    "down",
                )))
            })
            .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn connect_clamps_max_attempts_to_grpc_cap() {
        // The config value is clamped to gRPC's hard cap when the client is built.
        assert_eq!(
            10_u32.clamp(1, GRPC_MAX_ATTEMPTS_CAP),
            GRPC_MAX_ATTEMPTS_CAP
        );
        assert_eq!(0_u32.clamp(1, GRPC_MAX_ATTEMPTS_CAP), 1);
    }

    #[dbt_runtime::test]
    async fn grpc_client_disables_after_first_service_disabling_error() {
        let client = lazy_test_client(1);

        assert!(client.before_request().is_ok());
        assert!(matches!(
            client.after_request::<()>(Err(RunCacheServiceError::OrgDisabled {
                org_id: "test-org".to_string(),
            })),
            Err(RunCacheServiceError::Disabled)
        ));
        assert!(client.is_disabled());
        assert!(matches!(
            client.before_request(),
            Err(RunCacheServiceError::Disabled)
        ));
    }

    #[test]
    fn request_metadata_matches_python_client_headers() {
        let metadata =
            RunCacheClientMetadata::new("session-1", "system-user-1", "test-os", "inv-1", "run-1");
        let request = metadata.attach(Request::new(())).unwrap();

        assert!(
            request
                .metadata()
                .get(REQUEST_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap()
                .len()
                > 20
        );
        assert_eq!(
            request
                .metadata()
                .get(SESSION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "session-1"
        );
        assert!(
            request
                .metadata()
                .get(SUBMITTED_AT_EPOCH_HEADER)
                .unwrap()
                .to_str()
                .unwrap()
                .parse::<i64>()
                .unwrap()
                > 0
        );
        assert_eq!(
            request
                .metadata()
                .get(SYSTEM_USER_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "system-user-1"
        );
        assert_eq!(
            request
                .metadata()
                .get(OS_NAME_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "test-os"
        );
        assert_eq!(
            request
                .metadata()
                .get(INVOCATION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "inv-1"
        );
        assert_eq!(
            request
                .metadata()
                .get(CLOUD_RUN_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "run-1"
        );
    }

    #[test]
    fn invocation_info_headers_are_omitted_when_empty() {
        // cloud_run_id is absent when not running on the dbt platform; an empty
        // invocation_id (fails-soft) is likewise not sent.
        let metadata =
            RunCacheClientMetadata::new("session-1", "system-user-1", "test-os", "inv-1", "");
        let request = metadata.attach(Request::new(())).unwrap();

        assert_eq!(
            request
                .metadata()
                .get(INVOCATION_ID_HEADER)
                .unwrap()
                .to_str()
                .unwrap(),
            "inv-1"
        );
        assert!(request.metadata().get(CLOUD_RUN_ID_HEADER).is_none());
    }
}
