//! Node result lines: one per processed node, plus the test failure details that follow.
//!
//! Each case is a state the scheduler can report. Node types, materializations and
//! outcomes are mixed across cases rather than crossed, so every formatting branch is
//! covered once by a realistic node.

use dbt_telemetry::{
    ExecutionPhase, NodeCacheDetail, NodeCacheReason, NodeEvaluationDetail, NodeMaterialization,
    NodeOutcome, NodeProcessed, NodeSkipReason, NodeType, NodeWarningOutcome,
    SourceFreshnessDetail, SourceFreshnessOutcome, TestOutcome, node_processed::NodeOutcomeDetail,
};
use indoc::indoc;

use super::events::{
    aggregated_test_group, data_test, invocation, node, node_group, processed, test_detail,
    with_test_detail,
};
use super::support::{Case, DBT_LOG, Sink, TERMINAL, TEXT, Terminal, all_variants};
use crate::io_args::FsCommand;

pub(super) fn cases(cases: &mut Vec<Case>) {
    node_results(cases);
    reused_nodes(cases);
    test_results(cases);
    freshness_results(cases);
    skipped_tests(cases);
    aggregated_tests(cases);
    compile_reports_only_problems(cases);
}

/// A case processing the single node `node`.
#[track_caller]
fn node_case(name: impl std::fmt::Display, node: NodeProcessed) -> Case {
    Case::new(name).run(move |clock| node_group(|| processed(clock, node.clone())))
}

/// A model named `name`, materialized as `materialization`.
fn model(name: &str, materialization: NodeMaterialization) -> NodeProcessed {
    let mut model = node(NodeType::Model);
    model.unique_id = format!("model.jaffle_shop.{name}");
    model.name = name.to_string();
    model.identifier = Some(name.to_string());
    model.set_materialization(materialization);
    model
}

/// `node` as the `index`th of `total` nodes the run executes.
fn numbered(mut node: NodeProcessed, index: u32, total: u32) -> NodeProcessed {
    node.node_index = Some(index);
    node.node_count_total = Some(total);
    node
}

