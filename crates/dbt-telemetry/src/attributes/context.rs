use crate::proto::v1::public::events::fusion::phase::ExecutionPhase;
use dbt_tracing::TelemetryContext;

/// dbt context extracted from dbt spans/events and propagated to children and logs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DbtTelemetryContext {
    /// Current execution phase, if any.
    pub phase: Option<ExecutionPhase>,
    /// Unique ID of the current node, if any.
    pub unique_id: Option<String>,
}

impl From<DbtTelemetryContext> for TelemetryContext {
    fn from(value: DbtTelemetryContext) -> Self {
        TelemetryContext::new(value)
    }
}
