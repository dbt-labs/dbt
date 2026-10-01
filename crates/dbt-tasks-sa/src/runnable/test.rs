use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::mpsc;
use std::time::SystemTime;

use crate::materialize::materialize_test;
use dbt_common::FsResult;
use dbt_common::constants::{DBT_AGGREGATED_GENERIC_TEST_CONTEXT, RUNNING};
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::tracing::dbt_metrics::{FusionMetricKey, InvocationMetricKey};
use dbt_common::tracing::metrics::increment_metric;
use dbt_common::tracing::span_info::{
    find_and_record_span_status_from_attrs, record_span_status_from_attrs,
};
use dbt_common::warn_error_options::{
    SupportedLegacyWarnError, WarnErrorDecision, WarnErrorOptions,
};
use dbt_common::{ErrorCode, fs_err, unexpected_fs_err};
use dbt_jinja_utils::utils::add_task_context;
use dbt_pretty_table::{make_column_names, pretty_data_table};
use dbt_scheduler::instructions::SqlInstruction;
use dbt_schemas::schemas::DbtTest;
use dbt_schemas::schemas::common::Severity;
use dbt_schemas::schemas::{InternalDbtNode, InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::{BlockingTaskCtx, TaskRunnerCtx};
use dbt_tasks_core::pretty_table::from_pretty_table_error;
use dbt_tasks_core::run_cache::run_cache_service::CachedTestExecutionResult;
use dbt_tasks_core::span_manager::SpanTreeRequest;
use dbt_tasks_core::task::TaskResult;
use dbt_tasks_core::task::{AggregatedNodeGroup, TP, Task, TaskOp};
use dbt_tasks_core::task_spans::create_task_span_for_node;
use dbt_tasks_core::test_aggregation::GenericTestGroup;
use dbt_tasks_core::visitor::SkipReason;
use dbt_telemetry::{
    ExecutionPhase, NodeEvaluated, NodeOutcome, NodeOutcomeDetail, NodeSkipReason, NodeType,
    TestEvaluationDetail, TestOutcome,
};

use minijinja::Value as MinijinjaValue;
use minijinja::constants::{TARGET_PACKAGE_NAME, TARGET_UNIQUE_ID};
use parking_lot::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestExecutionStatus {
    Passed,
    Warned,
    Failed,
}

impl TestExecutionStatus {
    pub fn node_status(self) -> NodeStatus {
        match self {
            TestExecutionStatus::Passed => NodeStatus::TestPassed,
            TestExecutionStatus::Warned => NodeStatus::TestWarned,
            TestExecutionStatus::Failed => NodeStatus::Errored,
        }
    }

    fn test_outcome(self) -> TestOutcome {
        match self {
            TestExecutionStatus::Passed => TestOutcome::Passed,
            TestExecutionStatus::Warned => TestOutcome::Warned,
            TestExecutionStatus::Failed => TestOutcome::Failed,
        }
    }

    pub fn metric_key(self) -> Option<InvocationMetricKey> {
        match self {
            TestExecutionStatus::Passed => None,
            TestExecutionStatus::Warned => Some(InvocationMetricKey::TotalWarnings),
            TestExecutionStatus::Failed => Some(InvocationMetricKey::TotalErrors),
        }
    }
}

#[derive(Clone, Debug)]
pub struct TestReportedResult {
    pub failures: usize,
    pub status: TestExecutionStatus,
    pub diff: Option<String>,
    pub execution_result: Option<CachedTestExecutionResult>,
}

impl TestReportedResult {
    pub fn node_status(&self) -> NodeStatus {
        self.status.node_status()
    }

    pub fn test_outcome(&self) -> TestOutcome {
        self.status.test_outcome()
    }
}

pub fn status_with_warn_error_overrides(
    status: TestExecutionStatus,
    warn_error_options: &WarnErrorOptions,
) -> TestExecutionStatus {
    match (
        status,
        warn_error_options.decision_for_supported_legacy(SupportedLegacyWarnError::LogTestResult),
    ) {
        (TestExecutionStatus::Warned, WarnErrorDecision::UpgradeToError) => {
            TestExecutionStatus::Failed
        }
        (TestExecutionStatus::Warned, WarnErrorDecision::Silence) => TestExecutionStatus::Passed,
        _ => status,
    }
}

pub fn reported_test_verdict_from_components(
    severity: Option<&Severity>,
    should_warn: bool,
    should_error: bool,
) -> TestExecutionStatus {
    let severity = severity.cloned().unwrap_or_default();
    if matches!(severity, Severity::Error) && should_error {
        TestExecutionStatus::Failed
    } else if should_warn {
        TestExecutionStatus::Warned
    } else {
        TestExecutionStatus::Passed
    }
}

fn reported_test_verdict_from_materialize_result(
    severity: Option<&Severity>,
    test_result: &crate::materialize::TestResult,
) -> TestExecutionStatus {
    reported_test_verdict_from_components(
        severity,
        test_result.should_warn,
        test_result.should_error,
    )
}

pub fn record_test_metric(status: TestExecutionStatus) {
    if let Some(metric_key) = status.metric_key() {
        increment_metric(FusionMetricKey::InvocationMetric(metric_key), 1);
    }
}

pub fn insert_test_run_stat(
    ctx: &BlockingTaskCtx,
    unique_id: String,
    start: SystemTime,
    failures: usize,
    status: TestExecutionStatus,
) {
    let thread_id = ctx.thread_id;
    ctx.inner.run_stats.insert(
        unique_id.clone(),
        Stat::new(
            unique_id,
            start,
            Some(failures),
            status.node_status(),
            None,
            thread_id,
        ),
    );
}

pub(super) fn record_test_span_with_detail(
    result: &TestReportedResult,
    skip_reason: Option<NodeSkipReason>,
    store_failures: Option<bool>,
    statically_checked: Option<bool>,
) {
    let test_outcome = result.test_outcome();
    let failures = result.failures.min(i32::MAX as usize) as i32;
    let diff = result.diff.clone();

    find_and_record_span_status_from_attrs(move |attrs: &mut NodeEvaluated| {
        attrs.set_node_outcome(NodeOutcome::Success);
        if let Some(skip_reason) = skip_reason {
            attrs.set_node_skip_reason(skip_reason);
        }
        attrs.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(
                test_outcome,
                failures,
                diff,
                store_failures,
                statically_checked,
                None,
            ),
        ))
    });
}