fn node_results(cases: &mut Vec<Case>) {
    for node_type in all_variants!(NodeType {
        Unspecified,
        Model,
        Seed,
        Snapshot,
        Source,
        Test,
        UnitTest,
        Macro,
        DocsMacro,
        Analysis,
        Operation,
        Exposure,
        Metric,
        SavedQuery,
        SemanticModel,
        Function,
        Check,
        Plugin,
    }) {
        match node_type {
            NodeType::Model => {
                cases.push(
                    node_case("table model succeeded", node(NodeType::Model))
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started model analytics.customers
                                03:04:05.928901 [info ]:  Succeeded model analytics.customers (table) [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [3 of 12 in 0.25s]\n",
                        ),
                );

                let mut failed = model("orders", NodeMaterialization::View);
                failed.set_node_outcome(NodeOutcome::Error);
                failed.duration_ms = Some(1_234);
                cases.push(
                    node_case("view model failed", failed)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started model analytics.orders
                                03:04:06.912901 [info ]:     Failed model analytics.orders (view) [3 of 12 in 1.23s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>orders</> (view) [3 of 12 in 1.23s]\n",
                        ),
                );

                // Skipped nodes never ran, so they report no duration.
                let mut skipped = model("order_items", NodeMaterialization::Incremental);
                skipped.set_node_outcome(NodeOutcome::Skipped);
                skipped.set_node_skip_reason(NodeSkipReason::Upstream);
                cases.push(
                    node_case(
                        "incremental model skipped after an upstream failure",
                        skipped,
                    )
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Started model analytics.order_items
                            03:04:05.928901 [info ]:    Skipped model analytics.order_items (incremental) [3 of 12]
                        "},
                    )
                    .expect(
                        TERMINAL,
                        "<yellow+bold>   Skipped</> model <cyan+bold>analytics.</><blue+bold>order_items</> (incremental) [3 of 12]\n",
                    ),
                );

                let mut cancelled =
                    model("customer_metrics", NodeMaterialization::MaterializedView);
                cancelled.set_node_outcome(NodeOutcome::Canceled);
                cancelled.duration_ms = Some(61_000);
                cases.push(
                    node_case("materialized view cancelled", cancelled)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started model analytics.customer_metrics
                                03:05:06.678901 [info ]:  Cancelled model analytics.customer_metrics (mat_view) [3 of 12 in 1m 1s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<yellow+bold> Cancelled</> model <cyan+bold>analytics.</><blue+bold>customer_metrics</> (mat_view) [3 of 12 in 1m 1s]\n",
                        ),
                );

                let mut rebuilt = model("customers_scd", NodeMaterialization::Custom);
                rebuilt.custom_materialization = Some("scd_type_2".to_string());
                rebuilt.sao_enabled = Some(true);
                cases.push(
                    node_case(
                        "custom materialization rebuilt by state-aware orchestration",
                        rebuilt,
                    )
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Started model analytics.customers_scd
                            03:04:05.928901 [info ]:  Succeeded model analytics.customers_scd (scd_type_2 - New changes detected) [3 of 12 in 0.25s]
                        "},
                    )
                    .expect(
                        TERMINAL,
                        "<green+bold> Succeeded</> model <cyan+bold>analytics.</><blue+bold>customers_scd</> (scd_type_2 - New changes detected) [3 of 12 in 0.25s]\n",
                    ),
                );

                // The adapter is only reported when nodes of one invocation can run on different ones.
                let mut warned = model("live_orders", NodeMaterialization::StreamingTable);
                warned.adapter = Some("bigquery".to_string());
                warned.duration_ms = Some(3_725_000);
                warned.node_outcome_detail = Some(NodeOutcomeDetail::NodeEvaluationDetail(
                    NodeEvaluationDetail {
                        node_warning_outcome: NodeWarningOutcome::WithWarnings as i32,
                    },
                ));
                cases.push(
                    node_case(
                        "streaming table on another adapter, with warnings",
                        warned,
                    )
                    .expect(
                        DBT_LOG,
                        indoc! {"
                            03:04:05.678901 [info ]: Started model analytics.live_orders
                            04:06:10.678901 [info ]:     Warned model analytics.live_orders (streaming, bigquery) [3 of 12 in 1h 2m]
                        "},
                    )
                    .expect(
                        TERMINAL,
                        "<yellow+bold>    Warned</> model <cyan+bold>analytics.</><blue+bold>live_orders</> (streaming, bigquery) [3 of 12 in 1h 2m]\n",
                    ),
                );
            }
            NodeType::Seed => {
                // A measured zero is still a duration. Qualifiers past 200 characters are truncated.
                let mut seed = node(NodeType::Seed);
                seed.duration_ms = Some(0);
                seed.schema = Some(format!("dbt_ci_{}", "pr_16486_".repeat(25)));
                cases.push(
                    node_case("seed loaded instantly into a long schema", seed)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started seed dbt_ci_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_...raw_orders
                                03:04:05.678901 [info ]:  Succeeded seed  dbt_ci_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_...raw_orders (seed) [3 of 12 in 0.00s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> seed  <cyan+bold>dbt_ci_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_pr_16486_...</><blue+bold>raw_orders</> (seed) [3 of 12 in 0.00s]\n",
                        ),
                );

                // Nodes outside the selection take no execution index, and the terminal hides them.
                let mut unselected = node(NodeType::Seed);
                unselected.in_selection = false;
                unselected.node_index = None;
                unselected.node_count_total = None;
                cases.push(
                    node_case("seed outside the selection", unselected)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started seed analytics.raw_orders
                                03:04:05.928901 [info ]:  Succeeded seed  analytics.raw_orders (seed) [0.25s]
                            "},
                        )
                        .expect_no_output(TERMINAL),
                );
            }
            NodeType::Snapshot => {
                // The terminal hides nodes with nothing to do.
                let mut no_op = node(NodeType::Snapshot);
                no_op.set_node_outcome(NodeOutcome::Skipped);
                no_op.set_node_skip_reason(NodeSkipReason::NoOp);
                cases.push(
                    node_case("snapshot with nothing to do", no_op)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started snapshot snapshots.orders_snapshot
                                03:04:05.928901 [info ]:    Skipped snapshot snapshots.orders_snapshot (snapshot) [3 of 12]
                            "},
                        )
                        .expect_no_output(TERMINAL),
                );
            }
            NodeType::Analysis => {
                cases.push(
                    node_case("analysis compiled", node(NodeType::Analysis))
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started analysis analytics.order_trends
                                03:04:05.928901 [info ]:  Succeeded analysis analytics.order_trends (analysis) [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> analysis <cyan+bold>analytics.</><blue+bold>order_trends</> (analysis) [3 of 12 in 0.25s]\n",
                        ),
                );
            }
            NodeType::Function => {
                // Other nodes are shown by their database alias.
                let mut aliased = node(NodeType::Function);
                aliased.identifier = Some("fn_to_cents".to_string());
                cases.push(
                    node_case("function with an alias", aliased)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started function analytics.fn_to_cents
                                03:04:05.928901 [info ]:  Succeeded function analytics.fn_to_cents (function) [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> function <cyan+bold>analytics.</><blue+bold>fn_to_cents</> (function) [3 of 12 in 0.25s]\n",
                        ),
                );
            }
            NodeType::Check => {
                // Checks write no relation, so they show no materialization.
                cases.push(
                    node_case("check succeeded", node(NodeType::Check))
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started check analytics.orders_are_recent
                                03:04:05.928901 [info ]:  Succeeded check analytics.orders_are_recent [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> check <cyan+bold>analytics.</><blue+bold>orders_are_recent</> [3 of 12 in 0.25s]\n",
                        ),
                );
            }
            NodeType::Plugin => {
                // Plugin nodes show their own kind, and write no relation.
                cases.push(
                    node_case("plugin node succeeded", node(NodeType::Plugin))
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started widget orders_widget
                                03:04:05.928901 [info ]:  Succeeded widget orders_widget [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> widget <cyan+bold></><blue+bold>orders_widget</> [3 of 12 in 0.25s]\n",
                        ),
                );
            }
            NodeType::SavedQuery => {
                let mut saved_query = node(NodeType::SavedQuery);
                saved_query.unique_id = "saved_query.jaffle_shop.weekly_orders".to_string();
                saved_query.name = "weekly_orders".to_string();
                saved_query.identifier = Some("weekly_orders".to_string());
                cases.push(
                    node_case("saved query exported", saved_query)
                        .expect(
                            DBT_LOG,
                            indoc! {"
                                03:04:05.678901 [info ]: Started saved query analytics.weekly_orders
                                03:04:05.928901 [info ]:  Succeeded saved_query analytics.weekly_orders [3 of 12 in 0.25s]
                            "},
                        )
                        .expect(
                            TERMINAL,
                            "<green+bold> Succeeded</> saved_query <cyan+bold>analytics.</><blue+bold>weekly_orders</> [3 of 12 in 0.25s]\n",
                        ),
                );
            }
            // Covered by `test_results`, `skipped_tests`, `aggregated_tests` and
            // `freshness_results`.
            NodeType::Test | NodeType::UnitTest | NodeType::Source => {}
            // Not run as nodes, so they have no result line.
            NodeType::Unspecified
            | NodeType::Macro
            | NodeType::DocsMacro
            | NodeType::Operation
            | NodeType::Exposure
            | NodeType::Metric
            | NodeType::SemanticModel => {}
        }
    }
}

