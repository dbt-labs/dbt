use crate::{
    proto::query_cache::{GetExplainMessagesRequest, GetExplainMessagesResponse},
    service_client::{RunCacheServiceClient, RunCacheServiceError},
};

use super::types::{StateExplainOptions, StateExplainRecord};

/// Maximum number of execution decision ids to request per explain RPC.
pub const EXPLAIN_MAX_BATCH_SIZE: usize = 1000;

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
    let mut messages = Vec::new();
    for batch in execution_decision_ids.chunks(EXPLAIN_MAX_BATCH_SIZE) {
        let response = client
            .get_explain_messages(GetExplainMessagesRequest {
                execution_decision_ids: batch.to_vec(),
            })
            .await?;
        messages.extend(response.messages);
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
