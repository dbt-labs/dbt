use dbt_adapter::time_machine::is_replaying;
use dbt_adapter_core::AdapterType;
use dbt_cloud_config::ResolvedCloudConfig;
use dbt_common::io_args::FsCommand;
use dbt_common::tracing::dbt_emit::{
    emit_debug_log_message, emit_info_log_message, emit_warn_log_message,
};
use dbt_common::tracing::dbt_metrics::{FusionMetricKey, RunCacheServiceMetricKey};
use dbt_common::tracing::metrics::increment_metric;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::state::DbtProfile;
use dbt_state::metadata_cache::RunCacheMetadataCache;
use dbt_state::service_client::{
    ClientVersionStatus, GrpcRunCacheServiceClient, RunCacheClientMetadata, RunCacheServiceClient,
    SharedRunCacheServiceClient, format_error_chain, shared_run_cache_service_client,
    validate_client_version_fail_open,
};
use dbt_state::service_config::RunCacheServiceConfig;
use std::sync::Arc;
use tokio::sync::OnceCell;

use crate::RunTasksArgs;

#[derive(Clone)]
pub(crate) struct RunCacheServiceLifecycle {
    pub(crate) requested: bool,
    pub(crate) config: Option<RunCacheServiceConfig>,
    pub(crate) client: Option<SharedRunCacheServiceClient>,
}

#[derive(Clone)]
pub struct RunCacheLifecycle {
    pub(crate) service: RunCacheServiceLifecycle,
    pub(crate) metadata: Arc<RunCacheMetadataCache>,
}

static SINGLETON: OnceCell<Arc<RunCacheLifecycle>> = OnceCell::const_new();

impl RunCacheLifecycle {
    pub async fn initialize(
        arg: &RunTasksArgs,
        execute: Execute,
        adapter_type: AdapterType,
        cloud_config: Option<&ResolvedCloudConfig>,
        active_profile: &DbtProfile,
    ) -> FsResult<Self> {
        let service =
            initialize_run_cache_service(arg, execute, adapter_type, cloud_config, active_profile)
                .await?;
        let metadata_ttl_seconds = service
            .config
            .as_ref()
            .map(|config| config.metadata_cache_ttl_seconds)
            .unwrap_or_default();

        Ok(Self {
            service,
            metadata: Arc::new(RunCacheMetadataCache::with_ttl_seconds(
                metadata_ttl_seconds,
            )),
        })
    }

    pub async fn get_or_initialize(
        arg: &RunTasksArgs,
        execute: Execute,
        adapter_type: AdapterType,
        cloud_config: Option<&ResolvedCloudConfig>,
        active_profile: &DbtProfile,
    ) -> FsResult<Arc<RunCacheLifecycle>> {
        let run_cache_lifecycle = SINGLETON
            .get_or_try_init(async || {
                Self::initialize(arg, execute, adapter_type, cloud_config, active_profile)
                    .await
                    .map(Arc::new)
            })
            .await?
            .clone();

        Ok(run_cache_lifecycle)
    }

    pub fn is_requested(&self) -> bool {
        self.service.requested
    }

    pub fn service_client(&self) -> Option<SharedRunCacheServiceClient> {
        self.service.client.clone()
    }

    pub fn defer_to_target(&self, active_profile: &DbtProfile) -> Option<String> {
        self.service
            .config
            .as_ref()
            .map(|c| c.defer_to_target(active_profile))
    }
}

