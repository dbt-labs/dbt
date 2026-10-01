#[cfg(test)]
mod tests {
    use adbc_core::{
        error::{Error, Result},
        options::AdbcVersion,
    };
    use arrow_array::{
        cast::AsArray,
        types::{Decimal128Type, Int8Type, Int16Type, Int32Type, Int64Type},
    };
    use arrow_schema::DataType;
    use dbt_adbc::{Backend, connection, database, driver};

    use crate::{AdapterConfig, Auth, snowflake::SnowflakeAuth};

    /// Execute a statement through the "flock" driver.
    ///
    /// Requires:
    /// - `FLOCK_DRIVER_TESTS` env var set
    /// - `~/.dbt/dbt_cloud.yml` with valid cloud credentials
    #[test_with::env(FLOCK_DRIVER_TESTS)]
    #[test]
    fn statement_execute_flock() -> Result<()> {
        let backend = Backend::Snowflake;
        // Load the flock driver via the Remote strategy.
        let mut driver = driver::Builder::new(backend, driver::LoadStrategy::Remote)
            .with_adbc_version(AdbcVersion::V110)
            .try_load()?;

        let mut builder = database::Builder::new(backend);

        // dbt Cloud credentials from ~/.dbt/dbt_cloud.yml (with env-var overrides).
        let cloud_config_path = dbt_cloud_config::get_cloud_project_path()
            .map_err(|e| Error::with_message_and_status(e, adbc_core::error::Status::Internal))?;
        let cloud_yml = dbt_cloud_config::parse_cloud_config(&cloud_config_path)
            .map_err(|e| Error::with_message_and_status(e, adbc_core::error::Status::Internal))?;
        if let Some(resolved) = dbt_cloud_config::resolve_cloud_config(cloud_yml.as_ref(), None) {
            if let Some(project_id) = resolved.project_id.as_deref() {
                builder.with_named_option("dbt_cloud.project_id", project_id)?;
            }
            if let Some(credentials) = resolved.credentials {
                builder.with_named_option("dbt_cloud.token", credentials.token)?;
                builder.with_named_option("dbt_cloud.host", credentials.host)?;
                builder.with_named_option("dbt_cloud.account_id", credentials.account_id)?;
            }
        }

        let mut database = builder.build(&mut driver)?;
        let conn_builder = connection::Builder::default();
        let mut conn = conn_builder.build(&mut database)?;
        let mut stmt = conn.new_statement()?;
        stmt.set_sql_query("SELECT 21 + 21")?;
        let batch = stmt
            .execute()?
            .next()
            .expect("a record batch")
            .map_err(Error::from)?;
        let col = batch.column(0);
        let value: i64 = match col.data_type() {
            DataType::Int8 => col.as_primitive::<Int8Type>().value(0) as i64,
            DataType::Int16 => col.as_primitive::<Int16Type>().value(0) as i64,
            DataType::Int32 => col.as_primitive::<Int32Type>().value(0) as i64,
            DataType::Int64 => col.as_primitive::<Int64Type>().value(0),
            DataType::Decimal128(_, _) => col.as_primitive::<Decimal128Type>().value(0) as i64,
            dt => panic!("unexpected column type: {dt:?}"),
        };
        assert_eq!(value, 42);
        Ok(())
    }

