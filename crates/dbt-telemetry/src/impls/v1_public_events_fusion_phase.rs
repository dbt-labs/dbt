use std::fmt::Display;

use crate::proto::v1::public::events::fusion::phase::{ExecutionPhase, PhaseExecuted};

impl Display for ExecutionPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Unspecified => "Unspecified",
            Self::Clean => "Clean",
            Self::LoadProject => "LoadProject",
            Self::Parse => "Parse",
            Self::Schedule => "Schedule",
            Self::InitAdapter => "InitAdapter",
            Self::DeferHydration => "DeferHydration",
            Self::SchemaHydration => "SchemaHydration",
            Self::TaskGraphBuild => "TaskGraphBuild",
            Self::NodeCacheHydration => "NodeCacheHydration",
            Self::Render => "Render",
            Self::Analyze => "Analyze",
            Self::Run => "Run",
            Self::FreshnessAnalysis => "FreshnessAnalysis",
            Self::Lineage => "Lineage",
            Self::Debug => "Debug",
            Self::Compare => "Compare",
            Self::OnRunStart => "OnRunStart",
            Self::OnRunEnd => "OnRunEnd",
        };
        f.write_str(s)
    }
}

impl PhaseExecuted {
    /// Creates a new `ExecutionPhase` event indicating start of processing
    /// for phases that do not count nodes.
    ///
    /// This is thin wrapper around `new` that avoids the need to pass
    /// `None` for fields that are only known at the end of processing.
    ///
    /// # Arguments
    /// * `phase` - The current phase of execution.
    pub fn start_general(phase: ExecutionPhase) -> Self {
        Self::new(phase, None, None, None)
    }

    /// Creates a new `ExecutionPhase` event indicating start of processing
    /// for phases that count nodes.
    ///
    /// This is thin wrapper around `new` that avoids the need to pass
    /// `None` for fields that are only known at the end of processing.
    ///
    /// # Arguments
    /// * `phase` - The current phase of execution.
    /// * `node_count_total` - The total count of total individual nodes within the phase.
    pub fn start_with_node_count(phase: ExecutionPhase, node_count_total: u64) -> Self {
        Self::new(phase, Some(node_count_total), Some(0), Some(0))
    }
}