async fn initialize_run_cache_service(
    arg: &RunTasksArgs,
    execute: Execute,
    adapter_type: AdapterType,
    cloud_config: Option<&ResolvedCloudConfig>,
    active_profile: &DbtProfile,
) -> FsResult<RunCacheServiceLifecycle> {
    if !should_initialize_run_cache_service(arg, execute, adapter_type) {
        increment_metric(
            FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::Disabled),
            1,
        );
        if execute == Execute::Remote && RunCacheServiceConfig::is_explicitly_disabled_from_env() {
            emit_debug_log_message(
                "dbt State service disabled by configuration; executing normally",
            );
        }
        return Ok(RunCacheServiceLifecycle {
            requested: false,
            config: None,
            client: None,
        });
    }

    let config = match RunCacheServiceConfig::from_env_and_cloud_config(cloud_config) {
        Ok(config) => config,
        Err(err) => {
            increment_metric(
                FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::Disabled),
                1,
            );
            increment_metric(
                FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ClientInitFailure),
                1,
            );
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State service config failed: {}; executing normally",
                    format_error_chain(&err)
                ),
            );
            return Ok(RunCacheServiceLifecycle {
                requested: true,
                config: None,
                client: None,
            });
        }
    };

    if !config.enabled {
        increment_metric(
            FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::Disabled),
            1,
        );
        // User-facing operational signal: the service was explicitly requested but
        // disabled by config, so visibility into why no caching is happening should
        // be reachable with `--log-level debug` rather than trace-only.
        emit_debug_log_message("dbt State service disabled by configuration; executing normally");
        return Ok(RunCacheServiceLifecycle {
            requested: false,
            config: None,
            client: None,
        });
    }

    increment_metric(
        FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::Enabled),
        1,
    );

    // Thread dbt's per-invocation UUID (and the dbt platform run ID when present)
    // through to the service as request metadata so telemetry can be correlated
    // back to a single invocation end to end.
    let metadata = RunCacheClientMetadata {
        dbt_invocation_id: arg.io.invocation_id.to_string(),
        ..RunCacheClientMetadata::default()
    };
    let client =
        match GrpcRunCacheServiceClient::connect_with_metadata(config.clone(), metadata).await {
            Ok(client) => {
                increment_metric(
                    FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ClientInitSuccess),
                    1,
                );
                client
            }
            Err(err) => {
                increment_metric(
                    FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ClientInitFailure),
                    1,
                );
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    format!(
                        "dbt State service client initialization failed: {}; executing normally",
                        format_error_chain(&err)
                    ),
                );
                return Ok(disconnected_run_cache_service(config));
            }
        };

    let validation_status = validate_client_version_for_initialization(&client).await?;
    match validation_status {
        ClientVersionStatus::Supported => {
            increment_metric(
                FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ValidationSupported),
                1,
            );
            emit_info_log_message(format!(
                "dbt State is enabled (endpoint {}, defer_to {})",
                config.endpoint_uri(),
                config.defer_to_target(active_profile)
            ));
            let shared_client = shared_run_cache_service_client(client);
            Ok(RunCacheServiceLifecycle {
                requested: true,
                config: Some(config),
                client: Some(shared_client),
            })
        }
        ClientVersionStatus::Unsupported => {
            increment_metric(
                FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ValidationUnsupported),
                1,
            );
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                "dbt State service does not support this client version; executing normally",
            );
            Ok(RunCacheServiceLifecycle {
                requested: true,
                config: Some(config),
                client: None,
            })
        }
        ClientVersionStatus::Skipped => {
            increment_metric(
                FusionMetricKey::RunCacheService(RunCacheServiceMetricKey::ValidationSkipped),
                1,
            );
            let requested = validation_skipped_service_requested(&client);
            if requested {
                emit_warn_log_message(
                    ErrorCode::StateServiceWarn,
                    "dbt State service validation was skipped; executing normally",
                );
            }
            Ok(RunCacheServiceLifecycle {
                requested,
                config: Some(config),
                client: None,
            })
        }
    }
}

/// dbt State and the local cache path are mutually exclusive, so a client that failed
/// to connect must leave the service unrequested for the local path to stay active.
fn disconnected_run_cache_service(config: RunCacheServiceConfig) -> RunCacheServiceLifecycle {
    RunCacheServiceLifecycle {
        requested: false,
        config: Some(config),
        client: None,
    }
}

fn validation_skipped_service_requested<C>(client: &C) -> bool
where
    C: RunCacheServiceClient + ?Sized,
{
    !client.is_disabled()
}

