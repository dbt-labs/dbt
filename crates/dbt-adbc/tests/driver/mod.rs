//! ADBC driver tests
//!
//! These tests are disabled by default because they require real database
//! accounts.
//!
//! To enable these tests set the `ADBC_DRIVER_TESTS` environment variable
//! when building these tests.
//!

mod tests {
    use adbc_core::{
        error::{Error, Result},
        options::{AdbcVersion, OptionConnection},
    };
    use arrow_array::Array as _;
    use arrow_array::{cast::AsArray, types::*};
    use dbt_adbc::{
        Backend, Connection, Database, Driver, Statement, athena, bigquery, connection,
        database::{self, LogLevel},
        driver, salesforce, snowflake,
    };
    use std::collections::HashSet;
    use std::env;

    const ADBC_VERSION: AdbcVersion = AdbcVersion::V110;

    fn driver_for(backend: Backend) -> Result<Box<dyn Driver>> {
        driver::Builder::new(backend, driver::LoadStrategy::CdnCache)
            .with_adbc_version(ADBC_VERSION)
            .try_load()
    }

    fn database_builder_for(backend: Backend) -> Result<database::Builder> {
        let mut database_builder = match backend {
            Backend::Snowflake => database::Builder::from_snowsql_config(),
            Backend::BigQuery => {
                let mut builder = database::Builder::new(backend);
                let project_id = env::var("ADBC_BIGQUERY_PROJECT").unwrap_or_default();
                let dataset_id = env::var("ADBC_BIGQUERY_DATASET").unwrap_or_default();
                let auth_credentials =
                    env::var("ADBC_BIGQUERY_CREDENTIAL_FILE").unwrap_or_default();

                builder
                    .with_named_option(
                        bigquery::AUTH_TYPE,
                        bigquery::auth_type::JSON_CREDENTIAL_FILE,
                    )?
                    .with_named_option(bigquery::PROJECT_ID, project_id)?
                    .with_named_option(bigquery::DATASET_ID, dataset_id)?
                    .with_named_option(bigquery::AUTH_CREDENTIALS, auth_credentials)?;
                Ok(builder)
            }
            Backend::Postgres | Backend::Redshift => {
                // Configuration for Postgres:
                //     CREATE ROLE username WITH LOGIN PASSWORD 'an_secure_password';
                //     CREATE DATABASE adbc_test;
                //     GRANT CONNECT ON DATABASE adbc_test TO username;
                //     GRANT ALL PRIVILEGES ON DATABASE adbc_test TO username;
                // Shell:
                //     export ADBC_POSTGRES_URI="postgres://username:an_secure_password@localhost/adbc_test"
                let uri = env::var("ADBC_POSTGRES_URI").unwrap_or_else(|_| {
                    "postgres://username:rocks_password@localhost/adbc_test".to_owned()
                });
                let mut builder = database::Builder::new(backend);
                builder.with_parse_uri(uri)?;
                Ok(builder)
            }
            Backend::Databricks => {
                const HOST: &str = "adbc.databricks.host";
                const CATALOG: &str = "adbc.databricks.catalog";
                const SCHEMA: &str = "adbc.databricks.schema";
                const WAREHOUSE: &str = "adbc.databricks.warehouse";
                const TOKEN: &str = "adbc.databricks.token";

                let host = env::var("DATABRICKS_HOST").unwrap();
                let warehouse = env::var("DATABRICKS_WAREHOUSE").unwrap();
                let token = env::var("DATABRICKS_TOKEN").unwrap();

                let mut builder = database::Builder::new(backend);
                // optional
                if let Ok(catalog) = env::var("DATABRICKS_CATALOG") {
                    builder.with_named_option(CATALOG, catalog)?;
                }
                if let Ok(schema) = env::var("DATABRICKS_SCHEMA") {
                    builder.with_named_option(SCHEMA, schema)?;
                }

                builder
                    .with_named_option(HOST, host)?
                    .with_named_option(WAREHOUSE, warehouse)?
                    .with_named_option(TOKEN, token)?;
                Ok(builder)
            }
            Backend::Athena => {
                let mut builder = database::Builder::new(backend);
                let region = env::var("ATHENA_REGION").unwrap();
                let catalog = env::var("ATHENA_CATALOG").unwrap();
                let schema = env::var("ATHENA_SCHEMA").unwrap();
                let s3_staging_dir = env::var("ATHENA_S3_STAGING_DIR").unwrap();
                builder
                    .with_named_option(athena::REGION, region)?
                    .with_named_option(athena::CATALOG, catalog)?
                    .with_named_option(athena::SCHEMA, schema)?
                    .with_named_option(athena::S3_STAGING_DIR, s3_staging_dir)?;
                if let Ok(work_group) = env::var("ATHENA_WORK_GROUP") {
                    builder.with_named_option(athena::WORK_GROUP, work_group)?;
                }
                Ok(builder)
            }
            Backend::Spark => todo!("Spark is WIP"),
            Backend::SQLServer => todo!("SQL Server is WIP"),
            Backend::Salesforce => {
                let mut builder = database::Builder::new(backend);
                builder.with_named_option(salesforce::AUTH_TYPE, salesforce::auth_type::JWT)?;

                builder.with_named_option(salesforce::LOGIN_URL, "https://login.salesforce.com")?;
                builder.with_named_option(salesforce::USERNAME, "test@example.com")?;
                builder.with_named_option(salesforce::CLIENT_ID, "1")?;
                builder.with_named_option(salesforce::JWT_PRIVATE_KEY, "test")?;

                Ok(builder)
            }
            Backend::DuckDB | Backend::DuckDBExtended => {
                let mut builder = database::Builder::new(backend);
                let database_path = ":memory:".to_string();
                builder.with_named_option("path", database_path)?;
                Ok(builder)
            }
            Backend::LakeCompute => {
                unimplemented!("LakeCompute backend database builder in tests")
            }
            Backend::ClickHouse => {
                let mut builder = database::Builder::new(backend);
                let uri = env::var("ADBC_CLICKHOUSE_URI")
                    .unwrap_or_else(|_| "http://localhost:8123".to_owned());
                let username =
                    env::var("ADBC_CLICKHOUSE_USERNAME").unwrap_or_else(|_| "default".to_owned());
                let password = env::var("ADBC_CLICKHOUSE_PASSWORD").unwrap_or_default();
                builder
                    .with_parse_uri(uri)?
                    .with_username(username)
                    .with_password(password);
                Ok(builder)
            }
            Backend::Exasol => {
                let mut builder = database::Builder::new(backend);
                let uri = env::var("ADBC_EXASOL_URI")
                    .unwrap_or_else(|_| "exasol://localhost:8563".to_owned());
                let username =
                    env::var("ADBC_EXASOL_USERNAME").unwrap_or_else(|_| "sys".to_owned());
                let password =
                    env::var("ADBC_EXASOL_PASSWORD").unwrap_or_else(|_| "exasol".to_owned());
                let validate_cert =
                    env::var("ADBC_EXASOL_VALIDATE_CERT").unwrap_or_else(|_| "0".to_owned());
                // Append certificate validation param if not already in URI
                let uri = if uri.contains("validateservercertificate") {
                    uri
                } else if uri.contains('?') {
                    format!("{uri}&validateservercertificate={validate_cert}")
                } else {
                    format!("{uri}?validateservercertificate={validate_cert}")
                };
                builder
                    .with_parse_uri(uri)?
                    .with_username(username)
                    .with_password(password);
                Ok(builder)
            }
            Backend::Generic { .. } => unimplemented!("generic backend database builder in tests"),
        }?;
        if backend == Backend::Snowflake {
            database_builder
                .with_named_option(snowflake::LOG_TRACING, LogLevel::Warn.to_string())?;
        }
        Ok(database_builder)
    }

