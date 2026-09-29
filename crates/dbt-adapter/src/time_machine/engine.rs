//! Unified time machine engine for recording and replay.
//!
//! The `TimeMachine` enum provides a single abstraction for both:
//! - Recording adapter calls during execution
//! - Replaying recorded calls for compatibility testing

use std::sync::Arc;

use minijinja::Value;

use dbt_adapter_core::AdapterType;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::ResolvedQuoting;

use crate::relation::RelationObject;
use crate::time_machine::AdapterCallEvent;

use super::event::{MetadataCallArgs, RunCacheCloneEvent, SaoEvent};
use super::event_recorder::EventRecorder;
use super::event_replay::{Recording, ReplayError, ReplayMode, is_read_only_execute_call};
use super::semantic::SemanticCategory;
use super::serde::{ReplayCallContext, ReplayContext, json_to_value_with_context};
use super::validation::{IncomingEvent, TimeMachineEventValidationEngine, ValidationResult};

/// Unified time machine for recording or replaying adapter calls.
#[derive(Clone)]
pub enum TimeMachine {
    /// Recording mode - capture adapter calls
    Record(Arc<EventRecorder>),
    /// Replay mode - return recorded results
    Replay(Arc<EventReplayer>),
}

impl TimeMachine {
    /// Create a time machine in recording mode.
    pub fn recorder(recorder: Arc<EventRecorder>) -> Self {
        Self::Record(recorder)
    }

    /// Create a time machine in replay mode.
    pub fn replayer(replayer: Arc<EventReplayer>) -> Self {
        Self::Replay(replayer)
    }

    pub fn is_recording(&self) -> bool {
        matches!(self, Self::Record(_))
    }

    pub fn is_replaying(&self) -> bool {
        matches!(self, Self::Replay(_))
    }

    pub fn try_replay(
        &self,
        node_id: &str,
        method: &str,
        args: &[Value],
    ) -> Option<Result<Value, ReplayCallError>> {
        match self {
            Self::Record(_) => None, // Recording mode doesn't intercept
            Self::Replay(replayer) => Some(replayer.get_result(node_id, method, args)),
        }
    }

    /// Record an adapter call result.
    ///
    /// Only does something in recording mode.
    pub fn record_call(
        &self,
        node_id: impl Into<String>,
        method: impl Into<String>,
        args: serde_json::Value,
        result: serde_json::Value,
        success: bool,
        error: Option<String>,
    ) {
        if let Self::Record(recorder) = self {
            recorder.record_adapter_call(node_id, method, args, result, success, error);
        }
    }
}

/// Error returned when a replayed call fails.
#[derive(Debug, Clone)]
pub struct ReplayCallError {
    pub message: String,
    pub recorded_error: Option<String>,
}

impl std::fmt::Display for ReplayCallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(ref err) = self.recorded_error {
            write!(f, "Recorded call failed: {}", err)
        } else {
            write!(f, "{}", self.message)
        }
    }
}

impl std::error::Error for ReplayCallError {}

/// Event replayer - returns recorded results for adapter calls.
pub struct EventReplayer {
    recording: Recording,
    /// Replay ordering mode
    replay_mode: ReplayMode,
    /// Context for value reconstruction
    replay_ctx: ReplayContext,
    /// Validation engine for comparing events
    validation_engine: TimeMachineEventValidationEngine,
}

impl EventReplayer {
    /// Create a new replayer from a recording.
    pub fn new(recording: Recording) -> Self {
        // Parse adapter type from header
        let adapter_type = recording
            .header
            .adapter_type
            .parse()
            .unwrap_or(AdapterType::Snowflake);

        Self {
            recording,
            replay_mode: ReplayMode::default(),
            replay_ctx: ReplayContext {
                adapter_type,
                quoting: ResolvedQuoting::default(),
            },
            validation_engine: TimeMachineEventValidationEngine::new()
                .with_adapter_type(adapter_type),
        }
    }

    /// Load a replayer from a recording directory.
    pub fn load(path: impl AsRef<std::path::Path>) -> Result<Self, ReplayError> {
        let recording = Recording::load(path)?;
        Ok(Self::new(recording))
    }

