use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::materialize::materialize_unit_test_fast_pass;
use dbt_common::FsResult;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_metrics::{FusionMetricKey, InvocationMetricKey};
use dbt_common::tracing::metrics::increment_metric;
use dbt_common::tracing::span_info::{
    find_and_record_span_status_from_attrs, find_and_record_span_status_with_attrs,
};
use dbt_common::{ErrorCode, fs_err};
use dbt_jinja_utils::utils::add_task_context;
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::{DbtUnitTest, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::task::TaskResult;
use dbt_telemetry::{
    NodeErrorType, NodeEvaluated, NodeOutcome, NodeOutcomeDetail, NodeSkipReason,
    TestEvaluationDetail, TestOutcome,
};

use minijinja::Value;

pub fn execute_unit_test_remote(
    unit_test: &DbtUnitTest,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<(NodeStatus, Option<UnitTestExecutionResult>)> {
    let start = SystemTime::now();
    let mut base_context = ctx.inner.base_context.clone();
    let unique_id = &unit_test.common().unique_id;

    add_task_context(&mut base_context, unit_test.common(), &ctx.thread_id);

    // return early if the node is a ephemeral unit_test since we don't need to execute it.
    if unit_test.materialized() == DbtMaterialization::Ephemeral {
        // Record as no-op skip in span
        find_and_record_span_status_from_attrs(|attrs: &mut NodeEvaluated| {
            attrs.set_node_outcome(NodeOutcome::Skipped);
            attrs.set_node_skip_reason(NodeSkipReason::NoOp);
        });

        return Ok((NodeStatus::Succeeded, None));
    }

    let sql_instruction = match &task_result.lp_instruction {
        Some(_) => {
            return Err(fs_err!(
                ErrorCode::Unexpected,
                "Unit test {} received LP instruction but Execute::Remote expects SQL",
                unique_id
            ));
        }
        None => &task_result.sql_instruction,
    };

    let result = execute_unit_test_remote_inner(unit_test, ctx, sql_instruction, &base_context)?;

    // Process unit test result (metrics, stats, telemetry)
    Ok((
        process_unit_test_result(unit_test, ctx, start, &result)?,
        Some(result),
    ))
}

/// Result from executing a unit test (remote or sidecar)
#[derive(Debug, PartialEq)]
pub struct UnitTestExecutionResult {
    pub passed: bool,
    pub diff_num_rows: usize,
    pub diff: String,
}

/// Execute unit test via traditional warehouse/remote execution
fn execute_unit_test_remote_inner(
    unit_test: &DbtUnitTest,
    ctx: &TaskRunnerCtx,
    sql_instruction: &SqlInstruction,
    base_context: &BTreeMap<String, Value>,
) -> FsResult<UnitTestExecutionResult> {
    let unique_id = &unit_test.common().unique_id;

    match materialize_unit_test_fast_pass(
        &sql_instruction.sql,
        unit_test,
        // A unit test carries the tested model's adapter (`resolve_unit_tests`
        // sets `adapter: model_adapter.unwrap_or(default)`), so a test on a
        // non-default model must not materialize as the default.
        unit_test.node_adapter(),
        ctx.runtime_config(),
        ctx.env.clone(),
        base_context,
        &ctx.inner.arg.io,
    ) {
        Ok((is_test_succeed, diff_num_rows, diff)) => Ok(UnitTestExecutionResult {
            passed: is_test_succeed,
            diff_num_rows,
            diff,
        }),
        Err(e) => {
            // Collect the error and continue with other tests
            find_and_record_span_status_with_attrs(
                |attrs: &mut NodeEvaluated| {
                    attrs.set_node_outcome(NodeOutcome::Error);
                    attrs.set_node_error_type(if e.code == ErrorCode::JinjaError {
                        NodeErrorType::User
                    } else {
                        NodeErrorType::Internal
                    });
                },
                Some(e.to_string().as_str()),
            );

            // Count test errors towards global error metric
            increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalErrors),
                1,
            );

            let node_status = NodeStatus::Errored;
            let thread_id = ctx.thread_id;
            let start = SystemTime::now();

            // Add stats for this test
            ctx.inner.run_stats.insert(
                unique_id.to_string(),
                Stat::new(
                    unique_id.to_string(),
                    start,
                    None,
                    node_status,
                    Some(e.to_string()),
                    thread_id,
                ),
            );
            Err(e)
        }
    }
}

/// Process unit test result (metrics, stats, telemetry)
pub fn process_unit_test_result(
    unit_test: &DbtUnitTest,
    ctx: &TaskRunnerCtx,
    start: SystemTime,
    result: &UnitTestExecutionResult,
) -> FsResult<NodeStatus> {
    let unique_id = &unit_test.common().unique_id;

    if result.passed {
        let node_status = NodeStatus::TestPassed;

        // Record success in span
        find_and_record_span_status_from_attrs(|attrs: &mut NodeEvaluated| {
            attrs.set_node_outcome(NodeOutcome::Success);

            attrs.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
                TestEvaluationDetail::new(TestOutcome::Passed, 0, None, None, None, None),
            ));
        });
        let thread_id = ctx.thread_id;

        // Add stats for this test
        ctx.inner.run_stats.insert(
            unique_id.to_string(),
            Stat::new(
                unique_id.to_string(),
                start,
                None,
                node_status.clone(),
                None,
                thread_id,
            ),
        );
        Ok(node_status)
    } else {
        let node_status = NodeStatus::Errored;
        let mut diff = Some(result.diff.clone()); // Dance due to FnMut closure

        // Collect the test failure details
        find_and_record_span_status_from_attrs(|attrs: &mut NodeEvaluated| {
            // Node succeeded but test failed
            attrs.set_node_outcome(NodeOutcome::Success);

            attrs.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
                TestEvaluationDetail::new(
                    TestOutcome::Failed,
                    result.diff_num_rows.try_into().unwrap_or(i32::MAX),
                    diff.take(),
                    None,
                    None,
                    None,
                ),
            ))
        });

        // Count test errors towards global error metric
        increment_metric(
            FusionMetricKey::InvocationMetric(InvocationMetricKey::TotalErrors),
            1,
        );
        let thread_id = ctx.thread_id;

        // Add stats for this test
        ctx.inner.run_stats.insert(
            unique_id.to_string(),
            Stat::new(
                unique_id.to_string(),
                start,
                None,
                node_status.clone(),
                None,
                thread_id,
            ),
        );
        Ok(node_status)
    }
}