async fn validate_client_version_for_initialization<C>(client: &C) -> FsResult<ClientVersionStatus>
where
    C: RunCacheServiceClient + ?Sized,
{
    validate_client_version_fail_open(client)
        .await
        .map_err(|err| {
            fs_err!(
                ErrorCode::AuthFailed,
                "dbt State client validation failed: {}",
                format_error_chain(&err)
            )
        })
}

fn should_initialize_run_cache_service(
    arg: &RunTasksArgs,
    execute: Execute,
    adapter_type: AdapterType,
) -> bool {
    // Never contact the live dbt State service while replaying a time-machine
    // recording: replay must be fully reproducible from recorded events, with
    // no network/auth dependency on the real service.
    !is_replaying()
        && execute == Execute::Remote
        && adapter_supports_dbt_state(adapter_type)
        && arg.run_cache_service
}

/// Returns true when the adapter is supported by the dbt State service.
pub fn adapter_supports_dbt_state(adapter_type: AdapterType) -> bool {
    matches!(
        adapter_type,
        AdapterType::Snowflake
            | AdapterType::Databricks
            | AdapterType::Spark
            | AdapterType::Redshift
            | AdapterType::Bigquery
    )
}

pub fn run_cache_auto_defer_command(command: FsCommand) -> bool {
    matches!(
        command,
        FsCommand::Compile
            | FsCommand::Run
            | FsCommand::Build
            | FsCommand::Test
            | FsCommand::Seed
            | FsCommand::Snapshot
    )
}

#[cfg(test)]
mod tests {
    use super::{
        RunCacheLifecycle, RunCacheServiceLifecycle, RunTasksArgs, adapter_supports_dbt_state,
        disconnected_run_cache_service, should_initialize_run_cache_service,
        validate_client_version_for_initialization,
    };
    use async_trait::async_trait;
    use dbt_adapter_core::AdapterType;
    use dbt_common::ErrorCode;
    use dbt_schemas::IndexMap;
    use dbt_schemas::schemas::profiles::{DbConfig, DuckDbConfig, Execute};
    use dbt_schemas::state::{DbtProfile, ProfileAdapter};
    use dbt_state::metadata_cache::RunCacheMetadataCache;
    use dbt_state::proto::query_cache::{
        ConfirmExecutionRequest, ConfirmExecutionResponse, SubmitSqlResponse, SubmitValuesRequest,
    };
    use dbt_state::service_client::shared_run_cache_service_client;
    use dbt_state::service_client::{
        ClientVersionStatus, RunCacheServiceClient, RunCacheServiceError,
    };
    use dbt_state::service_config::RunCacheServiceConfig;
    use std::sync::Arc;

    fn args() -> RunTasksArgs {
        RunTasksArgs::default()
    }

    fn test_profile(target: &str, defer_to_target: Option<&str>) -> DbtProfile {
        let db_config = DbConfig::DuckDB(Box::<DuckDbConfig>::default());
        let default_adapter = db_config.adapter_type();
        DbtProfile {
            profile: "default".to_string(),
            target: target.to_string(),
            defer_to_target: defer_to_target.map(str::to_string),
            allow_clones: true,
            adapters: IndexMap::from([(default_adapter, ProfileAdapter::single(db_config))]),
            default_adapter,
            schema: "dbt_test".to_string(),
            database: "db".to_string(),
            relative_profile_path: std::path::PathBuf::new(),
            threads: None,
        }
    }

    fn requested_args() -> RunTasksArgs {
        let mut args = args();
        args.run_cache_service = true;
        args
    }

    #[test]
    fn client_init_failure_leaves_service_unrequested() {
        let lifecycle = disconnected_run_cache_service(RunCacheServiceConfig::disabled());

        assert!(!lifecycle.requested);
        assert!(lifecycle.config.is_some());
        assert!(lifecycle.client.is_none());
    }

