use std::sync::Arc;

use super::sqlx::Connection as _;
use super::sqlx::mysql::MySqlConnectOptions;
use adbc_core::error::{Error, Result, Status};
use adbc_core::options::{InfoCode, OptionConnection, OptionDatabase, OptionValue};
use arrow_array::{Array, StringArray};

use super::connection::SingleStoreConnection;
use super::statement::block_on;
use crate::Connection;
use crate::database::{Database, DatabaseInfo};
use crate::semaphore::Semaphore;

#[derive(Clone)]
pub struct SingleStoreDatabase {
    connect_options: MySqlConnectOptions,
    semaphore: Option<Arc<Semaphore>>,
}

impl SingleStoreDatabase {
    pub fn new(connect_options: MySqlConnectOptions, semaphore: Option<Arc<Semaphore>>) -> Self {
        Self {
            connect_options,
            semaphore,
        }
    }
}

impl Database for SingleStoreDatabase {
    fn new_connection(&self) -> Result<Box<dyn Connection>> {
        self.new_connection_with_opts(vec![])
    }

    fn new_connection_with_opts(
        &self,
        _opts: Vec<(OptionConnection, OptionValue)>,
    ) -> Result<Box<dyn Connection>> {
        let opts = self.connect_options.clone();
        let conn = block_on(async { super::sqlx::MySqlConnection::connect_with(&opts).await })
            .map_err(|e| {
                Error::with_message_and_status(
                    format!("Failed to connect to SingleStore: {e}"),
                    Status::IO,
                )
            })?;

        Ok(Box::new(SingleStoreConnection::new(
            conn,
            self.semaphore.clone(),
        )))
    }

    fn set_option(&mut self, _key: OptionDatabase, _value: OptionValue) -> Result<()> {
        Ok(())
    }

    fn get_option_string(&self, _key: OptionDatabase) -> Result<String> {
        Ok(String::new())
    }

    fn get_option_bytes(&self, _key: OptionDatabase) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn get_option_int(&self, _key: OptionDatabase) -> Result<i64> {
        Ok(0)
    }

    fn get_option_double(&self, _key: OptionDatabase) -> Result<f64> {
        Ok(0.0)
    }

    fn clone_box(&self) -> Box<dyn Database> {
        Box::new(self.clone())
    }
}

impl DatabaseInfo for SingleStoreDatabase {
    fn get_info(&mut self, _info_code: InfoCode) -> Result<Arc<dyn Array>> {
        Ok(Arc::new(StringArray::from(vec!["SingleStore"])))
    }
}
