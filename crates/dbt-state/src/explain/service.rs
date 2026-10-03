use std::sync::Arc;

use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

use crate::{
    proto::query_cache::{GetExplainMessagesRequest, GetExplainMessagesResponse},
    service_client::{RunCacheServiceClient, RunCacheServiceError},
};

use super::types::{StateExplainOptions, StateExplainRecord};

/// Maximum number of execution decision ids to request per explain RPC.
pub const EXPLAIN_MAX_BATCH_SIZE: usize = 50;

pub(super) fn should_fetch_service_explain(
    execution_decision_ids: &[String],
    options: &StateExplainOptions,
) -> bool {
    options.manage_state && !execution_decision_ids.is_empty()
}

pub(super) async fn service_explain_response_for_ids<C>(
    client: &C,
    execution_decision_ids: &[String],
) -> Result<GetExplainMessagesResponse, RunCacheServiceError>
where
    C: RunCacheServiceClient + ?Sized,
{
    // Calculate max_workers similar to Python: min(available_cpus, 4)
    // available_cpus = max(1, (cpu_count or 4) - 1)
    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let available_cpus = (cpu_count.saturating_sub(1)).max(1);
    let max_workers = available_cpus.min(4);

    let semaphore = Arc::new(Semaphore::new(max_workers));
    let mut futures = FuturesUnordered::new();

    for batch in execution_decision_ids.chunks(EXPLAIN_MAX_BATCH_SIZE) {
        let batch = batch.to_vec();
        let semaphore = semaphore.clone();

        futures.push(async move {
            // Acquire permit to limit concurrent requests
            let _permit: tokio::sync::OwnedSemaphorePermit = semaphore
                .acquire_owned()
                .await
                .expect("semaphore should not be closed");

            client
                .get_explain_messages(GetExplainMessagesRequest {
                    execution_decision_ids: batch,
                })
                .await
        });
    }

    let mut messages = Vec::new();
    while let Some(response) = futures.next().await {
        messages.extend(response?.messages);
    }

    Ok(GetExplainMessagesResponse { messages })
}

/// Extract service-side execution decision ids from state explain records.
pub fn execution_decision_ids(records: &[StateExplainRecord]) -> Vec<String> {
    records
        .iter()
        .filter_map(|record| record.execution_decision_id.as_deref())
        .filter(|id| !id.is_empty())
        .map(str::to_string)
        .collect()
}

/// Fetch service-side explain messages for records with execution decision ids.
pub async fn service_explain_response_with_client<C>(
    client: &C,
    records: &[StateExplainRecord],
) -> Result<Option<GetExplainMessagesResponse>, RunCacheServiceError>
where
    C: RunCacheServiceClient + ?Sized,
{
    let execution_decision_ids = execution_decision_ids(records);
    if execution_decision_ids.is_empty() {
        return Ok(None);
    }

    Ok(Some(
        service_explain_response_for_ids(client, &execution_decision_ids).await?,
    ))
}