    /// Set the replay ordering mode.
    ///
    /// - `Strict`: Events must match in exact sequence order (default)
    /// - `Semantic`: Write operations are barriers; reads can match flexibly within segments
    pub fn with_replay_mode(mut self, mode: ReplayMode) -> Self {
        self.replay_mode = mode;
        self
    }

    /// Get the current replay mode.
    pub fn replay_mode(&self) -> ReplayMode {
        self.replay_mode
    }

    /// Set custom quoting for relation reconstruction.
    pub fn with_quoting(mut self, quoting: ResolvedQuoting) -> Self {
        self.replay_ctx.quoting = quoting;
        self
    }

    /// Get the recording header.
    pub fn header(&self) -> &super::event::RecordingHeader {
        &self.recording.header
    }

    /// Get the result for an adapter call.
    ///
    /// Pure/Cache calls are filtered at the adapter level and should never reach here.
    pub fn get_result(
        &self,
        node_id: &str,
        method: &str,
        args: &[Value],
    ) -> Result<Value, ReplayCallError> {
        let call_category = SemanticCategory::from_adapter_method(method);
        let serialized_args = super::serde::serialize_args(args);

        // Detect execute/run_query calls whose SQL is read-only (SELECT, SHOW, ...).
        // These are compile-time probe queries from dbt macros (e.g. dbt_utils.date_spine
        // runs `SELECT datediff(...)` to compute n_periods). Python dbt records them after
        // SHOW PARAMETERS, but Fusion emits them earlier during Jinja compilation, causing
        // an ordering mismatch. We match them globally as unordered reads in both modes.
        let is_ro_exec =
            call_category.is_mutating() && is_read_only_execute_call(method, &serialized_args);

        // Dispatch to the appropriate matching strategy
        let event = match self.replay_mode {
            ReplayMode::Strict => {
                if is_ro_exec {
                    // Use global unordered matching so the sequential position is not
                    // corrupted by out-of-order probe queries.
                    self.recording
                        .take_ro_exec_read(node_id, method, &serialized_args)
                        .ok_or_else(|| ReplayCallError {
                            message: format!(
                                "No recorded event for read-only {} call '{}' on node '{}'. \
                                 The recording may be outdated or this probe was not captured.",
                                call_category, method, node_id
                            ),
                            recorded_error: None,
                        })?
                } else {
                    self.get_result_strict(node_id, method, &serialized_args, call_category)?
                }
            }
            ReplayMode::Semantic => {
                // In semantic mode, take_semantic_match handles the read-only execute
                // redirect internally (Write category with SELECT SQL → untracked read).
                // The effective category passed here is the original call_category; the
                // redirect happens inside take_semantic_match.
                match self.get_result_semantic(node_id, method, &serialized_args, call_category) {
                    Ok(event) => event,
                    Err(_)
                        if !call_category.is_mutating()
                            && self
                                .validation_engine
                                .is_known_nondeterministic_node(node_id) =>
                    {
                        // For read calls on nodes whose name marks them as known
                        // non-deterministic (e.g. elementary via `.elementary.` in `node_id`),
                        // return null when no matching event exists. This is about node
                        // identity, not about non-deterministic ordering of replay events.
                        tracing::warn!(
                            node_id,
                            method,
                            "Replay: returning null for unmatched read on non-deterministic node"
                        );
                        return Ok(Value::from(()));
                    }
                    Err(err) => return Err(err),
                }
            }
        };

        // Validate the event using the validation engine
        self.validate_event(node_id, method, &serialized_args, event)?;

        self.convert_event_to_result(event)
    }

    /// Validate an event using the validation engine.
    fn validate_event(
        &self,
        node_id: &str,
        method: &str,
        args: &serde_json::Value,
        event: &AdapterCallEvent,
    ) -> Result<(), ReplayCallError> {
        let incoming = IncomingEvent::new(node_id, method, args);
        match self.validation_engine.validate(&incoming, event) {
            ValidationResult::Match | ValidationResult::Skipped(_) => Ok(()),
            ValidationResult::Mismatch(mismatch) => Err(ReplayCallError {
                message: format!(
                    "Replay mismatch for '{}' on node '{}' (seq {}):\n{}",
                    method, node_id, event.seq, mismatch,
                ),
                recorded_error: None,
            }),
        }
    }

