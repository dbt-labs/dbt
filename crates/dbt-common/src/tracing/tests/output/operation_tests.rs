//! Generic operations with their items, and on-run hooks.

use dbt_telemetry::{
    ExecutionPhase, GenericOpExecuted, GenericOpItemProcessed, HookOutcome, HookProcessed, HookType,
};
use indoc::{formatdoc, indoc};

use super::support::{
    Case, Clock, DBT_LOG, DBT_LOG_DEBUG, Sink, TERMINAL, TERMINAL_QUIET, TEXT, TEXT_DEBUG,
    Terminal, all_variants,
};
use crate::{
    io_args::{FsCommand, ShowOptions},
    tracing::{
        emit::{create_debug_span, create_info_span},
        span_info::record_span_status,
    },
};

pub(super) fn cases(cases: &mut Vec<Case>) {
    operations(cases);
    operation_items(cases);
    hooks(cases);
}

/// How a span ended.
#[derive(Clone, Copy, Debug)]
enum Ending {
    /// No status recorded
    Unset,
    Ok,
    /// Failed with "connection reset"
    Error,
}

const ENDINGS: [Ending; 3] = [Ending::Unset, Ending::Ok, Ending::Error];

fn end(span: tracing::Span, ending: Ending) {
    match ending {
        Ending::Unset => {}
        Ending::Ok => record_span_status(&span, None),
        Ending::Error => record_span_status(&span, Some("connection reset")),
    }
    drop(span);
}

fn operation(item_count_total: Option<u64>) -> GenericOpExecuted {
    GenericOpExecuted {
        operation_id: "download_manifest".to_string(),
        display_action: "downloading manifest".to_string(),
        item_count_total,
    }
}

fn item(in_progress_action: &str, success_action: &str) -> GenericOpItemProcessed {
    GenericOpItemProcessed {
        operation_id: "download_manifest".to_string(),
        display_in_progress_action: in_progress_action.to_string(),
        display_on_success_action: success_action.to_string(),
        target: "manifest.json".to_string(),
    }
}

fn run_operation(clock: &Clock, operation: GenericOpExecuted, ending: Ending) {
    let span = create_info_span(operation);
    clock.advance_ms(2_500);
    end(span, ending);
}

fn hook(hook_type: HookType, outcome: HookOutcome, name: Option<&str>) -> HookProcessed {
    HookProcessed {
        package_name: "jaffle_shop".to_string(),
        name: name.map(str::to_string),
        hook_type: hook_type as i32,
        hook_index: 0,
        unique_id: "operation.jaffle_shop.jaffle_shop-on-run-end-0".to_string(),
        hook_outcome: outcome as i32,
        dbt_core_event_code: String::new(),
        phase: ExecutionPhase::OnRunEnd as i32,
    }
}

fn run_hook(clock: &Clock, hook: HookProcessed) {
    let span = create_info_span(hook);
    clock.advance_ms(90);
    drop(span);
}

