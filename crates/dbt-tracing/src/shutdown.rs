use super::error::TracingResult;

/// This trait is used by to allow gracefull shutdown of telemetry on exit.
/// All layers or supporting structures that require shutdown actions,
/// e.g. flushing file buffers, must return boxed trait object(s) on creation.
pub trait TelemetryShutdown: Send + Sync {
    fn shutdown(&mut self) -> TracingResult<()>;
}

pub type TelemetryShutdownItem = Box<dyn TelemetryShutdown>;