    /// Get result using strict sequential matching.
    fn get_result_strict(
        &self,
        node_id: &str,
        _method: &str,
        _args: &serde_json::Value,
        call_category: SemanticCategory,
    ) -> Result<&AdapterCallEvent, ReplayCallError> {
        if self.recording.peek_next(node_id).is_none() {
            return Err(ReplayCallError {
                message: format!(
                    "No recorded event for {} call on node '{}'. \
                     Recording may be incomplete or from a different code version.",
                    call_category, node_id
                ),
                recorded_error: None,
            });
        }

        // Consume and return the matching event for replay
        Ok(self
            .recording
            .take_next(node_id)
            .expect("event should exist after peek"))
    }

    /// Writes must match the next write barrier in sequence; reads can match any read in
    /// the current segment with matching args (and the same recorded read can satisfy
    /// multiple calls).
    fn get_result_semantic(
        &self,
        node_id: &str,
        method: &str,
        args: &serde_json::Value,
        call_category: SemanticCategory,
    ) -> Result<&AdapterCallEvent, ReplayCallError> {
        // Use semantic matching from the Recording
        self.recording
            .take_semantic_match(node_id, method, args, call_category)
            .ok_or_else(|| {
                let context = if call_category.is_mutating() {
                    "Write operations must match the next write barrier in sequence."
                } else {
                    "No matching read found in current segment. \
                     The same read can be matched multiple times, but at least one must exist."
                };
                ReplayCallError {
                    message: format!(
                        "No recorded event for {} call '{}' on node '{}'. {}",
                        call_category, method, node_id, context
                    ),
                    recorded_error: None,
                }
            })
    }

    /// Convert a matched event to a replay result.
    fn convert_event_to_result(&self, event: &AdapterCallEvent) -> Result<Value, ReplayCallError> {
        if !event.success {
            return Err(ReplayCallError {
                message: "Recorded call failed".to_string(),
                recorded_error: event.error.clone(),
            });
        }

        // Convert the recorded result back to a Value
        let call_ctx = build_replay_call_context(&self.replay_ctx, event);
        let value = json_to_value_with_context(&event.result, &call_ctx);
        Ok(value)
    }

    /// Falls back from `caller_id` to "global" because recording and replay can see
    /// different node IDs for what's semantically the same call.
    pub fn get_metadata_result(
        &self,
        caller_id: &str,
        method: &str,
        args: &MetadataCallArgs,
    ) -> Option<Result<serde_json::Value, ReplayCallError>> {
        let caller_ids_to_try = if caller_id == "global" {
            vec![caller_id]
        } else {
            vec![caller_id, "global"]
        };

        for try_caller_id in caller_ids_to_try {
            let result = match self.replay_mode {
                ReplayMode::Strict => self.try_get_metadata_result_strict(try_caller_id, method),
                ReplayMode::Semantic => {
                    self.try_get_metadata_result_semantic(try_caller_id, method, args)
                }
            };
            // Only return if we got a successful result or a write error.
            // For reads, continue searching (None means no match, Some(Err) for strict reads
            // should try cross-caller search first).
            match &result {
                Some(Ok(_)) => return result,
                Some(Err(_)) => {
                    // For semantic mode metadata reads, try cross-caller search before erroring
                    let category = SemanticCategory::from_metadata_method(method);
                    if matches!(self.replay_mode, ReplayMode::Semantic)
                        && matches!(category, SemanticCategory::MetadataRead)
                    {
                        // Continue to cross-caller search below
                        continue;
                    }
                    return result;
                }
                None => continue,
            }
        }

        // For semantic mode metadata reads, search across all callers as last resort.
        // Due to parallel execution, the same query might be recorded under a different caller.
        // We use superset matching: if recorded args contain all requested relations, it's a match.
        if matches!(self.replay_mode, ReplayMode::Semantic) {
            let category = SemanticCategory::from_metadata_method(method);
            if matches!(category, SemanticCategory::MetadataRead)
                && let Some(event) = self
                    .recording
                    .find_metadata_read_across_all_callers(method, args)
            {
                return self.convert_metadata_event_to_result(event);
            }
        }

        None
    }