    fn database_builder_for_duckdb_file(path: &str) -> Result<database::Builder> {
        let mut builder = database::Builder::new(Backend::DuckDBExtended);
        builder.with_named_option("path", path)?;
        Ok(builder)
    }

    fn duckdb_per_user_file_path(prefix: &str) -> std::path::PathBuf {
        let base_dir = dirs::cache_dir()
            .map(|path| path.join("com.getdbt").join("dbt-adbc-tests"))
            .unwrap_or_else(env::temp_dir);
        let file_name = format!("{prefix}.duckdb");

        if std::fs::create_dir_all(&base_dir).is_ok() {
            base_dir.join(file_name)
        } else {
            env::temp_dir().join(file_name)
        }
    }

    fn database_for(backend: Backend) -> Result<Box<dyn Database>> {
        let mut driver = driver_for(backend)?;
        let database_builder = database_builder_for(backend)?;
        database_builder.build(&mut driver)
    }

    fn connection_for(backend: Backend) -> Result<Box<dyn Connection>> {
        let mut database = database_for(backend)?;
        let builder = connection::Builder::default();
        builder.build(&mut database)
    }

    fn with_database(
        backend: Backend,
        func: impl FnOnce(Box<dyn Database>) -> Result<()>,
    ) -> Result<()> {
        database_for(backend).and_then(func)
    }