    #[test]
    fn lifecycle_does_not_request_service_without_explicit_request() {
        assert!(!should_initialize_run_cache_service(
            &args(),
            Execute::Remote,
            AdapterType::Snowflake,
        ));
    }

    #[test]
    fn lifecycle_requests_service_from_cli_flag() {
        assert!(should_initialize_run_cache_service(
            &requested_args(),
            Execute::Remote,
            AdapterType::Snowflake,
        ));
    }

    #[test]
    fn lifecycle_requires_remote_compute() {
        assert!(!should_initialize_run_cache_service(
            &requested_args(),
            Execute::Sidecar,
            AdapterType::Snowflake,
        ));
    }

    #[test]
    fn lifecycle_requires_supported_adapter() {
        assert!(!should_initialize_run_cache_service(
            &requested_args(),
            Execute::Remote,
            AdapterType::DuckDB,
        ));
    }

    // Serializes tests that touch the process-global time-machine state
    // (`GLOBAL_SESSION`/`GLOBAL_REPLAYER` in `dbt_adapter::time_machine`).
    static TIME_MACHINE_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    #[dbt_runtime::test]
    async fn should_initialize_run_cache_service_is_false_while_replaying() {
        use dbt_adapter::time_machine::{
            EventReplayer, get_or_init_recording, get_or_init_replayer, reset_time_machine_globals,
        };
        use dbt_common::cancellation::CancellationToken;

        let _guard = TIME_MACHINE_TEST_LOCK.lock().await;
        reset_time_machine_globals().await.unwrap();

        // Even with every other condition satisfied (service requested,
        // remote compute, supported adapter), replay must always disable
        // the live dbt State service.
        assert!(should_initialize_run_cache_service(
            &requested_args(),
            Execute::Remote,
            AdapterType::Snowflake,
        ));

        let dir = tempfile::tempdir().unwrap();
        let handle = get_or_init_recording(
            dir.path(),
            "snowflake",
            "test-invocation",
            None,
            CancellationToken::never_cancels(),
        );
        handle.shutdown().await.unwrap();
        reset_time_machine_globals().await.unwrap();
        get_or_init_replayer(|| Ok(Arc::new(EventReplayer::load(dir.path())?))).unwrap();

        assert!(!should_initialize_run_cache_service(
            &requested_args(),
            Execute::Remote,
            AdapterType::Snowflake,
        ));

        reset_time_machine_globals().await.unwrap();
    }