    /// Internal helper to try getting a metadata result using strict sequential matching.
    fn try_get_metadata_result_strict(
        &self,
        caller_id: &str,
        method: &str,
    ) -> Option<Result<serde_json::Value, ReplayCallError>> {
        // For metadata calls, we match by caller_id and method
        let event = match self.recording.peek_next_metadata(caller_id) {
            Some(event) => event,
            None => {
                // No recorded event for this caller_id
                return None;
            }
        };

        // Validate method name
        if event.method != method {
            return Some(Err(ReplayCallError {
                message: format!(
                    "Metadata method mismatch for caller '{}': expected '{}', got '{}' (seq {})",
                    caller_id, event.method, method, event.seq
                ),
                recorded_error: None,
            }));
        }

        // Consume the matching event for replay
        let event = self.recording.take_next_metadata(caller_id)?;
        self.convert_metadata_event_to_result(event)
    }

    /// Same write/read matching semantics as [`get_result_semantic`]. A caller with no
    /// recorded events at all is treated as a non-match so the search can fall back to
    /// other callers, rather than as an error.
    fn try_get_metadata_result_semantic(
        &self,
        caller_id: &str,
        method: &str,
        args: &MetadataCallArgs,
    ) -> Option<Result<serde_json::Value, ReplayCallError>> {
        // Determine the semantic category of this metadata method
        let category = SemanticCategory::from_metadata_method(method);

        // First check if this caller has any events at all
        // If not, return None to allow fallback to other callers (e.g., "global")
        if !self.recording.has_metadata_events_for_caller(caller_id) {
            return None;
        }

        match self
            .recording
            .take_semantic_metadata_match(caller_id, method, args, category)
        {
            Some(event) => self.convert_metadata_event_to_result(event),
            None => {
                // Caller has events but no match found
                let context = if category.is_mutating() {
                    "Write operations must match in sequence.".to_string()
                } else {
                    format!(
                        "No matching read with args {:?} found in current segment.",
                        args
                    )
                };
                Some(Err(ReplayCallError {
                    message: format!(
                        "No recorded event for metadata {} call '{}' on caller '{}'. {}",
                        category, method, caller_id, context
                    ),
                    recorded_error: None,
                }))
            }
        }
    }

    /// Convert a matched metadata event to a result.
    fn convert_metadata_event_to_result(
        &self,
        event: &super::event::MetadataCallEvent,
    ) -> Option<Result<serde_json::Value, ReplayCallError>> {
        if !event.success {
            return Some(Err(ReplayCallError {
                message: "Recorded metadata call failed".to_string(),
                recorded_error: event.error.clone(),
            }));
        }

        Some(Ok(event.result.clone()))
    }

    pub fn has_metadata_events(&self) -> bool {
        self.recording.total_metadata_events() > 0
    }

    /// Get SAO skip event for a node if exists.
    ///
    /// Returns the SAO event if the node was skipped due to a cache hit during recording.
    /// This enables replay to skip execution for nodes that were also skipped during recording.
    pub fn get_sao_event(&self, node_id: &str) -> Option<&SaoEvent> {
        self.recording.get_sao_event(node_id)
    }

    pub fn has_sao_event(&self, node_id: &str) -> bool {
        self.recording.has_sao_event(node_id)
    }

    /// Get the recorded dbt State service clone decision for a node if one exists.
    ///
    /// Returns the recorded decision if the node's execution was satisfied by
    /// a clone during recording, so replay can route the node back through
    /// the same clone path instead of falling through to a normal Execute.
    pub fn get_run_cache_clone_event(&self, node_id: &str) -> Option<&RunCacheCloneEvent> {
        self.recording.get_run_cache_clone_event(node_id)
    }