fn operations(cases: &mut Vec<Case>) {
    for ending in ENDINGS {
        let case = Case::new(format!("4 items, {ending:?}"))
            .run(move |clock| run_operation(clock, operation(Some(4)), ending));
        cases.push(match ending {
            Ending::Unset | Ending::Ok => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]:    Started downloading manifest (4 items)
                        03:04:08.178901 [info ]:   Finished [  2.50s] downloading manifest (4 items)
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <green+bold>  Finished</> [  2.50s] downloading manifest (4 items)
                    "#},
                )
                .expect(
                    TEXT,
                    indoc! {r#"
                        <green+bold>   Started</> downloading manifest (4 items)
                        <green+bold>  Finished</> [  2.50s] downloading manifest (4 items)
                    "#},
                ),
            // The error message replaces the item count.
            Ending::Error => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]:    Started downloading manifest (4 items)
                        03:04:08.178901 [info ]:     Failed [  2.50s] downloading manifest (error: connection reset)
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <red+bold>    Failed</> [  2.50s] downloading manifest (error: connection reset)
                    "#},
                )
                .expect(
                    TEXT,
                    indoc! {r#"
                        <green+bold>   Started</> downloading manifest (4 items)
                        <red+bold>    Failed</> [  2.50s] downloading manifest (error: connection reset)
                    "#},
                ),
        });
    }

    // Without an item count, or with a count of zero, no count is shown.
    for total in [None, Some(0)] {
        cases.push(
            Case::new(format!("{total:?} items"))
                .run(move |clock| run_operation(clock, operation(total), Ending::Ok))
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]:    Started downloading manifest
                        03:04:08.178901 [info ]:   Finished [  2.50s] downloading manifest
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <green+bold>  Finished</> [  2.50s] downloading manifest
                    "#},
                )
                .expect(
                    TEXT,
                    indoc! {r#"
                        <green+bold>   Started</> downloading manifest
                        <green+bold>  Finished</> [  2.50s] downloading manifest
                    "#},
                ),
        );
    }

    // Debug level operations are only shown with debug output.
    cases.push(
        Case::new("debug operation")
            .run(|clock| {
                let span = create_debug_span(operation(Some(2)));
                clock.advance_ms(30);
                drop(span);
            })
            .expect_no_output(DBT_LOG)
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.678901 [debug]:    Started downloading manifest (2 items)
                    03:04:05.708901 [debug]:   Finished [  0.03s] downloading manifest (2 items)
                "#},
            )
            .expect_no_output(TEXT)
            .expect(
                TEXT_DEBUG,
                indoc! {r#"
                    <green+bold>   Started</> downloading manifest (2 items)
                    <green+bold>  Finished</> [  0.03s] downloading manifest (2 items)
                "#},
            ),
    );

    // Resolve stages land on the parse progress bar instead of a spinner of their own.
    let resolve = GenericOpExecuted {
        operation_id: "resolve.build_model_nodes.jaffle_shop".to_string(),
        display_action: "building model nodes".to_string(),
        item_count_total: None,
    };
    cases.push(
        Case::new("resolve stage")
            .run(move |clock| run_operation(clock, resolve.clone(), Ending::Unset))
            .expect_no_output(TERMINAL)
            .expect(
                TEXT,
                indoc! {r#"
                    <green+bold>   Started</> building model nodes
                    <green+bold>  Finished</> [  2.50s] building model nodes
                "#},
            ),
    );
}

fn operation_items(cases: &mut Vec<Case>) {
    let terminal_lint = Sink::Terminal(
        "terminal lint",
        Terminal::INTERACTIVE.command(FsCommand::Extension("lint")),
    );
    let terminal_lint_all = Sink::Terminal(
        "terminal lint --show all",
        Terminal::INTERACTIVE
            .command(FsCommand::Extension("lint"))
            .show(&[ShowOptions::All]),
    );

    for ending in ENDINGS {
        let case = Case::new(format!("item, {ending:?}")).run(move |clock| {
            let op = create_info_span(operation(Some(1)));
            op.in_scope(|| {
                let span = create_info_span(item("downloading", "downloaded"));
                clock.advance_ms(700);
                end(span, ending);
            });
            drop(op);
        });
        cases.push(match ending {
            Ending::Unset | Ending::Ok => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]:    Started downloading manifest (1 items)
                        03:04:05.678901 [info ]:    Started downloading manifest.json
                        03:04:06.378901 [info ]: Downloaded [  0.70s] manifest.json
                        03:04:06.378901 [info ]:   Finished [  0.70s] downloading manifest (1 items)
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <green+bold>Downloaded</> [  0.70s] manifest.json
                        <green+bold>  Finished</> [  0.70s] downloading manifest (1 items)
                    "#},
                )
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                        <green+bold>   Started</> downloading manifest (1 items)
                           Started downloading manifest.json
                        <green+bold>Downloaded</> [  0.70s] manifest.json
                        <green+bold>  Finished</> [  0.70s] downloading manifest (1 items)
                    "#},
                ),
            // A failed item does not show its action.
            Ending::Error => case
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]:    Started downloading manifest (1 items)
                        03:04:05.678901 [info ]:    Started downloading manifest.json
                        03:04:06.378901 [info ]:     Failed [  0.70s] manifest.json (error: connection reset)
                        03:04:06.378901 [info ]:   Finished [  0.70s] downloading manifest (1 items)
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <red+bold>    Failed</> [  0.70s] manifest.json (error: connection reset)
                        <green+bold>  Finished</> [  0.70s] downloading manifest (1 items)
                    "#},
                )
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                        <green+bold>   Started</> downloading manifest (1 items)
                           Started downloading manifest.json
                        <red+bold>    Failed</> [  0.70s] manifest.json (error: connection reset)
                        <green+bold>  Finished</> [  0.70s] downloading manifest (1 items)
                    "#},
                )
                .expect_no_output(TERMINAL_QUIET),
        });
    }

    // Linting runs one item per file, so items need `--show all`.
    cases.push(
        Case::new("lint items")
            .run(|clock| {
                let span = create_info_span(item("linting", "linted"));
                clock.advance_ms(5);
                drop(span);
            })
            .expect_no_output(terminal_lint)
            .expect(
                terminal_lint_all,
                indoc! {r#"
                       Started linting manifest.json
                    <green+bold>    Linted</> [  0.01s] manifest.json
                "#},
            ),
    );
}

