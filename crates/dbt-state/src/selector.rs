use crate::hash::{NodeHashError, node_state_hashes};
use crate::node_state_config::resolve_compare_unrendered_code;
use crate::proto::query_cache::{
    ClientSelectorEvent, ClientTelemetryEvent, DbtNodeData, SelectorCriteria, SelectorRequest,
    SubmitTelemetryBatchRequest, client_telemetry_event,
};
use crate::service_client::{RunCacheServiceError, SharedRunCacheServiceClient};
use crate::service_config::RunCacheServiceConfigError;
use crate::telemetry::SharedEventOrder;
use dbt_common::path::DbtPath;
use dbt_schemas::schemas::{Nodes, macros::DbtMacro};
use dbt_schemas::state::DbtRuntimeConfig;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Inputs shared by requests to the run-cache state selector service.
///
/// This struct carries an already-initialized service client from the run-cache
/// lifecycle, ensuring that the selector path uses the same enablement and
/// fail-open policy as the rest of the run-cache integration.
#[derive(Clone)]
pub struct RunCacheStateSelectorArgs {
    pub client: SharedRunCacheServiceClient,
    pub defer_to: String,
    pub project_id: Option<String>,
    pub macros: BTreeMap<String, DbtMacro>,
    pub project_root: DbtPath,
    /// The service-wide default for `compare_unrendered_code`; nodes may override it.
    pub compare_unrendered_code: bool,
    /// Source of the `var()` values folded into the configs hash when a node resolves
    /// `compare_unrendered_code=true`.
    pub runtime_config: Arc<DbtRuntimeConfig>,
    /// Shared telemetry event order counter for monotonic ordering across
    /// state selector (compilation phase) and task execution (run phase).
    pub shared_event_order: SharedEventOrder,
}

static SELECTOR_CRITERIA_BY_SELECTOR: LazyLock<HashMap<&str, SelectorCriteria>> =
    LazyLock::new(|| {
        HashMap::from([
            ("new", SelectorCriteria::New),
            ("old", SelectorCriteria::Old),
            ("modified", SelectorCriteria::Modified),
            ("unmodified", SelectorCriteria::Unmodified),
            ("modified.body", SelectorCriteria::Body),
            ("modified.configs", SelectorCriteria::Configs),
            (
                "modified.persisted_descriptions",
                SelectorCriteria::PersistedDescriptions,
            ),
            ("modified.relation", SelectorCriteria::Relation),
            ("modified.macros", SelectorCriteria::Macros),
            ("modified.contract", SelectorCriteria::Contract),
        ])
    });

pub fn is_service_supported_state_selector(selector: &str) -> bool {
    parse_selector_criteria(selector).is_ok()
}

fn parse_selector_criteria(selector: &str) -> Result<SelectorCriteria, SelectorServiceError> {
    SELECTOR_CRITERIA_BY_SELECTOR.get(selector).copied().ok_or({
        let valid_selectors: Vec<_> = SELECTOR_CRITERIA_BY_SELECTOR.keys().copied().collect();
        SelectorServiceError::InvalidSelector(selector.to_string(), valid_selectors.join(", "))
    })
}

/// Maximum number of nodes to include in a single request batch.
pub const SELECTOR_MAX_BATCH_SIZE: usize = 10_000;

