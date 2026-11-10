use dbt_telemetry::{HookOutcome, NodeOutcome, NodeSkipReason, NodeType, TestOutcome};
use error::{FsError, FsResult};
use strum::FromRepr;

use dbt_tracing::metrics::{MetricKey, get_metric};

const METRIC_KEY_NAMESPACE_SHIFT: u64 = 56;
const METRIC_KEY_FAMILY_SHIFT: u64 = 48;
const METRIC_KEY_LANE_0_SHIFT: u64 = 40;
const METRIC_KEY_LANE_1_SHIFT: u64 = 32;
const METRIC_KEY_LANE_2_SHIFT: u64 = 24;
const METRIC_KEY_LANE_3_SHIFT: u64 = 16;
const METRIC_KEY_COMPONENT_MASK: u64 = 0xff;
const METRIC_KEY_RESERVED_MASK: u64 = 0xffff;

const FUSION_METRIC_NAMESPACE: u8 = 1;
const FUSION_INVOCATION_METRIC_FAMILY: u8 = 1;
const FUSION_NODE_COUNTS_METRIC_FAMILY: u8 = 2;
const FUSION_OUTCOME_COUNTS_METRIC_FAMILY: u8 = 3;
const FUSION_HOOK_COUNTS_METRIC_FAMILY: u8 = 4;
const FUSION_RUN_CACHE_SERVICE_METRIC_FAMILY: u8 = 5;

const OUTCOME_KIND_NODE: u8 = 1;
const OUTCOME_KIND_HOOK: u8 = 2;

fn checked_metric_key_u8<T>(value: T, context: &str) -> u8
where
    T: TryInto<u8>,
    T::Error: std::fmt::Debug,
{
    value.try_into().expect(context)
}

const fn pack_metric_key(namespace: u8, family: u8, lanes: [u8; 4]) -> MetricKey {
    MetricKey::from_raw(
        ((namespace as u64) << METRIC_KEY_NAMESPACE_SHIFT)
            | ((family as u64) << METRIC_KEY_FAMILY_SHIFT)
            | ((lanes[0] as u64) << METRIC_KEY_LANE_0_SHIFT)
            | ((lanes[1] as u64) << METRIC_KEY_LANE_1_SHIFT)
            | ((lanes[2] as u64) << METRIC_KEY_LANE_2_SHIFT)
            | ((lanes[3] as u64) << METRIC_KEY_LANE_3_SHIFT),
    )
}

