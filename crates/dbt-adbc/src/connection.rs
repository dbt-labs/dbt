//! ADBC Connection
//!
//!

use core::fmt;
use std::collections::HashSet;

use crate::driver_manager::ManagedConnection as ManagedAdbcConnection;
use adbc_core::options;
use adbc_core::{
    Connection as _, Optionable,
    error::Result,
    options::{OptionConnection, OptionValue},
};
use arrow_array::RecordBatchReader;
use arrow_schema::Schema;

use crate::statement::AdbcStatement;
use crate::{Backend, Statement};

mod builder;
pub use builder::*;

/// ADBC Connection.
///
/// Connections provide methods for query execution, managing prepared
/// statements, using transactions, and so on.
///
/// dyn-compatible trait inspired by the adbc_core::{Connection, Optionable} traits.
// TODO(felipecrv): remove the default impls containing unimplemented!() once implementations
// of this trait become more comprehensive.
pub trait Connection: Send {
    // adbc_core::Connection<StatementType = Statement> functions ----------------------------
    /// Allocate and initialize a new statement.
    fn new_statement(&mut self) -> Result<Box<dyn Statement>>;

    /// Updates the node_id associated with this [Connection].
    ///
    /// Called when a pooled connection is reused for a different node.
    /// Record/replay connections override this to update their recording context.
    fn update_node_id(&mut self, _node_id: Option<String>) {}

    /// Returns the config fingerprint of the engine that created this connection.
    ///
    /// Used by the connection pool to reuse a connection only among engines with
    /// an identical connection configuration; a connection whose fingerprint does
    /// not match the borrowing engine is discarded rather than reused.
    fn fingerprint(&self) -> u64 {
        0
    }

    /// Tags this connection with the config fingerprint of the engine that
    /// created it. The default is a no-op for connection types that do not track
    /// a fingerprint.
    fn set_fingerprint(&mut self, _fingerprint: u64) {}

    /// Cancel the in-progress operation on a connection.
    fn cancel(&mut self) -> Result<()>;

