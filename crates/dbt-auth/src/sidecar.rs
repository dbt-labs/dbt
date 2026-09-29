use crate::{Auth, AuthError, config::AdapterConfig};
use dbt_adbc::{Backend, database};

/// DuckDB authentication (no-op - local file database)
pub struct DuckDBAuth;

impl Auth for DuckDBAuth {
    fn backend(&self) -> Backend {
        Backend::DuckDBExtended
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend());

        // Set path if provided
        if let Some(path_value) = config.get("path") {
            if let Some(path) = path_value.as_str() {
                if path != ":memory:" {
                    builder.with_named_option("path", path.to_string())?;
                }
            }
        }

        Ok(builder)
    }
}