pub async fn evaluate_state_selector(
    nodes: &Nodes,
    args: &RunCacheStateSelectorArgs,
    selector: &str,
) -> Result<BTreeSet<String>, SelectorServiceError> {
    let project_id = args
        .project_id
        .as_deref()
        .ok_or(RunCacheServiceConfigError::ProjectIdRequired)?;

    let total = Instant::now();

    let hash_calc_start = Instant::now();
    let macro_resolver = |macro_id: &str| args.macros.get(macro_id);
    let mut node_data_list = Vec::new();
    for (unique_id, node) in nodes.iter() {
        // Plugin nodes are not in the manifest, and `state:` never selects them.
        if nodes.plugins.contains_key(unique_id) {
            continue;
        }
        let runtime_vars = resolve_compare_unrendered_code(node, args.compare_unrendered_code)
            .then_some(args.runtime_config.as_ref());
        let hashes = node_state_hashes(node, &args.project_root, macro_resolver, runtime_vars)?;

        let database = node.database();
        let schema = node.schema();
        let alias = node.alias();
        let node_database_representation: Option<String> =
            if !database.is_empty() && !schema.is_empty() && !alias.is_empty() {
                Some(format!("{database}.{schema}.{alias}"))
            } else {
                None
            };
        node_data_list.push(DbtNodeData {
            node_unique_id: unique_id.clone(),
            node_hash: hashes.node_hash,
            node_body_hash: hashes.node_body_hash,
            node_configs_hash: hashes.node_configs_hash,
            node_persisted_descriptions_hash: hashes.node_persisted_descriptions_hash,
            node_macros_hash: hashes.node_macros_hash,
            node_contract_hash: hashes.node_contract_hash,
            node_database_representation,
        });
    }
    let hash_calc_duration = hash_calc_start.elapsed();

    let criteria = parse_selector_criteria(selector)?;

    let mut result = BTreeSet::new();
    for batch in node_data_list.chunks(SELECTOR_MAX_BATCH_SIZE) {
        let request = SelectorRequest {
            target: args.defer_to.clone(),
            project_id: project_id.to_string(),
            selector_criteria: criteria as i32,
            nodes: batch.to_vec(),
        };
        let response = args.client.get_state_selection(request).await?;
        result.extend(response.node_unique_ids);
    }

    let total_duration = total.elapsed();

    emit_state_selector_telemetry(
        args,
        criteria,
        result.len() as i64,
        total_duration,
        hash_calc_duration,
    );

    Ok(result)
}

fn emit_state_selector_telemetry(
    args: &RunCacheStateSelectorArgs,
    criteria: SelectorCriteria,
    num_nodes: i64,
    total_duration: Duration,
    hash_calc_duration: Duration,
) {
    let event_order = args.shared_event_order.next();
    let event = ClientTelemetryEvent {
        request: Some(client_telemetry_event::Request::ClientSelectorEvent(
            ClientSelectorEvent {
                request_id: Uuid::new_v4().simple().to_string(),
                project_id: args.project_id.clone().unwrap_or_default(),
                dbt_target: args.defer_to.clone(),
                selector_criteria: criteria as i32,
                num_nodes,
                processing_time_ms: total_duration.as_millis() as i64,
                hash_calculation_time_ms: hash_calc_duration.as_millis() as i64,
            },
        )),
        event_order: Some(event_order),
    };

    let request = SubmitTelemetryBatchRequest {
        events: vec![event],
    };
    let client = args.client.clone();
    tokio::spawn(async move {
        if let Err(e) = client.submit_telemetry_batch(request).await {
            tracing::debug!("Failed to submit state selector telemetry: {e}");
        }
    });
}

