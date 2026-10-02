use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};
use dbt_adbc::{Backend, database};

pub struct SingleStoreAuth {
    #[allow(dead_code)]
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SingleStoreAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for SingleStoreAuth {
    fn backend(&self) -> Backend {
        Backend::SingleStore
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend());

        let user = config.require_string("user")?;
        let password = config.require_string("password")?;
        let host = config.require_string("host")?;
        let port = config.require_string("port")?;
        let dbname = config.require_string("database")?;

        builder.with_parse_uri(format!("mysql://{user}:{password}@{host}:{port}/{dbname}"))?;

        Ok(builder)
    }
}
