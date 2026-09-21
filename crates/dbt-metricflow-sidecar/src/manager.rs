//! Async manager for one MetricFlow sidecar (`mf_entry`) subprocess.
//!
//! Applies the same architectural pattern as Fusion's proprietary
//! `RunnerManager` (subprocess spawn, NDJSON read/write, request/response
//! correlation by id via `tokio::oneshot`, a dedicated background reader
//! task) — reimplemented independently here using only OSS dependencies
//! already in this workspace, since `RunnerManager` itself lives in a
//! proprietary crate this repo cannot depend on.
//!
//! This manager owns exactly one subprocess and does not pool multiple
//! instances. `explain()` calls MetricFlow's pure-Python, GIL-bound query
//! compiler — unlike the DuckDB-backed sidecar `RunnerManager` talks to,
//! there is no native multi-threading underneath to exploit. Real
//! concurrency for this workload comes from running multiple
//! `MetricflowSidecarManager` instances (one process each), not from
//! multiplexing many in-flight requests onto a single process. This manager
//! still correlates requests by id — mostly for fairness/interleaving and to
//! keep the protocol consistent with the pooling model, not because a single
//! instance is expected to deliver parallel throughput.

use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use dbt_common::{ErrorCode, FsResult, fs_err};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::timeout;
use uuid::Uuid;

use crate::messages::{ExplainParams, ReadyMessage, Request, ResponseKind, parse_response_line};

const READY_TIMEOUT: Duration = Duration::from_secs(5);

struct SidecarProcess {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    #[allow(dead_code)]
    reader_task: JoinHandle<()>,
}

/// Manages one `mf_entry` subprocess and its in-flight requests.
pub struct MetricflowSidecarManager {
    process: Mutex<Option<SidecarProcess>>,
    pending: Mutex<HashMap<String, oneshot::Sender<Result<ResponseKind, String>>>>,
}

