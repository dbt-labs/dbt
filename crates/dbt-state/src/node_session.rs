//! Per-node façade over [`TaskCache`] that speaks the domain verb:
//! *"take the slot for this node at this hash, or tell me what's already there"*.
//!
//! Callers interact with a [`NodeCacheSession`] instead of driving
//! `try_start_task` / `try_cleanup_task` / `start_heartbeat` directly. The
//! session owns the state machine, CAS tokens, heartbeat lifecycle, and
//! exponential backoff. Business-level reuse policy (hash equality, freshness
//! windows, upstream-change rules) lives with the caller.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use chrono::{DateTime, Duration, Utc};

use crate::task_cache::{
    BACKOFF_FACTOR, INITIAL_DELAY, MAX_DELAY, TaskCache, TaskInfo, TaskState, TaskValue,
};

/// Signalled when `acquire` has to wait for, or recover from, another worker.
///
/// Surfaced through the `on_contention` callback so the caller can emit
/// UI/telemetry without the cache crate knowing about either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Contention {
    /// Another worker holds the slot; we're sleeping with backoff.
    Waiting,
    /// A prior owner's heartbeat stopped; we're cleaning up their slot.
    Aborted,
}

/// Outcome of [`NodeCacheSession::acquire`].
pub enum Acquired {
    /// We own the slot. Heartbeat is running. Call
    /// [`ExecutionGuard::finalize`] after executing (it records the hash from
    /// acquire), or drop the guard on error — either way the heartbeat is released.
    Execute(ExecutionGuard),
    /// A completed entry already exists. Inspect `stored_hash` / `stop_time`
    /// to decide whether to reuse it; if not, call [`Evictor::evict`] and
    /// call `acquire` again.
    Completed {
        //TODO: Do we need to return this here as the caller already knows it
        stored_hash: String,
        stop_time: DateTime<Utc>,
        evict: Evictor,
    },
}

/// Holds the distributed lock acquired during [`NodeCacheSession::acquire`].
///
/// On the happy path, call [`ExecutionGuard::finalize`] to record the stop
/// value and release the lock. On error or panic, `Drop` stops the heartbeat.
pub struct ExecutionGuard {
    task_cache: Arc<dyn TaskCache>,
    unique_id: String,
    owner_id: String,
    /// Hash passed to [`NodeCacheSession::acquire`]; written on [`ExecutionGuard::finalize`].
    hash: String,
    stop_signal: Arc<AtomicBool>,
}

impl ExecutionGuard {
    pub fn unique_id(&self) -> &str {
        &self.unique_id
    }

    pub fn node_hash(&self) -> &str {
        &self.hash
    }

    /// Record the stop value (hash from acquire + upstream snapshot) and release the lock.
    pub async fn finalize(self, upstreams: HashMap<String, String>) -> Result<(), String> {
        let stop_value = TaskValue::new(self.owner_id.clone(), Utc::now(), Some(self.hash.clone()));
        let info = TaskInfo { upstreams };
        self.task_cache
            .stop_task(&self.unique_id, &stop_value, &info)
            .await?;
        // Heartbeat is also stopped by Drop, but explicit stop avoids a brief delay.
        self.stop_signal
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(())
    }
}

impl Drop for ExecutionGuard {
    fn drop(&mut self) {
        self.stop_signal
            .store(true, std::sync::atomic::Ordering::Release);
    }
}

/// CAS-scoped handle that lets the caller evict a completed entry they
/// already observed via [`Acquired::Completed`].
///
/// The captured start-value snapshot ensures we only delete the entry we
/// saw — not one a racing worker wrote in the meantime.
pub struct Evictor {
    task_cache: Arc<dyn TaskCache>,
    unique_id: String,
    old_start_value: Option<TaskValue>,
}

impl Evictor {
    pub async fn evict(self) -> Result<(), String> {
        self.task_cache
            .try_cleanup_task(&self.unique_id, &self.old_start_value)
            .await
    }
}

/// High-level façade over [`TaskCache`] bound to a single
/// `(unique_id, owner_id)` pair.
#[derive(Clone)]
pub struct NodeCacheSession {
    task_cache: Arc<dyn TaskCache>,
    unique_id: String,
    owner_id: String,
}

impl NodeCacheSession {
    pub fn new(task_cache: Arc<dyn TaskCache>, unique_id: String, owner_id: String) -> Self {
        Self {
            task_cache,
            unique_id,
            owner_id,
        }
    }

    pub fn unique_id(&self) -> &str {
        &self.unique_id
    }

