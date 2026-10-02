use std::future::Future;
use std::sync::Arc;

use super::sqlx::MySqlConnection;
use adbc_core::PartitionedResult;
use adbc_core::error::{Error, Result, Status};
use adbc_core::options::{OptionStatement, OptionValue};
use arrow_array::{RecordBatch, RecordBatchIterator, RecordBatchReader};
use arrow_schema::Schema;
use tokio::sync::Mutex;

use super::convert::mysql_rows_to_record_batch;
use crate::Statement;

fn get_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: std::sync::OnceLock<tokio::runtime::Runtime> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    })
}

pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(future))
    } else {
        get_runtime().block_on(future)
    }
}

pub struct SingleStoreStatement {
    conn: Arc<Mutex<MySqlConnection>>,
    sql: String,
}

impl SingleStoreStatement {
    pub fn new(conn: Arc<Mutex<MySqlConnection>>) -> Self {
        Self {
            conn,
            sql: String::new(),
        }
    }
}

impl Statement for SingleStoreStatement {
    fn bind(&mut self, _batch: RecordBatch) -> Result<()> {
        Err(Error::with_message_and_status(
            "bind is not supported in SingleStoreStatement",
            Status::NotImplemented,
        ))
    }

    fn bind_stream(&mut self, _reader: Box<dyn RecordBatchReader + Send>) -> Result<()> {
        Err(Error::with_message_and_status(
            "bind_stream is not supported in SingleStoreStatement",
            Status::NotImplemented,
        ))
    }

    fn execute<'a>(&'a mut self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let conn_arc = self.conn.clone();
        let sql = self.sql.clone();

        let (rows, fallback_schema) = block_on(async {
            use sqlx_core::column::Column;
            use sqlx_core::executor::Executor;
            use sqlx_core::type_info::TypeInfo;
            let mut conn = conn_arc.lock().await;
            let rows = super::sqlx::query(&sql).fetch_all(&mut *conn).await?;
            let fallback = if rows.is_empty() {
                conn.describe(&sql).await.ok().map(|desc| {
                    let fields = desc
                        .columns()
                        .iter()
                        .map(|c| {
                            let (_, dt) =
                                super::convert::ColumnBuilder::from_type_name(c.type_info().name());
                            arrow_schema::Field::new(c.name(), dt, true)
                        })
                        .collect::<Vec<_>>();
                    Arc::new(Schema::new(fields))
                })
            } else {
                None
            };
            Ok::<_, sqlx_core::Error>((rows, fallback))
        })
        .map_err(|e| {
            Error::with_message_and_status(
                format!("Failed to execute query in SingleStore: {e}"),
                Status::IO,
            )
        })?;

        let batch = if rows.is_empty() && fallback_schema.is_some() {
            let schema = fallback_schema.unwrap();
            RecordBatch::new_empty(schema)
        } else {
            mysql_rows_to_record_batch(&rows)?
        };
        let schema = batch.schema();
        let iter = RecordBatchIterator::new(vec![Ok(batch)].into_iter(), schema);
        Ok(Box::new(iter))
    }

    fn execute_update(&mut self) -> Result<Option<i64>> {
        let conn_arc = self.conn.clone();
        let sql = self.sql.clone();

        let result = block_on(async {
            let mut conn = conn_arc.lock().await;
            super::sqlx::query(&sql).execute(&mut *conn).await
        })
        .map_err(|e| {
            Error::with_message_and_status(
                format!("Failed to execute update in SingleStore: {e}"),
                Status::IO,
            )
        })?;

        Ok(Some(result.rows_affected() as i64))
    }

    fn execute_schema(&mut self) -> Result<Schema> {
        let reader = self.execute()?;
        Ok((*reader.schema()).clone())
    }

    fn execute_partitions(&mut self) -> Result<PartitionedResult> {
        Err(Error::with_message_and_status(
            "execute_partitions not supported in SingleStoreStatement",
            Status::NotImplemented,
        ))
    }

    fn get_parameter_schema(&self) -> Result<Schema> {
        Err(Error::with_message_and_status(
            "get_parameter_schema not supported in SingleStoreStatement",
            Status::NotImplemented,
        ))
    }

    fn prepare(&mut self) -> Result<()> {
        Ok(())
    }

    fn set_sql_query(&mut self, sql: &str) -> Result<()> {
        self.sql = sql.to_string();
        Ok(())
    }

    fn set_substrait_plan(&mut self, _plan: &[u8]) -> Result<()> {
        Err(Error::with_message_and_status(
            "set_substrait_plan not supported in SingleStoreStatement",
            Status::NotImplemented,
        ))
    }

    fn cancel(&mut self) -> Result<()> {
        Ok(())
    }

    fn set_option(&mut self, _key: OptionStatement, _value: OptionValue) -> Result<()> {
        Ok(())
    }

    fn get_option_string(&self, _key: OptionStatement) -> Result<String> {
        Ok(String::new())
    }

    fn get_option_bytes(&self, _key: OptionStatement) -> Result<Vec<u8>> {
        Ok(Vec::new())
    }

    fn get_option_int(&self, _key: OptionStatement) -> Result<i64> {
        Ok(0)
    }

    fn get_option_double(&self, _key: OptionStatement) -> Result<f64> {
        Ok(0.0)
    }
}