// NOTE: AggregatedTest{Render,Analyze}Task has an inner {Render,Analyze}Task for code reuse.
// However, this task can not resue RunRemoteTask because of the side effects (e.g. stats) there.
//
// but render/analyze hold a inner task is because we want to reuse the functions on Renderable and Analyzable
pub struct AggregatedTestRunRemoteTask {
    unique_id: String,
    group: Arc<GenericTestGroup>,
    member_tests: Vec<Arc<DbtTest>>,
    result_receiver: Mutex<Option<mpsc::Receiver<TaskResult>>>,
}

impl AggregatedTestRunRemoteTask {
    pub fn new(
        unique_id: String,
        group: Arc<GenericTestGroup>,
        member_tests: Vec<Arc<DbtTest>>,
        result_receiver: Option<mpsc::Receiver<TaskResult>>,
    ) -> Self {
        Self {
            unique_id,
            group,
            member_tests,
            result_receiver: Mutex::new(result_receiver),
        }
    }

    pub fn from_generic_test_group(
        group: &Arc<GenericTestGroup>,
        result_receiver: Option<mpsc::Receiver<TaskResult>>,
    ) -> Self {
        Self::new(
            group.unique_id.clone(),
            Arc::clone(group),
            group.member_tests.clone(),
            result_receiver,
        )
    }

    fn build_base_context(&self, ctx: &TaskRunnerCtx) -> BTreeMap<String, MinijinjaValue> {
        let mut base_context = ctx.inner.base_context.clone();
        base_context.insert(
            TARGET_PACKAGE_NAME.to_string(),
            MinijinjaValue::from(self.group.aggregated_test.common().package_name.clone()),
        );
        base_context.insert(
            TARGET_UNIQUE_ID.to_string(),
            MinijinjaValue::from(self.group.aggregated_test.common().unique_id.clone()),
        );
        base_context.insert(
            DBT_AGGREGATED_GENERIC_TEST_CONTEXT.to_string(),
            MinijinjaValue::from(Vec::<String>::new()),
        );
        base_context
    }

    async fn receive_sql_instruction(&self) -> FsResult<SqlInstruction> {
        let receiver = self.result_receiver.lock().take().ok_or_else(|| {
            fs_err!(
                ErrorCode::Unexpected,
                "Result receiver is not set for aggregated test {}",
                self.unique_id
            )
        })?;

        let task_result = match receiver.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => {
                return Err(fs_err!(
                    ErrorCode::Generic,
                    "Failed to receive run result for '{}'. Empty Channel.",
                    self.unique_id
                ));
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                return Err(fs_err!(
                    ErrorCode::Generic,
                    "Failed to receive run result for '{}'. Channel Disconnected.",
                    self.unique_id
                ));
            }
        };

