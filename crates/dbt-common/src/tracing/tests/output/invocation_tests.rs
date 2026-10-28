//! Invocation header and summary, and what the terminal holds back until the end.

use std::time::Duration;

use dbt_telemetry::{
    HookOutcome, HookProcessed, HookType, LogMessage, NodeOutcome, NodeSkipReason, NodeType,
    TestOutcome,
};
use indoc::{formatdoc, indoc};

use super::events::{data_test, invocation, node, node_group, processed, test_detail};
use super::support::{Case, Clock, DBT_LOG, TERMINAL, TERMINAL_NO_COLOR, TERMINAL_QUIET, TEXT};
use crate::tracing::{
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    emit::{create_info_span, emit_error_event, emit_warn_event},
    metrics::increment_metric,
};

pub(super) fn cases(cases: &mut Vec<Case>) {
    eventful_build_summary(cases);
    summary_variants(cases);
    summary_durations(cases);
}

fn warn(code: u32, message: &str) {
    emit_warn_event(
        LogMessage {
            code: Some(code),
            ..LogMessage::new_from_level(tracing::Level::WARN)
        },
        Some(message),
    );
}

fn error(code: u32, message: &str) {
    emit_error_event(
        LogMessage {
            code: Some(code),
            ..LogMessage::new_from_level(tracing::Level::ERROR)
        },
        Some(message),
    );
}

/// A build with a result of every kind, two problems and a failed test.
fn eventful_build(clock: &Clock) {
    increment_metric(FusionMetricKey::NodeCounts(NodeType::Model), 3);
    increment_metric(FusionMetricKey::NodeCounts(NodeType::Test), 2);
    increment_metric(FusionMetricKey::NodeCounts(NodeType::Seed), 1);

    node_group(|| {
        processed(clock, node(NodeType::Seed));

        let mut failed = data_test(TestOutcome::Failed, 2);
        test_detail(&mut failed).store_failures = Some(true);
        processed(clock, failed);
        processed(clock, data_test(TestOutcome::Warned, 1));

        let mut errored = node(NodeType::Model);
        errored.set_node_outcome(NodeOutcome::Error);
        processed(clock, errored);
        error(1060, "Model 'customers' failed: column 'id' does not exist");

        let mut skipped = node(NodeType::Model);
        skipped.name = "orders".to_string();
        skipped.identifier = Some("orders".to_string());
        skipped.set_node_outcome(NodeOutcome::Skipped);
        skipped.set_node_skip_reason(NodeSkipReason::Upstream);
        processed(clock, skipped);

        let mut reused = node(NodeType::Model);
        reused.name = "payments".to_string();
        reused.identifier = Some("payments".to_string());
        reused.set_node_outcome(NodeOutcome::Skipped);
        reused.set_node_skip_reason(NodeSkipReason::Cached);
        processed(clock, reused);
    });

    let hook = create_info_span(HookProcessed {
        package_name: "jaffle_shop".to_string(),
        name: Some("jaffle_shop-on-run-end-0".to_string()),
        hook_type: HookType::OnRunEnd as i32,
        hook_outcome: HookOutcome::Success as i32,
        ..Default::default()
    });
    clock.advance_ms(30);
    drop(hook);

    warn(
        1005,
        "Unused config key 'materialised' in `models/schema.yml`",
    );
    clock.advance(Duration::from_secs(73));
}

