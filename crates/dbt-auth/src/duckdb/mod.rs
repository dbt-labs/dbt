pub mod init;

use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};

use dbt_adbc::{Backend, database};

pub struct DuckDbAuth {
    backend: Backend,
    #[allow(dead_code, reason = "unused until DuckDB auth has a warning to raise")]
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl DuckDbAuth {
    pub fn new(backend: Backend, warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        debug_assert!(matches!(backend, Backend::DuckDB | Backend::DuckDBExtended));
        Self {
            backend,
            warning_printer,
        }
    }
}

impl Auth for DuckDbAuth {
    fn backend(&self) -> Backend {
        self.backend
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend());

        let target = init::DuckDbTarget::from_config(config)?;

        // DuckDB requires the database path to be specified
        // The path option from profiles.yml specifies where to store the database file
        if config.get_string("path").is_some() {
            let path = match &target {
                init::DuckDbTarget::MotherDuck { .. }
                | init::DuckDbTarget::MotherDuckWithToken { .. } => ":memory:",
                init::DuckDbTarget::Plain { path } => path,
            };
            builder
                .with_named_option("path", path)
                .map_err(|e| AuthError::Config(e.to_string()))?;
        }

        Ok(builder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use adbc_core::options::{OptionDatabase, OptionValue};

    fn config_from_yaml(yaml: &str) -> AdapterConfig {
        let value: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let mapping = match value {
            dbt_yaml::Value::Mapping(m, _) => m,
            _ => panic!("expected mapping"),
        };
        AdapterConfig::new(mapping)
    }

    #[test]
    fn configure_preserves_duckdb_backend_variant() {
        for backend in [Backend::DuckDB, Backend::DuckDBExtended] {
            let auth = DuckDbAuth::new(backend, Box::new(crate::NoopAuthWarningPrinter));
            let builder = auth
                .configure(&AdapterConfig::new(Default::default()))
                .unwrap();

            assert_eq!(auth.backend(), backend);
            assert_eq!(builder.backend, backend);
        }
    }

    #[test]
    fn configure_uses_in_memory_path_for_motherduck() {
        let auth = DuckDbAuth::new(
            Backend::DuckDBExtended,
            Box::new(crate::NoopAuthWarningPrinter),
        );
        let config = config_from_yaml(
            r#"
path: "md:stocks_dev"
"#,
        );

        let builder = auth.configure(&config).unwrap();
        assert!(builder.other.iter().any(|(name, value)| {
            matches!(
                (name, value),
                (
                    OptionDatabase::Other(option_name),
                    OptionValue::String(option_value)
                ) if option_name == "path" && option_value == ":memory:"
            )
        }));
        assert!(!builder.other.iter().any(|(name, _)| {
            matches!(
                name,
                OptionDatabase::Other(option_name) if option_name == "motherduck_token"
            )
        }));
    }

    #[test]
    fn configure_keeps_local_path() {
        let auth = DuckDbAuth::new(
            Backend::DuckDBExtended,
            Box::new(crate::NoopAuthWarningPrinter),
        );
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
"#,
        );

        let builder = auth.configure(&config).unwrap();
        assert!(builder.other.iter().any(|(name, value)| {
            matches!(
                (name, value),
                (
                    OptionDatabase::Other(option_name),
                    OptionValue::String(option_value)
                ) if option_name == "path" && option_value == "/tmp/local.duckdb"
            )
        }));
    }

    #[test]
    fn configure_rejects_database_main_for_memory_path() {
        let auth = DuckDbAuth::new(
            Backend::DuckDBExtended,
            Box::new(crate::NoopAuthWarningPrinter),
        );
        let config = config_from_yaml(
            r#"
database: "main"
"#,
        );

        assert!(auth.configure(&config).is_err());
    }

    #[test]
    fn configure_rejects_database_main_for_local_path() {
        let auth = DuckDbAuth::new(
            Backend::DuckDBExtended,
            Box::new(crate::NoopAuthWarningPrinter),
        );
        let config = config_from_yaml(
            r#"
path: "/tmp/local.duckdb"
database: "main"
"#,
        );

        assert!(auth.configure(&config).is_err());
    }
}
