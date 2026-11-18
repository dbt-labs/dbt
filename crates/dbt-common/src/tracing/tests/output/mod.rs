//! Exact output of the human-readable sinks: the terminal (TUI) and `logs/dbt.log`.
//!
//! CLI golden tests only record `--log-format text`, while most users read the interactive
//! terminal and the file log. These tests pin both line by line, so they double as a
//! catalog of every formatting variant.
//!
//! Each `*_tests.rs` file contributes [`support::Case`]s: a scenario, the sinks it is
//! rendered into and the exact output of each. A single test runs them all, reports every
//! difference at once, then checks that the cases emit every public event type, so a new
//! event cannot go untested. Events no sink renders have cases in `unrendered_tests.rs`.
//!
//! Each case drives the real layer through a tracing subscriber, the same way production
//! code emits events. A virtual clock stamps every span and log record, so timestamps and
//! durations are reproducible.
//!
//! Where a line depends on a proto enum, the cases come from [`support::all_variants`],
//! whose exhaustive `match` stops compiling when the proto gains a variant, so the new
//! variant has to be added to the matrix.
//!
//! Expected output is written in a readable form:
//! - ANSI SGR sequences become tags: `\x1b[32m\x1b[1mPassed\x1b[0m` is `<green+bold>Passed</>`.
//! - A line ending in whitespace gets a trailing `⏎`, so editors cannot strip the padding.
//! - Output that does not end with a newline gets a final `\ no newline at end` line.

mod deps_tests;
mod evaluation_tests;
mod events;
mod invocation_tests;
mod log_tests;
mod node_tests;
mod operation_tests;
mod phase_tests;
mod query_tests;
mod support;
mod unrendered_tests;

#[test]
fn output_cases() {
    let mut cases = Vec::new();
    deps_tests::cases(&mut cases);
    evaluation_tests::cases(&mut cases);
    invocation_tests::cases(&mut cases);
    log_tests::cases(&mut cases);
    node_tests::cases(&mut cases);
    operation_tests::cases(&mut cases);
    phase_tests::cases(&mut cases);
    query_tests::cases(&mut cases);
    unrendered_tests::cases(&mut cases);
    support::check_cases(cases);
}
