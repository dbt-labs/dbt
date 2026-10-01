use super::{ProjectEnv, Task, TestEnv, TestResult};
use async_trait::async_trait;
use dbt_common::stdfs;
use proto_rust::v1::public::fields::core_types::{
    CommandCompletedMsg, CompiledNodeMsg, CoreEventInfo, MarkSkippedChildrenMsg, NodeExecutingMsg,
    NodeFinishedMsg, ShowNodeMsg,
};
use serde::de::DeserializeOwned;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug)]
pub enum JsonLogEvent {
    CommandCompleted(CommandCompletedMsg),
    CompiledNode(CompiledNodeMsg),
    ShowNode(ShowNodeMsg),
    NodeExecuting(NodeExecutingMsg),
    NodeFinished(NodeFinishedMsg),
    MarkSkippedChildren(MarkSkippedChildrenMsg),
}

impl JsonLogEvent {
    fn command_completed(msg: CommandCompletedMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "CommandCompleted" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::CommandCompleted(msg))
    }

    fn compiled_node(msg: CompiledNodeMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "CompiledNode" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::CompiledNode(msg))
    }

    fn show_node(msg: ShowNodeMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "ShowNode" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::ShowNode(msg))
    }

    fn node_executing(msg: NodeExecutingMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "NodeExecuting" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::NodeExecuting(msg))
    }

    fn node_finished(msg: NodeFinishedMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "NodeFinished" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::NodeFinished(msg))
    }

    // Unfortunatelly apparently we write MarkSkippedChildren message
    // in a format similar to NodeExecuting, which is not comformant to
    // proto's, but ok for cloud clients somehow...
    fn mark_skipped_children(msg: MarkSkippedChildrenMsg) -> Option<Self> {
        let info = msg.info.as_ref()?;

        if info.name != "MarkSkippedChildren" {
            return None;
        }

        msg.data.as_ref()?;
        Some(Self::MarkSkippedChildren(msg))
    }

    pub fn info(&self) -> Option<&CoreEventInfo> {
        match self {
            Self::CommandCompleted(msg) => msg.info.as_ref(),
            Self::CompiledNode(msg) => msg.info.as_ref(),
            Self::ShowNode(msg) => msg.info.as_ref(),
            Self::NodeExecuting(msg) => msg.info.as_ref(),
            Self::NodeFinished(msg) => msg.info.as_ref(),
            Self::MarkSkippedChildren(msg) => msg.info.as_ref(),
        }
    }
}

/// A task wrapper that captures JSON logs from the stdout of the inner task
pub struct ExecuteAndCaptureLogs {
    inner: Box<dyn Task + Send + Sync>,
    captured_logs: Arc<Mutex<Vec<JsonLogEvent>>>,
    name: String,
}

impl ExecuteAndCaptureLogs {
    /// Create a new log capturing task that wraps an existing task
    pub fn new(name: String, inner: Box<dyn Task + Send + Sync>) -> Self {
        Self {
            inner,
            captured_logs: Arc::new(Mutex::new(Vec::new())),
            name,
        }
    }

    /// Get the captured logs after execution
    pub fn get_logs(&self) -> Vec<JsonLogEvent> {
        self.captured_logs.lock().unwrap().clone()
    }

    /// Parse logs from a file containing JSON log entries.
    ///
    /// Because dbt-core proto's do not allow discriminating between different message types,
    /// and message types actually overlap (same json may match multiple messages)
    /// we attempt to parse each line into each known message type => you can
    /// end up with multiple events per line.
    fn parse_logs_from_file(path: &PathBuf) -> Vec<JsonLogEvent> {
        let mut logs = Vec::new();
        if let Ok(content) = stdfs::read_to_string(path) {
            for line in content.lines() {
                // Skip empty lines
                if line.trim().is_empty() {
                    continue;
                }

                let _ = Self::try_parse::<CommandCompletedMsg, _>(
                    line,
                    &mut logs,
                    JsonLogEvent::command_completed,
                );
                let _ = Self::try_parse::<CompiledNodeMsg, _>(
                    line,
                    &mut logs,
                    JsonLogEvent::compiled_node,
                );
                let _ = Self::try_parse::<ShowNodeMsg, _>(line, &mut logs, JsonLogEvent::show_node);
                let _ = Self::try_parse::<NodeExecutingMsg, _>(
                    line,
                    &mut logs,
                    JsonLogEvent::node_executing,
                );
                let _ = Self::try_parse::<MarkSkippedChildrenMsg, _>(
                    line,
                    &mut logs,
                    JsonLogEvent::mark_skipped_children,
                );
                let _ = Self::try_parse::<NodeFinishedMsg, _>(
                    line,
                    &mut logs,
                    JsonLogEvent::node_finished,
                );
            }
        }

        logs
    }

    fn try_parse<T, F>(line: &str, sink: &mut Vec<JsonLogEvent>, wrap: F) -> bool
    where
        T: DeserializeOwned,
        F: FnOnce(T) -> Option<JsonLogEvent>,
    {
        match serde_json::from_str::<T>(line) {
            Ok(message) => {
                if let Some(event) = wrap(message) {
                    sink.push(event);
                    true
                } else {
                    false
                }
            }
            Err(_) => false,
        }
    }
}

