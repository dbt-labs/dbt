use dbt_telemetry::{
    ConnectionLimitWait, HookProcessed, Invocation, InvocationMetrics, NodeEvaluated, NodeEvent,
    NodeOutcome, NodeProcessed, NodeSkipReason, SourceFreshnessOutcome, has_node_warning,
    node_processed::NodeOutcomeDetail,
};
use dbt_tracing::{LogRecordInfo, SeverityNumber, SpanEndInfo};

use super::super::{
    data_provider::DataProvider,
    dbt_metrics::{
        FusionMetricKey, InvocationMetricKey, NodeSubOutcome, OutcomeCountsKey, OutcomeKind,
    },
    event_classifiers::is_exit_with_status_log,
    fs_error_log::get_log_message,
    layer::TelemetryMiddleware,
};

/// Middleware that aggregates telemetry metrics from span and log records.
pub struct TelemetryMetricAggregator;

impl TelemetryMiddleware for TelemetryMetricAggregator {
    fn on_span_end(
        &self,
        mut span: SpanEndInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        if let Some(invocation) = span.attributes.downcast_mut::<Invocation>() {
            // The Invocation span should be the root of our tracing span tree.
            // However, it may have a parent_span_id if one was explicitly provided via CLI
            // for OTEL trace correlation. In that case, span.parent_span_id should match
            // invocation.parent_span_id. If they differ, it means the span inherited a
            // parent from the tracing context, which shouldn't happen for Invocation spans.
            debug_assert!(
                span.parent_span_id.is_none() || span.parent_span_id == invocation.parent_span_id,
                "Expected Invocation span to be root, found inherited parent id: {:?} (invocation parent: {:?})",
                span.parent_span_id,
                invocation.parent_span_id
            );

            // Aggregate node outcome metrics into top-level metrics on invocation end
            let mut success = 0u64;
            let mut warning = 0u64;
            let mut error = 0u64;
            let mut skipped = 0u64;
            let mut reused = 0u64;
            let mut canceled = 0u64;
            let mut no_op = 0u64;

            for ((outcome, skip_reason, sub_outcome), count) in data_provider
                .get_all_metrics()
                .iter()
                .filter_map(|(key, count)| match FusionMetricKey::try_from(*key).ok() {
                    Some(FusionMetricKey::OutcomeCounts(outcome_key)) if *count > 0 => {
                        Some((outcome_key.into_parts(), *count))
                    }
                    _ => None,
                })
            {
                match outcome {
                    OutcomeKind::Node(outcome) => match outcome {
                        NodeOutcome::Success => match sub_outcome {
                            // TODO: FreshnessWarned/FreshnessFailed are intentionally left as
                            // success here. Source freshness outcomes use NodeOutcome::Success
                            // for all three results (pass/warn/fail), so freshness failures
                            // are currently undercounted in error/warn totals. Fixing this
                            // is out of scope for this PR — check with product/DX or wait
                            // for a user-reported issue before addressing.
                            Some(NodeSubOutcome::TestFailed) => error += count,
                            Some(NodeSubOutcome::TestWarned | NodeSubOutcome::NodeWarned) => {
                                warning += count
                            }
                            _ => success += count,
                        },
                        NodeOutcome::Error => error += count,
                        NodeOutcome::Skipped => {
                            if skip_reason == NodeSkipReason::Cached {
                                reused += count;
                            } else if skip_reason == NodeSkipReason::NoOp {
                                no_op += count;
                            } else {
                                skipped += count;
                            }
                        }
                        NodeOutcome::Canceled => canceled += count,
                        NodeOutcome::Unspecified => no_op += count,
                    },
                    OutcomeKind::Hook(hook_outcome) => match hook_outcome {
                        dbt_telemetry::HookOutcome::Success => success += count,
                        dbt_telemetry::HookOutcome::Error => error += count,
                        dbt_telemetry::HookOutcome::Canceled => canceled += count,
                        dbt_telemetry::HookOutcome::Unspecified => no_op += count,
                    },
                }
            }

            // Update aggregated metrics
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsSuccess),
                success,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsWarning),
                warning,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsError),
                error,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsReused),
                reused,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsSkipped),
                skipped,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsCanceled),
                canceled,
            );
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::NodeTotalsNoOp),
                no_op,
            );

            // Build node_type_counts hashmap from NodeCounts metrics
            #[allow(clippy::disallowed_types)]
            let node_type_counts: std::collections::HashMap<String, u64> = data_provider
                .get_all_metrics()
                .iter()
                .filter_map(|(key, count)| match FusionMetricKey::try_from(*key).ok() {
                    Some(FusionMetricKey::NodeCounts(node_type)) if *count > 0 => {
                        Some((node_type.as_static_ref().to_string(), *count))
                    }
                    Some(FusionMetricKey::HookCounts) if *count > 0 => {
                        Some(("hook".to_string(), *count))
                    }
                    _ => None,
                })
                .collect();

            // Build status_counts hashmap from aggregated node outcome totals
            #[allow(clippy::disallowed_types)]
            let mut status_counts = std::collections::HashMap::new();

            for (key, val) in &[
                ("success", success),
                ("warn", warning),
                ("error", error),
                ("reused", reused),
                ("skipped", skipped),
                ("canceled", canceled),
                ("no_op", no_op),
            ] {
                if *val == 0 {
                    continue;
                }

                status_counts.insert((*key).to_string(), *val);
            }

            // Store totals in invocation attributes
            invocation.metrics = Some(InvocationMetrics {
                total_errors: Some(data_provider.get_metric(FusionMetricKey::InvocationMetric(
                    InvocationMetricKey::TotalErrors,
                ))),
                total_warnings: Some(data_provider.get_metric(FusionMetricKey::InvocationMetric(
                    InvocationMetricKey::TotalWarnings,
                ))),
                autofix_suggestions: Some(data_provider.get_metric(
                    FusionMetricKey::InvocationMetric(InvocationMetricKey::AutoFixSuggestions),
                )),
                node_type_counts,
                status_counts,
            });
        }

        if span.attributes.is::<ConnectionLimitWait>() {
            let wait_ms = span
                .end_time_unix_nano
                .duration_since(span.start_time_unix_nano)
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64;

            data_provider.with_ancestor_attrs_mut::<NodeEvaluated>(|node| {
                node.idle_time_ms = Some(
                    node.idle_time_ms
                        .unwrap_or_default()
                        .saturating_add(wait_ms),
                );
            });
        }

        // Count node processed spans
        if let Some(attrs) = span.attributes.downcast_ref::<NodeProcessed>()
            && attrs.in_selection
        {
            let sub_outcome: Option<NodeSubOutcome> =
                if let Some(NodeOutcomeDetail::NodeTestDetail(ted)) = &attrs.node_outcome_detail {
                    NodeSubOutcome::from_test_outcome(ted.test_outcome())
                } else if let Some(NodeOutcomeDetail::NodeFreshnessOutcome(fd)) =
                    &attrs.node_outcome_detail
                {
                    match fd.node_freshness_outcome() {
                        SourceFreshnessOutcome::OutcomeWarned => {
                            Some(NodeSubOutcome::FreshnessWarned)
                        }
                        SourceFreshnessOutcome::OutcomeFailed => {
                            Some(NodeSubOutcome::FreshnessFailed)
                        }
                        _ => None,
                    }
                } else if has_node_warning(NodeEvent::Processed(attrs)) {
                    Some(NodeSubOutcome::NodeWarned)
                } else {
                    None
                };
            let key = OutcomeCountsKey::new(
                OutcomeKind::Node(attrs.node_outcome()),
                attrs.node_skip_reason(),
                sub_outcome,
            );
            data_provider.increment_metric(FusionMetricKey::OutcomeCounts(key), 1);
        }

        // Count hook processed spans - hooks are always counted regardless of selection
        if let Some(hook_attrs) = span.attributes.downcast_ref::<HookProcessed>() {
            // Count the hook
            data_provider.increment_metric(FusionMetricKey::HookCounts, 1);

            let key = OutcomeCountsKey::new(
                OutcomeKind::Hook(hook_attrs.hook_outcome()),
                NodeSkipReason::Unspecified,
                None,
            );
            data_provider.increment_metric(FusionMetricKey::OutcomeCounts(key), 1);
        }

        Some(span)
    }

    fn on_log_record(
        &self,
        log_record: LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        // ExitWithStatus is a pseudo error used only to short-circuit execution, so we
        // filter it from dbt-facing output
        if is_exit_with_status_log(&log_record) {
            return Some(log_record);
        }

        if get_log_message(&log_record.attributes).is_some() {
            match log_record.severity_number {
                SeverityNumber::Error => {
                    data_provider.increment_metric(
                        FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalErrors),
                        1,
                    );
                }
                SeverityNumber::Warn => {
                    data_provider.increment_metric(
                        FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalWarnings),
                        1,
                    );
                }
                _ => {}
            }
        }

        Some(log_record)
    }
}