    /// Get metadata about the database/driver.
    ///
    /// # Arguments
    ///
    /// - `codes` - Requested metadata. If `None`, retrieve all available metadata.
    ///
    /// # Result
    ///
    /// The result is an Arrow dataset with the following schema:
    ///
    /// Field Name                  | Field Type
    /// ----------------------------|------------------------
    /// info_name                   | uint32 not null
    /// info_value                  | INFO_SCHEMA
    ///
    /// INFO_SCHEMA is a dense union with members:
    ///
    /// Field Name (Type Code)      | Field Type
    /// ----------------------------|------------------------
    /// string_value (0)            | utf8
    /// bool_value (1)              | bool
    /// int64_value (2)             | int64
    /// int32_bitmask (3)           | int32
    /// string_list (4)             | list\<utf8\>
    /// int32_to_int32_list_map (5) | map\<int32, list\<int32\>\>
    fn get_info<'a>(
        &'a self,
        _codes: Option<HashSet<options::InfoCode>>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC connection info retrieval")
    }

    /// Get a hierarchical view of all catalogs, database schemas, tables, and
    /// columns.
    ///
    /// # Arguments
    ///
    /// - `depth` - The level of nesting to query.
    /// - `catalog` - Only show tables in the given catalog. If `None`,
    ///   do not filter by catalog. If an empty string, only show tables
    ///   without a catalog.  May be a search pattern.
    /// - `db_schema` - Only show tables in the given database schema. If
    ///   `None`, do not filter by database schema. If an empty string, only show
    ///   tables without a database schema. May be a search pattern.
    /// - `table_name` - Only show tables with the given name. If `None`, do not
    ///   filter by name. May be a search pattern.
    /// - `table_type` - Only show tables matching one of the given table
    ///   types. If `None`, show tables of any type. Valid table types can be fetched
    ///   from [Connection::get_table_types].
    /// - `column_name` - Only show columns with the given name. If
    ///   `None`, do not filter by name.  May be a search pattern..
    ///
    /// # Result
    ///
    /// The result is an Arrow dataset with the following schema:
    ///
    /// | Field Name               | Field Type               |
    /// |--------------------------|--------------------------|
    /// | catalog_name             | utf8                     |
    /// | catalog_db_schemas       | list\<DB_SCHEMA_SCHEMA\> |
    ///
    /// DB_SCHEMA_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type              |
    /// |--------------------------|-------------------------|
    /// | db_schema_name           | utf8                    |
    /// | db_schema_tables         | list\<TABLE_SCHEMA\>    |
    ///
    /// TABLE_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type                |
    /// |--------------------------|---------------------------|
    /// | table_name               | utf8 not null             |
    /// | table_type               | utf8 not null             |
    /// | table_columns            | list\<COLUMN_SCHEMA\>     |
    /// | table_constraints        | list\<CONSTRAINT_SCHEMA\> |
    ///
    /// COLUMN_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type              | Comments |
    /// |--------------------------|-------------------------|----------|
    /// | column_name              | utf8 not null           |          |
    /// | ordinal_position         | int32                   | (1)      |
    /// | remarks                  | utf8                    | (2)      |
    /// | xdbc_data_type           | int16                   | (3)      |
    /// | xdbc_type_name           | utf8                    | (3)      |
    /// | xdbc_column_size         | int32                   | (3)      |
    /// | xdbc_decimal_digits      | int16                   | (3)      |
    /// | xdbc_num_prec_radix      | int16                   | (3)      |
    /// | xdbc_nullable            | int16                   | (3)      |
    /// | xdbc_column_def          | utf8                    | (3)      |
    /// | xdbc_sql_data_type       | int16                   | (3)      |
    /// | xdbc_datetime_sub        | int16                   | (3)      |
    /// | xdbc_char_octet_length   | int32                   | (3)      |
    /// | xdbc_is_nullable         | utf8                    | (3)      |
    /// | xdbc_scope_catalog       | utf8                    | (3)      |
    /// | xdbc_scope_schema        | utf8                    | (3)      |
    /// | xdbc_scope_table         | utf8                    | (3)      |
    /// | xdbc_is_autoincrement    | bool                    | (3)      |
    /// | xdbc_is_generatedcolumn  | bool                    | (3)      |
    ///
    /// 1. The column's ordinal position in the table (starting from 1).
    /// 2. Database-specific description of the column.
    /// 3. Optional value.  Should be null if not supported by the driver.
    ///    `xdbc_` values are meant to provide JDBC/ODBC-compatible metadata
    ///    in an agnostic manner.
    ///
    /// CONSTRAINT_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type              | Comments |
    /// |--------------------------|-------------------------|----------|
    /// | constraint_name          | utf8                    |          |
    /// | constraint_type          | utf8 not null           | (1)      |
    /// | constraint_column_names  | list\<utf8\> not null     | (2)      |
    /// | constraint_column_usage  | list\<USAGE_SCHEMA\>      | (3)      |
    ///
    /// 1. One of `CHECK`, `FOREIGN KEY`, `PRIMARY KEY`, or `UNIQUE`.
    /// 2. The columns on the current table that are constrained, in
    ///    order.
    /// 3. For `FOREIGN KEY` only, the referenced table and columns.
    ///
    /// USAGE_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type              |
    /// |--------------------------|-------------------------|
    /// | fk_catalog               | utf8                    |
    /// | fk_db_schema             | utf8                    |
    /// | fk_table                 | utf8 not null           |
    /// | fk_column_name           | utf8 not null           |
    ///
    fn get_objects<'a>(
        &'a self,
        _depth: options::ObjectDepth,
        // NOTE(felipecrv): adbc_core should annotate these with '_ instead
        _catalog: Option<&'a str>,
        _db_schema: Option<&'a str>,
        _table_name: Option<&'a str>,
        _table_type: Option<Vec<&'a str>>,
        _column_name: Option<&'a str>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC hierarchical object retrieval")
    }

    /// Get the Arrow schema of a table.
    ///
    /// # Arguments
    ///
    /// - `catalog` - The catalog (or `None` if not applicable).
    /// - `db_schema` - The database schema (or `None` if not applicable).
    /// - `table_name` - The table name.
    fn get_table_schema(
        &self,
        _catalog: Option<&str>,
        _db_schema: Option<&str>,
        _table_name: &str,
    ) -> Result<Schema> {
        unimplemented!("ADBC table schema retrieval")
    }

    /// Get a list of table types in the database.
    ///
    /// # Result
    ///
    /// The result is an Arrow dataset with the following schema:
    ///
    /// Field Name     | Field Type
    /// ---------------|--------------
    /// table_type     | utf8 not null
    fn get_table_types<'a>(&'a self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC table types retrieval")
    }

    /// Get the names of statistics specific to this driver.
    ///
    /// # Result
    ///
    /// The result is an Arrow dataset with the following schema:
    ///
    /// Field Name     | Field Type
    /// ---------------|----------------
    /// statistic_name | utf8 not null
    /// statistic_key  | int16 not null
    ///
    /// # Since
    /// ADBC API revision 1.1.0
    fn get_statistic_names<'a>(&'a self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC statistic names retrieval")
    }

    /// Get statistics about the data distribution of table(s).
    ///
    /// # Arguments
    ///
    /// - `catalog` - The catalog (or `None` if not applicable). May be a search pattern.
    /// - `db_schema` - The database schema (or `None` if not applicable). May be a search pattern
    /// - `table_name` - The table name (or `None` if not applicable). May be a search pattern
    /// - `approximate` - If false, request exact values of statistics, else
    ///   allow for best-effort, approximate, or cached values. The database may
    ///   return approximate values regardless, as indicated in the result.
    ///   Requesting exact values may be expensive or unsupported.
    ///
    /// # Result
    ///
    /// The result is an Arrow dataset with the following schema:
    ///
    /// | Field Name               | Field Type                       |
    /// |--------------------------|----------------------------------|
    /// | catalog_name             | utf8                             |
    /// | catalog_db_schemas       | list\<DB_SCHEMA_SCHEMA\> not null|
    ///
    /// DB_SCHEMA_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type                        |
    /// |--------------------------|-----------------------------------|
    /// | db_schema_name           | utf8                              |
    /// | db_schema_statistics     | list\<STATISTICS_SCHEMA\> not null|
    ///
    /// STATISTICS_SCHEMA is a Struct with fields:
    ///
    /// | Field Name               | Field Type                       | Comments |
    /// |--------------------------|----------------------------------| -------- |
    /// | table_name               | utf8 not null                    |          |
    /// | column_name              | utf8                             | (1)      |
    /// | statistic_key            | int16 not null                   | (2)      |
    /// | statistic_value          | VALUE_SCHEMA not null            |          |
    /// | statistic_is_approximate | bool not null                    | (3)      |
    ///
    /// 1. If null, then the statistic applies to the entire table.
    /// 2. A dictionary-encoded statistic name (although we do not use the Arrow
    ///    dictionary type). Values in [0, 1024) are reserved for ADBC.  Other
    ///    values are for implementation-specific statistics.  For the definitions
    ///    of predefined statistic types, see [options::Statistics]. To get
    ///    driver-specific statistic names, use [Connection::get_statistic_names].
    /// 3. If true, then the value is approximate or best-effort.
    ///
    /// VALUE_SCHEMA is a dense union with members:
    ///
    /// | Field Name               | Field Type                       |
    /// |--------------------------|----------------------------------|
    /// | int64                    | int64                            |
    /// | uint64                   | uint64                           |
    /// | float64                  | float64                          |
    /// | binary                   | binary                           |
    ///
    /// # Since
    ///
    /// ADBC API revision 1.1.0
    fn get_statistics<'a>(
        &'a self,
        // NOTE(felipecrv): adbc_core should annotate these with '_ instead
        _catalog: Option<&'a str>,
        _db_schema: Option<&'a str>,
        _table_name: Option<&'a str>,
        _approximate: bool,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC statistics retrieval")
    }

    /// Commit any pending transactions. Only used if autocommit is disabled.
    ///
    /// Behavior is undefined if this is mixed with SQL transaction statements.
    fn commit(&mut self) -> Result<()>;

    /// Roll back any pending transactions. Only used if autocommit is disabled.
    ///
    /// Behavior is undefined if this is mixed with SQL transaction statements.
    fn rollback(&mut self) -> Result<()>;

    /// Retrieve a given partition of data.
    ///
    /// A partition can be retrieved from [Statement::execute_partitions].
    ///
    /// # Arguments
    ///
    /// - `partition` - The partition descriptor.
    fn read_partition<'a>(
        &'a self,
        // NOTE(felipecrv): adbc_core should annotate partition with '_ instead
        _partition: &'a [u8],
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        unimplemented!("ADBC partition-based data reading")
    }

    // adbc_core::Optionable<Option = OptionConnection> functions ----------------------------

    /// Set a post-init option.
    fn set_option(&mut self, _key: OptionConnection, _value: OptionValue) -> Result<()> {
        unimplemented!("ADBC connection option setting")
    }
    /// Get a string option value by key.
    fn get_option_string(&self, _key: OptionConnection) -> Result<String> {
        unimplemented!("ADBC connection string option retrieval")
    }
    /// Get a bytes option value by key.
    fn get_option_bytes(&self, _key: OptionConnection) -> Result<Vec<u8>> {
        unimplemented!("ADBC connection bytes option retrieval")
    }
    /// Get an integer option value by key.
    fn get_option_int(&self, _key: OptionConnection) -> Result<i64> {
        unimplemented!("ADBC connection integer option retrieval")
    }
    /// Get a float option value by key.
    fn get_option_double(&self, _key: OptionConnection) -> Result<f64> {
        unimplemented!("ADBC connection float option retrieval")
    }

    /// [Debug](std::fmt::Debug) implementation for Connection.
    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "dyn Connection")
    }
}