    fn with_connection(
        backend: Backend,
        func: impl FnOnce(&mut dyn Connection) -> Result<()>,
    ) -> Result<()> {
        // This always clones the connection because connection methods require
        // exclusive access (&mut Connection). The alternative would be an
        // `Arc<Mutex<Connection>>` however any test failure is a panic and
        // would trigger mutex poisoning.
        //
        // TODO(mbrobbel): maybe force interior mutability via the core traits?
        connection_for(backend).and_then(|mut conn| func(&mut *conn))
    }

    fn with_empty_statement(
        backend: Backend,
        func: impl FnOnce(Box<dyn Statement>) -> Result<()>,
    ) -> Result<()> {
        with_connection(backend, |connection| {
            connection.new_statement().and_then(func)
        })
    }

    /// Check the returned info by the driver using the database methods.
    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn database_get_info() -> Result<()> {
        with_database(Backend::Snowflake, |mut database| {
            assert_eq!(database.vendor_name(), Ok("Snowflake".to_owned()));
            assert!(
                database
                    .vendor_version()
                    .is_ok_and(|version| version.starts_with("v"))
            );
            assert!(database.vendor_arrow_version().is_ok());
            assert_eq!(database.vendor_sql(), Ok(true));
            assert_eq!(database.vendor_substrait(), Ok(false));
            assert_eq!(
                database.driver_name(),
                Ok("ADBC Snowflake Driver - Go".to_owned())
            );
            assert!(database.driver_version().is_ok());
            // XXX: re-enable when we fix driver builds to embed the version
            // assert!(database
            //     .driver_arrow_version()
            //     .is_ok_and(|version| version.starts_with("v")));
            assert_eq!(database.adbc_version(), Ok(ADBC_VERSION));
            Ok(())
        })
    }

