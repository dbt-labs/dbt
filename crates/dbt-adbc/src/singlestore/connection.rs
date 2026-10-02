use core::fmt;
use std::sync::Arc;

use super::sqlx::MySqlConnection;
use adbc_core::error::{Error, Result, Status};
use adbc_core::options::{OptionConnection, OptionValue};
use tokio::sync::Mutex;

use super::statement::{SingleStoreStatement, block_on};
use crate::semaphore::Semaphore;
use crate::{Connection, Statement};

pub struct SingleStoreConnection {
    conn: Arc<Mutex<MySqlConnection>>,
    _semaphore: Option<Arc<Semaphore>>,
}

impl SingleStoreConnection {
    pub fn new(conn: MySqlConnection, semaphore: Option<Arc<Semaphore>>) -> Self {
        Self {
            conn: Arc::new(Mutex::new(conn)),
            _semaphore: semaphore,
        }
    }

    fn exec_sql(&self, sql: &str, action: &str) -> Result<()> {
        let conn_arc = self.conn.clone();
        block_on(async {
            let mut conn = conn_arc.lock().await;
            super::sqlx::query(sql).execute(&mut *conn).await
        })
        .map_err(|e| {
            Error::with_message_and_status(
                format!("Failed to {action} transaction in SingleStore: {e}"),
                Status::IO,
            )
        })?;
        Ok(())
    }
}

impl Connection for SingleStoreConnection {
    fn new_statement(&mut self) -> Result<Box<dyn Statement>> {
        Ok(Box::new(SingleStoreStatement::new(self.conn.clone())))
    }

    fn cancel(&mut self) -> Result<()> {
        Ok(())
    }

    fn commit(&mut self) -> Result<()> {
        self.exec_sql("COMMIT", "commit")
    }

    fn rollback(&mut self) -> Result<()> {
        self.exec_sql("ROLLBACK", "rollback")
    }

    fn set_option(&mut self, _key: OptionConnection, _value: OptionValue) -> Result<()> {
        Ok(())
    }

    fn get_option_string(&self, _key: OptionConnection) -> Result<String> {
        Ok(String::new())
    }

    fn get_option_bytes(&self, _key: OptionConnection) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn get_option_int(&self, _key: OptionConnection) -> Result<i64> {
        Ok(0)
    }

    fn get_option_double(&self, _key: OptionConnection) -> Result<f64> {
        Ok(0.0)
    }

    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SingleStoreConnection")
    }
}
