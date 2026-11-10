use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;

use dbt_adbc::{Backend, database};
use std::borrow::Cow;

#[derive(Debug)]
enum PostgresAuthIR<'a> {
    Database {
        user: &'a str,
        password: &'a str,
        host: &'a str,
        port: Cow<'a, str>,
        database: &'a str,
    },
}

impl<'a> PostgresAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::Database {
                user,
                password,
                host,
                port,
                database,
            } => {
                builder.with_parse_uri(format!(
                    "postgresql://{user}:{password}@{host}:{port}/{database}",
                ))?;
            }
        }

        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<PostgresAuthIR<'a>, AuthError> {
    Ok(PostgresAuthIR::Database {
        user: config.require_str("user")?,
        password: config.require_str("password")?,
        host: config.require_str("host")?,
        // In profiles.yml `port` is commonly an integer; accept both numeric and string values.
        port: config.require_string("port")?,
        database: config.require_str("database")?,
    })
}

fn apply_connection_args(
    _config: &AdapterConfig,
    builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    Ok(builder)
}

pub struct PostgresAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl PostgresAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for PostgresAuth {
    fn backend(&self) -> Backend {
        Backend::Postgres
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::uri_value;
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    #[test]
    fn test_uri_contains_expected_parts() {
        let config = Mapping::from_iter([
            ("user".into(), "alice".into()),
            ("password".into(), "secret".into()),
            ("host".into(), "pg.local".into()),
            ("port".into(), "5432".into()),
            ("database".into(), "db".into()),
        ]);

        let builder = PostgresAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "postgresql://alice:secret@pg.local:5432/db");
        assert_contains!(&uri, "user=alice");
        assert_contains!(&uri, "password=secret");
    }

    #[test]
    fn test_port_accepts_integer_value() {
        let config: Mapping = dbt_yaml::from_str(
            r#"
user: alice
password: secret
host: pg.local
port: 5432
database: db
"#,
        )
        .expect("parse yaml");

        let builder = PostgresAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let uri = uri_value(&builder);
        assert_contains!(&uri, "postgresql://alice:secret@pg.local:5432/db");
    }
}
