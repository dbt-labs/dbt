//! Executed queries, in `dbt.log` and on the terminal.

use dbt_telemetry::{QueryExecuted, QueryOutcome};
use indoc::indoc;

use super::support::{Case, Clock, DBT_LOG, DBT_LOG_DEBUG, TEXT_DEBUG, all_variants};
use crate::tracing::emit::create_debug_span;

pub(super) fn cases(cases: &mut Vec<Case>) {
    queries(cases);
}

fn query(outcome: QueryOutcome) -> QueryExecuted {
    QueryExecuted {
        sql: "select *\nfrom analytics.customers".to_string(),
        sql_hash: "hash".to_string(),
        adapter_type: "snowflake".to_string(),
        query_description: Some("select customers".to_string()),
        query_id: Some("01b2c3d4-0000-1111".to_string()),
        unique_id: Some("model.jaffle_shop.customers".to_string()),
        query_outcome: outcome as i32,
        ..Default::default()
    }
}

fn execute(clock: &Clock, query: QueryExecuted) {
    let span = create_debug_span(query);
    clock.advance_ms(1_870);
    drop(span);
}

fn queries(cases: &mut Vec<Case>) {
    // The outcome does not change how a query is logged.
    for outcome in all_variants!(QueryOutcome {
        Unspecified,
        Success,
        Error,
        Canceled,
    }) {
        cases.push(
            Case::new(format!("{outcome:?}"))
                .run(move |clock| execute(clock, query(outcome)))
                .expect_no_output(DBT_LOG)
                .expect(
                    DBT_LOG_DEBUG,
                    indoc! {"
                        03:04:07.548901 [debug]: Query executed on node model.jaffle_shop.customers:
                        select *
                        from analytics.customers
                    "},
                )
                .expect(
                    TEXT_DEBUG,
                    indoc! {"
                        Query executed on node model.jaffle_shop.customers:
                        select *
                        from analytics.customers
                    "},
                ),
        );
    }

    cases.push(
        Case::new("not attributed to a node")
            .run(|clock| {
                execute(
                    clock,
                    QueryExecuted {
                        unique_id: None,
                        ..query(QueryOutcome::Success)
                    },
                )
            })
            .expect(
                DBT_LOG_DEBUG,
                indoc! {"
                    03:04:07.548901 [debug]: Query executed on node unknown:
                    select *
                    from analytics.customers
                "},
            )
            .expect(
                TEXT_DEBUG,
                indoc! {"
                    Query executed on node unknown:
                    select *
                    from analytics.customers
                "},
            ),
    );
}