#[async_trait]
impl Task for ExecuteAndCaptureLogs {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        // Run the inner task first
        self.inner.run(project_env, test_env, task_index).await?;

        // Determine the stdout file path
        let task_suffix = if task_index > 0 {
            format!("_{task_index}")
        } else {
            "".to_string()
        };

        let stdout_path = test_env
            .temp_dir
            .join(format!("{}{}.stdout", self.name, task_suffix));

        // Parse logs from stdout and stderr
        let stdout_logs = Self::parse_logs_from_file(&stdout_path);
        let stderr_path = test_env
            .temp_dir
            .join(format!("{}{}.stderr", self.name, task_suffix));
        let stderr_logs = Self::parse_logs_from_file(&stderr_path);

        // Combine logs from both stdout and stderr
        let mut logs = stdout_logs;
        logs.extend(stderr_logs);

        // Store the captured logs
        *self.captured_logs.lock().unwrap() = logs;

        Ok(())
    }

    fn is_counted(&self) -> bool {
        self.inner.is_counted()
    }
}

#[async_trait]
impl Task for Arc<ExecuteAndCaptureLogs> {
    async fn run(
        &self,
        project_env: &ProjectEnv,
        test_env: &TestEnv,
        task_index: usize,
    ) -> TestResult<()> {
        self.as_ref().run(project_env, test_env, task_index).await
    }

    fn is_counted(&self) -> bool {
        self.as_ref().is_counted()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pbjson_types::Timestamp;
    use proto_rust::v1::public::fields::core_types::{CommandCompleted, NodeExecuting, NodeInfo};

    #[test]
    fn test_parse_timestamp() {
        let s = r#"
            {
              "data": {
                "completed_at": "2025-09-20T02:43:29.433292Z",
                "elapsed": 0.3904609978199005,
                "log_version": 3,
                "success": false,
                "version": "2.0.0-preview.23"
              },
              "info": {
                "category": "",
                "code": "",
                "elapsed": "0.3904609978199005",
                "extra": {},
                "invocation_id": "01996501-508c-7163-ad76-6896107d00b6",
                "level": "info",
                "msg": "la",
                "name": "CommandCompleted",
                "pid": 77866,
                "thread": "tokio-runtime-worker",
                "ts": "2025-09-20T02:43:29.433306Z"
              }
            }
"#;

        let _: CommandCompletedMsg = serde_json::from_str(s).unwrap();
    }

    #[test]
    fn test_parse_logs_from_content() {
        let mut extra = std::collections::HashMap::new();
        extra.insert("detail".to_string(), "value".to_string());

        let command_completed = CommandCompletedMsg {
            info: Some(CoreEventInfo {
                name: "CommandCompleted".to_string(),
                code: "Q039".to_string(),
                msg: "Completed".to_string(),
                level: "info".to_string(),
                invocation_id: "test-123".to_string(),
                pid: 1234,
                thread: "main".to_string(),
                ts: Some(Timestamp {
                    seconds: 1,
                    nanos: 0,
                }),
                extra: extra.clone(),
                category: "test".to_string(),
            }),
            data: Some(CommandCompleted {
                command: "build".to_string(),
                success: true,
                completed_at: Some(Timestamp {
                    seconds: 2,
                    nanos: 0,
                }),
                elapsed: 0.5,
            }),
        };

        let node_executing = NodeExecutingMsg {
            info: Some(CoreEventInfo {
                name: "NodeExecuting".to_string(),
                code: "Q031".to_string(),
                msg: "Running model".to_string(),
                level: "debug".to_string(),
                invocation_id: "test-123".to_string(),
                pid: 1234,
                thread: "main".to_string(),
                ts: Some(Timestamp {
                    seconds: 3,
                    nanos: 0,
                }),
                extra,
                category: "node".to_string(),
            }),
            data: Some(NodeExecuting {
                node_info: Some(NodeInfo {
                    node_path: "model.path".to_string(),
                    node_name: "m1".to_string(),
                    unique_id: "model.test.m1".to_string(),
                    resource_type: "model".to_string(),
                    materialized: "table".to_string(),
                    node_status: "started".to_string(),
                    node_started_at: "2023-01-01T00:00:00Z".to_string(),
                    node_finished_at: String::new(),
                    meta: None,
                    node_relation: None,
                    node_checksum: String::new(),
                }),
            }),
        };

        let serialized = serde_json::to_string(&command_completed).unwrap();
        let serialized2 = serde_json::to_string(&node_executing).unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let log_file = temp_dir.path().join("test.log");
        stdfs::write(&log_file, format!("{serialized}\n{serialized2}\n")).unwrap();

        let logs = ExecuteAndCaptureLogs::parse_logs_from_file(&log_file);
        assert_eq!(logs.len(), 2);

        assert!(matches!(logs[0], JsonLogEvent::CommandCompleted(_)));
        assert!(matches!(logs[1], JsonLogEvent::NodeExecuting(_)));

        if let JsonLogEvent::NodeExecuting(parsed) = &logs[1] {
            let node_info = parsed
                .data
                .as_ref()
                .and_then(|data| data.node_info.as_ref())
                .expect("node info present");
            assert_eq!(node_info.unique_id, "model.test.m1");
            assert_eq!(parsed.info.as_ref().unwrap().name, "NodeExecuting");
        } else {
            panic!("Expected NodeExecuting event");
        }
    }
}
