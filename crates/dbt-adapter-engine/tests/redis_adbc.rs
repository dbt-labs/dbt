//! Live ADBC 1.1 audit using the same Rust driver manager as the native adapter.
use adbc_core::{
    error::Error,
    options::{AdbcVersion, ObjectDepth, OptionStatement, OptionValue},
};
use arrow_array::{Int64Array, RecordBatch, RecordBatchIterator};
use arrow_schema::{DataType, Field, Schema};
use dbt_adapter_core::AdapterType;
use dbt_adapter_engine::retry::QueryRetryPolicy;
use dbt_adbc::{Backend, Connection, Database, driver};
use dbt_auth::{AdapterConfig, auth_for_backend};
use dbt_common::Cancellable;
use std::{
    env,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::Arc,
};

fn database() -> Box<dyn Database> {
    let backend = Backend::Generic {
        library_name: "adbc_driver_redis",
        entrypoint: Some(b"AdbcDriverInit"),
    };
    let mut options = dbt_yaml::Mapping::new();
    if let Ok(ca) = env::var("REDIS_TLS_CA_FILE") {
        options.insert("adbc.redis.tls_ca_file".into(), ca.into());
    }
    let config = AdapterConfig::new(dbt_yaml::Mapping::from_iter([
        (
            "uri".into(),
            env::var("REDIS_URI").expect("REDIS_URI").into(),
        ),
        (
            "username".into(),
            env::var("REDIS_USERNAME").expect("REDIS_USERNAME").into(),
        ),
        (
            "password".into(),
            env::var("REDIS_PASSWORD").expect("REDIS_PASSWORD").into(),
        ),
        (
            "driver_options".into(),
            dbt_yaml::Value::Mapping(options, Default::default()),
        ),
    ]));
    let mut driver = driver::Builder::new(
        backend,
        driver::LoadStrategy::System(Some(
            env::var("REDIS_ADBC_DRIVER").expect("REDIS_ADBC_DRIVER"),
        )),
    )
    .with_adbc_version(AdbcVersion::V110)
    .try_load()
    .unwrap();
    auth_for_backend(backend)
        .configure(&config)
        .unwrap()
        .build(&mut driver)
        .unwrap()
}

fn query(conn: &mut dyn Connection, sql: &str) -> Result<Vec<RecordBatch>, Cancellable<Error>> {
    QueryRetryPolicy::new(AdapterType::RedisAdbc, &AdapterConfig::default()).execute(|| {
        let mut stmt = conn.new_statement()?;
        stmt.set_sql_query(sql)?;
        let reader = stmt.execute()?;
        Ok(reader.collect::<Result<Vec<_>, _>>().map_err(Error::from)?)
    })
}

#[test]
#[ignore = "requires a dedicated, verified TLS Redis Enterprise fixture and native driver"]
fn live_redis_adbc_protocol_and_concurrency() {
    let mut db = database();
    assert_eq!(db.adbc_version().unwrap(), AdbcVersion::V110);
    let db: Arc<dyn Database> = Arc::from(db);
    let mut conn = db.new_connection().unwrap();
    let schema = format!(
        "dbt_api_audit_{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    query(conn.as_mut(), &format!("create schema {schema}")).unwrap();
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        query(
            conn.as_mut(),
            &format!("create table {schema}.probe (id bigint)"),
        )
        .unwrap();
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query(&format!("insert into {schema}.probe values (?)"))
            .unwrap();
        stmt.prepare().unwrap();
        let fields = Arc::new(Schema::new(vec![Field::new("id", DataType::Int64, false)]));
        stmt.bind(
            RecordBatch::try_new(fields.clone(), vec![Arc::new(Int64Array::from(vec![7]))])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stmt.execute_update().unwrap(), Some(1));
        drop(stmt);

        let mut ingest = conn.new_statement().unwrap();
        for (key, value) in [
            ("adbc.ingest.target_table", "probe"),
            ("adbc.ingest.target_db_schema", schema.as_str()),
            ("adbc.ingest.mode", "adbc.ingest.mode.append"),
        ] {
            ingest
                .set_option(
                    OptionStatement::Other(key.into()),
                    OptionValue::String(value.into()),
                )
                .unwrap();
        }
        let batch =
            RecordBatch::try_new(fields.clone(), vec![Arc::new(Int64Array::from(vec![8, 9]))])
                .unwrap();
        ingest
            .bind_stream(Box::new(RecordBatchIterator::new(vec![Ok(batch)], fields)))
            .unwrap();
        assert_eq!(ingest.execute_update().unwrap(), Some(2));
        drop(ingest);

        let metadata = conn
            .get_objects(
                ObjectDepth::All,
                Some("redis"),
                Some(&schema),
                Some("probe"),
                None,
                None,
            )
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(metadata.iter().any(|batch| batch.num_rows() > 0));
        let table_schema = conn
            .get_table_schema(Some("redis"), Some(&schema), "probe")
            .unwrap();
        assert_eq!(table_schema.field(0).data_type(), &DataType::Int64);
        let mut stmt = conn.new_statement().unwrap();
        stmt.set_sql_query(&format!("select id from {schema}.probe where false"))
            .unwrap();
        assert_eq!(
            stmt.execute_schema().unwrap().field(0).data_type(),
            &DataType::Int64
        );
        assert_eq!(
            stmt.execute()
                .unwrap()
                .map(|batch| batch.unwrap().num_rows())
                .sum::<usize>(),
            0
        );
        drop(stmt);

        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|value| {
                    let db = db.clone();
                    let schema = &schema;
                    scope.spawn(move || {
                        let mut conn = db.new_connection().unwrap();
                        query(conn.as_mut(), "create temp table dbt_api_local (id bigint)")
                            .unwrap();
                        query(
                            conn.as_mut(),
                            &format!("insert into dbt_api_local values ({value})"),
                        )
                        .unwrap();
                        assert!(
                            query(conn.as_mut(), "select * from dbt_api_missing_table").is_err()
                        );
                        let batches = query(conn.as_mut(), "select id from dbt_api_local").unwrap();
                        let ids = batches[0]
                            .column(0)
                            .as_any()
                            .downcast_ref::<Int64Array>()
                            .unwrap();
                        assert_eq!(ids.value(0), value);
                        let batches = query(
                            conn.as_mut(),
                            &format!("select id from {schema}.probe order by id"),
                        )
                        .unwrap();
                        assert_eq!(
                            batches.iter().map(|batch| batch.num_rows()).sum::<usize>(),
                            3
                        );
                    })
                })
                .collect();
            for handle in handles {
                handle.join().unwrap();
            }
        });
    }));
    query(conn.as_mut(), &format!("drop schema {schema} cascade")).expect("audit schema cleanup");
    if let Err(error) = outcome {
        resume_unwind(error);
    }
}
