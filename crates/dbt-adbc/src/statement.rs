//! ADBC Statement
//!
//!

use std::fmt;

use crate::driver_manager::ManagedStatement as ManagedAdbcStatement;
use adbc_core::{
    Optionable, PartitionedResult, Statement as _,
    error::{Error, Result, Status},
    options::{OptionStatement, OptionValue},
};
use arrow_array::{RecordBatch, RecordBatchReader};
use arrow_schema::Schema;

use crate::Backend;

/// ADBC Statement.
///
/// dyn-compatible trait inspired by the adbc_core::{Statement, Optionable} traits.
pub trait Statement: Send {
    /// Bind Arrow data. This can be used for bulk inserts or prepared
    /// statements.
    fn bind(&mut self, batch: RecordBatch) -> Result<()>;

    /// Bind Arrow data. This can be used for bulk inserts or prepared
    /// statements.
    // TODO(alexandreyc): should we use a generic here instead of a trait object?
    // See: https://github.com/apache/arrow-adbc/pull/1725#discussion_r1567750972
    fn bind_stream(&mut self, reader: Box<dyn RecordBatchReader + Send>) -> Result<()>;

    /// Execute a statement and get the results.
    ///
    /// This invalidates any prior result sets.
    // TODO(alexandreyc): is the Send bound absolutely necessary? same question
    // for all methods that return an impl RecordBatchReader
    // See: https://github.com/apache/arrow-adbc/pull/1725#discussion_r1567748242
    fn execute<'a>(&'a mut self) -> Result<Box<dyn RecordBatchReader + Send + 'a>>;

    /// Execute a statement that doesn't have a result set and get the number
    /// of affected rows.
    ///
    /// This invalidates any prior result sets.
    ///
    /// # Result
    ///
    /// Will return the number of rows affected. If the affected row count is
    /// unknown or unsupported by the database, will return `None`.
    fn execute_update(&mut self) -> Result<Option<i64>>;

    /// Get the schema of the result set of a query without executing it.
    ///
    /// This invalidates any prior result sets.
    ///
    /// Depending on the driver, this may require first executing
    /// [Statement::prepare].
    ///
    /// # Since
    ///
    /// ADBC API revision 1.1.0
    fn execute_schema(&mut self) -> Result<Schema>;

    /// Execute a statement and get the results as a partitioned result set.
    fn execute_partitions(&mut self) -> Result<PartitionedResult>;

    /// Get the schema for bound parameters.
    ///
    /// This retrieves an Arrow schema describing the number, names, and
    /// types of the parameters in a parameterized statement. The fields
    /// of the schema should be in order of the ordinal position of the
    /// parameters; named parameters should appear only once.
    ///
    /// If the parameter does not have a name, or the name cannot be
    /// determined, the name of the corresponding field in the schema will
    /// be an empty string. If the type cannot be determined, the type of
    /// the corresponding field will be NA (NullType).
    ///
    /// This should be called after [Statement::prepare].
    fn get_parameter_schema(&self) -> Result<Schema>;

    /// Turn this statement into a prepared statement to be executed multiple
    /// times.
    ///
    /// This invalidates any prior result sets.
    fn prepare(&mut self) -> Result<()>;

    /// Set the SQL query to execute.
    ///
    /// The query can then be executed with [Statement::execute]. For queries
    /// expected to be executed repeatedly, call [Statement::prepare] first.
    fn set_sql_query(&mut self, sql: &str) -> Result<()>;

    /// Set the Substrait plan to execute.
    ///
    /// The query can then be executed with [Statement::execute]. For queries
    /// expected to be executed repeatedly, call [Statement::prepare] first.
    fn set_substrait_plan(&mut self, plan: &[u8]) -> Result<()>;

    /// Cancel execution of an in-progress query.
    ///
    /// This can be called during [Statement::execute] (or similar), or while
    /// consuming a result set returned from such.
    ///
    /// # Since
    ///
    /// ADBC API revision 1.1.0
    fn cancel(&mut self) -> Result<()>;

    // adbc_core::Optionable<Option = OptionStatement> functions -----------------------------

    /// Set a post-init option.
    fn set_option(&mut self, _key: OptionStatement, _value: OptionValue) -> Result<()> {
        unimplemented!("ADBC statement option setting")
    }
    /// Get a string option value by key.
    fn get_option_string(&self, _key: OptionStatement) -> Result<String> {
        unimplemented!("ADBC statement string option retrieval")
    }
    /// Get a bytes option value by key.
    fn get_option_bytes(&self, _key: OptionStatement) -> Result<Vec<u8>> {
        unimplemented!("ADBC statement bytes option retrieval")
    }
    /// Get an integer option value by key.
    fn get_option_int(&self, _key: OptionStatement) -> Result<i64> {
        unimplemented!("ADBC statement integer option retrieval")
    }
    /// Get a float option value by key.
    fn get_option_double(&self, _key: OptionStatement) -> Result<f64> {
        unimplemented!("ADBC statement float option retrieval")
    }

    /// [Debug](std::fmt::Debug) implementation for Connection.
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dyn Statement")
    }
}

