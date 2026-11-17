use std::borrow::Cow;

use adbc_core::options::{OptionDatabase, OptionValue};
use dbt_adbc::{Backend, database};
use url::Url;

use crate::{AdapterConfig, Auth, AuthError};

// Driver defaults: https://github.com/alberttwong/redis-adbc-driver/blob/8719760eff2844f49b5f242bf3c931075be1749e/go/driver.go
const DEFAULT_URI: &str = "redis://localhost:6379/0";
const DEFAULT_SCHEMA: &str = "public";

const FLIGHT_COOKIES: &str = "adbc.flight.sql.rpc.with_cookie_middleware";
const FLIGHT_ROOT_CERTS: &str = "adbc.flight.sql.client_option.tls_root_certs";

const FLIGHT_BACKEND: Backend = Backend::Generic {
    library_name: "adbc_driver_flightsql",
    entrypoint: Some(b"AdbcDriverFlightSQLInit"),
};

struct ConnectionConfig<'a> {
    backend: Backend,
    uri: Url,
    options: Vec<(OptionDatabase, Cow<'a, str>)>,
}

impl ConnectionConfig<'_> {
    fn apply(self) -> Result<database::Builder, AuthError> {
        let mut builder = database::Builder::new(self.backend);
        builder.with_uri(self.uri);
        for (key, value) in self.options {
            builder.with_typed_option(key, OptionValue::String(value.into_owned()))?;
        }
        Ok(builder)
    }
}

fn string_or_default<'a>(
    config: &'a AdapterConfig,
    key: &str,
    default: &'a str,
) -> Result<&'a str, AuthError> {
    match config.get(key) {
        None => Ok(default),
        Some(value) if value.is_null() => Ok(default),
        Some(value) => value
            .as_str()
            .ok_or_else(|| AuthError::config(format!("Redis '{key}' must be a string"))),
    }
}

fn scalar_string(value: &dbt_yaml::Value) -> Option<Cow<'_, str>> {
    match value {
        dbt_yaml::Value::String(..) | dbt_yaml::Value::Number(..) | dbt_yaml::Value::Bool(..) => {
            Some(crate::config::yml_value_to_string(value))
        }
        _ => None,
    }
}

