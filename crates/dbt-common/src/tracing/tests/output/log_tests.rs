//! Log messages and the other log records rendered as text: progress, prints, listings,
//! previews and compiled code.

use dbt_error::ErrorCode;
use dbt_telemetry::{
    CompiledCode, CompiledCodeInline, ExecutionPhase, ListItemOutput, LogMessage, ProgressMessage,
    ShowDataOutput, ShowResult, StateModifiedDiff, UserLogMessage,
};
use indoc::{formatdoc, indoc};

use super::events::invocation;
use super::support::{
    Case, Clock, DBT_LOG, DBT_LOG_DEBUG, DBT_LOG_TRACE, Sink, TERMINAL, TERMINAL_QUIET, TEXT_DEBUG,
    Terminal, all_variants,
};
use crate::{
    io_args::{FsCommand, ShowOptions},
    tracing::{
        dbt_emit,
        emit::{
            emit_debug_event, emit_error_event, emit_info_event, emit_trace_event, emit_warn_event,
        },
    },
};
use dbt_tracing::{SeverityNumber, TelemetryAttributes};

pub(super) fn cases(cases: &mut Vec<Case>) {
    log_messages(cases);
    user_log_messages(cases);
    progress_messages(cases);
    prints(cases);
    state_modified_diffs(cases);
    list_items(cases);
    previews(cases);
    compiled_code(cases);
}

/// Root for cases with warnings or errors: the terminal prints them when the invocation
/// ends. `man` prints no summary of its own, so `dbt.log` only gains its header.
fn terminal_root() -> TelemetryAttributes {
    invocation("man").into()
}

/// Emits `attrs` with `body` at `severity`.
fn emit(severity: SeverityNumber, attrs: impl Into<TelemetryAttributes>, body: &str) {
    let attrs = attrs.into();
    match severity {
        SeverityNumber::Error => emit_error_event(attrs, Some(body)),
        SeverityNumber::Warn => emit_warn_event(attrs, Some(body)),
        SeverityNumber::Info => emit_info_event(attrs, Some(body)),
        SeverityNumber::Unspecified => unreachable!("records are never emitted unspecified"),
        SeverityNumber::Debug => emit_debug_event(attrs, Some(body)),
        SeverityNumber::Trace => emit_trace_event(|| (attrs, Some(body.to_string()))),
    }
}

/// Every severity a record can be emitted at. `Unspecified` is never emitted.
fn severities() -> impl Iterator<Item = SeverityNumber> {
    all_variants!(SeverityNumber {
        Unspecified,
        Trace,
        Debug,
        Info,
        Warn,
        Error,
    })
    .into_iter()
    .filter(|severity| *severity != SeverityNumber::Unspecified)
}