impl MetricflowSidecarManager {
    /// Spawn the sidecar binary at `binary_path` and wait for its ready handshake.
    #[tracing::instrument(skip_all, fields(binary_path = %binary_path.display()))]
    pub async fn spawn(binary_path: &Path) -> FsResult<Arc<Self>> {
        let mut child = Command::new(binary_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| {
                fs_err!(
                    ErrorCode::ExecutorFailed,
                    "failed to spawn mf_entry sidecar: {e}"
                )
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| fs_err!(ErrorCode::ExecutorFailed, "mf_entry stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| fs_err!(ErrorCode::ExecutorFailed, "mf_entry stdout unavailable"))?;
        // stderr is intentionally not captured here — mf_entry.py redirects library
        // logging there, and this manager doesn't yet do anything with it. A future
        // slice could forward it to tracing the way the proprietary RunnerManager does.
        drop(child.stderr.take());

        let mut stdout = BufReader::new(stdout);
        let ready = Self::read_ready_message(&mut stdout).await?;
        if ready.status != "ready" {
            return Err(fs_err!(
                ErrorCode::ExecutorFailed,
                "expected ready status from mf_entry, got: {}",
                ready.status
            ));
        }
        if ready.protocol_version != 1 {
            return Err(fs_err!(
                ErrorCode::ExecutorFailed,
                "unsupported mf-ipc protocol version: expected 1, got {}",
                ready.protocol_version
            ));
        }

        let manager = Arc::new(Self {
            process: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
        });

        let reader_manager = Arc::clone(&manager);
        let reader_task = tokio::spawn(async move { reader_manager.read_loop(stdout).await });

        {
            let mut guard = manager.process.lock().await;
            *guard = Some(SidecarProcess {
                child,
                stdin,
                reader_task,
            });
        }

        Ok(manager)
    }

    async fn read_ready_message(stdout: &mut BufReader<ChildStdout>) -> FsResult<ReadyMessage> {
        let mut line = String::new();
        timeout(READY_TIMEOUT, stdout.read_line(&mut line))
            .await
            .map_err(|_| {
                fs_err!(
                    ErrorCode::TaskTimeout,
                    "timed out waiting for mf_entry ready message"
                )
            })?
            .map_err(|e| {
                fs_err!(
                    ErrorCode::IoError,
                    "failed to read mf_entry ready message: {e}"
                )
            })?;

        serde_json::from_str(line.trim()).map_err(|e| {
            fs_err!(
                ErrorCode::JsonInvalid,
                "failed to parse mf_entry ready message: {e}. Got: {line}"
            )
        })
    }

    /// Compile a metric query to SQL without executing it.
    #[tracing::instrument(skip_all)]
    pub async fn explain(&self, params: ExplainParams) -> FsResult<String> {
        match self
            .submit(Request::explain(new_request_id(), params))
            .await?
        {
            ResponseKind::Explain { sql } => Ok(sql),
            ResponseKind::Ok => Err(fs_err!(
                ErrorCode::Unexpected,
                "explain got an ok response with no sql"
            )),
            ResponseKind::Error { error } => Err(fs_err!(
                ErrorCode::SidecarError,
                "{}: {}",
                error.error_type,
                error.message
            )),
        }
    }

    /// Health check — round trips through the sidecar without touching the manifest/engine.
    pub async fn ping(&self) -> FsResult<()> {
        self.submit_ok(Request::ping(new_request_id())).await
    }

    /// Graceful shutdown request. Waits for the sidecar's ok response; does not
    /// itself wait for the process to exit (the process is killed on drop regardless).
    pub async fn shutdown(&self) -> FsResult<()> {
        self.submit_ok(Request::shutdown(new_request_id())).await
    }

    async fn submit_ok<P: Serialize>(&self, request: Request<P>) -> FsResult<()> {
        match self.submit(request).await? {
            ResponseKind::Ok => Ok(()),
            ResponseKind::Explain { .. } => Err(fs_err!(
                ErrorCode::Unexpected,
                "expected an ok response, got an explain response"
            )),
            ResponseKind::Error { error } => Err(fs_err!(
                ErrorCode::SidecarError,
                "{}: {}",
                error.error_type,
                error.message
            )),
        }
    }

    async fn submit<P: Serialize>(&self, request: Request<P>) -> FsResult<ResponseKind> {
        let id = request.id.clone();
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            pending.insert(id.clone(), tx);
        }

        if let Err(e) = self.write_request(&request).await {
            let mut pending = self.pending.lock().await;
            pending.remove(&id);
            return Err(e);
        }

        let result = rx.await.map_err(|e| {
            fs_err!(
                ErrorCode::ConcurrencyError,
                "mf_entry closed the response channel for request {id}: {e}"
            )
        })?;
        result.map_err(|e| fs_err!(ErrorCode::SidecarError, "{e}"))
    }

    async fn write_request<P: Serialize>(&self, request: &Request<P>) -> FsResult<()> {
        let line = serde_json::to_string(request).map_err(|e| {
            fs_err!(
                ErrorCode::JsonInvalid,
                "failed to serialize mf-ipc request: {e}"
            )
        })?;

        let mut guard = self.process.lock().await;
        let process = guard.as_mut().ok_or_else(|| {
            fs_err!(
                ErrorCode::ExecutorFailed,
                "mf_entry process not initialized"
            )
        })?;

        process
            .stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| fs_err!(ErrorCode::IoError, "failed to write mf-ipc request: {e}"))?;
        process.stdin.write_all(b"\n").await.map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "failed to write newline to mf_entry: {e}"
            )
        })?;
        process
            .stdin
            .flush()
            .await
            .map_err(|e| fs_err!(ErrorCode::IoError, "failed to flush mf-ipc request: {e}"))
    }

    async fn read_loop(self: Arc<Self>, mut stdout: BufReader<ChildStdout>) {
        let mut line = String::new();
        loop {
            line.clear();
            match stdout.read_line(&mut line).await {
                Ok(0) => {
                    self.drain_pending(
                        "mf_entry stdout closed (EOF) with requests still in flight",
                    )
                    .await;
                    break;
                }
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match parse_response_line(trimmed) {
                        Ok((id, kind)) => self.complete(&id, Ok(kind)).await,
                        Err(e) => tracing::warn!("failed to parse mf-ipc response line: {e}"),
                    }
                }
                Err(e) => {
                    self.drain_pending(&format!("failed to read mf_entry stdout: {e}"))
                        .await;
                    break;
                }
            }
        }
    }

    async fn complete(&self, id: &str, result: Result<ResponseKind, String>) {
        let sender = {
            let mut pending = self.pending.lock().await;
            pending.remove(id)
        };
        match sender {
            Some(sender) => {
                let _ = sender.send(result);
            }
            None => tracing::warn!("received mf-ipc response for unknown request id {id}"),
        }
    }

    async fn drain_pending(&self, message: &str) {
        let mut pending = self.pending.lock().await;
        for (_, sender) in pending.drain() {
            let _ = sender.send(Err(message.to_string()));
        }
    }
}

fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}