impl fmt::Debug for dyn Connection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.debug_fmt(f)
    }
}

/// ADBC Connection.
///
/// A [`Connection`] is a single, logical connection to a database. Connections are
/// created by a [`Database`] instance and are used to execute SQL queries and
/// manage transactions.
#[allow(dead_code)]
pub(crate) struct AdbcConnection(
    pub(crate) Backend,
    pub(crate) ManagedAdbcConnection,
    /// Generation of the engine that created this connection (0 if untagged).
    pub(crate) u64,
);

impl fmt::Debug for AdbcConnection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AdbcConnection({:?}, ManagedAdbcConnection)", self.0)
    }
}

impl Connection for AdbcConnection {
    fn fingerprint(&self) -> u64 {
        self.2
    }

    fn set_fingerprint(&mut self, fingerprint: u64) {
        self.2 = fingerprint;
    }

    fn new_statement(&mut self) -> Result<Box<dyn Statement>> {
        let managed_adbc_stmt = self.1.new_statement()?;
        let adbc_stmt = AdbcStatement(self.0, managed_adbc_stmt);
        Ok(Box::new(adbc_stmt))
    }

    fn cancel(&mut self) -> Result<()> {
        self.1.cancel()
    }

    fn get_info<'a>(
        &'a self,
        codes: Option<HashSet<options::InfoCode>>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.get_info(codes)?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn get_objects<'a>(
        &'a self,
        depth: options::ObjectDepth,
        catalog: Option<&'a str>,
        db_schema: Option<&'a str>,
        table_name: Option<&'a str>,
        table_type: Option<Vec<&'a str>>,
        column_name: Option<&'a str>,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.get_objects(
            depth,
            catalog,
            db_schema,
            table_name,
            table_type,
            column_name,
        )?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn get_table_schema(
        &self,
        catalog: Option<&str>,
        db_schema: Option<&str>,
        table_name: &str,
    ) -> Result<Schema> {
        self.1.get_table_schema(catalog, db_schema, table_name)
    }

    fn get_table_types<'a>(&'a self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.get_table_types()?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn get_statistic_names<'a>(&'a self) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.get_statistic_names()?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn get_statistics<'a>(
        &'a self,
        catalog: Option<&'a str>,
        db_schema: Option<&'a str>,
        table_name: Option<&'a str>,
        approximate: bool,
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self
            .1
            .get_statistics(catalog, db_schema, table_name, approximate)?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn commit(&mut self) -> Result<()> {
        self.1.commit()
    }

    fn rollback(&mut self) -> Result<()> {
        self.1.rollback()
    }

    fn read_partition<'a>(
        &'a self,
        partition: &'a [u8],
    ) -> Result<Box<dyn RecordBatchReader + Send + 'a>> {
        let reader = self.1.read_partition(partition)?;
        let reader = Box::new(reader);
        Ok(reader)
    }

    fn set_option(&mut self, key: OptionConnection, value: OptionValue) -> Result<()> {
        self.1.set_option(key, value)
    }

    fn get_option_string(&self, key: OptionConnection) -> Result<String> {
        self.1.get_option_string(key)
    }

    fn get_option_bytes(&self, key: OptionConnection) -> Result<Vec<u8>> {
        self.1.get_option_bytes(key)
    }

    fn get_option_int(&self, key: OptionConnection) -> Result<i64> {
        self.1.get_option_int(key)
    }

    fn get_option_double(&self, key: OptionConnection) -> Result<f64> {
        self.1.get_option_double(key)
    }

    fn debug_fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}