#[derive(Debug, thiserror::Error)]
pub enum SelectorServiceError {
    #[error("Invalid state selector {0}. Valid selectors are: {1}")]
    InvalidSelector(String, String),
    #[error("Hash calculation failed: {0}")]
    HashError(#[from] NodeHashError),
    #[error("Service error: {0}")]
    ServiceError(#[from] RunCacheServiceError),
    #[error("Config error: {0}")]
    ConfigError(#[from] RunCacheServiceConfigError),
}

#[cfg(test)]
mod tests {
    use super::{
        RunCacheStateSelectorArgs, SELECTOR_MAX_BATCH_SIZE, SelectorServiceError,
        evaluate_state_selector, is_service_supported_state_selector, parse_selector_criteria,
    };
    use crate::proto::query_cache::{SelectorRequest, SelectorResponse};
    use crate::service_client::{
        RunCacheServiceClient, RunCacheServiceError, shared_run_cache_service_client,
    };
    use dbt_common::path::DbtPath;
    use dbt_schemas::schemas::{DbtModel, Nodes};
    use std::collections::BTreeMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn supports_only_the_service_backed_state_selector_values() {
        for selector in [
            "modified",
            "new",
            "old",
            "unmodified",
            "modified.body",
            "modified.contract",
            "modified.configs",
            "modified.relation",
            "modified.persisted_descriptions",
            "modified.macros",
        ] {
            assert!(is_service_supported_state_selector(selector));
        }

        for selector in ["modified.foo", "new.foo", "Modified", "state:modified"] {
            assert!(!is_service_supported_state_selector(selector));
        }
    }

    #[test]
    fn rejects_unsupported_state_selectors() {
        assert!(matches!(
            parse_selector_criteria("modified.foo"),
            Err(SelectorServiceError::InvalidSelector(selector, _)) if selector == "modified.foo"
        ));
    }

    fn model_node(unique_id: &str) -> DbtModel {
        let mut m = DbtModel::default();
        m.__common_attr__.unique_id = unique_id.to_string();
        m.__common_attr__.fqn = vec!["project".to_string(), unique_id.to_string()];
        m
    }

    fn nodes_with_count(count: usize) -> Nodes {
        let models: BTreeMap<String, Arc<DbtModel>> = (0..count)
            .map(|idx| {
                let unique_id = format!("model.pkg.model_{idx}");
                (unique_id.clone(), Arc::new(model_node(&unique_id)))
            })
            .collect();
        Nodes {
            models,
            ..Default::default()
        }
    }

    #[derive(Default)]
    struct MockSelectorState {
        request_batch_sizes: Vec<usize>,
        configs_hashes: Vec<Option<String>>,
    }

    struct MockSelectorClient {
        responses: Mutex<Vec<SelectorResponse>>,
        state: Arc<Mutex<MockSelectorState>>,
    }

    impl MockSelectorClient {
        fn new(responses: Vec<SelectorResponse>) -> (Self, Arc<Mutex<MockSelectorState>>) {
            let state = Arc::new(Mutex::new(MockSelectorState::default()));
            (
                Self {
                    responses: Mutex::new(responses),
                    state: Arc::clone(&state),
                },
                state,
            )
        }
    }

    #[async_trait::async_trait]
    impl RunCacheServiceClient for MockSelectorClient {
        async fn validate_client_version(
            &self,
        ) -> Result<crate::service_client::ClientVersionStatus, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_enriched_sql(
            &self,
            _request: crate::proto::query_cache::SubmitEnrichedSqlRequest,
        ) -> Result<crate::proto::query_cache::SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn submit_values(
            &self,
            _request: crate::proto::query_cache::SubmitValuesRequest,
        ) -> Result<crate::proto::query_cache::SubmitSqlResponse, RunCacheServiceError> {
            Err(RunCacheServiceError::Disabled)
        }

        async fn confirm_execution(
            &self,
            _request: crate::proto::query_cache::ConfirmExecutionRequest,
        ) -> Result<crate::proto::query_cache::ConfirmExecutionResponse, RunCacheServiceError>
        {
            Err(RunCacheServiceError::Disabled)
        }

        async fn get_state_selection(
            &self,
            request: SelectorRequest,
        ) -> Result<SelectorResponse, RunCacheServiceError> {
            self.state
                .lock()
                .unwrap()
                .request_batch_sizes
                .push(request.nodes.len());
            self.state
                .lock()
                .unwrap()
                .configs_hashes
                .extend(request.nodes.iter().map(|n| n.node_configs_hash.clone()));

            let response = self
                .responses
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(SelectorResponse {
                    node_unique_ids: Vec::new(),
                });
            Ok(response)
        }
    }

    fn selector_args(client: MockSelectorClient) -> RunCacheStateSelectorArgs {
        RunCacheStateSelectorArgs {
            client: shared_run_cache_service_client(client),
            defer_to: "prod".to_string(),
            project_id: Some("project-123".to_string()),
            macros: BTreeMap::new(),
            project_root: DbtPath::from("test-project"),
            compare_unrendered_code: false,
            runtime_config: Arc::new(dbt_schemas::state::DbtRuntimeConfig::default()),
            shared_event_order: crate::telemetry::SharedEventOrder::new(),
        }
    }

    #[tokio::test]
    async fn evaluate_state_selector_folds_vars_into_configs_hash_when_comparing_unrendered_code() {
        use dbt_schemas::schemas::DbtPluginNode;
        use dbt_schemas::state::DbtVars;

        let mut model = model_node("model.pkg.model_0");
        model.__common_attr__.package_name = "pkg".to_string();
        model.__common_attr__.raw_code = Some("select {{ var('dry_run', false) }}".to_string());
        let mut plugin = DbtPluginNode::default();
        plugin.__common_attr__.unique_id = "widget.pkg.widget_0".to_string();
        plugin.__plugin_attr__.plugin_type = "widget".to_string();
        let nodes = Nodes {
            models: BTreeMap::from([("model.pkg.model_0".to_string(), Arc::new(model))]),
            plugins: BTreeMap::from([("widget.pkg.widget_0".to_string(), Arc::new(plugin))]),
            ..Default::default()
        };

        let mut pkg_cfg = dbt_schemas::state::DbtRuntimeConfig::default();
        pkg_cfg.inner.vars.insert(
            "dry_run".to_string(),
            DbtVars::Value(dbt_yaml::Value::bool(true)),
        );
        let mut cfg = dbt_schemas::state::DbtRuntimeConfig::default();
        cfg.dependencies
            .insert("pkg".to_string(), Arc::new(pkg_cfg));

        let mut sent = Vec::new();
        for compare in [false, true] {
            let (client, state) = MockSelectorClient::new(vec![]);
            let mut args = selector_args(client);
            args.compare_unrendered_code = compare;
            args.runtime_config = Arc::new(cfg.clone());
            evaluate_state_selector(&nodes, &args, "modified")
                .await
                .unwrap();
            sent.push(state.lock().unwrap().configs_hashes.clone());
        }

        // The default model has no unrendered config, so only the folded vars can produce a hash.
        assert_eq!(sent[0], vec![None]);
        assert_eq!(sent[1].len(), 1);
        assert!(sent[1][0].is_some());
    }

    #[tokio::test]
    async fn evaluate_state_selector_batches_large_node_sets() {
        let node_count = SELECTOR_MAX_BATCH_SIZE + 1;
        let nodes = nodes_with_count(node_count);

        let (client, state) = MockSelectorClient::new(vec![
            SelectorResponse {
                node_unique_ids: vec![format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE)],
            },
            SelectorResponse {
                node_unique_ids: vec!["model.pkg.model_0".to_string()],
            },
        ]);

        let args = selector_args(client);
        let result = evaluate_state_selector(&nodes, &args, "modified")
            .await
            .unwrap();

        assert_eq!(result.len(), 2);
        assert!(result.contains("model.pkg.model_0"));
        assert!(result.contains(&format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE)));

        let batch_sizes = &state.lock().unwrap().request_batch_sizes;
        assert_eq!(batch_sizes.len(), 2);
        assert_eq!(batch_sizes[0], SELECTOR_MAX_BATCH_SIZE);
        assert_eq!(batch_sizes[1], 1);
    }

    #[tokio::test]
    async fn evaluate_state_selector_batches_request_sizes_and_merges_results() {
        let node_count = 2 * SELECTOR_MAX_BATCH_SIZE;
        let nodes = nodes_with_count(node_count);

        let (client, state) = MockSelectorClient::new(vec![
            SelectorResponse {
                node_unique_ids: vec![
                    format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE),
                    format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE + 1),
                ],
            },
            SelectorResponse {
                node_unique_ids: vec![
                    "model.pkg.model_0".to_string(),
                    "model.pkg.model_1".to_string(),
                ],
            },
        ]);