/// A node of `node` reused for `reason`.
fn reused(mut node: NodeProcessed, detail: NodeCacheDetail) -> NodeProcessed {
    node.set_node_outcome(NodeOutcome::Skipped);
    node.set_node_skip_reason(NodeSkipReason::Cached);
    node.node_outcome_detail = Some(NodeOutcomeDetail::NodeCacheDetail(detail));
    node
}

fn cache_detail(reason: NodeCacheReason) -> NodeCacheDetail {
    NodeCacheDetail {
        node_cache_reason: reason as i32,
        build_after_seconds: None,
        last_updated_seconds: None,
    }
}

/// Every reason a node is reused, each on a different node.
fn reused_nodes(cases: &mut Vec<Case>) {
    for reason in all_variants!(NodeCacheReason {
        NoChanges,
        StillFresh,
        UpdateCriteriaNotMet,
        ClonedExisting,
        ClonedExistingStillFresh,
    }) {
        cases.push(match reason {
            NodeCacheReason::NoChanges => node_case(
                "table model reused without upstream changes",
                reused(node(NodeType::Model), cache_detail(reason)),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started model analytics.customers
                    03:04:05.928901 [info ]:     Reused model analytics.customers (table - No new changes on any upstreams) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Reused</> model <cyan+bold>analytics.</><blue+bold>customers</> (table - No new changes on any upstreams) [3 of 12]\n",
            ),
            NodeCacheReason::StillFresh => node_case(
                "incremental model reused within its lag tolerance",
                reused(
                    model("order_items", NodeMaterialization::Incremental),
                    NodeCacheDetail {
                        build_after_seconds: Some(3_600),
                        last_updated_seconds: Some(754),
                        ..cache_detail(reason)
                    },
                ),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started model analytics.order_items
                    03:04:05.928901 [info ]:     Reused model analytics.order_items (incremental - New changes detected within lag tolerance of 1h. Last updated 12m 34s ago) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Reused</> model <cyan+bold>analytics.</><blue+bold>order_items</> (incremental - New changes detected within lag tolerance of 1h. Last updated 12m 34s ago) [3 of 12]\n",
            ),
            NodeCacheReason::UpdateCriteriaNotMet => node_case(
                "seed reused, update criteria not met",
                reused(node(NodeType::Seed), cache_detail(reason)),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started seed analytics.raw_orders
                    03:04:05.928901 [info ]:     Reused seed  analytics.raw_orders (seed - No new changes on all upstreams) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Reused</> seed  <cyan+bold>analytics.</><blue+bold>raw_orders</> (seed - No new changes on all upstreams) [3 of 12]\n",
            ),
            NodeCacheReason::ClonedExisting => {
                let mut cloned = model("orders", NodeMaterialization::View);
                cloned.adapter = Some("snowflake".to_string());
                node_case(
                    "view cloned from another environment, on another adapter",
                    reused(cloned, cache_detail(reason)),
                )
                .expect(
                    DBT_LOG,
                    indoc! {"
                        03:04:05.678901 [info ]: Started model analytics.orders
                        03:04:05.928901 [info ]:     Reused model analytics.orders (view, snowflake - Cloned from other environment) [3 of 12]
                    "},
                )
                .expect(
                    TERMINAL,
                    "<green+bold>    Reused</> model <cyan+bold>analytics.</><blue+bold>orders</> (view, snowflake - Cloned from other environment) [3 of 12]\n",
                )
            }
            NodeCacheReason::ClonedExistingStillFresh => node_case(
                "snapshot cloned within its tolerance",
                reused(
                    node(NodeType::Snapshot),
                    NodeCacheDetail {
                        build_after_seconds: Some(86_400),
                        ..cache_detail(reason)
                    },
                ),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started snapshot snapshots.orders_snapshot
                    03:04:05.928901 [info ]:     Reused snapshot snapshots.orders_snapshot (snapshot - Cloned from other environment within tolerance) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Reused</> snapshot <cyan+bold>snapshots.</><blue+bold>orders_snapshot</> (snapshot - Cloned from other environment within tolerance) [3 of 12]\n",
            ),
        });
    }
}

/// Data tests show only their name unless they store failures, and where they are defined
/// once they do not pass. Failure details are printed when the invocation ends, so tests
/// that do not pass run under one.
fn test_results(cases: &mut Vec<Case>) {
    // The full name is shown even when the identifier was truncated, and state-aware
    // orchestration adds nothing to a test.
    let mut passed = data_test(TestOutcome::Passed, 0);
    passed.identifier = Some("not_null_customers_c3b1f9e2a7".to_string());
    passed.sao_enabled = Some(true);
    cases.push(
        node_case("data test passed", passed)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Passed test  not_null_customers_customer_id [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> [3 of 12 in 0.25s]\n",
            ),
    );

    // Warnings are not failures, so they carry no details.
    cases.push(
        node_case("data test warned", data_test(TestOutcome::Warned, 3))
            .root(invocation("man"))
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Warned test  not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<yellow+bold>    Warned</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]\n",
            ),
    );

    let mut singular = data_test(TestOutcome::Failed, 3);
    singular.name = "assert_positive_totals".to_string();
    singular.relative_path = "tests/assert_positive_totals.sql".to_string();
    singular.defined_at_line = None;
    singular.defined_at_col = None;
    cases.push(
        node_case("singular data test failed", singular)
            .root(invocation("man"))
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.assert_positive_totals
                    03:04:05.928901 [info ]:     Failed test  assert_positive_totals (tests/assert_positive_totals.sql) [3 of 12 in 0.25s]
                    03:04:05.928901 [info ]: Test failed (3 failed row(s)): assert_positive_totals
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> test  <cyan+bold></><blue+bold>assert_positive_totals</> (tests/assert_positive_totals.sql) [3 of 12 in 0.25s]

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (3 failed row(s)): assert_positive_totals
                "},
            ),
    );

    let mut stored = data_test(TestOutcome::Failed, 3);
    test_detail(&mut stored).store_failures = Some(true);
    cases.push(
        node_case("data test failed, storing failures", stored)
            .root(invocation("man"))
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Failed test  analytics_dbt_test__audit.not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    03:04:05.928901 [info ]: Test failed (3 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> test  <cyan+bold>analytics_dbt_test__audit.</><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (3 failed row(s)): not_null_customers_customer_id
                    See test failures:
                    select * from warehouse.analytics_dbt_test__audit.not_null_customers_customer_id
                "},
            ),
    );

    // A test that could not run has no verdict.
    let mut errored = node(NodeType::Test);
    errored.set_node_outcome(NodeOutcome::Error);
    cases.push(
        node_case("data test errored", errored)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Failed test  not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<red+bold>    Failed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]\n",
            ),
    );

    // Statically checked and reused verdicts never ran, so they report no duration.
    let mut statically_checked = data_test(TestOutcome::Passed, 0);
    test_detail(&mut statically_checked).statically_checked = Some(true);
    cases.push(
        node_case("data test checked statically", statically_checked)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Passed test  not_null_customers_customer_id (Statically checked) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (Statically checked) [3 of 12]\n",
            ),
    );

    let mut reused_verdict = data_test(TestOutcome::Warned, 3);
    reused_verdict.set_node_skip_reason(NodeSkipReason::Cached);
    cases.push(
        node_case("data test warning reused", reused_verdict)
            .root(invocation("man"))
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.928901 [info ]:     Warned test  not_null_customers_customer_id (models/schema.yml:12:15) [3 of 12]
                "},
            )
            .expect(
                TERMINAL,
                "<yellow+bold>    Warned</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12]\n",
            ),
    );

    cases.push(
        node_case(
            "unit test passed",
            with_test_detail(node(NodeType::UnitTest), TestOutcome::Passed, 0),
        )
        .expect(
            DBT_LOG,
            indoc! {"
                03:04:05.678901 [info ]: Started unit test analytics_dbt_test__audit.test_customer_totals
                03:04:05.928901 [info ]:     Passed unit_test analytics_dbt_test__audit.test_customer_totals [3 of 12 in 0.25s]
            "},
        )
        .expect(
            TERMINAL,
            "<green+bold>    Passed</> unit_test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>test_customer_totals</> [3 of 12 in 0.25s]\n",
        ),
    );

    let mut with_diff = with_test_detail(node(NodeType::UnitTest), TestOutcome::Failed, 1);
    with_diff.defined_at_col = None;
    test_detail(&mut with_diff).diff_table =
        Some("| id | total |\n| -- | ----- |\n| 1  | 10 -> 12 |".to_string());
    cases.push(
        node_case("unit test failed with a diff", with_diff)
            .root(invocation("man"))
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started unit test analytics_dbt_test__audit.test_customer_totals
                    03:04:05.928901 [info ]:     Failed unit_test analytics_dbt_test__audit.test_customer_totals (models/schema.yml:40) [3 of 12 in 0.25s]
                    03:04:05.928901 [info ]: Test failed: test_customer_totals
                    | id | total |
                    | -- | ----- |
                    | 1  | 10 -> 12 |
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> unit_test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>test_customer_totals</> (models/schema.yml:40) [3 of 12 in 0.25s]

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</>: test_customer_totals
                    | id | total |
                    | -- | ----- |
                    | 1  | 10 -> 12 |
                "},
            ),
    );
}

