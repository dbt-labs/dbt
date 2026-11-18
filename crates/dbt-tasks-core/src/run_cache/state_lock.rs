//! The per-node lock taken in front of a dbt State decision when the
//! `lock_with_state` flag is on. Acquisition lives behind
//! [`crate::run_task_hooks::RunTaskHooks::acquire_state_lock`]; release is
//! shared here so the runner and the hook agree on how failures surface.

use dbt_common::ErrorCode;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_state::node_session::ExecutionGuard;

/// Release a lock taken by `acquire_state_lock`. A Redis error is reported as
/// `StateLockUnavailable` and never fails the node: the holder's heartbeat
/// has already stopped, so waiters recover through the stale path.
pub async fn release_state_lock(guard: ExecutionGuard, unique_id: &str) {
    if let Err(err) = guard.release().await {
        emit_warn_log_message(
            ErrorCode::StateLockUnavailable,
            format!(
                "dbt State lock release failed for node {unique_id}: {err}; waiters recover once the heartbeat goes stale"
            ),
        );
    }
}