        let args = selector_args(client);
        let result = evaluate_state_selector(&nodes, &args, "new").await.unwrap();

        assert_eq!(result.len(), 4);
        assert!(result.contains("model.pkg.model_0"));
        assert!(result.contains("model.pkg.model_1"));
        assert!(result.contains(&format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE)));
        assert!(result.contains(&format!("model.pkg.model_{}", SELECTOR_MAX_BATCH_SIZE + 1)));

        let batch_sizes = &state.lock().unwrap().request_batch_sizes;
        assert_eq!(batch_sizes.len(), 2);
        assert_eq!(batch_sizes[0], SELECTOR_MAX_BATCH_SIZE);
        assert_eq!(batch_sizes[1], SELECTOR_MAX_BATCH_SIZE);
    }

    #[tokio::test]
    async fn evaluate_state_selector_single_batch_for_small_node_set() {
        let node_count = 100;
        let nodes = nodes_with_count(node_count);

        let (client, state) = MockSelectorClient::new(vec![SelectorResponse {
            node_unique_ids: vec!["model.pkg.model_50".to_string()],
        }]);

        let args = selector_args(client);
        let result = evaluate_state_selector(&nodes, &args, "modified")
            .await
            .unwrap();

        assert_eq!(result.len(), 1);
        assert!(result.contains("model.pkg.model_50"));

        let batch_sizes = &state.lock().unwrap().request_batch_sizes;
        assert_eq!(batch_sizes.len(), 1);
        assert_eq!(batch_sizes[0], 100);
    }

    #[tokio::test]
    async fn evaluate_state_selector_empty_nodes_makes_no_requests() {
        let nodes = Nodes::default();

        let (client, state) = MockSelectorClient::new(vec![]);
        let args = selector_args(client);
        let result = evaluate_state_selector(&nodes, &args, "modified")
            .await
            .unwrap();

        assert!(result.is_empty());

        let batch_sizes = &state.lock().unwrap().request_batch_sizes;
        assert!(batch_sizes.is_empty());
    }
}
