//! Redis transport session state belongs to each pooled connection.
use adbc_core::{
    error::Result,
    options::{OptionDatabase, OptionValue},
};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::ident::escape_string_literal;
use dbt_adbc::{Connection, database};
use dbt_auth::AdapterConfig;

pub(super) fn connection_fingerprint(
    adapter: AdapterType,
    config: &AdapterConfig,
    options: &[(OptionDatabase, OptionValue)],
) -> database::Fingerprint {
    // Flight does not accept schema/time_zone as database options. Include them
    // only in the pool key so a connection cannot leak settings across profiles.
    let extra = match adapter {
        AdapterType::RedisAdbc => ["driver", "transport", "schema", "time_zone"]
            .into_iter()
            .map(|key| {
                (
                    OptionDatabase::Other(format!("dbt.session.{key}")),
                    OptionValue::String(config.get_str(key).unwrap_or_default().into()),
                )
            })
            .collect::<Vec<_>>(),
        _ => Vec::new(),
    };
    database::Builder::fingerprint(options.iter().chain(extra.iter()))
}

pub(crate) fn session_sql(config: &AdapterConfig) -> Vec<String> {
    let schema = escape_string_literal(
        config.get_str("schema").unwrap_or("public"),
        AdapterType::RedisAdbc,
    );
    let mut sql = vec!["select 1".to_owned(), format!("set schema '{schema}'")];
    if let Some(zone) = config.get_str_non_empty("time_zone") {
        sql.push(format!(
            "set time zone '{}'",
            escape_string_literal(zone, AdapterType::RedisAdbc)
        ));
    }
    sql
}

pub(super) fn initialize_session(conn: &mut dyn Connection, config: &AdapterConfig) -> Result<()> {
    // A query establishes the server cookie before the SETs. Consume and release
    // every reader before starting the next RPC on the connection.
    for query in session_sql(config) {
        let mut statement = conn.new_statement()?;
        statement.set_sql_query(&query)?;
        statement
            .execute()?
            .collect::<std::result::Result<Vec<_>, _>>()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_are_sql_literals_and_probe_precedes_settings() {
        let config = AdapterConfig::new(dbt_yaml::Mapping::from_iter([
            ("schema".into(), "a'b\"c".into()),
            ("time_zone".into(), "America/New_York".into()),
        ]));
        assert_eq!(
            session_sql(&config),
            [
                "select 1",
                "set schema 'a''b\"c'",
                "set time zone 'America/New_York'"
            ]
        );
    }

    #[test]
    fn pool_key_includes_session_settings_and_library() {
        let base = AdapterConfig::default();
        let fingerprint = connection_fingerprint(AdapterType::RedisAdbc, &base, &[]);
        for field in ["driver", "transport", "schema", "time_zone"] {
            let config = AdapterConfig::new(dbt_yaml::Mapping::from_iter([(
                field.into(),
                "different".into(),
            )]));
            assert_ne!(
                fingerprint,
                connection_fingerprint(AdapterType::RedisAdbc, &config, &[])
            );
        }
    }
}