    /// Compares connection/execute/fetch latency between the "flock" driver
    /// and a direct (non-flock) ADBC connection to Snowflake.
    ///
    /// This is a manual benchmark, not a correctness test: no assertions are
    /// made on the timing numbers themselves, they are just printed.
    ///
    /// Requires:
    /// - `FLOCK_DRIVER_TESTS` env var set
    /// - `~/.dbt/dbt_cloud.yml` with valid cloud credentials (for the flock path)
    /// - `~/.dbt/profiles.yml` with a `fusion_tests` profile whose `snowflake`
    ///   output has real (non-dummy) credentials, as set up by
    ///   `cargo xtask init-creds` (for the raw ADBC path)
    #[test_with::env(FLOCK_DRIVER_TESTS)]
    #[test]
    fn latency_flock_vs_raw_snowflake() -> Result<()> {
        const QUERY: &str = "SELECT 21 + 21";

        struct Timings {
            connect: std::time::Duration,
            execute: std::time::Duration,
            fetch: std::time::Duration,
            total: std::time::Duration,
        }

        // --- flock path -----------------------------------------------------
        let flock_timings = {
            let total_start = std::time::Instant::now();

            let backend = Backend::Snowflake;
            let mut driver = driver::Builder::new(backend, driver::LoadStrategy::Remote)
                .with_adbc_version(AdbcVersion::V110)
                .try_load()?;

            let mut builder = database::Builder::new(backend);

            let cloud_config_path = dbt_cloud_config::get_cloud_project_path().map_err(|e| {
                Error::with_message_and_status(e, adbc_core::error::Status::Internal)
            })?;
            let cloud_yml =
                dbt_cloud_config::parse_cloud_config(&cloud_config_path).map_err(|e| {
                    Error::with_message_and_status(e, adbc_core::error::Status::Internal)
                })?;
            if let Some(resolved) = dbt_cloud_config::resolve_cloud_config(cloud_yml.as_ref(), None)
            {
                if let Some(project_id) = resolved.project_id.as_deref() {
                    builder.with_named_option("dbt_cloud.project_id", project_id)?;
                }
                if let Some(credentials) = resolved.credentials {
                    builder.with_named_option("dbt_cloud.token", credentials.token)?;
                    builder.with_named_option("dbt_cloud.host", credentials.host)?;
                    builder.with_named_option("dbt_cloud.account_id", credentials.account_id)?;
                }
            }
            let connect_start = std::time::Instant::now();
            let mut database = builder.build(&mut driver)?;
            let conn_builder = connection::Builder::default();
            let mut conn = conn_builder.build(&mut database)?;
            let connect = connect_start.elapsed();

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query(QUERY)?;

            let execute_start = std::time::Instant::now();
            let mut reader = stmt.execute()?;
            let execute = execute_start.elapsed();

            let fetch_start = std::time::Instant::now();
            let batch = reader
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            assert_eq!(
                batch.column(0).as_primitive::<Decimal128Type>().value(0),
                42
            );
            for result in reader {
                result.map_err(Error::from)?;
            }
            let fetch = fetch_start.elapsed();

            let total = total_start.elapsed();
            Timings {
                connect,
                execute,
                fetch,
                total,
            }
        };

        // --- raw snowflake path ----------------------------------------------
        let snowflake_timings = {
            let total_start = std::time::Instant::now();

            let backend = Backend::Snowflake;
            let mut driver = driver::Builder::new(backend, driver::LoadStrategy::CdnCache)
                .with_adbc_version(AdbcVersion::V110)
                .try_load()?;

            // Reuse the shared `fusion_tests` live-test credentials from
            // `~/.dbt/profiles.yml` (populated via `cargo xtask init-creds`),
            // the same credentials every other Fusion adapter/SLT live test uses.
            let profiles_path = std::path::PathBuf::from(std::env::var("HOME").expect("HOME set"))
                .join(".dbt")
                .join("profiles.yml");
            let raw = std::fs::read_to_string(&profiles_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", profiles_path.display()));
            let doc: dbt_yaml::Value =
                dbt_yaml::from_str(&raw).expect("profiles.yml must be valid YAML");
            let snowflake_mapping = doc
                .get("fusion_tests")
                .and_then(|v| v.get("outputs"))
                .and_then(|v| v.get("snowflake"))
                .and_then(|v| match v {
                    dbt_yaml::Value::Mapping(m, _) => Some(m.clone()),
                    _ => None,
                })
                .expect("fusion_tests.outputs.snowflake mapping in ~/.dbt/profiles.yml");
            let config = AdapterConfig::new(snowflake_mapping);

            let auth = SnowflakeAuth::new(Box::new(crate::NoopAuthWarningPrinter));
            let outcome = auth.configure(&config).map_err(|e| {
                Error::with_message_and_status(format!("{e:?}"), adbc_core::error::Status::Internal)
            })?;

            let connect_start = std::time::Instant::now();
            let mut database = outcome.build(&mut driver)?;
            let conn_builder = connection::Builder::default();
            let mut conn = conn_builder.build(&mut database)?;
            let connect = connect_start.elapsed();

            let mut stmt = conn.new_statement()?;
            stmt.set_sql_query(QUERY)?;

            let execute_start = std::time::Instant::now();
            let mut reader = stmt.execute()?;
            let execute = execute_start.elapsed();

            let fetch_start = std::time::Instant::now();
            let batch = reader
                .next()
                .expect("a record batch")
                .map_err(Error::from)?;
            assert_eq!(
                batch.column(0).as_primitive::<Decimal128Type>().value(0),
                42
            );
            for result in reader {
                result.map_err(Error::from)?;
            }
            let fetch = fetch_start.elapsed();

            let total = total_start.elapsed();
            Timings {
                connect,
                execute,
                fetch,
                total,
            }
        };

        println!("=== Flock vs raw Snowflake latency ===");
        println!("{:<12}{:<13}{:<13}", "", "flock", "snowflake");
        println!(
            "{:<12}{:<13}{:<13}",
            "connect",
            format!("{:.2?}", flock_timings.connect),
            format!("{:.2?}", snowflake_timings.connect)
        );
        println!(
            "{:<12}{:<13}{:<13}",
            "execute",
            format!("{:.2?}", flock_timings.execute),
            format!("{:.2?}", snowflake_timings.execute)
        );
        println!(
            "{:<12}{:<13}{:<13}",
            "fetch",
            format!("{:.2?}", flock_timings.fetch),
            format!("{:.2?}", snowflake_timings.fetch)
        );
        println!(
            "{:<12}{:<13}{:<13}",
            "total",
            format!("{:.2?}", flock_timings.total),
            format!("{:.2?}", snowflake_timings.total)
        );

        Ok(())
    }
}
