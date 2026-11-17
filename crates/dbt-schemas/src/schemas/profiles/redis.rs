use super::*;

/// Wire protocol; the SQL dialect and adapter identity remain Redis.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, DbtSchema)]
#[serde(rename_all = "lowercase")]
pub enum RedisTransport {
    Redis,
    Flightsql,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, DbtSchema, Merge)]
#[merge(strategy = merge_strategies_extend::overwrite_option)]
#[serde(rename_all = "snake_case")]
pub struct RedisAdbcDbConfig {
    #[serde(flatten)]
    #[merge(strategy = merge_strategies_extend::overwrite_always)]
    pub extra: HashMap<String, YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transport: Option<RedisTransport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_ca_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_zone: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub threads: Option<StringOrInteger>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aggregate_pushdown: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rename_rekey: Option<YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_timeout: Option<YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub write_timeout: Option<YmlValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub driver_options: Option<HashMap<String, YmlValue>>,
}

impl RedisAdbcDbConfig {
    pub(super) fn into_target(self, adapter_type: String) -> Result<TargetContext, String> {
        if !self.extra.is_empty() {
            return Err("Unsupported Redis profile option".to_string());
        }
        if self
            .database
            .as_deref()
            .is_some_and(|database| database != "redis")
        {
            return Err("Redis only supports the redis catalog".to_string());
        }
        Ok(TargetContext::RedisAdbc(CommonTargetContext {
            database: self.database.unwrap_or_else(|| "redis".to_string()),
            schema: self.schema.unwrap_or_else(|| "public".to_string()),
            type_: adapter_type,
            threads: self
                .threads
                .map(|value| {
                    value
                        .to_string()
                        .parse::<u16>()
                        .ok()
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            "Redis 'threads' must be a positive integer below 65536".to_string()
                        })
                })
                .transpose()?,
        }))
    }
}

#[cfg(test)]
mod redis_adbc_tests {
    use super::*;

    #[test]
    fn flight_profile_is_typed_and_target_has_no_client_credentials() {
        let config: DbConfig = dbt_yaml::from_str("type: redis_adbc\ntransport: flightsql\nuri: grpc+tls://localhost:8815\npassword: secret-value\ntls_ca_file: /private/ca.pem\ndriver_options: {adbc.flight.sql.rpc.timeout_seconds.query: 30}\n").unwrap();
        let DbConfig::Redis_adbc(ref redis) = config else {
            panic!("Redis expected")
        };
        assert_eq!(redis.transport, Some(RedisTransport::Flightsql));
        let target = dbt_yaml::to_string(&TargetContext::try_from(config).unwrap()).unwrap();
        for secret in [
            "secret-value",
            "grpc+tls",
            "/private/ca.pem",
            "driver_options",
        ] {
            assert!(!target.contains(secret));
        }
        assert!(dbt_yaml::from_str::<DbConfig>("type: redis_adbc\ntransport: invalid\n").is_err());
    }

    #[test]
    fn native_profile_round_trips_and_hides_credentials_from_target() {
        let config: DbConfig = dbt_yaml::from_str(
            "type: redis_adbc\ndriver: /tmp/libadbc_driver_redis.so\nuri: redis://alice:uri-secret@localhost:6379/0\nusername: alice\npassword: profile-secret\nschema: analytics\nthreads: 2\n"
        ).unwrap();
        assert_eq!(config.adapter_type(), AdapterType::RedisAdbc);
        assert_eq!(config.get_database_or_default(), "redis");
        assert!(config.get_unique_field().is_none());
        let serialized = dbt_yaml::to_string(&config).unwrap();
        let parsed: DbConfig = dbt_yaml::from_str(&serialized).unwrap();
        assert_eq!(parsed, config);
        let context = TargetContext::try_from(config).unwrap();
        let serialized = dbt_yaml::to_string(&context).unwrap();
        for expected in [
            "type: redis_adbc",
            "schema: analytics",
            "database: redis",
            "threads: 2",
        ] {
            assert!(serialized.contains(expected));
        }
        assert!(!serialized.contains("secret"));
        for key in DbConfig::connection_keys_for(AdapterType::RedisAdbc) {
            assert!(!["uri", "password"].contains(key));
        }
    }

    #[test]
    fn profile_defaults_and_thread_validation() {
        for threads in ["0", "-1", "65536", "abc"] {
            let config: DbConfig =
                dbt_yaml::from_str(&format!("type: redis_adbc\nthreads: {threads}\n")).unwrap();
            assert!(TargetContext::try_from(config).is_err());
        }
        let config: DbConfig = dbt_yaml::from_str("type: redis_adbc\n").unwrap();
        let TargetContext::RedisAdbc(context) = TargetContext::try_from(config).unwrap() else {
            panic!("Redis context expected")
        };
        assert_eq!(context.database, "redis");
        assert_eq!(context.schema, "public");
        let config =
            dbt_yaml::from_str::<DbConfig>("type: redis_adbc\nunknown_option: true\n").unwrap();
        assert!(TargetContext::try_from(config).is_err());
        assert!(dbt_yaml::from_str::<DbConfig>("type: unknown_redis\n").is_err());
        let config: DbConfig = dbt_yaml::from_str("type: redis_adbc\ndatabase: other\n").unwrap();
        assert!(TargetContext::try_from(config).is_err());
    }
}