    /// Get the recorded clone decision for a node and clone phase.
    pub fn get_run_cache_clone_event_for_phase(
        &self,
        node_id: &str,
        dev_clone: bool,
    ) -> Option<&RunCacheCloneEvent> {
        self.recording
            .get_run_cache_clone_event_for_phase(node_id, dev_clone)
    }

    /// Get total number of SAO skip events in the recording.
    pub fn total_sao_events(&self) -> usize {
        self.recording.total_sao_events()
    }

    /// Get the result for a run_remote_adhoc query.
    ///
    /// Returns deserialized Arrow batches and schema from the recording.
    pub fn get_run_remote_adhoc_result(
        &self,
    ) -> Option<Result<(Vec<arrow::array::RecordBatch>, arrow_schema::SchemaRef), ReplayCallError>>
    {
        let event = self.recording.take_next_run_remote_adhoc()?;

        if !event.success {
            return Some(Err(ReplayCallError {
                message: format!(
                    "Recorded run_remote_adhoc query failed: {}",
                    event.error.as_deref().unwrap_or("unknown error")
                ),
                recorded_error: event.error.clone(),
            }));
        }

        match super::serializable_impls::ipc_base64_to_batches(&event.result_ipc_base64) {
            Some((batches, schema)) => Some(Ok((batches, schema))),
            None => Some(Err(ReplayCallError {
                message: "Failed to decode recorded Arrow IPC data for run_remote_adhoc"
                    .to_string(),
                recorded_error: None,
            })),
        }
    }

    /// Get the next cache invalidation event's invalidated nodes.
    ///
    /// Returns `Some(nodes)` if a cache invalidation event was recorded,
    /// `None` if no more cache invalidation events exist.
    pub fn get_cache_invalidations(&self) -> Option<Vec<String>> {
        self.recording
            .take_next_cache_invalidation()
            .map(|e| e.invalidated_nodes.clone())
    }

    /// Reset replay state for all nodes.
    pub fn reset(&self) {
        self.recording.reset();
    }

    /// Get statistics about the recording.
    pub fn stats(&self) -> ReplayerStats {
        ReplayerStats {
            total_events: self.recording.total_events(),
            adapter_events: self.recording.total_adapter_events(),
            metadata_events: self.recording.total_metadata_events(),
            sao_events: self.recording.total_sao_events(),
            node_count: self.recording.node_ids().count(),
            metadata_caller_count: self.recording.metadata_caller_ids().count(),
        }
    }
}

/// Extends replay context with additional per-call context extracted from arguments for events
/// that require it.
fn build_replay_call_context(
    replay_ctx: &ReplayContext,
    event: &AdapterCallEvent,
) -> ReplayCallContext {
    let call_ctx = replay_ctx.clone().into();
    let relation_type = match event.method.as_str() {
        "get_relation_config" => GetRelationConfig::try_from((&event.args, &call_ctx))
            .ok()
            .and_then(|args| args.relation_type),
        _ => None,
    };
    call_ctx.with_relation_type(relation_type)
}

struct GetRelationConfig {
    relation_type: Option<RelationType>,
}

impl TryFrom<(&serde_json::Value, &ReplayCallContext)> for GetRelationConfig {
    type Error = ();

    fn try_from(
        (args, ctx): (&serde_json::Value, &ReplayCallContext),
    ) -> Result<Self, Self::Error> {
        let relation = args.as_array().and_then(|args| args.first()).ok_or(())?;
        let relation = json_to_value_with_context(relation, ctx)
            .downcast_object::<RelationObject>()
            .ok_or(())?;
        Ok(Self {
            relation_type: relation.relation_type(),
        })
    }
}

/// Statistics about a replayer's recording.
#[derive(Debug, Clone)]
pub struct ReplayerStats {
    pub total_events: usize,
    pub adapter_events: usize,
    pub metadata_events: usize,
    pub sao_events: usize,
    pub node_count: usize,
    pub metadata_caller_count: usize,
}