/// A node of `node_type` whose freshness check reached `outcome`, last updated
/// `age_seconds` ago.
fn freshness_checked(
    node_type: NodeType,
    outcome: SourceFreshnessOutcome,
    age_seconds: Option<i64>,
) -> NodeProcessed {
    let mut checked = node(node_type);
    checked.set_last_phase(ExecutionPhase::FreshnessAnalysis);
    checked.node_outcome_detail = Some(NodeOutcomeDetail::NodeFreshnessOutcome(
        SourceFreshnessDetail {
            node_freshness_outcome: outcome as i32,
            age_seconds,
        },
    ));
    checked
}

fn freshness_results(cases: &mut Vec<Case>) {
    for outcome in all_variants!(SourceFreshnessOutcome {
        OutcomePassed,
        OutcomeWarned,
        OutcomeFailed,
    }) {
        cases.push(match outcome {
            SourceFreshnessOutcome::OutcomePassed => node_case(
                "fresh source",
                freshness_checked(NodeType::Source, outcome, Some(3_725)),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started source jaffle_raw.orders
                    03:04:05.928901 [info ]:     Passed source jaffle_raw.orders (last updated 1h 2m 5s ago) [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<green+bold>    Passed</> source <cyan+bold>jaffle_raw.</><blue+bold>orders</> (last updated 1h 2m 5s ago) [3 of 12 in 0.25s]\n",
            ),
            SourceFreshnessOutcome::OutcomeWarned => node_case(
                "source past its warning threshold",
                freshness_checked(NodeType::Source, outcome, Some(90_000)),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started source jaffle_raw.orders
                    03:04:05.928901 [info ]:     Warned source jaffle_raw.orders (last updated 1day 1h ago) [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<yellow+bold>    Warned</> source <cyan+bold>jaffle_raw.</><blue+bold>orders</> (last updated 1day 1h ago) [3 of 12 in 0.25s]\n",
            ),
            SourceFreshnessOutcome::OutcomeFailed => node_case(
                "stale source of unknown age",
                freshness_checked(NodeType::Source, outcome, None),
            )
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started source jaffle_raw.orders
                    03:04:05.928901 [info ]:      Stale source jaffle_raw.orders (last updated unknown ago) [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<red+bold>     Stale</> source <cyan+bold>jaffle_raw.</><blue+bold>orders</> (last updated unknown ago) [3 of 12 in 0.25s]\n",
            ),
        });
    }

    let mut errored = node(NodeType::Source);
    errored.set_last_phase(ExecutionPhase::FreshnessAnalysis);
    errored.set_node_outcome(NodeOutcome::Error);
    cases.push(
        node_case("source freshness check errored", errored)
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started source jaffle_raw.orders
                    03:04:05.928901 [info ]:     Failed source jaffle_raw.orders [3 of 12 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                "<red+bold>    Failed</> source <cyan+bold>jaffle_raw.</><blue+bold>orders</> [3 of 12 in 0.25s]\n",
            ),
    );

    // Models have no source name, so they are qualified by their schema.
    cases.push(
        node_case(
            "model past its warning threshold",
            freshness_checked(
                NodeType::Model,
                SourceFreshnessOutcome::OutcomeWarned,
                Some(7_200),
            ),
        )
        .expect(
            DBT_LOG,
            indoc! {"
                03:04:05.678901 [info ]: Started model analytics.customers
                03:04:05.928901 [info ]:     Warned model analytics.customers (last updated 2h ago) [3 of 12 in 0.25s]
            "},
        )
        .expect(
            TERMINAL,
            "<yellow+bold>    Warned</> model <cyan+bold>analytics.</><blue+bold>customers</> (last updated 2h ago) [3 of 12 in 0.25s]\n",
        ),
    );
}

/// A test of `node_type` named `name`, skipped because its model failed.
fn skipped_test(node_type: NodeType, name: &str, index: u32, total: u32) -> NodeProcessed {
    let mut test = numbered(node(node_type), index, total);
    test.unique_id = format!("{}.jaffle_shop.{name}", node_type.as_static_ref());
    test.name = name.to_string();
    test.set_node_outcome(NodeOutcome::Skipped);
    test.set_node_skip_reason(NodeSkipReason::Upstream);
    test
}

/// The `customers` model failing as the first of `total` nodes.
fn failed_customers(total: u32) -> NodeProcessed {
    let mut failed = numbered(node(NodeType::Model), 1, total);
    failed.set_node_outcome(NodeOutcome::Error);
    failed
}

/// The terminal collapses tests skipped because of an upstream failure into one line,
/// printed once a node that was not skipped finishes, or when all nodes are done.
fn skipped_tests(cases: &mut Vec<Case>) {
    cases.push(
        Case::new("failed model skips its test, then another model runs")
            .run(|clock| {
                node_group(|| {
                    processed(clock, failed_customers(3));
                    processed(
                        clock,
                        skipped_test(NodeType::Test, "not_null_customers_customer_id", 2, 3),
                    );
                    processed(
                        clock,
                        numbered(model("orders", NodeMaterialization::Table), 3, 3),
                    );
                })
            })
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started model analytics.customers
                    03:04:05.928901 [info ]:     Failed model analytics.customers (table) [1 of 3 in 0.25s]
                    03:04:05.928901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:06.178901 [info ]:    Skipped test  not_null_customers_customer_id (models/schema.yml:12:15) [2 of 3]
                    03:04:06.178901 [info ]: Started model analytics.orders
                    03:04:06.428901 [info ]:  Succeeded model analytics.orders (table) [3 of 3 in 0.25s]
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [1 of 3 in 0.25s]
                    <yellow+bold>   Skipped</> test  '<yellow+bold>not_null_customers_customer_id</>'
                    <green+bold> Succeeded</> model <cyan+bold>analytics.</><blue+bold>orders</> (table) [3 of 3 in 0.25s]
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [1 of 3 in 0.25s]
                    <yellow+bold>   Skipped</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [2 of 3]
                    <green+bold> Succeeded</> model <cyan+bold>analytics.</><blue+bold>orders</> (table) [3 of 3 in 0.25s]
                "},
            ),
    );

    // More than three names are summarized.
    cases.push(
        Case::new("failed model skips its tests and unit tests, last in the run")
            .run(|clock| {
                node_group(|| {
                    processed(clock, failed_customers(5));
                    for (index, (node_type, name)) in [
                        (NodeType::Test, "not_null_customers_customer_id"),
                        (NodeType::Test, "unique_customers_customer_id"),
                        (NodeType::UnitTest, "test_customer_totals"),
                        (NodeType::Test, "accepted_values_customers_status"),
                    ]
                    .into_iter()
                    .enumerate()
                    {
                        processed(clock, skipped_test(node_type, name, index as u32 + 2, 5));
                    }
                })
            })
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started model analytics.customers
                    03:04:05.928901 [info ]:     Failed model analytics.customers (table) [1 of 5 in 0.25s]
                    03:04:05.928901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:06.178901 [info ]:    Skipped test  not_null_customers_customer_id (models/schema.yml:12:15) [2 of 5]
                    03:04:06.178901 [info ]: Started test analytics_dbt_test__audit.unique_customers_customer_id
                    03:04:06.428901 [info ]:    Skipped test  unique_customers_customer_id (models/schema.yml:12:15) [3 of 5]
                    03:04:06.428901 [info ]: Started unit test analytics_dbt_test__audit.test_customer_totals
                    03:04:06.678901 [info ]:    Skipped unit_test analytics_dbt_test__audit.test_customer_totals (models/schema.yml:40:7) [4 of 5]
                    03:04:06.678901 [info ]: Started test analytics_dbt_test__audit.accepted_values_customers_status
                    03:04:06.928901 [info ]:    Skipped test  accepted_values_customers_status (models/schema.yml:12:15) [5 of 5]
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [1 of 5 in 0.25s]
                    <yellow+bold>   Skipped</> test,unit_test '<yellow+bold>not_null_customers_customer_id</>', '<yellow+bold>unique_customers_customer_id</>' and 2 others
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [1 of 5 in 0.25s]
                    <yellow+bold>   Skipped</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [2 of 5]
                    <yellow+bold>   Skipped</> test  <cyan+bold></><blue+bold>unique_customers_customer_id</> (models/schema.yml:12:15) [3 of 5]
                    <yellow+bold>   Skipped</> unit_test <cyan+bold>analytics_dbt_test__audit.</><blue+bold>test_customer_totals</> (models/schema.yml:40:7) [4 of 5]
                    <yellow+bold>   Skipped</> test  <cyan+bold></><blue+bold>accepted_values_customers_status</> (models/schema.yml:12:15) [5 of 5]
                "},
            ),
    );

    // Skipped models keep a line of their own.
    cases.push(
        Case::new("failed model skips unit tests and a downstream model")
            .run(|clock| {
                node_group(|| {
                    processed(clock, failed_customers(4));
                    processed(
                        clock,
                        skipped_test(NodeType::UnitTest, "test_customer_totals", 2, 4),
                    );
                    processed(
                        clock,
                        skipped_test(NodeType::UnitTest, "test_customer_names", 3, 4),
                    );
                    let mut downstream =
                        numbered(model("customer_orders", NodeMaterialization::Table), 4, 4);
                    downstream.set_node_outcome(NodeOutcome::Skipped);
                    downstream.set_node_skip_reason(NodeSkipReason::Upstream);
                    processed(clock, downstream);
                })
            })
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [1 of 4 in 0.25s]
                    <yellow+bold>   Skipped</> unit_test '<yellow+bold>test_customer_totals</>', '<yellow+bold>test_customer_names</>'
                    <yellow+bold>   Skipped</> model <cyan+bold>analytics.</><blue+bold>customer_orders</> (table) [4 of 4]
                "},
            ),
    );
}