const fn key_namespace(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_NAMESPACE_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_family(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_FAMILY_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_lane_0(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_LANE_0_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_lane_1(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_LANE_1_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_lane_2(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_LANE_2_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_lane_3(key: MetricKey) -> u8 {
    ((key.into_raw() >> METRIC_KEY_LANE_3_SHIFT) & METRIC_KEY_COMPONENT_MASK) as u8
}

const fn key_reserved_bits(key: MetricKey) -> u16 {
    (key.into_raw() & METRIC_KEY_RESERVED_MASK) as u16
}

/// Fusion metric-key encoding.
///
/// The top two bytes reserve the metric namespace and family:
/// - bits `63..56`: namespace
/// - bits `55..48`: family
///
/// Fusion then uses four one-byte payload lanes and leaves the low 16 bits reserved:
/// - bits `47..40`: lane 0
/// - bits `39..32`: lane 1
/// - bits `31..24`: lane 2
/// - bits `23..16`: lane 3
/// - bits `15..0`: reserved, must stay zero
///
/// Family payloads:
/// - invocation metrics: `lane0 = InvocationMetricKey`
/// - node counts: `lane0 = NodeType`
/// - outcome counts: `lane0 = outcome kind`, `lane1 = outcome value`,
///   `lane2 = NodeSkipReason`, `lane3 = Option<NodeSubOutcome>`
/// - hook counts: no payload lanes
#[repr(u8)]
#[cfg_attr(test, derive(strum::EnumIter))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromRepr)]
pub enum InvocationMetricKey {
    TotalErrors = 0,
    TotalWarnings,
    AutoFixSuggestions,
    // Run summary totals based on node outcomes. These may change or fold into
    // becoming an actual log report later.
    NodeTotalsSuccess,
    NodeTotalsWarning,
    NodeTotalsError,
    NodeTotalsReused,
    NodeTotalsSkipped,
    NodeTotalsCanceled,
    NodeTotalsNoOp,
    /// Member generic tests that ran inside an aggregated query.
    AggregatedTests,
    /// Aggregated generic-test queries issued.
    AggregatedTestQueries,
    /// Data tests proven redundant by static analysis and skipped.
    StaticallyCheckedTests,
}

#[repr(u8)]
#[cfg_attr(test, derive(strum::EnumIter))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromRepr)]
pub enum RunCacheServiceMetricKey {
    Enabled = 0,
    Disabled,
    ClientInitSuccess,
    ClientInitFailure,
    ValidationSupported,
    ValidationUnsupported,
    ValidationSkipped,
    SubmitAttempt,
    SubmitSuccess,
    SubmitFailure,
    DecisionReadyToExecute,
    DecisionSkipExecution,
    DecisionReadyToClone,
    DecisionUnknown,
    MetadataCacheHit,
    MetadataCacheMiss,
    MetadataLookupFailure,
    UnsupportedNode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OutcomeKind {
    Node(NodeOutcome),
    Hook(HookOutcome),
}

/// A sub-outcome discriminator for [`OutcomeCountsKey`] that refines how a
/// node's primary outcome is bucketed in aggregation. Only variants that
/// change routing are included; plain success maps to `None`.
#[repr(u8)]
#[cfg_attr(test, derive(strum::EnumIter))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, FromRepr)]
pub enum NodeSubOutcome {
    /// Test or unit-test node warned (failures between warn/error thresholds).
    TestWarned = 1,
    /// Test or unit-test node failed (failures above error threshold).
    TestFailed = 2,
    /// Source freshness exceeded warning threshold (`OutcomeWarned` + `NodeOutcome::Success`).
    FreshnessWarned = 3,
    /// Source freshness exceeded error threshold (`OutcomeFailed` + `NodeOutcome::Success`).
    FreshnessFailed = 4,
    /// Non-test, non-freshness node completed with warnings (`NodeWarningOutcome::WithWarnings`).
    NodeWarned = 5,
}

impl NodeSubOutcome {
    /// Maps a [`TestOutcome`] to its corresponding sub-outcome.
    /// Returns `None` for `Passed` (routes to the plain success bucket).
    pub fn from_test_outcome(t: TestOutcome) -> Option<Self> {
        match t {
            TestOutcome::Passed => None,
            TestOutcome::Warned => Some(Self::TestWarned),
            TestOutcome::Failed => Some(Self::TestFailed),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OutcomeCountsKey(OutcomeKind, NodeSkipReason, Option<NodeSubOutcome>);

impl OutcomeCountsKey {
    pub fn new(
        outcome: OutcomeKind,
        skip_reason: NodeSkipReason,
        sub_outcome: Option<NodeSubOutcome>,
    ) -> Self {
        Self(outcome, skip_reason, sub_outcome)
    }

    pub fn outcome(&self) -> OutcomeKind {
        self.0
    }

    pub fn skip_reason(&self) -> NodeSkipReason {
        self.1
    }

    pub fn sub_outcome(&self) -> Option<NodeSubOutcome> {
        self.2
    }

    pub fn into_parts(self) -> (OutcomeKind, NodeSkipReason, Option<NodeSubOutcome>) {
        (self.0, self.1, self.2)
    }
}

#[cfg_attr(test, derive(strum::EnumDiscriminants))]
#[cfg_attr(
    test,
    strum_discriminants(derive(strum::EnumIter), name(FusionMetricKeyKind))
)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FusionMetricKey {
    InvocationMetric(InvocationMetricKey),
    RunCacheService(RunCacheServiceMetricKey),
    NodeCounts(NodeType),
    OutcomeCounts(OutcomeCountsKey),
    HookCounts,
}

impl From<FusionMetricKey> for MetricKey {
    fn from(key: FusionMetricKey) -> Self {
        match key {
            FusionMetricKey::InvocationMetric(metric) => pack_metric_key(
                FUSION_METRIC_NAMESPACE,
                FUSION_INVOCATION_METRIC_FAMILY,
                [metric as u8, 0, 0, 0],
            ),
            FusionMetricKey::RunCacheService(metric) => pack_metric_key(
                FUSION_METRIC_NAMESPACE,
                FUSION_RUN_CACHE_SERVICE_METRIC_FAMILY,
                [metric as u8, 0, 0, 0],
            ),
            FusionMetricKey::NodeCounts(node_type) => pack_metric_key(
                FUSION_METRIC_NAMESPACE,
                FUSION_NODE_COUNTS_METRIC_FAMILY,
                [
                    // SAFETY: Fusion lane widths are fixed. The fusion metric
                    // roundtrip test iterates the current taxonomy so lane
                    // exhaustion fails loudly if it grows beyond this packing.
                    checked_metric_key_u8(
                        node_type as i32,
                        "NodeType must fit in the fusion node-count metric lane",
                    ),
                    0,
                    0,
                    0,
                ],
            ),
            FusionMetricKey::OutcomeCounts(outcome_key) => {
                let (outcome_kind, outcome_value) = match outcome_key.outcome() {
                    OutcomeKind::Node(node_outcome) => (
                        OUTCOME_KIND_NODE,
                        // SAFETY: Fusion lane widths are fixed. The fusion
                        // metric roundtrip test iterates the current taxonomy
                        // so lane exhaustion fails loudly if it grows beyond
                        // this packing.
                        checked_metric_key_u8(
                            node_outcome as i32,
                            "NodeOutcome must fit in the fusion outcome metric lane",
                        ),
                    ),
                    OutcomeKind::Hook(hook_outcome) => (
                        OUTCOME_KIND_HOOK,
                        // SAFETY: Fusion lane widths are fixed. The fusion
                        // metric roundtrip test iterates the current taxonomy
                        // so lane exhaustion fails loudly if it grows beyond
                        // this packing.
                        checked_metric_key_u8(
                            hook_outcome as i32,
                            "HookOutcome must fit in the fusion outcome metric lane",
                        ),
                    ),
                };

                pack_metric_key(
                    FUSION_METRIC_NAMESPACE,
                    FUSION_OUTCOME_COUNTS_METRIC_FAMILY,
                    [
                        outcome_kind,
                        outcome_value,
                        // SAFETY: Fusion lane widths are fixed. The fusion
                        // metric roundtrip test iterates the current taxonomy
                        // so lane exhaustion fails loudly if it grows beyond
                        // this packing.
                        checked_metric_key_u8(
                            outcome_key.skip_reason() as i32,
                            "NodeSkipReason must fit in the fusion outcome metric lane",
                        ),
                        outcome_key
                            .sub_outcome()
                            .map(|value| value as u8)
                            .unwrap_or(0),
                    ],
                )
            }
            FusionMetricKey::HookCounts => pack_metric_key(
                FUSION_METRIC_NAMESPACE,
                FUSION_HOOK_COUNTS_METRIC_FAMILY,
                [0, 0, 0, 0],
            ),
        }
    }
}

impl TryFrom<MetricKey> for FusionMetricKey {
    type Error = ();

    fn try_from(key: MetricKey) -> Result<Self, Self::Error> {
        if key_namespace(key) != FUSION_METRIC_NAMESPACE || key_reserved_bits(key) != 0 {
            return Err(());
        }

        match key_family(key) {
            FUSION_INVOCATION_METRIC_FAMILY => {
                if key_lane_1(key) != 0 || key_lane_2(key) != 0 || key_lane_3(key) != 0 {
                    return Err(());
                }

                Ok(Self::InvocationMetric(
                    InvocationMetricKey::from_repr(key_lane_0(key)).ok_or(())?,
                ))
            }
            FUSION_NODE_COUNTS_METRIC_FAMILY => {
                if key_lane_1(key) != 0 || key_lane_2(key) != 0 || key_lane_3(key) != 0 {
                    return Err(());
                }

                let node_type = NodeType::try_from(i32::from(key_lane_0(key))).map_err(|_| ())?;
                Ok(Self::NodeCounts(node_type))
            }
            FUSION_RUN_CACHE_SERVICE_METRIC_FAMILY => {
                if key_lane_1(key) != 0 || key_lane_2(key) != 0 || key_lane_3(key) != 0 {
                    return Err(());
                }

                Ok(Self::RunCacheService(
                    RunCacheServiceMetricKey::from_repr(key_lane_0(key)).ok_or(())?,
                ))
            }
            FUSION_OUTCOME_COUNTS_METRIC_FAMILY => {
                let outcome = match key_lane_0(key) {
                    OUTCOME_KIND_NODE => OutcomeKind::Node(
                        NodeOutcome::try_from(i32::from(key_lane_1(key))).map_err(|_| ())?,
                    ),
                    OUTCOME_KIND_HOOK => OutcomeKind::Hook(
                        HookOutcome::try_from(i32::from(key_lane_1(key))).map_err(|_| ())?,
                    ),
                    _ => return Err(()),
                };
                let skip_reason =
                    NodeSkipReason::try_from(i32::from(key_lane_2(key))).map_err(|_| ())?;
                let sub_outcome = match key_lane_3(key) {
                    0 => None,
                    lane => Some(NodeSubOutcome::from_repr(lane).ok_or(())?),
                };

                Ok(Self::OutcomeCounts(OutcomeCountsKey::new(
                    outcome,
                    skip_reason,
                    sub_outcome,
                )))
            }
            FUSION_HOOK_COUNTS_METRIC_FAMILY => {
                if key_lane_0(key) != 0
                    || key_lane_1(key) != 0
                    || key_lane_2(key) != 0
                    || key_lane_3(key) != 0
                {
                    return Err(());
                }

                Ok(Self::HookCounts)
            }
            _ => Err(()),
        }
    }
}

/// Get the `TotalErrors` invocation metric value.
pub fn get_error_count() -> u64 {
    get_metric(FusionMetricKey::InvocationMetric(
        InvocationMetricKey::TotalErrors,
    ))
}

/// Produce an error that forces returning an exit code based on the current
/// error counter. The return effect is achieved through [FsError::ExitWithStatus].
///
/// If there were any errors recorded, exit code will be 1, otherwise 0.
pub fn return_exit_code_from_error_counter() -> Box<FsError> {
    let exit_code = if get_error_count() > 0 { 1 } else { 0 };
    FsError::exit_with_status(exit_code)
}

/// Check the error count and return an status code so the CLI produces a status code immediately.
///
/// This is good to run right at the end of functions responsible to handling a CLI sub-command.
pub fn error_count_checkpoint() -> FsResult<()> {
    if get_error_count() > 0 {
        Err(FsError::exit_with_status(1))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use strum::IntoEnumIterator;

    use super::*;

    fn all_fusion_metric_keys() -> Vec<FusionMetricKey> {
        let mut keys = InvocationMetricKey::iter()
            .map(FusionMetricKey::InvocationMetric)
            .collect::<Vec<_>>();

        keys.extend(RunCacheServiceMetricKey::iter().map(FusionMetricKey::RunCacheService));
        keys.extend(NodeType::iter().map(FusionMetricKey::NodeCounts));
        keys.push(FusionMetricKey::HookCounts);

        for node_outcome in NodeOutcome::iter() {
            for skip_reason in NodeSkipReason::iter() {
                keys.push(FusionMetricKey::OutcomeCounts(OutcomeCountsKey::new(
                    OutcomeKind::Node(node_outcome),
                    skip_reason,
                    None,
                )));

                for sub_outcome in NodeSubOutcome::iter() {
                    keys.push(FusionMetricKey::OutcomeCounts(OutcomeCountsKey::new(
                        OutcomeKind::Node(node_outcome),
                        skip_reason,
                        Some(sub_outcome),
                    )));
                }
            }
        }

        for hook_outcome in HookOutcome::iter() {
            for skip_reason in NodeSkipReason::iter() {
                keys.push(FusionMetricKey::OutcomeCounts(OutcomeCountsKey::new(
                    OutcomeKind::Hook(hook_outcome),
                    skip_reason,
                    None,
                )));

                for sub_outcome in NodeSubOutcome::iter() {
                    keys.push(FusionMetricKey::OutcomeCounts(OutcomeCountsKey::new(
                        OutcomeKind::Hook(hook_outcome),
                        skip_reason,
                        Some(sub_outcome),
                    )));
                }
            }
        }

        keys
    }

    #[test]
    fn all_fusion_metric_keys_cover_every_top_level_type() {
        let keys = all_fusion_metric_keys();

        for kind in FusionMetricKeyKind::iter() {
            assert!(
                keys.iter()
                    .copied()
                    .any(|key| FusionMetricKeyKind::from(key) == kind),
                "all_fusion_metric_keys() must generate at least one {kind:?} key",
            );
        }
    }

    #[test]
    fn fusion_metric_keys_roundtrip_through_metric_key() {
        for key in all_fusion_metric_keys() {
            let raw_key: MetricKey = key.into();
            assert_eq!(FusionMetricKey::try_from(raw_key), Ok(key));
        }
    }
}