impl fmt::Debug for dyn Statement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.debug_fmt(f)
    }
}

/// ADBC Statement.
#[allow(dead_code)]
pub(crate) struct AdbcStatement(pub(crate) Backend, pub(crate) ManagedAdbcStatement);

impl Statement for AdbcStatement {
    fn bind(&mut self, batch: RecordBatch) -> Result<()> {
        self.1.bind(batch)
    }

    fn bind_stream(&mut self, reader: Box<dyn RecordBatchReader + Send>) -> Result<()> {
        self.1.bind_stream(reader)
    }

    fn execute<'a>(&'a mut self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.execute()?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn execute_update(&mut self) -> Result<Option<i64>> {
        self.1.execute_update()
    }

    fn execute_schema(&mut self) -> Result<Schema> {
        self.1.execute_schema()
    }

    fn execute_partitions(&mut self) -> Result<PartitionedResult> {
        self.1.execute_partitions()
    }

    fn get_parameter_schema(&self) -> Result<Schema> {
        self.1.get_parameter_schema()
    }

    fn prepare(&mut self) -> Result<()> {
        self.1.prepare()
    }

    fn set_sql_query(&mut self, sql: &str) -> Result<()> {
        // Because caller might hot have sql (e.g., ingest)
        match sql {
            "" => Ok(()),
            sql => self.1.set_sql_query(sql),
        }
    }

    fn set_substrait_plan(&mut self, plan: &[u8]) -> Result<()> {
        self.1.set_substrait_plan(plan)
    }

    fn cancel(&mut self) -> Result<()> {
        self.1.cancel()
    }

    // adbc_core::Optionable<Option = OptionStatement> functions -----------------------------

    fn set_option(&mut self, key: OptionStatement, value: OptionValue) -> Result<()> {
        if let OptionStatement::Other(name) = &key
            && name.starts_with("dbt.")
        {
            // some wrappers intercept dbt-specific options, but we ignore them here in case
            // they reach the innermost [Statement] implementation
            return Ok(());
        }
        self.1.set_option(key, value)
    }

    fn get_option_string(&self, key: OptionStatement) -> Result<String> {
        if let OptionStatement::Other(name) = &key
            && name.starts_with("dbt.")
        {
            let message = format!(
                "dbt-specific option '{}' is not queryable from AdbcStatement",
                name
            );
            Error::with_message_and_status(message, Status::NotFound);
        }
        self.1.get_option_string(key)
    }

    fn get_option_bytes(&self, key: OptionStatement) -> Result<Vec<u8>> {
        self.1.get_option_bytes(key)
    }

    fn get_option_int(&self, key: OptionStatement) -> Result<i64> {
        self.1.get_option_int(key)
    }

    fn get_option_double(&self, key: OptionStatement) -> Result<f64> {
        self.1.get_option_double(key)
    }

    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::write!(f, "AdbcStatement")
    }
}