fn hooks(cases: &mut Vec<Case>) {
    // The hook type only shows when the hook starts.
    for hook_type in all_variants!(HookType {
        Unspecified,
        OnRunStart,
        OnRunEnd,
        PreHook,
        PostHook,
    }) {
        let started = match hook_type {
            HookType::Unspecified => "Started hook",
            HookType::OnRunStart => "Started on-run-start",
            HookType::OnRunEnd => "Started on-run-end",
            HookType::PreHook => "Started pre-hook",
            HookType::PostHook => "Started post-hook",
        };

        let name = Some("jaffle_shop-on-run-end-0");
        cases.push(
            Case::new(format!("{hook_type:?}, name {name:?}"))
                .run(move |clock| run_hook(clock, hook(hook_type, HookOutcome::Success, name)))
                .expect_no_output(DBT_LOG)
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                    "#},
                )
                .expect(
                    TEXT,
                    indoc! {r#"
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                    "#},
                )
                .expect(
                    TEXT_DEBUG,
                    &formatdoc! {r#"
                        {started} <blue+bold>jaffle_shop-on-run-end-0</>
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                    "#},
                ),
        );

        let name = None;
        cases.push(
            Case::new(format!("{hook_type:?}, name {name:?}"))
                .run(move |clock| run_hook(clock, hook(hook_type, HookOutcome::Success, name)))
                .expect_no_output(DBT_LOG)
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold></>
                    "#},
                )
                .expect(
                    TEXT,
                    indoc! {r#"
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold></>
                    "#},
                )
                .expect(
                    TEXT_DEBUG,
                    &formatdoc! {r#"
                        {started}
                        <green+bold>  Finished</> [  0.09s] hook  <blue+bold></>
                    "#},
                ),
        );
    }

    for outcome in all_variants!(HookOutcome {
        Unspecified,
        Success,
        Error,
        Canceled,
    }) {
        let (terminal, debug) = match outcome {
            HookOutcome::Unspecified => (
                // Every line starts with whitespace, which `indoc!` would strip.
                "  Finished [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>\n",
                indoc! {r#"
                    Started on-run-end <blue+bold>jaffle_shop-on-run-end-0</>
                      Finished [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
            ),
            HookOutcome::Success => (
                indoc! {r#"
                    <green+bold>  Finished</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
                indoc! {r#"
                    Started on-run-end <blue+bold>jaffle_shop-on-run-end-0</>
                    <green+bold>  Finished</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
            ),
            HookOutcome::Error => (
                indoc! {r#"
                    <red+bold>    Failed</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
                indoc! {r#"
                    Started on-run-end <blue+bold>jaffle_shop-on-run-end-0</>
                    <red+bold>    Failed</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
            ),
            HookOutcome::Canceled => (
                indoc! {r#"
                    <yellow+bold> Cancelled</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
                indoc! {r#"
                    Started on-run-end <blue+bold>jaffle_shop-on-run-end-0</>
                    <yellow+bold> Cancelled</> [  0.09s] hook  <blue+bold>jaffle_shop-on-run-end-0</>
                "#},
            ),
        };
        cases.push(
            Case::new(format!("{outcome:?}"))
                .run(move |clock| {
                    run_hook(
                        clock,
                        hook(
                            HookType::OnRunEnd,
                            outcome,
                            Some("jaffle_shop-on-run-end-0"),
                        ),
                    )
                })
                .expect(TERMINAL, terminal)
                .expect(TEXT_DEBUG, debug),
        );
    }
}
