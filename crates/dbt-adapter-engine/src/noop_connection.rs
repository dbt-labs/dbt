use arrow_schema::Schema;
use dbt_adbc::*;

pub struct NoopConnection;

impl Connection for NoopConnection {
    // Unsupported operations return an error rather than panicking so callers can handle it gracefully;
    // cancel/commit/rollback are no-ops since this connection never holds transaction state.
    fn new_statement(&mut self) -> adbc_core::error::Result<Box<dyn Statement>> {
        Err(adbc_core::error::Error::with_message_and_status(
            "NoopConnection does not support statement creation",
            adbc_core::error::Status::NotImplemented,
        ))
    }

    fn cancel(&mut self) -> adbc_core::error::Result<()> {
        Ok(())
    }

    fn commit(&mut self) -> adbc_core::error::Result<()> {
        Ok(())
    }

    fn rollback(&mut self) -> adbc_core::error::Result<()> {
        Ok(())
    }

    fn get_table_schema(
        &self,
        _catalog: Option<&str>,
        _db_schema: Option<&str>,
        _table_name: &str,
    ) -> adbc_core::error::Result<Schema> {
        Err(adbc_core::error::Error::with_message_and_status(
            "NoopConnection does not support table schema retrieval",
            adbc_core::error::Status::NotImplemented,
        ))
    }

    fn get_objects<'a>(
        &'a self,
        _depth: adbc_core::options::ObjectDepth,
        _catalog: Option<&'a str>,
        _db_schema: Option<&'a str>,
        _table_name: Option<&'a str>,
        _table_type: Option<Vec<&'a str>>,
        _column_name: Option<&'a str>,
    ) -> adbc_core::error::Result<Box<dyn arrow_array::RecordBatchReader + Send + 'a>> {
        Err(adbc_core::error::Error::with_message_and_status(
            "NoopConnection does not support object retrieval",
            adbc_core::error::Status::NotImplemented,
        ))
    }
}