        Ok(task_result
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::Generic,
                    "Task result is not set for aggregated test {}",
                    self.unique_id
                )
            })?
            .sql_instruction)
    }

    fn get_member_test_results(
        &self,
        test_results: &[crate::materialize::TestResult],
        warn_error_options: &WarnErrorOptions,
    ) -> (Vec<(String, TestReportedResult)>, TestExecutionStatus) {
        let mut column_results = HashMap::new();
        for result in test_results {
            if let Some(column_name) = &result.column_name {
                let normalized =
                    dbt_tasks_core::test_aggregation::normalize_column_name(column_name);
                column_results.insert(normalized, result);
            }
        }

        // Ordered like `self.group.tests` so member result lines are emitted deterministically.
        let mut member_results = Vec::with_capacity(self.group.tests.len());
        let mut worst_status = TestExecutionStatus::Passed;

        for test in &self.group.tests {
            let column_name =
                dbt_tasks_core::test_aggregation::normalize_column_name(&test.column_name);
            let column_result = column_results.get(&column_name).copied();

            let status = match column_result {
                Some(result) => {
                    reported_test_verdict_from_materialize_result(test.severity.as_ref(), result)
                }
                None => TestExecutionStatus::Passed,
            };
            let status = status_with_warn_error_overrides(status, warn_error_options);
            worst_status = worst_status.max(status);

            let failures = column_result
                .and_then(|result| usize::try_from(result.failures.max(0)).ok())
                .unwrap_or(0);

            member_results.push((
                test.unique_id.clone(),
                TestReportedResult {
                    failures,
                    status,
                    diff: None,
                    execution_result: Some(CachedTestExecutionResult {
                        failures: failures as i64,
                        should_warn: column_result
                            .map(|result| result.should_warn)
                            .unwrap_or(false),
                        should_error: column_result
                            .map(|result| result.should_error)
                            .unwrap_or(false),
                    }),
                },
            ));
        }

        (member_results, worst_status)
    }

    async fn run_task_inner(
        &self,
        ctx: &mut TaskRunnerCtx,
        start: SystemTime,
        span_by_id: &mut HashMap<String, (Arc<DbtTest>, tracing::Span)>,
    ) -> FsResult<NodeStatus> {
        if let Some(reporter) = ctx.inner.arg.io.status_reporter.as_ref() {
            for test in &self.group.member_tests {
                // Status line cites the YAML where the test is defined; runtime errors
                // (emitted from materialize_test) carry their own phase-accurate paths.
                let display_path = test
                    .get_node_path(
                        NodePathKind::Definition,
                        ctx.inner.arg.io.in_dir.as_path(),
                        ctx.inner.arg.io.out_dir.as_path(),
                    )
                    .display()
                    .to_string();
                reporter.show_progress(RUNNING, display_path.as_ref(), None);
            }
        }

        let base_context = self.build_base_context(ctx);
        let sql_instruction = self.receive_sql_instruction().await?;

        let adapter_type = self.group.aggregated_test.node_adapter();
        let test = self.group.aggregated_test.clone();
        let ctx_inner = ctx.clone();

        let (test_results, failing_rows_opt, main_response) =
            TaskOp::Blocking(Box::new(move || {
                materialize_test(
                    &sql_instruction.sql,
                    &test,
                    ctx_inner.generic_test_relationships(),
                    adapter_type,
                    ctx_inner.runtime_config(),
                    &ctx_inner.inner.materialization_resolver,
                    ctx_inner.env.clone(),
                    &base_context,
                    &ctx_inner.inner.arg.io,
                )
            }))
            .run()
            .await??;

        let (member_results, worst_status) =
            self.get_member_test_results(&test_results, &ctx.inner.arg.warn_error_options);
        let total_failures: usize = member_results.iter().map(|(_, r)| r.failures).sum();

        // TODO(pc) recreate per-test diff
        let diff_output = if let Some(batch) = failing_rows_opt {
            if matches!(
                worst_status,
                TestExecutionStatus::Failed | TestExecutionStatus::Warned
            ) && total_failures > 0
            {
                let column_names = make_column_names(batch.schema().as_ref());
                let table = pretty_data_table(
                    "",
                    "",
                    &column_names,
                    &[batch],
                    dbt_pretty_table::DisplayFormat::Table,
                    ctx.inner.arg.limit,
                    true,
                    Some(total_failures),
                )
                .map_err(from_pretty_table_error)?;
                Some(table)
            } else {
                None
            }
        } else {
            None
        };

        increment_metric(
            FusionMetricKey::InvocationMetric(InvocationMetricKey::AggregatedTestQueries),
            1,
        );
        increment_metric(
            FusionMetricKey::InvocationMetric(InvocationMetricKey::AggregatedTests),
            member_results.len() as u64,
        );

        for (unique_id, result) in &member_results {
            // Members share the aggregated query, so they share its main response.
            if let Some(main_response) = &main_response {
                ctx.inner
                    .main_adapter_responses
                    .insert(unique_id.clone(), main_response.clone());
            }
            let (_, span) = span_by_id
                .remove(unique_id)
                .expect("span should exist for unique_id");
            self.report_member_result(ctx, span, start, unique_id, result, diff_output.as_ref())
                .await?;
        }

        Ok(worst_status.node_status())
    }

    async fn report_member_result(
        &self,
        ctx: &TaskRunnerCtx,
        span: tracing::Span,
        _start: SystemTime,
        unique_id: &str,
        result: &TestReportedResult,
        diff_output: Option<&String>,
    ) -> FsResult<()> {
        let diff = result.diff.clone().or_else(|| diff_output.cloned());
        let group_unique_id = self.group.unique_id.clone();
        record_test_metric(result.status);

        record_span_status_from_attrs(&span, move |attrs| {
            if let Some(ev) = attrs.downcast_mut::<NodeEvaluated>() {
                ev.set_node_outcome(NodeOutcome::Success);
                ev.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
                    TestEvaluationDetail::new(
                        result.test_outcome(),
                        result.failures.min(i32::MAX as usize) as i32,
                        diff,
                        None,
                        None,
                        Some(group_unique_id),
                    ),
                ));
            }
        });

        insert_test_run_stat(
            &ctx.blocking_ctx(),
            unique_id.to_string(),
            SystemTime::now(),
            result.failures,
            result.status,
        );
        if let Some(execution_result) = result.execution_result {
            ctx.inner
                .data_test_execution_results
                .insert(unique_id.to_string(), execution_result);
        }

        ctx.inner
            .span_manager()
            .handle_task_finished(span, &Ok(result.node_status()));

        Ok(())
    }
}

impl Task for AggregatedTestRunRemoteTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let start = SystemTime::now();
            let mut spans_by_id: HashMap<_, _> = self
                .member_tests
                .iter()
                .cloned()
                .map(|node| {
                    let span = create_task_span_for_node(
                        node.as_ref(),
                        ExecutionPhase::Run,
                        ctx.inner.span_manager().as_ref(),
                        &ctx.inner.arg.io.in_dir,
                        &ctx.inner.arg.io.out_dir,
                    )?;
                    Ok((node.common().unique_id.clone(), (node, span)))
                })
                .collect::<FsResult<_>>()?;

            let status = self.run_task_inner(ctx, start, &mut spans_by_id).await;

            if status.is_err() {
                for (_, span) in spans_by_id.into_values() {
                    ctx.inner.span_manager().handle_task_finished(span, &status);
                }
            }

            status
        })
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Test
    }

    fn task_type(&self) -> &str {
        "run"
    }

    fn work_node_id(&self) -> &str {
        &self.unique_id
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        self.member_tests
            .iter()
            .cloned()
            .map(|node| node as Arc<dyn InternalDbtNodeAttributes>)
            .collect()
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Run)
    }

    fn aggregated_node_group(&self) -> Option<AggregatedNodeGroup> {
        Some(AggregatedNodeGroup {
            unique_id: self.group.unique_id.clone(),
            macro_name: self.group.macro_name.clone(),
            attached_node: self.group.attached_node.clone(),
        })
    }

    fn telemetry_request(
        &self,
        _in_dir: &Path,
        _out_dir: &Path,
        _skip_reason: Option<&SkipReason>,
    ) -> SpanTreeRequest<FsResult<NodeStatus>, SkipReason> {
        SpanTreeRequest::use_current()
    }
}