fn parse_config(config: &AdapterConfig) -> Result<ConnectionConfig<'_>, AuthError> {
    const FIELDS: &[&str] = &[
        "type",
        "transport",
        "tls_ca_file",
        "driver",
        "uri",
        "username",
        "password",
        "database",
        "schema",
        "threads",
        "time_zone",
        "aggregate_pushdown",
        "rename_rekey",
        "read_timeout",
        "write_timeout",
        "driver_options",
    ];
    for (key, _) in config.repr() {
        if !key.as_str().is_some_and(|key| FIELDS.contains(&key)) {
            return Err(AuthError::config("Unsupported Redis profile option"));
        }
    }
    if string_or_default(config, "database", "redis")? != "redis" {
        return Err(AuthError::config("Redis only supports the 'redis' catalog"));
    }
    let flight = match string_or_default(config, "transport", "redis")? {
        "redis" => false,
        "flightsql" => true,
        _ => {
            return Err(AuthError::config(
                "Redis transport must be redis or flightsql",
            ));
        }
    };
    if flight {
        return parse_flight_config(config);
    }
    if config.get_str_non_empty("tls_ca_file").is_some() {
        return Err(AuthError::config(
            "tls_ca_file is a Flight client option; use adbc.redis.tls_ca_file for native Redis",
        ));
    }
    let uri = Url::parse(string_or_default(config, "uri", DEFAULT_URI)?)
        .map_err(|_| AuthError::config("Invalid Redis URI"))?;
    if !matches!(uri.scheme(), "redis" | "rediss") || uri.host_str().is_none() {
        return Err(AuthError::config(
            "Redis URI must use redis:// or rediss:// with a host",
        ));
    }
    let schema = string_or_default(config, "schema", DEFAULT_SCHEMA)?;
    if schema.is_empty() {
        return Err(AuthError::config("Redis 'schema' must not be empty"));
    }
    let mut options = vec![(
        OptionDatabase::Other("adbc.redis.default_schema".into()),
        Cow::Borrowed(schema),
    )];

    // Separate nonempty credentials override URI credentials, as in the v1 adapter.
    // https://github.com/alberttwong/redis-dbt-adapter-with-project/blob/76bfecfd62e3c5c7dce153185fcb6f617262e700/v1/adapter/dbt/adapters/redis_adbc/connections.py
    let username = string_or_default(config, "username", "")?;
    if !username.is_empty() {
        options.push((OptionDatabase::Username, Cow::Borrowed(username)));
    }
    if let Some(value) = config.get("password").filter(|value| !value.is_null()) {
        let password = scalar_string(value)
            .ok_or_else(|| AuthError::config("Redis 'password' must be a scalar"))?;
        if !password.is_empty() {
            options.push((OptionDatabase::Password, password));
        }
    }
    let time_zone = string_or_default(config, "time_zone", "")?;
    if !time_zone.is_empty() {
        options.push((
            OptionDatabase::Other("adbc.redis.time_zone".into()),
            Cow::Borrowed(time_zone),
        ));
    }
    const PROFILE_OPTIONS: &[(&str, &str)] = &[
        ("aggregate_pushdown", "adbc.redis.aggregate_pushdown"),
        ("rename_rekey", "adbc.redis.rename_rekey"),
        ("read_timeout", "adbc.redis.read_timeout"),
        ("write_timeout", "adbc.redis.write_timeout"),
    ];
    for (field, option) in PROFILE_OPTIONS {
        if let Some(value) = config.get(field).filter(|value| !value.is_null()) {
            let value = scalar_string(value)
                .ok_or_else(|| AuthError::config(format!("Redis '{field}' must be a scalar")))?;
            if !value.is_empty() {
                options.push((OptionDatabase::Other((*option).to_owned()), value));
            }
        }
    }
    if let Some(value) = config
        .get("driver_options")
        .filter(|value| !value.is_null())
    {
        let mapping = value
            .as_mapping()
            .ok_or_else(|| AuthError::config("Redis driver_options must be a mapping"))?;
        for (key, value) in mapping {
            let key = key
                .as_str()
                .ok_or_else(|| AuthError::config("Redis driver_options keys must be strings"))?;
            if matches!(
                key,
                "uri"
                    | "username"
                    | "password"
                    | "adbc.redis.default_schema"
                    | "adbc.redis.time_zone"
            ) || PROFILE_OPTIONS.iter().any(|(_, option)| *option == key)
            {
                return Err(AuthError::config(
                    "driver_options cannot override a named Redis profile option",
                ));
            }
            let value = scalar_string(value)
                .filter(|_| !value.is_null())
                .ok_or_else(|| {
                    AuthError::config(
                        "Redis driver_options values must be strings, numbers, or booleans",
                    )
                })?;
            options.push((OptionDatabase::Other(key.to_owned()), value));
        }
    }
    Ok(ConnectionConfig {
        backend: Backend::Generic {
            library_name: "adbc_driver_redis",
            entrypoint: Some(b"AdbcDriverInit"),
        },
        uri,
        options,
    })
}