    /// Drive the distributed-lock state machine until we either own the slot
    /// (returning [`Acquired::Execute`]) or observe a completed entry
    /// (returning [`Acquired::Completed`] + an [`Evictor`]).
    ///
    /// The caller's `on_contention` is invoked each time we have to wait for
    /// another worker ([`Contention::Waiting`]) or clean up a stale slot
    /// ([`Contention::Aborted`]).
    pub async fn acquire<F>(
        &self,
        current_hash: &str,
        on_contention: &F,
    ) -> Result<Acquired, String>
    where
        F: Fn(Contention) + ?Sized,
    {
        let mut delay = INITIAL_DELAY;

        loop {
            let (task_state, (old_start_value, _heartbeat_value, stop_value)) =
                self.task_cache.get_task_state(&self.unique_id).await?;

            match task_state {
                TaskState::NotStarted => {
                    let new_start_value = TaskValue::new(
                        self.owner_id.clone(),
                        Utc::now(),
                        Some(current_hash.to_string()),
                    );
                    let ok = self
                        .task_cache
                        .try_start_task(&self.unique_id, &new_start_value, &old_start_value)
                        .await?;
                    if !ok {
                        delay = INITIAL_DELAY;
                        continue;
                    }

                    let stop_signal = Arc::new(AtomicBool::new(false));
                    let heartbeat_value = TaskValue::new(
                        self.owner_id.clone(),
                        Utc::now(),
                        Some(current_hash.to_string()),
                    );
                    Arc::clone(&self.task_cache)
                        .start_heartbeat(&self.unique_id, &heartbeat_value, stop_signal.clone())
                        .await?;

                    return Ok(Acquired::Execute(ExecutionGuard {
                        task_cache: Arc::clone(&self.task_cache),
                        unique_id: self.unique_id.clone(),
                        owner_id: self.owner_id.clone(),
                        hash: current_hash.to_string(),
                        stop_signal,
                    }));
                }

                TaskState::InProgress => {
                    on_contention(Contention::Waiting);
                    tokio::time::sleep(std::time::Duration::from_micros(
                        delay.num_microseconds().unwrap_or(1_000) as u64,
                    ))
                    .await;
                    let next = (delay.num_microseconds().unwrap() * BACKOFF_FACTOR)
                        .min(MAX_DELAY.num_microseconds().unwrap());
                    delay = Duration::microseconds(next);
                    continue;
                }

                TaskState::Completed => {
                    let stored_hash = stop_value
                        .as_ref()
                        .and_then(|v| v.value.clone())
                        .unwrap_or_default();
                    // Default to a very old time if no timestamp is available
                    // so callers treat the entry as past any freshness window.
                    let stop_time = stop_value
                        .as_ref()
                        .map(|v| v.timestamp)
                        .unwrap_or_else(|| Utc::now() - Duration::days(365 * 10));

                    return Ok(Acquired::Completed {
                        stored_hash,
                        stop_time,
                        evict: Evictor {
                            task_cache: Arc::clone(&self.task_cache),
                            unique_id: self.unique_id.clone(),
                            old_start_value,
                        },
                    });
                }

                TaskState::Stale => {
                    on_contention(Contention::Aborted);
                    self.task_cache
                        .try_cleanup_task(&self.unique_id, &old_start_value)
                        .await?;
                    continue;
                }
            }
        }
    }

    /// Fetch the stored upstream-hashes snapshot recorded at the previous
    /// successful completion, if any.
    pub async fn get_info(&self) -> Result<Option<TaskInfo>, String> {
        self.task_cache.get_info(&self.unique_id).await
    }

    /// Delete all cache entries for this node.
    pub async fn invalidate(&self) -> Result<(), String> {
        self.task_cache.invalidate_task(&self.unique_id).await
    }

    /// Pre-seed a completed entry without executing — used when a node's
    /// relation already exists in the warehouse and we want future runs to
    /// treat it as a cache hit.
    ///
    /// Timestamps are deliberately set ~10 years in the past so the entry
    /// doesn't interact with `build_after` freshness windows.
    pub async fn write_completed(
        &self,
        hash: &str,
        upstreams: HashMap<String, String>,
    ) -> Result<(), String> {
        let long_ago = Utc::now() - Duration::days(365 * 10);
        let start_value = TaskValue::new(self.owner_id.clone(), long_ago, Some(hash.to_string()));
        let stop_value = TaskValue::new(self.owner_id.clone(), long_ago, Some(hash.to_string()));
        let info = TaskInfo { upstreams };

        self.task_cache
            .try_start_task(&self.unique_id, &start_value, &None)
            .await?;
        self.task_cache
            .stop_task(&self.unique_id, &stop_value, &info)
            .await?;
        Ok(())
    }
}

/// Extension trait so callers can spell `task_cache.session(unique_id, owner)`
/// instead of `NodeCacheSession::new(task_cache, ...)`.
pub trait TaskCacheExt {
    fn session(self, unique_id: String, owner_id: String) -> NodeCacheSession;
}

impl TaskCacheExt for Arc<dyn TaskCache> {
    fn session(self, unique_id: String, owner_id: String) -> NodeCacheSession {
        NodeCacheSession::new(self, unique_id, owner_id)
    }
}
