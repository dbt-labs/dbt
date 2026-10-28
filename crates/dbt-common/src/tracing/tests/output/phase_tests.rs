//! Phases, asset parsing and thread pool waits.

use dbt_telemetry::{AssetParsed, ExecutionPhase, PhaseExecuted, ThreadPoolWait};
use indoc::{formatdoc, indoc};

use super::support::{
    Case, Clock, DBT_LOG, DBT_LOG_DEBUG, TERMINAL, TEXT, TEXT_DEBUG, all_variants,
};
use crate::tracing::emit::{create_debug_span, create_info_span};

pub(super) fn cases(cases: &mut Vec<Case>) {
    phase_lines(cases);
    asset_parsed_lines(cases);
    thread_pool_wait_lines(cases);
}

/// Phases are only reported in the file log; the terminal shows them as progress bars.
fn phase_lines(cases: &mut Vec<Case>) {
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
        // Every phase is logged the same way, only its description differs.
        let description = match phase {
            ExecutionPhase::Unspecified => "unknown phase",
            ExecutionPhase::Clean => "cleaning",
            ExecutionPhase::LoadProject => "loading project",
            ExecutionPhase::Parse => "parsing",
            ExecutionPhase::Schedule => "scheduling",
            ExecutionPhase::Compare => "comparing",
            ExecutionPhase::InitAdapter => "initializing adapter",
            ExecutionPhase::DeferHydration => "deferring state",
            ExecutionPhase::SchemaHydration => "downloading schemas",
            ExecutionPhase::TaskGraphBuild => "building task graph",
            ExecutionPhase::NodeCacheHydration => "hydrating",
            ExecutionPhase::Render => "rendering",
            ExecutionPhase::Analyze => "analyzing",
            ExecutionPhase::Run => "running",
            ExecutionPhase::FreshnessAnalysis => "analyzing freshness",
            ExecutionPhase::Lineage => "generating lineage",
            ExecutionPhase::Debug => "debugging",
            ExecutionPhase::OnRunStart => "on-run-start hooks",
            ExecutionPhase::OnRunEnd => "on-run-end hooks",
        };
        cases.push(
            Case::new(format!("{phase:?}"))
                .run(move |clock| {
                    let span = create_info_span(PhaseExecuted {
                        phase: phase as i32,
                        ..Default::default()
                    });
                    clock.advance_ms(1_500);
                    drop(span);
                })
                .expect(
                    DBT_LOG,
                    &formatdoc! {r#"
                        03:04:05.678901 [info ]: Started {description}
                        03:04:07.178901 [info ]: Finished {description} [  1.50s]
                    "#},
                )
                .expect_no_output(TEXT),
        );
    }

    for (label, total, skipped, errors, expected) in [
        (
            "with nodes",
            Some(12),
            Some(2),
            Some(1),
            indoc! {r#"
                03:04:05.678901 [info ]: Started running (12 nodes)
                03:05:28.678901 [info ]: Finished running [  1m23s] (12 nodes, 2 skipped, 1 errors)
            "#},
        ),
        (
            "with nodes and no counts",
            Some(12),
            None,
            None,
            indoc! {r#"
                03:04:05.678901 [info ]: Started running (12 nodes)
                03:05:28.678901 [info ]: Finished running [  1m23s] (12 nodes, 0 skipped, 0 errors)
            "#},
        ),
        (
            "with zero nodes",
            Some(0),
            Some(0),
            Some(0),
            indoc! {r#"
                03:04:05.678901 [info ]: Started running
                03:05:28.678901 [info ]: Finished running [  1m23s]
            "#},
        ),
    ] {
        cases.push(
            Case::new(format!("Run, {label}"))
                .run(move |clock| {
                    let span = create_info_span(PhaseExecuted {
                        phase: ExecutionPhase::Run as i32,
                        node_count_total: total,
                        node_count_skipped: skipped,
                        node_count_error: errors,
                    });
                    clock.advance_ms(83_000);
                    drop(span);
                })
                .expect(DBT_LOG, expected),
        );
    }
}

fn asset(display_path: &str, phase: ExecutionPhase) -> AssetParsed {
    AssetParsed {
        package_name: "jaffle_shop".to_string(),
        name: "customers".to_string(),
        relative_path: display_path.to_string(),
        display_path: display_path.to_string(),
        unique_id: None,
        phase: phase as i32,
    }
}

fn parse(clock: &Clock, asset: AssetParsed) {
    let span = create_debug_span(asset);
    clock.advance_ms(12);
    drop(span);
}

fn asset_parsed_lines(cases: &mut Vec<Case>) {
    cases.push(
        Case::new("model")
            .run(|clock| parse(clock, asset("models/customers.sql", ExecutionPhase::Parse)))
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.690901 [debug]: Finished parsing models/customers.sql [  0.01s]
                "#},
            )
            .expect_no_output(TERMINAL)
            .expect_no_output(TEXT)
            .expect(
                TEXT_DEBUG,
                indoc! {r#"
                    <green+bold>   Parsing</> models/customers.sql
                "#},
            ),
    );

    // Generated generic tests are only logged to the file.
    cases.push(
        Case::new("generic test")
            .run(|clock| {
                parse(
                    clock,
                    asset(
                        "target/generic_tests/not_null_customers_id.sql",
                        ExecutionPhase::Parse,
                    ),
                )
            })
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.690901 [debug]: Finished parsing target/generic_tests/not_null_customers_id.sql [  0.01s]
                "#},
            )
            .expect_no_output(TERMINAL)
            .expect_no_output(TEXT)
            .expect_no_output(TEXT_DEBUG),
    );

    // An asset parsed while rendering is still reported as parsed.
    cases.push(
        Case::new("model while rendering")
            .run(|clock| parse(clock, asset("models/customers.sql", ExecutionPhase::Render)))
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.690901 [debug]: Finished parsing models/customers.sql [  0.01s]
                "#},
            )
            .expect_no_output(TERMINAL)
            .expect_no_output(TEXT)
            .expect(
                TEXT_DEBUG,
                indoc! {r#"
                    <green+bold>   Parsing</> models/customers.sql
                "#},
            ),
    );
}

fn thread_pool_wait_lines(cases: &mut Vec<Case>) {
    // A batch wait is logged the same as a node wait.
    for (label, batch_id) in [("node", None), ("batch", Some("20260102".to_string()))] {
        cases.push(
            Case::new(label)
                .run(move |clock| {
                    let span = create_debug_span(ThreadPoolWait {
                        unique_id: Some("model.jaffle_shop.customers".to_string()),
                        batch_id: batch_id.clone(),
                    });
                    clock.advance_ms(2_340);
                    drop(span);
                })
                .expect_no_output(DBT_LOG)
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {r#"
                        03:04:05.678901 [debug]: Started thread pool wait
                        03:04:08.018901 [debug]: Finished thread pool wait [  2.34s]
                    "#},
                )
                .expect_no_output(TERMINAL)
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                        Started thread pool wait
                        Finished thread pool wait [  2.34s]
                    "#},
                ),
        );
    }
}