fn parse_flight_config(config: &AdapterConfig) -> Result<ConnectionConfig<'_>, AuthError> {
    let uri = Url::parse(string_or_default(config, "uri", "")?)
        .map_err(|_| AuthError::config("Invalid Flight SQL URI"))?;
    if uri.scheme() != "grpc+tls"
        || uri.host_str().is_none()
        || !uri.username().is_empty()
        || uri.password().is_some()
        || uri.query().is_some()
        || uri.fragment().is_some()
        || !matches!(uri.path(), "" | "/")
    {
        return Err(AuthError::config(
            "Flight SQL requires grpc+tls://host:port; set credentials separately",
        ));
    }
    if string_or_default(config, "schema", DEFAULT_SCHEMA)?.is_empty() {
        return Err(AuthError::config("Redis 'schema' must not be empty"));
    }
    string_or_default(config, "time_zone", "")?;
    for field in [
        "aggregate_pushdown",
        "rename_rekey",
        "read_timeout",
        "write_timeout",
    ] {
        if config
            .get(field)
            .is_some_and(|value| !value.is_null() && value.as_str() != Some(""))
        {
            return Err(AuthError::config(
                "Configure Redis execution options on the Flight server",
            ));
        }
    }
    let mut options = vec![(
        OptionDatabase::Other(FLIGHT_COOKIES.into()),
        Cow::Borrowed("true"),
    )];
    for (field, key) in [
        ("username", OptionDatabase::Username),
        ("password", OptionDatabase::Password),
    ] {
        if let Some(value) = config.get(field).filter(|value| !value.is_null()) {
            let value = if field == "password" {
                scalar_string(value)
                    .ok_or_else(|| AuthError::config("Redis 'password' must be a scalar"))?
            } else {
                Cow::Borrowed(string_or_default(config, field, "")?)
            };
            if !value.is_empty() {
                options.push((key, value));
            }
        }
    }
    let ca_file = string_or_default(config, "tls_ca_file", "")?;
    if !ca_file.is_empty() {
        let roots = std::fs::read_to_string(ca_file)
            .map_err(|_| AuthError::config("Cannot read Flight TLS CA file"))?;
        options.push((
            OptionDatabase::Other(FLIGHT_ROOT_CERTS.into()),
            Cow::Owned(roots),
        ));
    }
    if let Some(value) = config
        .get("driver_options")
        .filter(|value| !value.is_null())
    {
        let mapping = value
            .as_mapping()
            .ok_or_else(|| AuthError::config("Redis driver_options must be a mapping"))?;
        for (key, value) in mapping {
            // Restrict this initial client contract to non-secret RPC settings.
            // Arbitrary headers/private keys would escape through generic builder diagnostics.
            let key = key
                .as_str()
                .ok_or_else(|| AuthError::config("Flight option keys must be strings"))?;
            if !matches!(
                key,
                "adbc.flight.sql.rpc.timeout_seconds.connect"
                    | "adbc.flight.sql.rpc.timeout_seconds.fetch"
                    | "adbc.flight.sql.rpc.timeout_seconds.query"
                    | "adbc.flight.sql.rpc.timeout_seconds.update"
            ) {
                return Err(AuthError::config(
                    "Unsupported Flight client option; Redis options belong on the server",
                ));
            }
            let value = scalar_string(value)
                .ok_or_else(|| AuthError::config("Flight timeout must be a nonnegative number"))?;
            if !value
                .parse::<f64>()
                .is_ok_and(|n| n.is_finite() && n >= 0.0)
            {
                return Err(AuthError::config(
                    "Flight timeout must be a nonnegative number",
                ));
            }
            options.push((OptionDatabase::Other(key.into()), value));
        }
    }
    Ok(ConnectionConfig {
        backend: FLIGHT_BACKEND,
        uri,
        options,
    })
}

pub struct RedisAuth {
    backend: Backend,
}

impl RedisAuth {
    pub fn new(backend: Backend) -> Self {
        Self { backend }
    }
}

