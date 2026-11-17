//! Per-phase node evaluation lines and microbatches.

use dbt_telemetry::{
    ExecutionPhase, MicrobatchBatchSize, MicrobatchEvaluated, NodeMaterialization, NodeOutcome,
    NodeOutcomeDetail, NodeSkipReason, NodeType, TestEvaluationDetail, TestOutcome,
};
use indoc::{formatdoc, indoc};

use super::events::{evaluated, node, node_group, processed};
use super::support::{
    Case, Clock, DBT_LOG, DBT_LOG_DEBUG, Sink, TERMINAL, TERMINAL_QUIET, TEXT_DEBUG, Terminal,
    all_variants,
};
use crate::{
    io_args::{FsCommand, ShowOptions},
    tracing::emit::{create_debug_span, create_info_span},
};

pub(super) fn cases(cases: &mut Vec<Case>) {
    evaluation_by_phase(cases);
    evaluation_by_outcome(cases);
    legacy_start_lines(cases);
    microbatches(cases);
}

/// Emits `node` as a finished `NodeEvaluated` span, at the level the scheduler uses.
fn evaluate(clock: &Clock, node: dbt_telemetry::NodeEvaluated) {
    let span = match node.phase() {
        ExecutionPhase::Run | ExecutionPhase::Compare => create_info_span(node),
        _ => create_debug_span(node),
    };
    clock.advance_ms(420);
    drop(span);
}

/// Text output that also shows the start lines of the render and run phases.
const TEXT_PROGRESS_SETUP: Terminal = Terminal::TEXT.show(&[
    ShowOptions::Progress,
    ShowOptions::ProgressRender,
    ShowOptions::ProgressRun,
    ShowOptions::Completed,
]);
const TEXT_PROGRESS: Sink = Sink::Terminal(
    "text --show progress-render,progress-run",
    TEXT_PROGRESS_SETUP,
);

fn evaluation_by_phase(cases: &mut Vec<Case>) {
    // Phases without a legacy start line are logged the same way at debug level, only
    // their description differs.
    let debug_only = |case: Case, description: &str| {
        case.expect(
            DBT_LOG_DEBUG,
            &formatdoc! {r#"
                03:04:05.678901 [debug]: Started {description} model analytics.customers
                03:04:06.098901 [debug]: Finished {description} [  0.42s] model analytics.customers [success]
            "#},
        )
        .expect(
            TEXT_DEBUG,
            &formatdoc! {r#"
                Started {description} model <cyan+bold>analytics.</><blue+bold>customers</>
                Finished {description} [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
            "#},
        )
    };

    for phase in all_variants!(ExecutionPhase {
        Unspecified,
        Clean,
        LoadProject,
        Parse,
        Schedule,
        Compare,
        InitAdapter,
        DeferHydration,
        SchemaHydration,
        TaskGraphBuild,
        NodeCacheHydration,
        Render,
        Analyze,
        Run,
        FreshnessAnalysis,
        Lineage,
        Debug,
        OnRunStart,
        OnRunEnd,
    }) {
        let model = evaluated(&node(NodeType::Model), phase);
        let case = Case::new(format!("{phase:?}")).run(move |clock| evaluate(clock, model.clone()));
        cases.push(match phase {
            ExecutionPhase::Unspecified => debug_only(case, "unknown phase"),
            ExecutionPhase::Clean => debug_only(case, "cleaning"),
            ExecutionPhase::LoadProject => debug_only(case, "loading project"),
            ExecutionPhase::Parse => debug_only(case, "parsing"),
            ExecutionPhase::Schedule => debug_only(case, "scheduling"),
            // Phases with a legacy start line
            ExecutionPhase::Compare => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Started comparing model analytics.customers
                        03:04:06.098901 [info ]: Finished comparing [  0.42s] model analytics.customers [success]
                    "#},
                )
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Started comparing model analytics.customers
                        03:04:06.098901 [info ]: Finished comparing [  0.42s] model analytics.customers [success]
                    "#},
                )
                .expect_no_output(TERMINAL)
                .expect(TEXT_PROGRESS, " Comparing models/customers.sql\n")
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                         Comparing models/customers.sql
                        Finished comparing [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
                    "#},
                ),
            ExecutionPhase::InitAdapter => debug_only(case, "initializing adapter"),
            ExecutionPhase::DeferHydration => debug_only(case, "deferring state"),
            ExecutionPhase::SchemaHydration => debug_only(case, "downloading schemas"),
            ExecutionPhase::TaskGraphBuild => debug_only(case, "building task graph"),
            ExecutionPhase::NodeCacheHydration => debug_only(case, "hydrating"),
            ExecutionPhase::Render => case
                .expect_no_output(DBT_LOG)
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {r#"
                        03:04:05.678901 [debug]: Started rendering model analytics.customers
                        03:04:06.098901 [debug]: Finished rendering [  0.42s] model analytics.customers [success]
                    "#},
                )
                .expect_no_output(TERMINAL)
                .expect(TEXT_PROGRESS, " Rendering models/customers.sql\n")
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                         Rendering models/customers.sql
                        Finished rendering [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
                    "#},
                ),
            ExecutionPhase::Analyze => debug_only(case, "analyzing"),
            ExecutionPhase::Run => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Started running model analytics.customers
                        03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [success]
                    "#},
                )
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Started running model analytics.customers
                        03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [success]
                    "#},
                )
                .expect_no_output(TERMINAL)
                .expect(TEXT_PROGRESS, "   Running models/customers.sql\n")
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                           Running models/customers.sql
                        Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
                    "#},
                ),
            ExecutionPhase::FreshnessAnalysis => debug_only(case, "analyzing freshness"),
            ExecutionPhase::Lineage => debug_only(case, "generating lineage"),
            ExecutionPhase::Debug => debug_only(case, "debugging"),
            ExecutionPhase::OnRunStart => debug_only(case, "on-run-start hooks"),
            ExecutionPhase::OnRunEnd => debug_only(case, "on-run-end hooks"),
        });
    }
}