fn eventful_build_summary(cases: &mut Vec<Case>) {
    cases.push(
        Case::new("build with every kind of result")
            .root(invocation("build"))
            .run(eventful_build)
            .expect(
                DBT_LOG,
                indoc! {r#"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started seed analytics.raw_orders
                    03:04:05.928901 [info ]:  Succeeded seed  analytics.raw_orders (seed) [3 of 12 in 0.25s]
                    03:04:05.928901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:06.178901 [info ]:     Failed test  analytics_dbt_test__audit.not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    03:04:06.178901 [info ]: Test failed (2 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:06.178901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:06.428901 [info ]:     Warned test  not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    03:04:06.428901 [info ]: Started model analytics.customers
                    03:04:06.678901 [info ]:     Failed model analytics.customers (table) [3 of 12 in 0.25s]
                    03:04:06.678901 [error]: [UnusedConfigKey (dbt1060)]: Model 'customers' failed: column 'id' does not exist
                    03:04:06.678901 [info ]: Started model analytics.orders
                    03:04:06.928901 [info ]:    Skipped model analytics.orders (table) [3 of 12]
                    03:04:06.928901 [info ]: Started model analytics.payments
                    03:04:07.178901 [info ]:     Reused model analytics.payments (table) [3 of 12]
                    03:04:07.208901 [warn ]: [InvalidConfig (dbt1005)]: Unused config key 'materialised' in `models/schema.yml`
                    03:05:20.208901 [info ]: ⏎
                    ==================== Execution Summary =====================
                    Finished 'build' with 1 warning and 1 error for target 'dev' [1m 15s]
                    Processed: 1 hook | 3 models | 2 tests | 1 seed
                    Summary: 7 total | 2 success | 1 reused | 1 warn | 2 error | 1 skipped
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"
                    <green+bold> Succeeded</> seed  <cyan+bold>analytics.</><blue+bold>raw_orders</> (seed) [3 of 12 in 0.25s]
                    <red+bold>    Failed</> test  <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    <yellow+bold>    Warned</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [3 of 12 in 0.25s]
                    <yellow+bold>   Skipped</> model <cyan+bold>analytics.</><blue+bold>orders</> (table) [3 of 12]
                    <green+bold>    Reused</> model <cyan+bold>analytics.</><blue+bold>payments</> (table) [3 of 12]
                    <green+bold>  Finished</> [  0.03s] hook  <blue+bold>jaffle_shop-on-run-end-0</>

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (2 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>build</>' with <red+bold>1 warning</> and <red+bold>1 error</> for target '<white+bold>dev</>' <dim>[1m 15s]</>
                    Processed: 1 hook | 3 models | 2 tests | 1 seed
                    Summary: <white+bold>7 total</> | <green+bold>2 success</> | <magenta+bold>1 reused</> | <yellow+bold>1 warn</> | <red+bold>2 error</> | <dim>1 skipped</>
                "#},
            )
            .expect_stderr(
                TERMINAL,
                indoc! {r#"

                    <dim>=================== Errors and Warnings ====================</>
                    <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: Model '<yellow>customers</>' failed: column '<yellow>id</>' does not exist
                    <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: Unused config key 'materialised' in `<yellow>models/schema.yml</>`
                "#},
            )
            .expect(
                TERMINAL_NO_COLOR,
                indoc! {r#"
                     Succeeded seed  analytics.raw_orders (seed) [3 of 12 in 0.25s]
                        Failed test  analytics_dbt_test__audit.not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                        Warned test  not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                        Failed model analytics.customers (table) [3 of 12 in 0.25s]
                       Skipped model analytics.orders (table) [3 of 12]
                        Reused model analytics.payments (table) [3 of 12]
                      Finished [  0.03s] hook  jaffle_shop-on-run-end-0

                    ====================== Test Failures =======================
                    Test failed (2 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id

                    ==================== Execution Summary =====================
                    Finished 'build' with 1 warning and 1 error for target 'dev' [1m 15s]
                    Processed: 1 hook | 3 models | 2 tests | 1 seed
                    Summary: 7 total | 2 success | 1 reused | 1 warn | 2 error | 1 skipped
                "#},
            )
            .expect_stderr(
                TERMINAL_NO_COLOR,
                indoc! {r#"

                    =================== Errors and Warnings ====================
                    [error] [UnusedConfigKey (dbt1060)]: Model 'customers' failed: column 'id' does not exist
                    [warning] [InvalidConfig (dbt1005)]: Unused config key 'materialised' in `models/schema.yml`
                "#},
            )
            .expect(
                TEXT,
                indoc! {r#"
                    <green+bold> Succeeded</> seed  <cyan+bold>analytics.</><blue+bold>raw_orders</> (seed) [3 of 12 in 0.25s]
                    <red+bold>    Failed</> test  <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    <yellow+bold>    Warned</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [3 of 12 in 0.25s]
                    <yellow+bold>   Skipped</> model <cyan+bold>analytics.</><blue+bold>orders</> (table) [3 of 12]
                    <green+bold>    Reused</> model <cyan+bold>analytics.</><blue+bold>payments</> (table) [3 of 12]
                    <green+bold>  Finished</> [  0.03s] hook  <blue+bold>jaffle_shop-on-run-end-0</>

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (2 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>build</>' with <red+bold>1 warning</> and <red+bold>1 error</> for target '<white+bold>dev</>' <dim>[1m 15s]</>
                    Processed: 1 hook | 3 models | 2 tests | 1 seed
                    Summary: <white+bold>7 total</> | <green+bold>2 success</> | <magenta+bold>1 reused</> | <yellow+bold>1 warn</> | <red+bold>2 error</> | <dim>1 skipped</>
                "#},
            )
            .expect_stderr(
                TEXT,
                indoc! {r#"

                    <dim>=================== Errors and Warnings ====================</>
                    <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: Model '<yellow>customers</>' failed: column '<yellow>id</>' does not exist
                    <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: Unused config key 'materialised' in `<yellow>models/schema.yml</>`
                "#},
            )
            .expect(
                TERMINAL_QUIET,
                indoc! {r#"

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (2 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id
                "#},
            )
            .expect_stderr(
                TERMINAL_QUIET,
                indoc! {r#"

                    <dim>=================== Errors and Warnings ====================</>
                    <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: Model '<yellow>customers</>' failed: column '<yellow>id</>' does not exist
                    <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: Unused config key 'materialised' in `<yellow>models/schema.yml</>`
                "#},
            ),
    );
}

fn summary_variants(cases: &mut Vec<Case>) {
    // These commands print the same summary, only the command name differs.
    for command in [
        "build", "compile", "run", "sample", "seed", "snapshot", "test", "parse", "debug",
    ] {
        cases.push(
            Case::new(format!("{command}, nothing ran"))
                .root(invocation(command))
                .run(|clock| clock.advance_ms(1_234))
                .expect(
                    DBT_LOG,
                    &formatdoc! {r#"
                        ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                        03:04:06.912901 [info ]: ⏎
                        ==================== Execution Summary =====================
                        Finished '{command}' successfully for target 'dev' [1.2s]
                    "#},
                )
                .expect(
                    TERMINAL,
                    &formatdoc! {r#"

                        <dim>==================== Execution Summary =====================</>
                        Finished '<white+bold>{command}</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[1.2s]</>
                    "#},
                ),
        );
    }

    // These commands print no summary.
    for command in ["man", "login"] {
        cases.push(
            Case::new(format!("{command}, nothing ran"))
                .root(invocation(command))
                .run(|clock| clock.advance_ms(1_234))
                .expect(
                    DBT_LOG,
                    indoc! {r#"
                        ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    "#},
                )
                .expect_no_output(TERMINAL),
        );
    }

    let mut no_target = invocation("run");
    no_target.eval_args.as_mut().expect("eval args").target = None;
    cases.push(
        Case::new("no target")
            .root(no_target)
            .expect(
                DBT_LOG,
                indoc! {r#"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: ⏎
                    ==================== Execution Summary =====================
                    Finished 'run' successfully [0ns]
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>run</>' <green+bold>successfully</> <dim>[0ns]</>
                "#},
            ),
    );

    let mut no_args = invocation("run");
    no_args.eval_args = None;
    cases.push(
        Case::new("no command")
            .root(no_args)
            .expect(
                DBT_LOG,
                indoc! {r#"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: ⏎
                    ==================== Execution Summary =====================
                    Finished 'unknown' successfully [0ns]
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>unknown</>' <green+bold>successfully</> <dim>[0ns]</>
                "#},
            ),
    );

    for (label, warnings, errors, dbt_log, terminal, terminal_stderr) in [
        (
            "1 warning",
            1,
            0,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [warn ]: [InvalidConfig (dbt1005)]: a warning
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'run' with 1 warning for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>run</>' with <yellow+bold>1 warning</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
            indoc! {r#"

                <dim>=================== Errors and Warnings ====================</>
                <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: a warning
            "#},
        ),
        (
            "2 warnings",
            2,
            0,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [warn ]: [InvalidConfig (dbt1005)]: a warning
                03:04:05.678901 [warn ]: [InvalidConfig (dbt1005)]: a warning
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'run' with 2 warnings for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>run</>' with <yellow+bold>2 warnings</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
            indoc! {r#"

                <dim>=================== Errors and Warnings ====================</>
                <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: a warning
                <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: a warning
            "#},
        ),
        (
            "1 error",
            0,
            1,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [error]: [UnusedConfigKey (dbt1060)]: an error
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'run' with 1 error for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>run</>' with <red+bold>1 error</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
            indoc! {r#"

                <dim>=================== Errors and Warnings ====================</>
                <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: an error
            "#},
        ),
        (
            "2 errors",
            0,
            2,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [error]: [UnusedConfigKey (dbt1060)]: an error
                03:04:05.678901 [error]: [UnusedConfigKey (dbt1060)]: an error
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'run' with 2 errors for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>run</>' with <red+bold>2 errors</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
            indoc! {r#"

                <dim>=================== Errors and Warnings ====================</>
                <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: an error
                <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: an error
            "#},
        ),
        (
            "warnings and errors",
            2,
            1,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [warn ]: [InvalidConfig (dbt1005)]: a warning
                03:04:05.678901 [warn ]: [InvalidConfig (dbt1005)]: a warning
                03:04:05.678901 [error]: [UnusedConfigKey (dbt1060)]: an error
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'run' with 2 warnings and 1 error for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>run</>' with <red+bold>2 warnings</> and <red+bold>1 error</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
            indoc! {r#"

                <dim>=================== Errors and Warnings ====================</>
                <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: a warning
                <yellow+bold>[warning]</> [InvalidConfig (dbt1005)]: a warning
                <red+bold>[error]</> [UnusedConfigKey (dbt1060)]: an error
            "#},
        ),
    ] {
        cases.push(
            Case::new(label)
                .root(invocation("run"))
                .run(move |_| {
                    for _ in 0..warnings {
                        warn(1005, "a warning");
                    }
                    for _ in 0..errors {
                        error(1060, "an error");
                    }
                })
                .expect(DBT_LOG, dbt_log)
                .expect(TERMINAL, terminal)
                .expect_stderr(TERMINAL, terminal_stderr),
        );
    }

    cases.push(
        Case::new("autofix suggestions")
            .root(invocation("parse"))
            .run(|_| {
                increment_metric(
                    FusionMetricKey::InvocationMetric(InvocationMetricKey::AutoFixSuggestions),
                    2,
                );
            })
            .expect(
                DBT_LOG,
                indoc! {r#"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: suggestion: Run 'dbt deps' to see the latest dbt v2 compatible packages. For compatibility errors, try the autofix script: https://github.com/dbt-labs/dbt-autofix. Upgrading to dbt v2 requires updates to your project -- work with your team and follow our upgrade guide: https://docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2
                    03:04:05.678901 [info ]: ⏎
                    ==================== Execution Summary =====================
                    Finished 'parse' successfully for target 'dev' [0ns]
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"
                    <blue+bold>suggestion:</> Run '<yellow+bold>dbt deps</>' to see the latest dbt v2 compatible packages. For compatibility errors, try the autofix script: <blue+bold>https://github.com/dbt-labs/dbt-autofix</>. Upgrading to dbt v2 requires updates to your project -- work with your team and follow our upgrade guide: <blue+bold>https://docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2</>

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>parse</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
                "#},
            )
            .expect(
                TERMINAL_QUIET,
                indoc! {r#"
                    <blue+bold>suggestion:</> Run '<yellow+bold>dbt deps</>' to see the latest dbt v2 compatible packages. For compatibility errors, try the autofix script: <blue+bold>https://github.com/dbt-labs/dbt-autofix</>. Upgrading to dbt v2 requires updates to your project -- work with your team and follow our upgrade guide: <blue+bold>https://docs.getdbt.com/docs/dbt-versions/dbt-upgrade/upgrading-to-v2</>
                "#},
            ),
    );

    for (label, statically_checked, aggregated_tests, aggregated_queries, dbt_log, terminal) in [
        (
            "statically checked tests",
            3,
            0,
            0,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Test Optimization =====================
                3 tests statically checked (0 queries)

                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Test Optimization =====================</>
                3 tests statically checked (0 queries)

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
        (
            "1 statically checked test",
            1,
            0,
            0,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Test Optimization =====================
                1 test statically checked (0 queries)

                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Test Optimization =====================</>
                1 test statically checked (0 queries)

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
        (
            "batched tests",
            0,
            8,
            2,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Test Optimization =====================
                8 tests batched (reduced to 2 queries)

                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Test Optimization =====================</>
                8 tests batched (reduced to 2 queries)

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
        (
            "1 batched query",
            0,
            2,
            1,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Test Optimization =====================
                2 tests batched (reduced to 1 query)

                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Test Optimization =====================</>
                2 tests batched (reduced to 1 query)

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
        (
            "batched tests without queries",
            0,
            8,
            0,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
        (
            "both optimizations",
            4,
            4,
            2,
            indoc! {r#"
                ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                03:04:05.678901 [info ]: ⏎
                ==================== Test Optimization =====================
                4 tests statically checked (0 queries)
                4 tests batched (reduced to 2 queries)

                ==================== Execution Summary =====================
                Finished 'test' successfully for target 'dev' [0ns]
            "#},
            indoc! {r#"

                <dim>==================== Test Optimization =====================</>
                4 tests statically checked (0 queries)
                4 tests batched (reduced to 2 queries)

                <dim>==================== Execution Summary =====================</>
                Finished '<white+bold>test</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[0ns]</>
            "#},
        ),
    ] {
        cases.push(
            Case::new(label)
                .root(invocation("test"))
                .run(move |_| {
                    for (key, value) in [
                        (
                            InvocationMetricKey::StaticallyCheckedTests,
                            statically_checked,
                        ),
                        (InvocationMetricKey::AggregatedTests, aggregated_tests),
                        (
                            InvocationMetricKey::AggregatedTestQueries,
                            aggregated_queries,
                        ),
                    ] {
                        increment_metric(FusionMetricKey::InvocationMetric(key), value);
                    }
                })
                .expect(DBT_LOG, dbt_log)
                .expect(TERMINAL, terminal),
        );
    }

    cases.push(
        Case::new("one node of each kind")
            .root(invocation("build"))
            .run(|clock| {
                for node_type in [
                    NodeType::Check,
                    NodeType::Function,
                    NodeType::UnitTest,
                    NodeType::Source,
                    NodeType::Snapshot,
                    NodeType::Model,
                    NodeType::Analysis,
                    NodeType::Exposure,
                ] {
                    increment_metric(FusionMetricKey::NodeCounts(node_type), 1);
                }
                node_group(|| {
                    let mut cancelled = node(NodeType::Model);
                    cancelled.set_node_outcome(NodeOutcome::Canceled);
                    processed(clock, cancelled);
                    let mut no_op = node(NodeType::Model);
                    no_op.set_node_outcome(NodeOutcome::Skipped);
                    no_op.set_node_skip_reason(NodeSkipReason::NoOp);
                    processed(clock, no_op);
                });
            })
            .expect(
                DBT_LOG,
                indoc! {r#"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started model analytics.customers
                    03:04:05.928901 [info ]:  Cancelled model analytics.customers (table) [3 of 12 in 0.25s]
                    03:04:05.928901 [info ]: Started model analytics.customers
                    03:04:06.178901 [info ]:    Skipped model analytics.customers (table) [3 of 12]
                    03:04:06.178901 [info ]: ⏎
                    ==================== Execution Summary =====================
                    Finished 'build' successfully for target 'dev' [500ms]
                    Processed: 1 model | 1 snapshot | 1 source | 1 exposure | 1 analysis | 1 unit test | 1 check | 1 function
                    Summary: 2 total | 1 canceled | 1 no-op
                "#},
            )
            .expect(
                TERMINAL,
                indoc! {r#"
                    <yellow+bold> Cancelled</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [3 of 12 in 0.25s]

                    <dim>==================== Execution Summary =====================</>
                    Finished '<white+bold>build</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[500ms]</>
                    Processed: 1 model | 1 snapshot | 1 source | 1 exposure | 1 analysis | 1 unit test | 1 check | 1 function
                    Summary: <white+bold>2 total</> | <dim>1 canceled</> | <dim>1 no-op</>
                "#},
            ),
    );
}

fn summary_durations(cases: &mut Vec<Case>) {
    // Only the elapsed time differs, so each row carries how it is shown.
    for (elapsed, shown) in [
        (Duration::ZERO, "0ns"),
        (Duration::from_nanos(800), "800ns"),
        (Duration::from_micros(450), "450us"),
        (Duration::from_millis(5), "5ms"),
        (Duration::from_millis(999), "999ms"),
        (Duration::from_millis(1_049), "1.0s"),
        (Duration::from_millis(59_960), "60.0s"),
        (Duration::from_secs(60), "1m"),
        (Duration::from_millis(60_400), "1m"),
        (Duration::from_secs(61), "1m 1s"),
        (Duration::from_secs(3_599), "59m 59s"),
        (Duration::from_secs(3_600), "1h"),
        (Duration::from_secs(3_660), "1h 1m"),
        (Duration::from_secs(3_661), "1h 1m 1s"),
        (Duration::from_secs(90_000), "25h"),
    ] {
        cases.push(
            Case::new(format!("{elapsed:?}"))
                .root(invocation("debug"))
                .run(move |clock| clock.advance(elapsed))
                .expect(
                    TERMINAL,
                    &formatdoc! {r#"

                        <dim>==================== Execution Summary =====================</>
                        Finished '<white+bold>debug</>' <green+bold>successfully</> for target '<white+bold>dev</>' <dim>[{shown}]</>
                    "#},
                ),
        );
    }
}