impl Auth for RedisAuth {
    fn backend(&self) -> Backend {
        self.backend
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        parse_config(config)?.apply()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth_for_backend;
    use crate::test_options::{other_option_value, uri_value};
    use dbt_yaml::{Mapping, Value};

    const BACKEND: Backend = Backend::Generic {
        library_name: "adbc_driver_redis",
        entrypoint: Some(b"AdbcDriverInit"),
    };

    fn configure(
        fields: impl IntoIterator<Item = (&'static str, Value)>,
    ) -> Result<database::Builder, AuthError> {
        auth_for_backend(BACKEND).configure(&AdapterConfig::new(Mapping::from_iter(
            fields.into_iter().map(|(key, value)| (key.into(), value)),
        )))
    }

    #[test]
    fn v1_options_preserve_scalar_shapes_and_block_credential_overrides() {
        let mapping = dbt_yaml::from_str::<Value>("aggregate_pushdown: exact\nrename_rekey: true\nread_timeout: 30\nwrite_timeout: 10m\ndriver_options:\n  adbc.redis.some_option: false\n").unwrap();
        let config = AdapterConfig::new(mapping.as_mapping().unwrap().clone());
        let builder = parse_config(&config).unwrap().apply().unwrap();
        for (key, expected) in [
            ("aggregate_pushdown", "exact"),
            ("rename_rekey", "true"),
            ("read_timeout", "30"),
            ("write_timeout", "10m"),
            ("some_option", "false"),
        ] {
            assert_eq!(
                other_option_value(&builder, &format!("adbc.redis.{key}")),
                Some(expected)
            );
        }
        for option in [
            "uri",
            "username",
            "password",
            "adbc.redis.default_schema",
            "adbc.redis.aggregate_pushdown",
            "adbc.redis.rename_rekey",
            "adbc.redis.time_zone",
            "adbc.redis.read_timeout",
            "adbc.redis.write_timeout",
        ] {
            let value: Value =
                dbt_yaml::from_str(&format!("driver_options:\n  {option}: override\n")).unwrap();
            assert!(
                parse_config(&AdapterConfig::new(value.as_mapping().unwrap().clone())).is_err()
            );
        }
        for invalid in ["[]", "{bad: [1]}", "{bad: null}"] {
            let value: Value = dbt_yaml::from_str(&format!("driver_options: {invalid}\n")).unwrap();
            assert!(
                parse_config(&AdapterConfig::new(value.as_mapping().unwrap().clone())).is_err()
            );
        }
    }

    #[test]
    fn defaults_and_borrowed_options() {
        let builder = configure([]).unwrap();
        assert_eq!(builder.backend, BACKEND);
        assert_eq!(uri_value(&builder), DEFAULT_URI);
        assert_eq!(
            other_option_value(&builder, "adbc.redis.default_schema"),
            Some("public")
        );
        assert!(builder.username.is_none());
        assert!(builder.password.is_none());
        assert!(other_option_value(&builder, "adbc.redis.time_zone").is_none());

        let config = AdapterConfig::new(Mapping::from_iter([
            ("username".into(), "alice".into()),
            ("password".into(), "secret".into()),
            ("schema".into(), "analytics".into()),
        ]));
        let parsed = parse_config(&config).unwrap();
        assert!(
            parsed
                .options
                .iter()
                .all(|(_, value)| matches!(value, Cow::Borrowed(_)))
        );
    }

    #[test]
    fn explicit_credentials_override_uri_without_being_logged() {
        let builder = configure([
            (
                "uri",
                "rediss://uri-user:uri-secret@localhost:6380/2".into(),
            ),
            ("username", "profile-user".into()),
            ("password", "profile-secret".into()),
            ("schema", "analytics".into()),
            ("time_zone", "America/New_York".into()),
        ])
        .unwrap();
        assert_eq!(builder.username.as_deref(), Some("profile-user"));
        assert_eq!(builder.password.as_deref(), Some("profile-secret"));
        assert_eq!(
            other_option_value(&builder, "adbc.redis.default_schema"),
            Some("analytics")
        );
        assert_eq!(
            other_option_value(&builder, "adbc.redis.time_zone"),
            Some("America/New_York")
        );
        let debug = format!("{builder:?}");
        assert!(!debug.contains("profile-secret"));
        assert!(!debug.contains("uri-secret"));
    }

    #[test]
    fn empty_profile_credentials_preserve_uri_credentials() {
        let builder = configure([
            ("uri", "redis://alice:secret@localhost:6379/0".into()),
            ("username", "".into()),
            ("password", "".into()),
        ])
        .unwrap();
        assert_eq!(builder.username.as_deref(), Some("alice"));
        assert_eq!(builder.password.as_deref(), Some("secret"));
    }

    #[test]
    fn scalar_password_is_coerced_at_the_builder_boundary() {
        let builder = configure([("password", Value::number(1234.into()))]).unwrap();
        assert_eq!(builder.password.as_deref(), Some("1234"));
    }

    #[test]
    fn null_credentials_preserve_uri_credentials() {
        let builder = configure([
            ("uri", "redis://alice:secret@localhost:6379/0".into()),
            ("username", Value::null()),
            ("password", Value::null()),
            ("time_zone", Value::null()),
        ])
        .unwrap();
        assert_eq!(builder.username.as_deref(), Some("alice"));
        assert_eq!(builder.password.as_deref(), Some("secret"));
        assert!(other_option_value(&builder, "adbc.redis.time_zone").is_none());
    }

    #[test]
    fn invalid_configuration_does_not_echo_credentials() {
        for fields in [
            vec![("uri", "https://alice:secret@localhost/".into())],
            vec![("uri", "redis:///".into())],
            vec![("uri", Value::bool(false))],
            vec![("database", "other".into())],
            vec![("schema", "".into())],
            vec![("username", Value::bool(true))],
            vec![("driver_options", Value::bool(true))],
            vec![("password", Value::Sequence(vec![], Default::default()))],
        ] {
            let error = configure(fields).unwrap_err();
            assert!(!error.msg().contains("secret"));
        }
    }

    #[test]
    fn flight_maps_only_client_options_and_keeps_secrets_out_of_debug() {
        let builder = configure([
            ("transport", "flightsql".into()),
            ("uri", "grpc+tls://localhost:8815".into()),
            ("username", "flight-user".into()),
            ("password", "flight-secret".into()),
            ("schema", "analytics".into()),
            ("time_zone", "America/New_York".into()),
        ])
        .unwrap();
        assert_eq!(builder.backend, FLIGHT_BACKEND);
        assert_eq!(other_option_value(&builder, FLIGHT_COOKIES), Some("true"));
        assert_eq!(builder.password.as_deref(), Some("flight-secret"));
        assert!(!format!("{builder:?}").contains("flight-secret"));
        assert!(
            builder
                .other
                .iter()
                .all(|(key, _)| !key.as_ref().starts_with("adbc.redis."))
        );
    }

    #[test]
    fn flight_rejects_insecure_uris_and_server_or_secret_options() {
        for yaml in [
            "uri: grpc://localhost:8815",
            "uri: redis://localhost:6379",
            "uri: grpc+tls://alice:secret@localhost:8815",
            "uri: grpc+tls://localhost:8815?secret=value",
            "uri: grpc+tls://localhost:8815/path",
            "aggregate_pushdown: exact",
            "rename_rekey: false",
            "read_timeout: 30",
            "driver_options: {adbc.redis.stream_results: true}",
            "driver_options: {adbc.flight.sql.rpc.with_cookie_middleware: false}",
            "driver_options: {adbc.flight.sql.authorization_header: secret}",
            "driver_options: {adbc.flight.sql.rpc.timeout_seconds.query: -1}",
            "driver_options: {adbc.flight.sql.rpc.timeout_seconds.query: .nan}",
        ] {
            let mut fields: Mapping =
                dbt_yaml::from_str("transport: flightsql\nuri: grpc+tls://localhost:8815\n")
                    .unwrap();
            let overrides: Mapping = dbt_yaml::from_str(yaml).unwrap();
            fields.extend(overrides);
            let err = parse_config(&AdapterConfig::new(fields))
                .err()
                .expect("invalid Flight profile");
            assert!(!err.msg().contains("secret"));
        }
        let fields: Mapping = dbt_yaml::from_str("transport: flightsql\nuri: grpc+tls://localhost:8815\ndriver_options: {adbc.flight.sql.rpc.timeout_seconds.query: 12.5}").unwrap();
        let builder = parse_config(&AdapterConfig::new(fields))
            .unwrap()
            .apply()
            .unwrap();
        assert_eq!(
            other_option_value(&builder, "adbc.flight.sql.rpc.timeout_seconds.query"),
            Some("12.5")
        );
    }

    #[test]
    #[ignore = "requires REDIS_ADBC_DRIVER and a test Redis at REDIS_URI"]
    fn live_driver_query() -> adbc_core::error::Result<()> {
        use arrow_array::{Int64Array, cast::AsArray, types::Int64Type};
        use dbt_adbc::{connection, driver};
        use std::env;

        let path = env::var("REDIS_ADBC_DRIVER").expect("absolute REDIS_ADBC_DRIVER path");
        assert!(std::path::Path::new(&path).is_absolute());
        let mut driver = driver::Builder::new(BACKEND, driver::LoadStrategy::System(Some(path)))
            .with_adbc_version(adbc_core::options::AdbcVersion::V110)
            .try_load()?;
        let builder = configure([
            ("uri", env::var("REDIS_URI").expect("test REDIS_URI").into()),
            (
                "username",
                env::var("REDIS_USERNAME").unwrap_or_default().into(),
            ),
            (
                "password",
                env::var("REDIS_PASSWORD").unwrap_or_default().into(),
            ),
        ])
        .expect("valid Redis connection configuration");
        let mut database = builder.build(&mut driver)?;
        let mut connection = connection::Builder::default().build(&mut database)?;
        let mut statement = connection.new_statement()?;
        statement.set_sql_query("select 1 as redis_adbc_probe")?;
        let batches = statement.execute()?.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
        let values: &Int64Array = batches[0].column(0).as_primitive::<Int64Type>();
        assert_eq!(values.value(0), 1);
        Ok(())
    }
}
