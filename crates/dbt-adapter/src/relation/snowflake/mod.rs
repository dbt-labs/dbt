pub(crate) mod config;

use dbt_schemas::schemas::relations::base::BaseRelation;

/// Create a working clone without replacing an existing destination or copying grants.
pub fn create_table_clone_sql(
    destination: &dyn BaseRelation,
    source: &dyn BaseRelation,
    transient: bool,
) -> String {
    let lifecycle = if transient { "transient " } else { "" };
    format!(
        "create {lifecycle}table {} clone {}",
        destination.render_self_as_str(),
        source.render_self_as_str()
    )
}

/// Reserve a working-table name atomically before its first materialization.
/// The normal CTAS replaces this empty placeholder before any audits run.
pub fn create_table_claim_sql(destination: &dyn BaseRelation, transient: bool) -> String {
    let lifecycle = if transient { "transient " } else { "" };
    format!(
        "create {lifecycle}table {} (__DBT_WAP_PLACEHOLDER boolean)",
        destination.render_self_as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::Relation;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::schemas::common::ResolvedQuoting;

    #[test]
    fn working_table_claim_is_create_only_and_preserves_quoting_and_lifecycle() {
        let destination = Relation::new(
            AdapterType::Snowflake,
            "My\"Database".to_owned(),
            "My.Schema".to_owned(),
            "__DBT_WAP_TEST".to_owned(),
        )
        .with_quoting(ResolvedQuoting::trues());
        for (transient, lifecycle) in [(false, ""), (true, "transient ")] {
            assert_eq!(
                create_table_claim_sql(&destination, transient),
                format!(
                    "create {lifecycle}table \"My\"\"Database\".\"My.Schema\".\"__DBT_WAP_TEST\" \
                     (__DBT_WAP_PLACEHOLDER boolean)"
                )
            );
        }
    }
}