    /// Check execute of statement with `SELECT 21 + 21` query.
    fn execute_statement(backend: Backend) -> Result<()> {
        with_empty_statement(backend, |mut statement| {
            statement.set_sql_query("SELECT 21 + 21")?;
            let batch = statement
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            match backend {
                Backend::Snowflake => {
                    assert_eq!(
                        batch.column(0).as_primitive::<Decimal128Type>().value(0),
                        42
                    );
                }
                Backend::Postgres
                | Backend::Redshift
                | Backend::Databricks
                | Backend::DuckDB
                | Backend::DuckDBExtended => {
                    assert_eq!(batch.column(0).as_primitive::<Int32Type>().value(0), 42);
                }
                Backend::ClickHouse => {
                    assert_eq!(batch.column(0).as_primitive::<UInt16Type>().value(0), 42);
                }
                Backend::Exasol => {
                    // Exasol returns DECIMAL for integer arithmetic
                    assert_eq!(
                        batch.column(0).as_primitive::<Decimal128Type>().value(0),
                        42
                    );
                }
                _ => {
                    // BigQuery and others use Int64. We change this function as we expand the set
                    // of database integrations in ADBC.
                    assert_eq!(batch.column(0).as_primitive::<Int64Type>().value(0), 42);
                }
            }
            Ok(())
        })
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn statement_execute_snowflake() -> Result<()> {
        execute_statement(Backend::Snowflake)
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn statement_execute_bigquery() -> Result<()> {
        execute_statement(Backend::BigQuery)
    }

    #[test_with::env(ADBC_POSTGRES_URI)]
    #[test]
    fn statement_execute_redshift() -> Result<()> {
        execute_statement(Backend::Redshift)
    }

    #[test_with::env(ADBC_POSTGRES_URI)]
    #[test]
    fn statement_execute_postgres() -> Result<()> {
        execute_statement(Backend::Postgres)
    }

    #[test_with::env(DATABRICKS_TOKEN)]
    #[test]
    fn statement_execute_databricks() -> Result<()> {
        execute_statement(Backend::Databricks)
    }

    #[test]
    fn statement_execute_duckdb() -> Result<()> {
        execute_statement(Backend::DuckDBExtended)
    }

    #[test]
    fn duckdb_file_persistence() -> Result<()> {
        use std::fs;

        // Create a deterministic per-user file path
        let db_path = duckdb_per_user_file_path("dbt_adbc_test_persistence");
        let db_path_str = db_path.to_string_lossy().to_string();

        // Clean up any existing file
        let _ = fs::remove_file(&db_path);

        // First connection: create table and insert data
        {
            let mut driver = driver_for(Backend::DuckDBExtended)?;
            let builder = database_builder_for_duckdb_file(&db_path_str)?;
            let mut database = builder.build(&mut driver)?;
            let mut conn = connection::Builder::default().build(&mut database)?;

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("CREATE TABLE test_persist (id INTEGER, name VARCHAR)")?;
            let _ = stmt.execute()?;

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("INSERT INTO test_persist VALUES (1, 'alice'), (2, 'bob')")?;
            let _ = stmt.execute()?;
        }
        // Connection dropped here, file should be written

        // Second connection: verify data persisted
        {
            let mut driver = driver_for(Backend::DuckDBExtended)?;
            let builder = database_builder_for_duckdb_file(&db_path_str)?;
            let mut database = builder.build(&mut driver)?;
            let mut conn = connection::Builder::default().build(&mut database)?;

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("SELECT COUNT(*) FROM test_persist")?;
            let batch = stmt
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;

            let count = batch.column(0).as_primitive::<Int64Type>().value(0);
            assert_eq!(count, 2, "Expected 2 rows to persist");
        }

        // Cleanup
        let _ = fs::remove_file(&db_path);

        Ok(())
    }

    #[test]
    fn duckdb_data_types_bool() -> Result<()> {
        with_empty_statement(Backend::DuckDBExtended, |mut statement| {
            statement.set_sql_query(
                r#"SELECT * FROM (
                     VALUES
                     (true, false, NULL),
                     (false, true, true)
                   ) AS tbl(bool_a, bool_b, bool_c)"#,
            )?;
            let batch = statement
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            let schema = batch.schema();
            assert_eq!(schema.field(0).name(), "bool_a");
            assert_eq!(schema.field(1).name(), "bool_b");
            assert_eq!(schema.field(2).name(), "bool_c");

            let a = batch.column(0).as_boolean();
            assert!(a.value(0));
            assert!(!a.value(1));

            let b = batch.column(1).as_boolean();
            assert!(!b.value(0));
            assert!(b.value(1));

            let c = batch.column(2).as_boolean();
            assert!(c.is_null(0));
            // Don't check value for null - just verify nullability
            assert!(!c.is_null(1));
            assert!(c.value(1));

            Ok(())
        })
    }

    #[test]
    fn duckdb_data_types_integer() -> Result<()> {
        with_empty_statement(Backend::DuckDBExtended, |mut statement| {
            statement.set_sql_query(
                r#"SELECT * FROM (
                    VALUES
                    (16::smallint,     32,                  64,                    32.0::real,         64.0::double),
                    (NULL,             32,                  NULL,                  32.0::real,         NULL),
                    (32767::smallint,  2147483647::integer, 1000000000000000000::bigint, 1000000.0::real, 1000000000000000000.0::double)
                ) AS tbl(i16, i32, i64, f32, f64)"#,
            )?;
            let batch = statement
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            let schema = batch.schema();
            assert_eq!(schema.field(0).name(), "i16");
            assert_eq!(schema.field(1).name(), "i32");
            assert_eq!(schema.field(2).name(), "i64");
            assert_eq!(schema.field(3).name(), "f32");
            assert_eq!(schema.field(4).name(), "f64");

            let int16 = batch.column(0).as_primitive::<Int16Type>();
            assert_eq!(int16.value(0), 16);
            assert!(int16.is_null(1));
            assert_eq!(int16.value(2), 32767);

            let int32 = batch.column(1).as_primitive::<Int32Type>();
            assert_eq!(int32.value(0), 32);
            assert_eq!(int32.value(1), 32);
            assert_eq!(int32.value(2), 2147483647);

            let int64 = batch.column(2).as_primitive::<Int64Type>();
            assert_eq!(int64.value(0), 64);
            assert!(int64.is_null(1));
            assert_eq!(int64.value(2), 1000000000000000000i64);

            let float = batch.column(3).as_primitive::<Float32Type>();
            assert_eq!(float.value(0), 32.0);
            assert_eq!(float.value(1), 32.0);
            assert_eq!(float.value(2), 1000000.0f32);

            let double = batch.column(4).as_primitive::<Float64Type>();
            assert_eq!(double.value(0), 64.0);
            assert!(double.is_null(1));
            assert_eq!(double.value(2), 1000000000000000000.0f64);

            Ok(())
        })
    }