fn log_messages(cases: &mut Vec<Case>) {
    let level = |severity: SeverityNumber| match severity {
        SeverityNumber::Error => tracing::Level::ERROR,
        SeverityNumber::Warn => tracing::Level::WARN,
        SeverityNumber::Info | SeverityNumber::Unspecified => tracing::Level::INFO,
        SeverityNumber::Debug => tracing::Level::DEBUG,
        SeverityNumber::Trace => tracing::Level::TRACE,
    };

    for severity in severities() {
        for code in [None, Some(ErrorCode::UnusedConfigKey as u32), Some(65_000)] {
            let message = LogMessage {
                code,
                ..LogMessage::new_from_level(level(severity))
            };
            let case = Case::new(format!("{severity:?}, code {code:?}"))
                .root(terminal_root())
                .run(move |_| {
                    emit(
                        severity,
                        message.clone(),
                        "Unused config key 'materialised' in models/schema.yml",
                    )
                });
            // Only a code `ErrorCode` knows is printed.
            let known_code = code == Some(ErrorCode::UnusedConfigKey as u32);
            cases.push(match (severity, known_code) {
                (SeverityNumber::Trace, false) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [trace]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    // Trace records are not shown with `--debug`.
                    .expect_no_output(TEXT_DEBUG),
                (SeverityNumber::Trace, true) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [trace]: [UnusedConfigKey (dbt1060)]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    // Trace records are not shown with `--debug`.
                    .expect_no_output(TEXT_DEBUG),
                (SeverityNumber::Debug, false) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [debug]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect(
                        TEXT_DEBUG,
                        indoc! {r#"
                            Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Debug, true) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [debug]: [UnusedConfigKey (dbt1060)]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect(
                        TEXT_DEBUG,
                        indoc! {r#"
                            [UnusedConfigKey (dbt1060)]: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Info, false) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [info ]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect(
                        TEXT_DEBUG,
                        indoc! {r#"
                            Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Info, true) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [info ]: [UnusedConfigKey (dbt1060)]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect(
                        TEXT_DEBUG,
                        indoc! {r#"
                            [UnusedConfigKey (dbt1060)]: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Warn, false) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [warn ]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect_stderr(
                        TEXT_DEBUG,
                        indoc! {r#"

                            <dim>=================== Errors and Warnings ====================</>
                            <yellow+bold>[warning]</>: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Warn, true) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [warn ]: [UnusedConfigKey (dbt1060)]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect_stderr(
                        TEXT_DEBUG,
                        indoc! {r#"

                            <dim>=================== Errors and Warnings ====================</>
                            <yellow+bold>[warning]</> [UnusedConfigKey (dbt1060)]: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Error, false) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [error]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect_stderr(
                        TEXT_DEBUG,
                        indoc! {r#"

                            <dim>=================== Errors and Warnings ====================</>
                            <red+bold>[error]</>: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Error, true) => case
                    .expect(
                        DBT_LOG_TRACE,
                        indoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [error]: [UnusedConfigKey (dbt1060)]: Unused config key 'materialised' in models/schema.yml
                        "#},
                    )
                    .expect_stderr(
                        TEXT_DEBUG,
                        indoc! {r#"

                            <dim>=================== Errors and Warnings ====================</>
                            <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: Unused config key '<yellow>materialised</>' in models/schema.yml
                        "#},
                    ),
                (SeverityNumber::Unspecified, _) => {
                    unreachable!("records are never emitted unspecified")
                }
            });
        }
    }

    // How the body is rendered, after the `[UnusedConfigKey (dbt1060)]: ` prefix.
    for (label, body, file_log, terminal) in [
        (
            "single quotes",
            "Model 'customers' depends on 'orders' and 'payments'",
            "Model 'customers' depends on 'orders' and 'payments'",
            "Model '<yellow>customers</>' depends on '<yellow>orders</>' and '<yellow>payments</>'",
        ),
        (
            "backticks win over single quotes",
            "Run `dbt deps` to install 'dbt_utils'",
            "Run `dbt deps` to install 'dbt_utils'",
            "Run `<yellow>dbt deps</>` to install 'dbt_utils'",
        ),
        (
            "unbalanced quote",
            "Couldn't find 'customers",
            "Couldn't find 'customers",
            "Couldn'<yellow>t find </>'customers",
        ),
        (
            "doesn't is not a quote",
            "Column 'id' doesn't exist in 'orders'",
            "Column 'id' doesn't exist in 'orders'",
            "Column '<yellow>id</>' does not exist in '<yellow>orders</>'",
        ),
        (
            "doesn't between quotes",
            "Model 'it doesn't matter' is unused",
            "Model 'it doesn't matter' is unused",
            "Model '<yellow>it does not matter</>' is unused",
        ),
        (
            "doesn't with backticks",
            "Package `dbt_utils` doesn't exist",
            "Package `dbt_utils` doesn't exist",
            "Package `<yellow>dbt_utils</>` does not exist",
        ),
        (
            "doesn't with an unbalanced quote",
            "Model 'customers doesn't exist",
            "Model 'customers doesn't exist",
            "Model 'customers doesn't exist",
        ),
        (
            "ansi codes in the body",
            "\x1b[31mred\x1b[0m alert",
            "red alert",
            "<red>red</> alert",
        ),
        (
            "multiple lines",
            "first line\n  second line\n\nlast line",
            "first line\n  second line\n\nlast line",
            "first line\n  second line\n\nlast line",
        ),
    ] {
        for severity in [SeverityNumber::Info, SeverityNumber::Warn] {
            let message = LogMessage {
                code: Some(ErrorCode::UnusedConfigKey as u32),
                ..LogMessage::new_from_level(level(severity))
            };
            let case = Case::new(format!("{label}, {severity:?}"))
                .root(terminal_root())
                .run(move |_| emit(severity, message.clone(), body));
            cases.push(match severity {
                SeverityNumber::Info => case
                    .expect(
                        DBT_LOG,
                        &formatdoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [info ]: [UnusedConfigKey (dbt1060)]: {file_log}
                        "#},
                    )
                    .expect(
                        TERMINAL,
                        &format!("[UnusedConfigKey (dbt1060)]: {terminal}\n"),
                    ),
                SeverityNumber::Warn => case
                    .expect(
                        DBT_LOG,
                        &formatdoc! {r#"
                            ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                            03:04:05.678901 [warn ]: [UnusedConfigKey (dbt1060)]: {file_log}
                        "#},
                    )
                    .expect_stderr(
                        TERMINAL,
                        &formatdoc! {r#"

                            <dim>=================== Errors and Warnings ====================</>
                            <yellow+bold>[warning]</> [UnusedConfigKey (dbt1060)]: {terminal}
                        "#},
                    ),
                _ => unreachable!("only info and warn messages are rendered here"),
            });
        }
    }

    cases.push(
        Case::new("warnings and errors are collected at the end")
            .root(terminal_root())
            .run(|_| {
                emit(
                    SeverityNumber::Warn,
                    LogMessage::new_from_level(tracing::Level::WARN),
                    "first warning",
                );
                emit(
                    SeverityNumber::Info,
                    LogMessage::new_from_level(tracing::Level::INFO),
                    "info in between",
                );
                emit(
                    SeverityNumber::Error,
                    LogMessage::new_from_level(tracing::Level::ERROR),
                    "an error",
                );
                emit(
                    SeverityNumber::Warn,
                    LogMessage::new_from_level(tracing::Level::WARN),
                    "second warning",
                );
            })
            .expect(
                TERMINAL,
                indoc! {r#"
                    info in between
                "#},
            )
            .expect_stderr(
                TERMINAL,
                indoc! {r#"

                    <dim>=================== Errors and Warnings ====================</>
                    <yellow+bold>[warning]</>: first warning
                    <red+bold>[error]</>: an error
                    <yellow+bold>[warning]</>: second warning
                "#},
            )
            .expect(
                TERMINAL_QUIET,
                indoc! {r#"
                    info in between
                "#},
            )
            .expect_stderr(
                TERMINAL_QUIET,
                indoc! {r#"

                    <dim>=================== Errors and Warnings ====================</>
                    <yellow+bold>[warning]</>: first warning
                    <red+bold>[error]</>: an error
                    <yellow+bold>[warning]</>: second warning
                "#},
            ),
    );
}

fn user_log_messages(cases: &mut Vec<Case>) {
    let user_log = |is_print: bool, severity: SeverityNumber| {
        move |_: &Clock| {
            emit(
                severity,
                UserLogMessage {
                    is_print,
                    ..Default::default()
                },
                "Hello from 'jinja'\nwith a second line",
            )
        }
    };

    // `print()` and `log(info=True)` are rendered the same way.
    for (label, is_print) in [("print()", true), ("log(info=True)", false)] {
        cases.push(
            Case::new(label)
                .run(user_log(is_print, SeverityNumber::Info))
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Hello from 'jinja'
                        with a second line
                    "#},
                )
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Hello from 'jinja'
                        with a second line
                    "#},
                )
                .expect(
                    TERMINAL,
                    indoc! {r#"
                        Hello from 'jinja'
                        with a second line
                    "#},
                )
                .expect(
                    TEXT_DEBUG,
                    indoc! {r#"
                        Hello from 'jinja'
                        with a second line
                    "#},
                )
                .expect(
                    TERMINAL_QUIET,
                    indoc! {r#"
                        Hello from 'jinja'
                        with a second line
                    "#},
                ),
        );
    }

    cases.push(
        Case::new("log()")
            .run(user_log(false, SeverityNumber::Debug))
            .expect_no_output(DBT_LOG)
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.678901 [debug]: Hello from 'jinja'
                    with a second line
                "#},
            )
            .expect_no_output(TERMINAL)
            .expect(
                TEXT_DEBUG,
                indoc! {r#"
                    Hello from 'jinja'
                    with a second line
                "#},
            ),
    );
}

fn progress_messages(cases: &mut Vec<Case>) {
    let progress =
        |action: &str, description: Option<&str>, phase: Option<ExecutionPhase>| ProgressMessage {
            action: action.to_string(),
            target: "models/customers.sql".to_string(),
            description: description.map(str::to_string),
            phase: phase.map(|phase| phase as i32),
            ..Default::default()
        };

    for severity in severities() {
        let (dbt_log, debug) = match severity {
            // Trace records are not shown with `--debug`.
            SeverityNumber::Trace => (
                "03:04:05.678901 [trace]: Debugging models/customers.sql\n",
                None,
            ),
            SeverityNumber::Debug => (
                "03:04:05.678901 [debug]: Debugging models/customers.sql\n",
                Some(" Debugging models/customers.sql\n"),
            ),
            SeverityNumber::Info => (
                "03:04:05.678901 [info ]: Debugging models/customers.sql\n",
                Some("<green+bold> Debugging</> models/customers.sql\n"),
            ),
            SeverityNumber::Warn => (
                "03:04:05.678901 [warn ]: Debugging models/customers.sql\n",
                Some("<yellow+bold> Debugging</> models/customers.sql\n"),
            ),
            SeverityNumber::Error => (
                "03:04:05.678901 [error]: Debugging models/customers.sql\n",
                Some("<red+bold> Debugging</> models/customers.sql\n"),
            ),
            SeverityNumber::Unspecified => unreachable!("records are never emitted unspecified"),
        };
        cases.push(
            Case::new(format!("{severity:?}"))
                .run(move |_| emit(severity, progress("Debugging", None, None), ""))
                .expect(DBT_LOG_TRACE, dbt_log)
                .expect_or_no_output(TEXT_DEBUG, debug),
        );
    }

    for (label, message, dbt_log, terminal, debug) in [
        (
            "description",
            progress("Checking", Some("3 of 4"), None),
            "03:04:05.678901 [info ]: Checking models/customers.sql (3 of 4)\n",
            Some("<green+bold>  Checking</> models/customers.sql (3 of 4)\n"),
            "<green+bold>  Checking</> models/customers.sql (3 of 4)\n",
        ),
        (
            "empty description",
            progress("Checking", Some(""), None),
            "03:04:05.678901 [info ]: Checking models/customers.sql\n",
            Some("<green+bold>  Checking</> models/customers.sql\n"),
            "<green+bold>  Checking</> models/customers.sql\n",
        ),
        (
            "action of 10 characters",
            progress("Validating", None, None),
            "03:04:05.678901 [info ]: Validating models/customers.sql\n",
            Some("<green+bold>Validating</> models/customers.sql\n"),
            "<green+bold>Validating</> models/customers.sql\n",
        ),
        (
            "action longer than 10 characters",
            progress("Downloading", None, None),
            "03:04:05.678901 [info ]: Downloading models/customers.sql\n",
            Some("<green+bold>Downloading</> models/customers.sql\n"),
            "<green+bold>Downloading</> models/customers.sql\n",
        ),
        (
            "action with non-ascii text",
            progress("Prüfen", None, None),
            "03:04:05.678901 [info ]: Prüfen models/customers.sql\n",
            Some("<green+bold>    Prüfen</> models/customers.sql\n"),
            "<green+bold>    Prüfen</> models/customers.sql\n",
        ),
        (
            "parse phase",
            progress("Parsing", None, Some(ExecutionPhase::Parse)),
            "03:04:05.678901 [info ]: Parsing models/customers.sql\n",
            None,
            "<green+bold>   Parsing</> models/customers.sql\n",
        ),
        (
            "schema hydration phase",
            progress("Fetching", None, Some(ExecutionPhase::SchemaHydration)),
            "03:04:05.678901 [info ]: Fetching models/customers.sql\n",
            Some("<green+bold>  Fetching</> models/customers.sql\n"),
            "<green+bold>  Fetching</> models/customers.sql\n",
        ),
    ] {
        cases.push(
            Case::new(label)
                .run(move |_| emit(SeverityNumber::Info, message.clone(), ""))
                .expect(DBT_LOG, dbt_log)
                .expect_or_no_output(TERMINAL, terminal)
                .expect(TEXT_DEBUG, debug)
                .expect_no_output(TERMINAL_QUIET),
        );
    }
}

fn prints(cases: &mut Vec<Case>) {
    cases.push(
        Case::new("println")
            .run(|_| dbt_emit::println("Hello 'world'"))
            .expect_no_output(DBT_LOG)
            .expect(
                TERMINAL,
                indoc! {r#"
                    Hello 'world'
                "#},
            )
            .expect(
                TERMINAL_QUIET,
                indoc! {r#"
                    Hello 'world'
                "#},
            ),
    );
    cases.push(
        Case::new("print without a newline")
            .run(|_| {
                dbt_emit::print("no newline");
                dbt_emit::print(", continued\n");
            })
            .expect(
                TERMINAL,
                indoc! {r#"
                    no newline, continued
                "#},
            ),
    );
    cases.push(
        Case::new("print_err")
            .run(|_| dbt_emit::print_err(ErrorCode::InvalidConfig, "Bad 'config' value"))
            .expect_no_output(DBT_LOG)
            .expect_stderr(
                TERMINAL,
                indoc! {r#"
                    <red+bold>[error]</> [InvalidConfig (dbt1005)]: Bad '<yellow>config</>' value
                "#},
            )
            .expect_stderr(
                TERMINAL_QUIET,
                indoc! {r#"
                    <red+bold>[error]</> [InvalidConfig (dbt1005)]: Bad '<yellow>config</>' value
                "#},
            ),
    );
}

fn state_modified_diffs(cases: &mut Vec<Case>) {
    let diff = |self_value: Option<&str>, other_value: Option<&str>, unique_id: Option<&str>| {
        StateModifiedDiff {
            unique_id: unique_id.map(str::to_string),
            node_type_or_category: "model".to_string(),
            check: "config".to_string(),
            self_value: self_value.map(str::to_string),
            other_value: other_value.map(str::to_string),
        }
    };

    for (label, diff, dbt_log) in [
        (
            "both values",
            diff(
                Some("{\"materialized\": \"table\"}"),
                Some("view"),
                Some("model.jaffle_shop.customers"),
            ),
            indoc! {r#"
                03:04:05.678901 [trace]: [state_mod_diff] unique_id=model.jaffle_shop.customers, node_type_or_category=model, check=config
                   self:  "{\"materialized\": \"table\"}"
                   other:  "view"
            "#},
        ),
        (
            "only self",
            diff(Some("table"), None, Some("model.jaffle_shop.customers")),
            indoc! {r#"
                03:04:05.678901 [trace]: [state_mod_diff] unique_id=model.jaffle_shop.customers, node_type_or_category=model, check=config
                   self:  "table"
            "#},
        ),
        (
            "no values and no unique id",
            diff(None, None, None),
            indoc! {r#"
                03:04:05.678901 [trace]: [state_mod_diff] unique_id=unknown, node_type_or_category=model, check=config
            "#},
        ),
    ] {
        cases.push(
            Case::new(label)
                .run(move |_| emit(SeverityNumber::Trace, diff.clone(), ""))
                .expect(DBT_LOG_TRACE, dbt_log)
                // Diffs are trace records, which are not shown with `--debug`.
                .expect_no_output(TEXT_DEBUG),
        );
    }
}

fn list_items(cases: &mut Vec<Case>) {
    let item = |content: &str| ListItemOutput {
        content: content.to_string(),
        ..Default::default()
    };
    let terminal_list = Sink::Terminal(
        "terminal list",
        Terminal::INTERACTIVE.command(FsCommand::List),
    );
    let terminal_list_quiet = Sink::Terminal(
        "terminal list --quiet",
        Terminal {
            show: &[],
            quiet: true,
            command: FsCommand::List,
            ..Terminal::INTERACTIVE
        },
    );
    let terminal_build_nodes = Sink::Terminal(
        "terminal build --show nodes",
        Terminal::INTERACTIVE.show(&[ShowOptions::Nodes]),
    );

    cases.push(
        Case::new("two items")
            .run(move |_| {
                emit(
                    SeverityNumber::Info,
                    item("model.jaffle_shop.customers"),
                    "",
                );
                emit(SeverityNumber::Info, item("model.jaffle_shop.orders"), "");
            })
            .expect(
                DBT_LOG,
                indoc! {r#"
                    =======================Selected nodes=======================
                    03:04:05.678901 [info ]: model.jaffle_shop.customers
                    03:04:05.678901 [info ]: model.jaffle_shop.orders
                "#},
            )
            .expect(
                terminal_list,
                indoc! {r#"
                    <dim>=======================Selected nodes=======================</>
                    model.jaffle_shop.customers
                    model.jaffle_shop.orders
                "#},
            )
            .expect(
                terminal_list_quiet,
                indoc! {r#"
                    model.jaffle_shop.customers
                    model.jaffle_shop.orders
                "#},
            )
            .expect_no_output(TERMINAL)
            .expect(
                terminal_build_nodes,
                indoc! {r#"
                    <dim>=======================Selected nodes=======================</>
                    model.jaffle_shop.customers
                    model.jaffle_shop.orders
                "#},
            ),
    );
}

fn previews(cases: &mut Vec<Case>) {
    let table = "| id | name  |\n| -- | ----- |\n| 1  | Alice |";

    cases.push(
        Case::new("show data")
            .run(move |_| {
                emit(
                    SeverityNumber::Info,
                    ShowDataOutput {
                        content: table.to_string(),
                        node_name: "customers".to_string(),
                        ..Default::default()
                    },
                    "",
                )
            })
            .expect(
                DBT_LOG,
                indoc! {r#"
                    03:04:05.678901 [info ]: | id | name  |
                    | -- | ----- |
                    | 1  | Alice |
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"
                    | id | name  |
                    | -- | ----- |
                    | 1  | Alice |
                "#},
            )
            .expect(
                TERMINAL_QUIET,
                indoc! {r#"
                    | id | name  |
                    | -- | ----- |
                    | 1  | Alice |
                "#},
            ),
    );

    // Both write the same `dbt.log`.
    for (label, content, terminal, quiet) in [
        (
            "show result",
            table.to_string(),
            indoc! {r#"

                <blue+bold>Previewing node 'customers':</>
                | id | name  |
                | -- | ----- |
                | 1  | Alice |
            "#},
            indoc! {r#"
                | id | name  |
                | -- | ----- |
                | 1  | Alice |
            "#},
        ),
        (
            "show result with ansi codes",
            format!("\x1b[1m{table}\x1b[0m"),
            indoc! {r#"

                <blue+bold>Previewing node 'customers':</>
                <bold>| id | name  |
                | -- | ----- |
                | 1  | Alice |</>
            "#},
            indoc! {r#"
                <bold>| id | name  |
                | -- | ----- |
                | 1  | Alice |</>
            "#},
        ),
    ] {
        cases.push(
            Case::new(label)
                .run(move |_| {
                    emit(
                        SeverityNumber::Info,
                        ShowResult {
                            content: content.clone(),
                            title: "Previewing node 'customers':".to_string(),
                            ..Default::default()
                        },
                        "",
                    )
                })
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        03:04:05.678901 [info ]: Previewing node 'customers':
                        | id | name  |
                        | -- | ----- |
                        | 1  | Alice |
                    "#},
                )
                .expect(TERMINAL, terminal)
                .expect(TERMINAL_QUIET, quiet),
        );
    }
}

fn compiled_code(cases: &mut Vec<Case>) {
    let terminal_compile = Sink::Terminal(
        "terminal compile",
        Terminal::INTERACTIVE.command(FsCommand::Compile),
    );
    let terminal_compile_render = Sink::Terminal(
        "terminal compile --show progress-render",
        Terminal::INTERACTIVE
            .command(FsCommand::Compile)
            .show(&[ShowOptions::ProgressRender]),
    );
    let terminal_compile_quiet = Sink::Terminal(
        "terminal compile --quiet",
        Terminal {
            show: &[],
            quiet: true,
            command: FsCommand::Compile,
            ..Terminal::INTERACTIVE
        },
    );
    let terminal_compile_nothing = Sink::Terminal(
        "terminal compile --show none",
        Terminal::INTERACTIVE.command(FsCommand::Compile).show(&[]),
    );

    cases.push(
        Case::new("project node")
            .run(|_| {
                emit(
                    SeverityNumber::Debug,
                    CompiledCode {
                        relative_path: "models/customers.sql".to_string(),
                        sql: "select 1 as id".to_string(),
                        unique_id: "model.jaffle_shop.customers".to_string(),
                        node_name: "customers".to_string(),
                    },
                    "",
                )
            })
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.678901 [debug]: Compiled SQL for node model.jaffle_shop.customers at models/customers.sql
                "#},
            )
            .expect_no_output(terminal_compile)
            .expect_no_output(terminal_compile_render)
            .expect_no_output(terminal_compile_quiet)
            .expect_no_output(terminal_compile_nothing)
            .expect_no_output(TERMINAL),
    );
    cases.push(
        Case::new("inline query")
            .run(|_| {
                emit(
                    SeverityNumber::Info,
                    CompiledCodeInline {
                        sql: "select *\nfrom analytics.customers".to_string(),
                    },
                    "",
                )
            })
            .expect(
                DBT_LOG_DEBUG,
                indoc! {r#"
                    03:04:05.678901 [info ]: Compiled inline node is:
                    select *
                    from analytics.customers
                "#},
            )
            .expect(
                terminal_compile,
                indoc! {r#"
                    <blue+bold>Compiled inline node is:</>
                    select *
                    from analytics.customers
                "#},
            )
            .expect(
                terminal_compile_render,
                indoc! {r#"
                    <blue+bold>Compiled inline node is:</>
                    select *
                    from analytics.customers
                "#},
            )
            .expect(
                terminal_compile_quiet,
                indoc! {r#"
                    select *
                    from analytics.customers
                "#},
            )
            .expect_no_output(terminal_compile_nothing)
            .expect(
                TERMINAL,
                indoc! {r#"
                    <blue+bold>Compiled inline node is:</>
                    select *
                    from analytics.customers
                "#},
            ),
    );
}
