#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    FailedUpstream(String), // upstream work node unique_id
    FailedPhase,
    Reused,
}