    #[test]
    fn dbt_state_supported_adapters_are_explicit() {
        assert!(adapter_supports_dbt_state(AdapterType::Snowflake));
        assert!(adapter_supports_dbt_state(AdapterType::Databricks));
        assert!(adapter_supports_dbt_state(AdapterType::Spark));
        assert!(adapter_supports_dbt_state(AdapterType::Redshift));
        assert!(adapter_supports_dbt_state(AdapterType::Bigquery));

        assert!(!adapter_supports_dbt_state(AdapterType::DuckDB));
        assert!(!adapter_supports_dbt_state(AdapterType::Postgres));
        assert!(!adapter_supports_dbt_state(AdapterType::ClickHouse));
        assert!(!adapter_supports_dbt_state(AdapterType::Fabric));
        assert!(!adapter_supports_dbt_state(AdapterType::Salesforce));
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
                RunCacheServiceError::Disabled => RunCacheServiceError::Disabled,
                _ => unreachable!("validation tests only use Auth and Disabled"),
            })
        }

        async fn submit_enriched_sql(
            &self,
            _request: dbt_state::proto::query_cache::SubmitEnrichedSqlRequest,
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
    async fn initialization_validation_auth_error_fails_closed() {
        let client = ValidationClient(RunCacheServiceError::Auth("bad credentials".to_string()));

        let err = validate_client_version_for_initialization(&client)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("bad credentials"));
        assert_eq!(err.code, ErrorCode::AuthFailed);
    }

    #[dbt_runtime::test]
    async fn initialization_validation_non_auth_error_fails_open_to_skipped() {
        let client = ValidationClient(RunCacheServiceError::Disabled);

        assert_eq!(
            validate_client_version_for_initialization(&client)
                .await
                .unwrap(),
            ClientVersionStatus::Skipped
        );
    }

    struct DisabledValidationClient;

    #[async_trait]
    impl RunCacheServiceClient for DisabledValidationClient {
        fn is_disabled(&self) -> bool {
            true
        }

        async fn validate_client_version(
            &self,
        ) -> Result<ClientVersionStatus, RunCacheServiceError> {
            unreachable!("service-requested helper should not validate")
        }

        async fn submit_enriched_sql(
            &self,
            _request: dbt_state::proto::query_cache::SubmitEnrichedSqlRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            unreachable!("service-requested helper should not submit SQL")
        }

        async fn submit_values(
            &self,
            _request: SubmitValuesRequest,
        ) -> Result<SubmitSqlResponse, RunCacheServiceError> {
            unreachable!("service-requested helper should not submit values")
        }

        async fn confirm_execution(
            &self,
            _request: ConfirmExecutionRequest,
        ) -> Result<ConfirmExecutionResponse, RunCacheServiceError> {
            unreachable!("service-requested helper should not confirm execution")
        }
    }

    #[test]
    fn skipped_validation_from_disabled_client_is_no_longer_requested() {
        assert!(!super::validation_skipped_service_requested(
            &DisabledValidationClient
        ));
    }

    #[test]
    fn skipped_validation_from_enabled_client_remains_requested() {
        let client = ValidationClient(RunCacheServiceError::Disabled);

        assert!(super::validation_skipped_service_requested(&client));
    }

    #[test]
    fn test_service_client_returns_none_without_a_client() {
        let lifecycle = RunCacheLifecycle {
            service: RunCacheServiceLifecycle {
                requested: true,
                config: Some(RunCacheServiceConfig::disabled()),
                client: None,
            },
            metadata: Arc::new(RunCacheMetadataCache::with_ttl_seconds(0)),
        };

        assert!(lifecycle.service_client().is_none());
    }

    #[test]
    fn test_service_client_returns_client() {
        let client = shared_run_cache_service_client(DisabledValidationClient);
        let lifecycle = RunCacheLifecycle {
            service: RunCacheServiceLifecycle {
                requested: true,
                config: Some(RunCacheServiceConfig::disabled()),
                client: Some(client),
            },
            metadata: Arc::new(RunCacheMetadataCache::with_ttl_seconds(0)),
        };

        assert!(lifecycle.service_client().is_some());
    }
    #[dbt_runtime::test]
    async fn get_or_initialize_reuses_lifecycle() {
        // initialize a run cache lifecycle when one currently does not exist
        let first = RunCacheLifecycle::get_or_initialize(
            &args(),
            Execute::Sidecar,
            AdapterType::Snowflake,
            None,
            &test_profile("dev", None),
        )
        .await
        .unwrap();

        // second call to get_or_initialize
        let second = RunCacheLifecycle::get_or_initialize(
            &args(),
            Execute::Sidecar,
            AdapterType::Snowflake,
            None,
            &test_profile("dev", None),
        )
        .await
        .unwrap();

        assert!(
            Arc::ptr_eq(&first, &second),
            "get_or_initialize must return the cached singleton, not re-initialize"
        );
    }

    #[dbt_runtime::test]
    async fn get_or_initialize_concurrent_callers() {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                tokio::spawn(async move {
                    let arg = args();
                    RunCacheLifecycle::get_or_initialize(
                        &arg,
                        Execute::Sidecar,
                        AdapterType::Snowflake,
                        None,
                        &test_profile("dev", None),
                    )
                    .await
                    .unwrap()
                })
            })
            .collect();

        let mut results = Vec::with_capacity(handles.len());
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        let first = &results[0];
        assert!(
            results.iter().all(|r| Arc::ptr_eq(r, first)),
            "concurrent callers must observe same singleton instance, not separate initializations"
        )
    }
}