fn evaluation_by_outcome(cases: &mut Vec<Case>) {
    for outcome in all_variants!(NodeOutcome {
        Unspecified,
        Success,
        Error,
        Canceled,
        Skipped,
    }) {
        let (dbt_log, debug) = match outcome {
            NodeOutcome::Unspecified => (
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [no-op]
                "#},
                Some(indoc! {r#"
                       Running models/customers.sql
                    Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<yellow+bold>no-op</>]
                "#}),
            ),
            NodeOutcome::Success => (
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [success]
                "#},
                Some(indoc! {r#"
                       Running models/customers.sql
                    Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
                "#}),
            ),
            NodeOutcome::Error => (
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [error]
                "#},
                Some(indoc! {r#"
                       Running models/customers.sql
                    Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<red+bold>error</>]
                "#}),
            ),
            NodeOutcome::Canceled => (
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [cancelled]
                "#},
                Some(indoc! {r#"
                       Running models/customers.sql
                    Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<yellow+bold>cancelled</>]
                "#}),
            ),
            NodeOutcome::Skipped => (
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [skipped]
                "#},
                None,
            ),
        };
        let mut model = evaluated(&node(NodeType::Model), ExecutionPhase::Run);
        model.set_node_outcome(outcome);
        cases.push(
            Case::new(format!("outcome {outcome:?}"))
                .run(move |clock| evaluate(clock, model.clone()))
                .expect(DBT_LOG, dbt_log)
                .expect_or_no_output(TEXT_DEBUG, debug),
        );
    }

    for (outcome, skip_reason, dbt_log, debug) in [
        (
            NodeOutcome::Skipped,
            NodeSkipReason::Upstream,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [skipped]
            "#},
            None,
        ),
        (
            NodeOutcome::Success,
            NodeSkipReason::Upstream,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [success]
            "#},
            Some(indoc! {r#"
                   Running models/customers.sql
                Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
            "#}),
        ),
        (
            NodeOutcome::Skipped,
            NodeSkipReason::Cached,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [reused]
            "#},
            None,
        ),
        (
            NodeOutcome::Success,
            NodeSkipReason::Cached,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [-------] model analytics.customers [success]
            "#},
            Some(indoc! {r#"
                   Running models/customers.sql
                Finished running [-------] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
            "#}),
        ),
        (
            NodeOutcome::Skipped,
            NodeSkipReason::NoOp,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [no-op]
            "#},
            None,
        ),
        (
            NodeOutcome::Success,
            NodeSkipReason::NoOp,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running model analytics.customers
                03:04:06.098901 [info ]: Finished running [  0.42s] model analytics.customers [success]
            "#},
            Some(indoc! {r#"
                   Running models/customers.sql
                Finished running [  0.42s] model <cyan+bold>analytics.</><blue+bold>customers</> [<green+bold>success</>]
            "#}),
        ),
    ] {
        let mut model = evaluated(&node(NodeType::Model), ExecutionPhase::Run);
        model.set_node_outcome(outcome);
        model.set_node_skip_reason(skip_reason);
        cases.push(
            Case::new(format!("{outcome:?}, skip reason {skip_reason:?}"))
                .run(move |clock| evaluate(clock, model.clone()))
                .expect(DBT_LOG, dbt_log)
                .expect_or_no_output(TEXT_DEBUG, debug),
        );
    }

    for (test_outcome, statically_checked, dbt_log, debug) in [
        (
            TestOutcome::Passed,
            None,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [  0.42s] test analytics_dbt_test__audit.not_null_customers_customer_id [pass]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [  0.42s] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<green+bold>pass</>]
            "#},
        ),
        (
            TestOutcome::Passed,
            Some(true),
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [-------] test analytics_dbt_test__audit.not_null_customers_customer_id [pass]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [-------] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<green+bold>pass</>]
            "#},
        ),
        (
            TestOutcome::Warned,
            None,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [  0.42s] test analytics_dbt_test__audit.not_null_customers_customer_id [warn]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [  0.42s] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<yellow+bold>warn</>]
            "#},
        ),
        (
            TestOutcome::Warned,
            Some(true),
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [-------] test analytics_dbt_test__audit.not_null_customers_customer_id [warn]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [-------] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<yellow+bold>warn</>]
            "#},
        ),
        (
            TestOutcome::Failed,
            None,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [  0.42s] test analytics_dbt_test__audit.not_null_customers_customer_id [fail]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [  0.42s] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<red+bold>fail</>]
            "#},
        ),
        (
            TestOutcome::Failed,
            Some(true),
            indoc! {r#"
                03:04:05.678901 [info ]: Started running test analytics_dbt_test__audit.not_null_customers_customer_id
                03:04:06.098901 [info ]: Finished running [-------] test analytics_dbt_test__audit.not_null_customers_customer_id [fail]
            "#},
            indoc! {r#"
                   Running models/schema.yml:12:15 (not_null_customers_customer_id)
                Finished running [-------] test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> [<red+bold>fail</>]
            "#},
        ),
    ] {
        let mut test = evaluated(&node(NodeType::Test), ExecutionPhase::Run);
        test.node_outcome_detail = Some(NodeOutcomeDetail::NodeTestDetail(
            TestEvaluationDetail::new(test_outcome, 1, None, None, statically_checked, None),
        ));
        let name = match statically_checked {
            Some(_) => format!("test {test_outcome:?}, statically checked"),
            None => format!("test {test_outcome:?}"),
        };
        cases.push(
            Case::new(name)
                .run(move |clock| evaluate(clock, test.clone()))
                .expect(DBT_LOG, dbt_log)
                .expect(TEXT_DEBUG, debug),
        );
    }

    // Time spent waiting on the thread pool is not reported as work.
    let mut idle = evaluated(&node(NodeType::Model), ExecutionPhase::Run);
    idle.idle_time_ms = Some(300);
    cases.push(
        Case::new("partly idle")
            .run(move |clock| evaluate(clock, idle.clone()))
            .expect(
                DBT_LOG,
                indoc! {r#"
                    03:04:05.678901 [info ]: Started running model analytics.customers
                    03:04:06.098901 [info ]: Finished running [  0.12s] model analytics.customers [success]
                "#},
            ),
    );
}

/// The text start line of a node names its file rather than its relation.
fn legacy_start_lines(cases: &mut Vec<Case>) {
    let in_yaml = |node_type: NodeType, line: Option<u32>, col: Option<u32>| {
        let mut yaml = node(node_type);
        yaml.relative_path = "models/schema.yml".to_string();
        yaml.defined_at_line = line;
        yaml.defined_at_col = col;
        yaml
    };
    let mut singular_test = node(NodeType::Test);
    singular_test.relative_path = "tests/assert_positive_totals.sql".to_string();

    // Each node's start line in the render, run and compare phases, if it has one.
    for (label, node, [render, run, compare]) in [
        (
            "model",
            node(NodeType::Model),
            [
                Some(" Rendering models/customers.sql\n"),
                Some("   Running models/customers.sql\n"),
                Some(" Comparing models/customers.sql\n"),
            ],
        ),
        (
            "seed",
            node(NodeType::Seed),
            [
                None,
                Some("   Running seeds/raw_orders.csv\n"),
                Some(" Comparing seeds/raw_orders.csv\n"),
            ],
        ),
        (
            "yaml test",
            in_yaml(NodeType::Test, Some(12), Some(15)),
            [
                None,
                Some("   Running models/schema.yml:12:15 (not_null_customers_customer_id)\n"),
                Some(" Comparing models/schema.yml:12:15 (not_null_customers_customer_id)\n"),
            ],
        ),
        (
            "yaml test, no column",
            in_yaml(NodeType::Test, Some(12), None),
            [
                None,
                Some("   Running models/schema.yml:12 (not_null_customers_customer_id)\n"),
                Some(" Comparing models/schema.yml:12 (not_null_customers_customer_id)\n"),
            ],
        ),
        (
            "yaml test, no line",
            in_yaml(NodeType::Test, None, None),
            [
                None,
                Some("   Running models/schema.yml (not_null_customers_customer_id)\n"),
                Some(" Comparing models/schema.yml (not_null_customers_customer_id)\n"),
            ],
        ),
        (
            "singular test",
            singular_test,
            [
                Some(" Rendering tests/assert_positive_totals.sql\n"),
                Some("   Running tests/assert_positive_totals.sql\n"),
                Some(" Comparing tests/assert_positive_totals.sql\n"),
            ],
        ),
        (
            "yaml unit test",
            in_yaml(NodeType::UnitTest, Some(40), Some(7)),
            [
                Some(" Rendering models/schema.yml:40:7 (test_customer_totals)\n"),
                Some("   Running models/schema.yml:40:7 (test_customer_totals)\n"),
                Some(" Comparing models/schema.yml:40:7 (test_customer_totals)\n"),
            ],
        ),
        (
            "yaml snapshot",
            in_yaml(NodeType::Snapshot, Some(5), Some(3)),
            [
                Some(" Rendering models/schema.yml:5:3 (orders_snapshot)\n"),
                Some("   Running models/schema.yml:5:3 (orders_snapshot)\n"),
                Some(" Comparing models/schema.yml:5:3 (orders_snapshot)\n"),
            ],
        ),
    ] {
        for (phase, expected) in [
            (ExecutionPhase::Render, render),
            (ExecutionPhase::Run, run),
            (ExecutionPhase::Compare, compare),
        ] {
            let evaluated = evaluated(&node, phase);
            cases.push(
                Case::new(format!("{label}, {phase:?}"))
                    .run(move |clock| evaluate(clock, evaluated.clone()))
                    .expect_or_no_output(TEXT_PROGRESS, expected),
            );
        }
    }

    for (command, expected) in [
        (
            FsCommand::Show,
            "Previewing model (model.jaffle_shop.customers)\n",
        ),
        (FsCommand::Clone, "   Cloning models/customers.sql\n"),
    ] {
        let run = evaluated(&node(NodeType::Model), ExecutionPhase::Run);
        cases.push(
            Case::new(format!("{command:?} command"))
                .run(move |clock| evaluate(clock, run.clone()))
                .expect(
                    Sink::Terminal(
                        "text --show progress-render,progress-run",
                        TEXT_PROGRESS_SETUP.command(command),
                    ),
                    expected,
                ),
        );
    }
}

fn batch(size: MicrobatchBatchSize, index: u32, outcome: NodeOutcome) -> MicrobatchEvaluated {
    MicrobatchEvaluated {
        unique_id: "model.jaffle_shop.customers".to_string(),
        schema: "analytics".to_string(),
        identifier: "customers".to_string(),
        batch_id: format!("2026010{index}"),
        batch_size: size as i32,
        batch_index: index,
        total_batches: 2,
        event_time_start: format!("2026-01-0{index}T05:00:00+00:00"),
        event_time_end: format!("2026-01-0{}T05:00:00+00:00", index + 1),
        node_outcome: outcome as i32,
        idle_time_ms: 100,
        dbt_core_event_code: None,
    }
}

/// Runs an incremental model over two batches, the second of which fails.
fn microbatch_model(clock: &Clock, size: MicrobatchBatchSize) {
    let mut model = node(NodeType::Model);
    model.set_materialization(NodeMaterialization::Incremental);
    model.set_node_outcome(NodeOutcome::Error);
    node_group(|| {
        let span = create_info_span(model);
        for (index, outcome) in [(1, NodeOutcome::Success), (2, NodeOutcome::Error)] {
            let batch = create_info_span(batch(size, index, outcome));
            clock.advance_ms(1_200);
            drop(batch);
        }
        drop(span);
    });
}

fn microbatches(cases: &mut Vec<Case>) {
    for size in all_variants!(MicrobatchBatchSize {
        Unspecified,
        Hour,
        Day,
        Month,
        Year,
    }) {
        // The batch size only changes how the two batches are named.
        let (first, second) = match size {
            MicrobatchBatchSize::Unspecified => ("20260101", "20260102"),
            MicrobatchBatchSize::Hour => ("2026-01-01T05", "2026-01-02T05"),
            MicrobatchBatchSize::Day => ("2026-01-01", "2026-01-02"),
            MicrobatchBatchSize::Month => ("2026-01", "2026-01"),
            MicrobatchBatchSize::Year => ("2026", "2026"),
        };
        cases.push(
            Case::new(format!("batch size {size:?}"))
                .run(move |clock| microbatch_model(clock, size))
                .expect(
                    DBT_LOG,
                    &formatdoc! {r#"
                        03:04:05.678901 [info ]: Started model analytics.customers
                        03:04:05.678901 [info ]:    Started batch {first} of analytics.customers [1 of 2]
                        03:04:06.878901 [info ]:  Succeeded batch {first} of analytics.customers [1 of 2 in 1.10s]
                        03:04:06.878901 [info ]:    Started batch {second} of analytics.customers [2 of 2]
                        03:04:08.078901 [error]:     Failed batch {second} of analytics.customers [2 of 2 in 1.10s]
                        03:04:08.078901 [info ]:     Failed model analytics.customers (incremental) [3 of 12 in 0.25s]
                    "#},
                )
                .expect(
                    TERMINAL,
                    &formatdoc! {r#"
                        <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (incremental) [3 of 12 in 0.25s]
                            ├─ <green+bold>Succeeded</> batch {first} [1 of 2 in 1.10s]
                            └─ <red+bold>Failed</> batch {second} [2 of 2 in 1.10s]
                    "#},
                )
                .expect(
                    TEXT_PROGRESS,
                    &formatdoc! {r#"
                           Started batch {first} of <cyan+bold>analytics.</><blue+bold>customers</> [1 of 2]
                        <green+bold> Succeeded</> batch {first} of <cyan+bold>analytics.</><blue+bold>customers</> [1 of 2 in 1.10s]
                           Started batch {second} of <cyan+bold>analytics.</><blue+bold>customers</> [2 of 2]
                        <red+bold>    Failed</> batch {second} of <cyan+bold>analytics.</><blue+bold>customers</> [2 of 2 in 1.10s]
                        <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (incremental) [3 of 12 in 0.25s]
                    "#},
                )
                .expect_no_output(TERMINAL_QUIET),
        );
    }

    let mut unparsable = batch(MicrobatchBatchSize::Day, 1, NodeOutcome::Success);
    unparsable.event_time_start = "yesterday".to_string();
    cases.push(
        Case::new("unparsable batch start")
            .run(move |clock| {
                let span = create_info_span(unparsable.clone());
                clock.advance_ms(50);
                drop(span);
            })
            .expect(
                DBT_LOG,
                indoc! {r#"
                    03:04:05.678901 [info ]:    Started batch 20260101 of analytics.customers [1 of 2]
                    03:04:05.728901 [info ]:  Succeeded batch 20260101 of analytics.customers [1 of 2 in 0.00s]
                "#},
            ),
    );

    // Interactive debug output keeps one line per batch.
    let terminal_debug = Sink::Terminal(
        "terminal --debug",
        Terminal {
            level: tracing::level_filters::LevelFilter::DEBUG,
            ..Terminal::INTERACTIVE
        },
    );
    cases.push(
        Case::new("Day, interactive debug")
            .run(|clock| microbatch_model(clock, MicrobatchBatchSize::Day))
            .expect(
                terminal_debug,
                indoc! {r#"
                    <green+bold> Succeeded</> batch 2026-01-01 of <cyan+bold>analytics.</><blue+bold>customers</> [1 of 2 in 1.10s]
                    <red+bold>    Failed</> batch 2026-01-02 of <cyan+bold>analytics.</><blue+bold>customers</> [2 of 2 in 1.10s]
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (incremental) [3 of 12 in 0.25s]
                "#},
            ),
    );

    // A model that finished without batches prints its line alone.
    let mut no_batches = node(NodeType::Model);
    no_batches.set_materialization(NodeMaterialization::Incremental);
    cases.push(
        Case::new("incremental model without batches")
            .run(move |clock| node_group(|| processed(clock, no_batches.clone())))
            .expect(
                TERMINAL,
                indoc! {r#"
                    <green+bold> Succeeded</> model <cyan+bold>analytics.</><blue+bold>customers</> (incremental) [3 of 12 in 0.25s]
                "#},
            ),
    );
}