    #[test]
    fn duckdb_data_types_string() -> Result<()> {
        with_empty_statement(Backend::DuckDBExtended, |mut statement| {
            // Note: Using CONCAT instead of || operator to avoid Arrow StringViewArray panic
            statement.set_sql_query(
                r#"SELECT * FROM (
                    VALUES
                    (42, 'Snowman ☃'),
                    (43, NULL),
                    (NULL, CONCAT(REPEAT('A string that is longer than 64 characters because it goes on and on about nothing in particular ☃', 16), ''))
                ) AS tbl(id, name)"#,
            )?;
            let batch = statement
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            let schema = batch.schema();
            let fields = schema.fields();
            assert_eq!(fields[0].name(), "id");
            assert_eq!(fields[1].name(), "name");

            let int_col = batch.column(0).as_primitive::<Int32Type>();
            assert!(int_col.len() == 3);

            // (42, 'Snowman ☃')
            assert!(int_col.is_valid(0));
            assert_eq!(int_col.value(0), 42);

            // (43, NULL)
            assert!(int_col.is_valid(1));
            assert_eq!(int_col.value(1), 43);

            // (NULL, 'A string that is...')
            assert!(int_col.is_null(2));

            Ok(())
        })
    }

    #[test]
    fn duckdb_null_handling() -> Result<()> {
        with_empty_statement(Backend::DuckDBExtended, |mut statement| {
            statement.set_sql_query(
                r#"SELECT * FROM (
                    VALUES
                    (NULL::integer, NULL::varchar, NULL::boolean),
                    (1, 'test', true),
                    (NULL::integer, 'not null', false)
                ) AS tbl(int_col, str_col, bool_col)"#,
            )?;
            let batch = statement
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;

            let int_col = batch.column(0).as_primitive::<Int32Type>();
            assert!(int_col.is_null(0));
            assert!(!int_col.is_null(1));
            assert_eq!(int_col.value(1), 1);
            assert!(int_col.is_null(2));

            let bool_col = batch.column(2).as_boolean();
            assert!(bool_col.is_null(0));
            assert!(!bool_col.is_null(1));
            assert!(bool_col.value(1));
            assert!(!bool_col.is_null(2));
            assert!(!bool_col.value(2));

            Ok(())
        })
    }

    #[test]
    fn duckdb_empty_result() -> Result<()> {
        with_empty_statement(Backend::DuckDBExtended, |mut statement| {
            statement.set_sql_query("SELECT 1 AS one WHERE 1 = 0")?;
            let mut batch_reader = statement.execute()?;
            let batch = batch_reader.next();
            // DuckDB returns an empty batch rather than None
            match batch {
                Some(Ok(b)) => assert_eq!(b.num_rows(), 0),
                Some(Err(e)) => return Err(e.into()),
                None => {} // Also acceptable
            }
            Ok(())
        })
    }

    #[ignore = "Spark is WIP"]
    #[test]
    fn statement_execute_spark() -> Result<()> {
        execute_statement(Backend::Spark)
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    fn commit_snowflake() -> Result<()> {
        // https://github.com/apache/arrow-adbc/issues/2581
        with_connection(Backend::Snowflake, |conn| {
            conn.set_option(OptionConnection::AutoCommit, "false".into())?;
            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("SELECT 'could be an insert statement'")?;
            let batch = stmt
                .execute()?
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            assert_eq!(
                batch.column(0).as_string::<i32>().value(0),
                "could be an insert statement"
            );
            conn.commit()
        })
    }

    #[test_with::env(ADBC_DRIVER_TESTS)]
    #[test]
    /// Check execute schema of statement with `SHOW WAREHOUSES` query.
    fn statement_execute_schema() -> Result<()> {
        let backend = Backend::Snowflake;
        with_empty_statement(backend, |mut statement| {
            statement.set_sql_query("SHOW WAREHOUSES")?;
            let schema = statement.execute_schema()?;
            let field_names = schema
                .fields()
                .into_iter()
                .map(|field| field.name().as_ref())
                .collect::<HashSet<_>>();
            let expected_field_names = [
                "name",
                "state",
                "type",
                "size",
                "running",
                "queued",
                "is_default",
                "is_current",
                "auto_suspend",
                "auto_resume",
                "available",
                "provisioning",
                "quiescing",
                "other",
                "created_on",
                "resumed_on",
                "updated_on",
                "owner",
                "comment",
                "resource_monitor",
                "actives",
                "pendings",
                "failed",
                "suspended",
                "uuid",
                // "budget",
                "owner_role_type",
            ]
            .into_iter()
            .collect::<HashSet<_>>();
            assert_eq!(
                expected_field_names
                    .difference(&field_names)
                    .collect::<Vec<_>>(),
                Vec::<&&str>::default()
            );
            Ok(())
        })
    }

    /// Verifies that DuckDB releases the file lock when the database handle is dropped,
    /// allowing a second connection to open the same file for writing.
    ///
    /// This test exists because the LSP holds a DuckDB database handle in a cache
    /// (DatabaseMap in AdbcEngine) and connections in thread-local storage, which
    /// keeps the file locked and prevents CLI commands from accessing the database.
    #[test]
    fn duckdb_file_lock_released_after_drop() -> Result<()> {
        use std::fs;

        let db_path = duckdb_per_user_file_path("dbt_adbc_test_lock");
        let db_path_str = db_path.to_string_lossy().to_string();

        // Clean up any existing file
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));

        // Open database and create a table, then drop everything
        {
            let mut driver = driver_for(Backend::DuckDBExtended)?;
            let builder = database_builder_for_duckdb_file(&db_path_str)?;
            let mut database = builder.build(&mut driver)?;
            let mut conn = connection::Builder::default().build(&mut database)?;

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("CREATE TABLE lock_test (id INTEGER)")?;
            let _ = stmt.execute()?;

            // Explicitly drop in order: statement, connection, database
            drop(stmt);
            drop(conn);
            drop(database);
            drop(driver);
        }

        // After dropping, a second process/connection should be able to open
        // the same file for writing (no lingering lock).
        {
            let mut driver = driver_for(Backend::DuckDBExtended)?;
            let builder = database_builder_for_duckdb_file(&db_path_str)?;
            let mut database = builder.build(&mut driver)?;
            let mut conn = connection::Builder::default().build(&mut database)?;

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("INSERT INTO lock_test VALUES (42)")?;
            let _ = stmt.execute()?;
        }

        // Cleanup
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));

        Ok(())
    }

    /// Verifies that DuckDB file locks are cross-process: while one process
    /// holds a database handle open, another process cannot open it for writing.
    /// Dropping the handle releases the lock, allowing the other process in.
    ///
    /// This simulates the LSP scenario: the LSP process holds a DuckDB database
    /// handle in its DatabaseMap cache. The CLI (a separate process) cannot
    /// access the database until the LSP releases it.
    #[test]
    fn duckdb_file_lock_is_cross_process() -> Result<()> {
        use std::fs;
        use std::process::Command;

        // Skip if duckdb CLI is not installed
        if Command::new("duckdb").arg("--version").output().is_err() {
            eprintln!("Skipping: duckdb CLI not installed");
            return Ok(());
        }

        let db_path = duckdb_per_user_file_path("dbt_adbc_test_cross_process_lock");
        let db_path_str = db_path.to_string_lossy().to_string();

        // Clean up any existing file
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));

        // Open database and keep the handle alive (simulates LSP holding the lock)
        let mut driver = driver_for(Backend::DuckDBExtended)?;
        let builder = database_builder_for_duckdb_file(&db_path_str)?;
        let mut database = builder.build(&mut driver)?;

        // Use it to create a table
        {
            let mut conn = connection::Builder::default().build(&mut database)?;
            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query("CREATE TABLE cross_proc_test (id INTEGER)")?;
            let _ = stmt.execute()?;
        }
        // Connection dropped, but database handle still alive

        // Try to access from a subprocess while we hold the handle.
        // The DuckDB CLI should fail with a lock error.
        let output = Command::new("duckdb")
            .arg(&db_path_str)
            .arg("-c")
            .arg("INSERT INTO cross_proc_test VALUES (1);")
            .output()
            .expect("failed to spawn duckdb CLI");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains("lock"),
            "Expected cross-process lock error while database handle is alive, got: {}",
            stderr
        );

        // Drop the database handle (simulates clearing the DatabaseMap)
        drop(database);
        drop(driver);

        // Now the subprocess should succeed
        let output = Command::new("duckdb")
            .arg(&db_path_str)
            .arg("-c")
            .arg("INSERT INTO cross_proc_test VALUES (1);")
            .output()
            .expect("failed to spawn duckdb CLI");

        assert!(
            output.status.success(),
            "Expected CLI access to succeed after dropping database handle, got: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        // Cleanup
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));

        Ok(())
    }

    /// Simulates the LSP bug: a connection kept alive (as in a thread-local cache)
    /// prevents the file lock from being released even after the database handle
    /// (DatabaseMap) is cleared.
    ///
    /// This is the OLD behavior: ConnectionGuard stashes the connection in a
    /// thread-local, so dropping the database handle alone is not enough.
    #[test]
    fn duckdb_connection_kept_alive_holds_lock_after_database_drop() -> Result<()> {
        use std::fs;
        use std::process::Command;

        if Command::new("duckdb").arg("--version").output().is_err() {
            eprintln!("Skipping: duckdb CLI not installed");
            return Ok(());
        }

        let db_path = duckdb_per_user_file_path("dbt_adbc_test_conn_holds_lock");
        let db_path_str = db_path.to_string_lossy().to_string();
        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));

        // Create database and a connection
        let mut driver = driver_for(Backend::DuckDBExtended)?;
        let builder = database_builder_for_duckdb_file(&db_path_str)?;
        let mut database = builder.build(&mut driver)?;
        let mut conn = connection::Builder::default().build(&mut database)?;

        // Use the connection
        let mut stmt = conn.new_statement()?;
        stmt.set_sql_query("CREATE TABLE conn_lock_test (id INTEGER)")?;
        let _ = stmt.execute()?;
        drop(stmt);

        // Simulate clearing the DatabaseMap (drop the database handle)
        // but keep the connection alive (simulates thread-local stashing)
        drop(database);
        drop(driver);

        // The file should STILL be locked because the connection is alive
        let output = Command::new("duckdb")
            .arg(&db_path_str)
            .arg("-c")
            .arg("INSERT INTO conn_lock_test VALUES (1);")
            .output()
            .expect("failed to spawn duckdb CLI");

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains("lock"),
            "Expected lock error while connection is alive (simulates old behavior), got: {}",
            stderr
        );

        // Now drop the connection (simulates the fix: stash_on_drop = false)
        drop(conn);

        // File should now be unlocked
        let output = Command::new("duckdb")
            .arg(&db_path_str)
            .arg("-c")
            .arg("INSERT INTO conn_lock_test VALUES (1);")
            .output()
            .expect("failed to spawn duckdb CLI");

        assert!(
            output.status.success(),
            "Expected CLI access to succeed after dropping connection, got: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let _ = fs::remove_file(&db_path);
        let _ = fs::remove_file(format!("{}.wal", db_path_str));
        Ok(())
    }

    #[test_with::env(ADBC_CLICKHOUSE_URI)]
    #[test]
    fn statement_execute_clickhouse() -> Result<()> {
        execute_statement(Backend::ClickHouse)
    }

    #[test_with::env(ADBC_EXASOL_URI)]
    #[test]
    fn statement_execute_exasol() -> Result<()> {
        execute_statement(Backend::Exasol)
    }
}
