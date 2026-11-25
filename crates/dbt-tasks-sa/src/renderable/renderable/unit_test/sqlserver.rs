//! SQL Server unit-test SQL shaping.
//!
//! T-SQL allows `WITH` only at the start of a statement, so the model under test
//! can't bring its own CTEs into the body of the `<model>_actual` CTE (Msg 156).
//! v1's `sqlserver__get_unit_test_sql`, which v2 never calls, created the model
//! as a view. Instead, SQL Server lifts the model's CTEs into the unit test's
//! `WITH` list, ahead of `<model>_actual`, whose body keeps the model's final
//! statement. The query keeps every other adapter's shape and needs no DDL.

use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::utils::{splice_ctes_into_leading_with, split_leading_ctes};

use crate::sql::dialect::sqlparser_dialect_for;

const MODEL_START: &str = "--dbt_sqlserver_unit_test:model_start\n";
const MODEL_END: &str = "\n--dbt_sqlserver_unit_test:model_end\n";

/// Brackets the model's raw SQL so it can be found once the unit test is
/// rendered and its refs are swapped for fixture CTE names.
pub(super) fn mark_model(raw_sql: &str) -> String {
    format!("\n{MODEL_START}{raw_sql}{MODEL_END}")
}

/// Moves the rendered model's own CTEs out of the `actual_cte` body and in
/// front of it. A model without a leading `WITH` is left as it is.
pub(super) fn lift_model_ctes(rendered: &str, actual_cte: &str) -> FsResult<String> {
    let missing = |what: &str| {
        fs_err!(
            ErrorCode::Unexpected,
            "Internal error: {what} is missing from the rendered SQL Server unit test"
        )
    };
    let start = rendered
        .find(MODEL_START)
        .ok_or_else(|| missing("the model start marker"))?
        + MODEL_START.len();
    let end = start
        + rendered[start..]
            .find(MODEL_END)
            .ok_or_else(|| missing("the model end marker"))?;
    let Some((model_ctes, statement)) = split_leading_ctes(
        &rendered[start..end],
        sqlparser_dialect_for(AdapterType::SqlServer),
    ) else {
        return Ok(rendered.to_string());
    };
    let declaration = rendered[..start]
        .rfind(&format!("{actual_cte} as ("))
        .ok_or_else(|| missing("the actual CTE"))?;
    Ok(format!(
        "{}{model_ctes},\n\t{}{}{}",
        &rendered[..declaration],
        &rendered[declaration..start],
        statement.trim_start(),
        &rendered[end..]
    ))
}

/// `WITH <ctes> <sql>`, splicing `ctes` into `sql`'s own `WITH` when it has one.
/// SQL Server can't nest a `WITH`.
pub(super) fn prefix_ctes(ctes: &str, sql: &str) -> String {
    match splice_ctes_into_leading_with(sql, ctes) {
        Some(sql) => sql,
        None => format!("WITH {ctes} {sql}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The unit test as rendered, with the given's ref already swapped
    /// for its fixture CTE.
    fn rendered(model_sql: &str) -> String {
        format!(
            "-- Build actual result given inputs\nWITH\n\
             \tTestDB_dbo_base as (select 1 as id),\n  \
             \tTestDB_dbo_m_expect as (select 1 as id),\n  \
             \tTestDB_dbo_m_actual as ({})\n\
             SELECT * FROM (SELECT id FROM TestDB_dbo_m_actual) unit_test_diff",
            mark_model(model_sql)
        )
    }

    #[test]
    fn lifts_the_model_ctes_ahead_of_the_actual_cte() {
        let sql = lift_model_ctes(
            &rendered("with src as (select id from TestDB_dbo_base)\nselect id from src"),
            "TestDB_dbo_m_actual",
        )
        .unwrap();
        assert_eq!(
            sql,
            format!(
                "-- Build actual result given inputs\nWITH\n\
                 \tTestDB_dbo_base as (select 1 as id),\n  \
                 \tTestDB_dbo_m_expect as (select 1 as id),\n  \
                 \tsrc as (select id from TestDB_dbo_base),\n\
                 \tTestDB_dbo_m_actual as (\n{MODEL_START}select id from src{MODEL_END})\n\
                 SELECT * FROM (SELECT id FROM TestDB_dbo_m_actual) unit_test_diff"
            )
        );
    }

    #[test]
    fn model_without_with_is_unchanged() {
        let rendered = rendered("select id from TestDB_dbo_base");
        assert_eq!(
            lift_model_ctes(&rendered, "TestDB_dbo_m_actual").unwrap(),
            rendered
        );
    }

    #[test]
    fn missing_marker_is_an_error() {
        assert!(lift_model_ctes("select 1", "x").is_err());
    }
}