/// A member of the aggregated `not_null` query, named `name`.
fn member(name: &str, outcome: TestOutcome, duration_ms: u64) -> NodeProcessed {
    let mut test = data_test(outcome, if outcome == TestOutcome::Passed { 0 } else { 2 });
    test.name = name.to_string();
    test.duration_ms = Some(duration_ms);
    test_detail(&mut test).batch_unique_id =
        Some("test.jaffle_shop.aggregated_not_null_customers".to_string());
    test
}

/// Tests batched into one query are reported as a block on the terminal: a header with the
/// worst verdict and the query's duration, then one branch per member.
fn aggregated_tests(cases: &mut Vec<Case>) {
    cases.push(
        Case::new("batched tests all passed")
            .run(|clock| {
                node_group(|| {
                    aggregated_test_group("not_null", "model.jaffle_shop.customers", || {
                        for (index, name) in ["customer_id", "first_name"].into_iter().enumerate() {
                            processed(
                                clock,
                                member(
                                    &format!("not_null_customers_{name}"),
                                    TestOutcome::Passed,
                                    100 * (index as u64 + 1),
                                ),
                            );
                        }
                    })
                })
            })
            .expect(
                DBT_LOG,
                indoc! {"
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.778901 [info ]:     Passed test  not_null_customers_customer_id (Batched) [3 of 12 in 0.10s]
                    03:04:05.778901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_first_name
                    03:04:05.978901 [info ]:     Passed test  not_null_customers_first_name (Batched) [3 of 12 in 0.20s]
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold>    Passed</> test  <cyan+bold></><blue+bold>not_null on customers</> (2 batched) [0.20s]
                        ├─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</>
                        └─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_first_name</>
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <green+bold>    Passed</> test  <cyan+bold></><blue+bold>not_null on customers</> (2 batched) [0.20s]
                        ├─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</>
                        └─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_first_name</>
                "},
            ),
    );

    // The header shows the worst verdict; a failure's details follow at the end.
    cases.push(
        Case::new("batched tests with a warning and a failure")
            .root(invocation("man"))
            .run(|clock| {
                node_group(|| {
                    aggregated_test_group("not_null", "model.jaffle_shop.customers", || {
                        for (index, (name, outcome)) in [
                            ("customer_id", TestOutcome::Passed),
                            ("first_name", TestOutcome::Warned),
                            ("last_name", TestOutcome::Failed),
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            processed(
                                clock,
                                member(
                                    &format!("not_null_customers_{name}"),
                                    outcome,
                                    100 * (index as u64 + 1),
                                ),
                            );
                        }
                    })
                })
            })
            .expect(
                DBT_LOG,
                indoc! {"
                    ==================== 2026-01-02T03:04:05.678901Z | 0199c2a1-4b5e-7c3d-9a8f-1e2d3c4b5a69 ====================
                    03:04:05.678901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_customer_id
                    03:04:05.778901 [info ]:     Passed test  not_null_customers_customer_id (Batched) [3 of 12 in 0.10s]
                    03:04:05.778901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_first_name
                    03:04:05.978901 [info ]:     Warned test  not_null_customers_first_name (Batched - models/schema.yml:12:15) [3 of 12 in 0.20s]
                    03:04:05.978901 [info ]: Started test analytics_dbt_test__audit.not_null_customers_last_name
                    03:04:06.278901 [info ]:     Failed test  not_null_customers_last_name (Batched - models/schema.yml:12:15) [3 of 12 in 0.30s]
                    03:04:06.278901 [info ]: Test failed (2 failed row(s)): not_null_customers_last_name
                "},
            )
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> test  <cyan+bold></><blue+bold>not_null on customers</> (3 batched) [0.30s]
                        ├─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</>
                        ├─ <yellow+bold>Warned</> test  <cyan+bold></><blue+bold>not_null_customers_first_name</> (models/schema.yml:12:15)
                        └─ <red+bold>Failed</> test  <cyan+bold></><blue+bold>not_null_customers_last_name</> (models/schema.yml:12:15)

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (2 failed row(s)): not_null_customers_last_name
                "},
            )
            .expect(
                TEXT,
                indoc! {"
                    <red+bold>    Failed</> test  <cyan+bold></><blue+bold>not_null on customers</> (3 batched) [0.30s]
                        ├─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</>
                        ├─ <yellow+bold>Warned</> test  <cyan+bold></><blue+bold>not_null_customers_first_name</> (models/schema.yml:12:15)
                        └─ <red+bold>Failed</> test  <cyan+bold></><blue+bold>not_null_customers_last_name</> (models/schema.yml:12:15)

                    <dim>====================== Test Failures =======================</>
                    <red+bold>Test failed</> (2 failed row(s)): not_null_customers_last_name
                "},
            ),
    );

    cases.push(
        Case::new("batched tests of a versioned model on another adapter")
            .run(|clock| {
                node_group(|| {
                    aggregated_test_group("unique", "model.jaffle_shop.customers.v2", || {
                        let mut passed = member("unique_customers_id", TestOutcome::Passed, 40);
                        passed.adapter = Some("snowflake".to_string());
                        processed(clock, passed);
                        let mut email = member("unique_customers_email", TestOutcome::Passed, 60);
                        email.adapter = Some("snowflake".to_string());
                        processed(clock, email);
                    })
                })
            })
            .expect(
                TERMINAL,
                indoc! {"
                    <green+bold>    Passed</> test  <cyan+bold></><blue+bold>unique on customers.v2</> (snowflake - 2 batched) [0.06s]
                        ├─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>unique_customers_id</> (snowflake)
                        └─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>unique_customers_email</> (snowflake)
                "},
            ),
    );

    // Members that errored carry no verdict, so they keep a line of their own.
    cases.push(
        Case::new("batched tests with an errored member")
            .run(|clock| {
                node_group(|| {
                    aggregated_test_group("not_null", "model.jaffle_shop.customers", || {
                        processed(
                            clock,
                            member("not_null_customers_id", TestOutcome::Passed, 40),
                        );
                        let mut errored = node(NodeType::Test);
                        errored.set_node_outcome(NodeOutcome::Error);
                        processed(clock, errored);
                    })
                })
            })
            .expect(
                TERMINAL,
                indoc! {"
                    <red+bold>    Failed</> test  <cyan+bold></><blue+bold>not_null_customers_customer_id</> (models/schema.yml:12:15) [3 of 12 in 0.25s]
                    <green+bold>    Passed</> test  <cyan+bold></><blue+bold>not_null on customers</> (1 batched) [0.04s]
                        └─ <green+bold>Passed</> test  <cyan+bold></><blue+bold>not_null_customers_id</>
                "},
            ),
    );
}

/// `dbt compile` does not report nodes that compiled fine, unless debugging.
fn compile_reports_only_problems(cases: &mut Vec<Case>) {
    let terminal_compile = Sink::Terminal(
        "terminal compile",
        Terminal::INTERACTIVE.command(FsCommand::Compile),
    );
    let text_compile_debug = Sink::Terminal(
        "text compile --debug --show all",
        Terminal::DEBUG.command(FsCommand::Compile),
    );

    cases.push(
        node_case("compile, succeeded", node(NodeType::Model))
            .expect_no_output(terminal_compile)
            .expect_no_output(text_compile_debug),
    );

    let mut errored = node(NodeType::Model);
    errored.set_node_outcome(NodeOutcome::Error);
    cases.push(
        node_case("compile, failed", errored)
            .expect(
                terminal_compile,
                "<red+bold>    Failed</> model <cyan+bold>analytics.</><blue+bold>customers</> (table) [3 of 12 in 0.25s]\n",
            )
            .expect_no_output(text_compile_debug),
    );
}