pub fn execute_test_remote(
    test: &DbtTest,
    ctx: &TaskRunnerCtx,
    task_result: &TaskResult,
) -> FsResult<NodeStatus> {
    let start = SystemTime::now();
    let unique_id = &test.common().unique_id;
    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, test.common(), &ctx.thread_id);

    let sql_instruction = match &task_result.lp_instruction {
        Some(_) => {
            return Err(unexpected_fs_err!(
                "Test {} received LP instruction but Execute::Remote expects SQL",
                unique_id
            ));
        }
        None => &task_result.sql_instruction,
    };

    let result = execute_test_remote_inner(test, ctx, sql_instruction, &base_context)?;

    // Process test result (metrics, stats, telemetry)
    process_test_result(test, &ctx.blocking_ctx(), start, result)
}

/// Execute test via traditional warehouse/remote execution
fn execute_test_remote_inner(
    test: &DbtTest,
    ctx: &TaskRunnerCtx,
    sql_instruction: &SqlInstruction,
    base_context: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<TestReportedResult> {
    // Any render/execution error (including database errors) surfaces as a
    // hard error, independent of severity — matching dbt Core and the sidecar
    // path. The runner framework turns the returned Err into NodeStatus::Errored
    // and records it. Severity is only consulted for successfully-returned
    // results below.
    let (test_results, failing_rows_opt, main_response) = materialize_test(
        &sql_instruction.sql,
        test,
        ctx.generic_test_relationships(),
        test.node_adapter(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        base_context,
        &ctx.inner.arg.io,
    )?;
    if let Some(main_response) = main_response {
        ctx.inner
            .main_adapter_responses
            .insert(test.__common_attr__.unique_id.clone(), main_response);
    }
    let test_result = test_results
        .into_iter()
        .next()
        .expect("materialize_test should return a test result");

    let status = reported_test_verdict_from_materialize_result(
        test.deprecated_config.severity.as_ref(),
        &test_result,
    );

    // If test failed or warned with failures, create verdict table for deferred printing
    let diff = if matches!(
        status,
        TestExecutionStatus::Warned | TestExecutionStatus::Failed
    ) && test_result.failures > 0
        && let Some(batch) = failing_rows_opt
    {
        let column_names = make_column_names(batch.schema().as_ref());
        let table = pretty_data_table(
            "",
            "",
            &column_names,
            &[batch],
            dbt_pretty_table::DisplayFormat::Table,
            ctx.inner.arg.limit,
            true,
            Some(test_result.failures as usize),
        )
        .map_err(from_pretty_table_error)?;

        Some(table)
    } else {
        None
    };

    Ok(TestReportedResult {
        failures: test_result.failures as usize,
        status: status_with_warn_error_overrides(status, &ctx.inner.arg.warn_error_options),
        diff,
        execution_result: Some(CachedTestExecutionResult {
            failures: test_result.failures,
            should_warn: test_result.should_warn,
            should_error: test_result.should_error,
        }),
    })
}

/// Process test execution result and record metrics/stats
pub fn process_test_result(
    test: &DbtTest,
    ctx: &BlockingTaskCtx,
    start: SystemTime,
    result: TestReportedResult,
) -> FsResult<NodeStatus> {
    let unique_id = &test.common().unique_id;
    record_test_metric(result.status);

    let node_status = result.node_status();

    record_test_span_with_detail(&result, None, test.deprecated_config.store_failures, None);

    insert_test_run_stat(
        ctx,
        unique_id.clone(),
        start,
        result.failures,
        result.status,
    );
    if let Some(execution_result) = result.execution_result {
        ctx.inner
            .data_test_execution_results
            .insert(unique_id.clone(), execution_result);
    }

    Ok(node_status)
}

pub fn process_statically_checked_test_result(
    test: &DbtTest,
    ctx: &TaskRunnerCtx,
    start: SystemTime,
) -> NodeStatus {
    let result = TestReportedResult {
        failures: 0,
        status: TestExecutionStatus::Passed,
        diff: None,
        execution_result: None,
    };
    record_test_span_with_detail(&result, None, None, Some(true));
    increment_metric(
        FusionMetricKey::InvocationMetric(InvocationMetricKey::StaticallyCheckedTests),
        1,
    );

    let unique_id = &test.common().unique_id;
    ctx.inner.run_stats.insert(
        unique_id.clone(),
        Stat::new(
            unique_id.clone(),
            start,
            Some(0),
            NodeStatus::StaticallyCheckedDataTest,
            None,
            ctx.thread_id,
        ),
    );

    NodeStatus::StaticallyCheckedDataTest
}
