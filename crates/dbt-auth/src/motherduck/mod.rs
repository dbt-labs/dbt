use dbt_adbc::{Backend, database};
use url::Url;

use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};

/// Seconds. "Kind of high" because a MotherDuck instance that is not running
/// has to cold start. Once it is on, connecting is fast. 240 covers the p99
/// cold start of a giga-size instance.
const DEFAULT_CONNECT_TIMEOUT: u64 = 240;

/// The PostgreSQL default port, since MotherDuck is reached over its Postgres
/// endpoint. The driver defaults to the same value.
/// https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L373
const DEFAULT_PORT: u16 = 5432;

#[derive(Debug)]
enum MotherDuckCredential<'a> {
    /// `token:` in the profile.
    Profile(&'a str),
    /// No `token:` in the profile, so `MOTHERDUCK_TOKEN` from the environment.
    Env(String),
}

#[derive(Debug)]
enum MotherDuckLocation<'a> {
    /// `host:` in the profile, used as given.
    Host(&'a str),
    /// No `host:`, so derived from the region.
    Derived(String),
}

#[derive(Debug)]
struct MotherDuckAuthIR<'a> {
    /// Sent as the ADBC password option, never as a URI query value.
    /// https://github.com/adbc-drivers/motherduck/pull/36
    credential: MotherDuckCredential<'a>,
    location: MotherDuckLocation<'a>,
    port: u16,
}

impl MotherDuckAuthIR<'_> {
    fn apply(
        self,
        mut builder: database::Builder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<database::Builder, AuthError> {
        let Self {
            credential,
            location,
            port,
        } = self;
        let host = match &location {
            MotherDuckLocation::Host(host) => *host,
            MotherDuckLocation::Derived(host) => host.as_str(),
        };

        // The driver accepts only motherduck://, md:// and md: URIs.
        // https://github.com/adbc-drivers/motherduck/blob/8a35a7277de72f2a930f6b68ea4b300ee32e78a6/src/motherduck/motherduck_database.cc#L54
        let mut uri = Url::parse("motherduck://localhost").map_err(|e| {
            AuthError::config(format!(
                "could not build the MotherDuck connection URI: {e}. This does not depend on your profile, so please report it as a bug."
            ))
        })?;
        uri.set_host(Some(host)).map_err(|e| {
            AuthError::config(format!("host '{host}' is not a valid hostname: {e}"))
        })?;
        uri.set_port(Some(port)).map_err(|_| {
            AuthError::config("could not set the port on the MotherDuck connection URI")
        })?;
        builder.with_uri(uri);

        match &credential {
            MotherDuckCredential::Profile(token) => builder.with_password(*token),
            MotherDuckCredential::Env(token) => builder.with_password(token.as_str()),
        };
        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<MotherDuckAuthIR<'a>, AuthError> {
    if config.contains_key("user") || config.contains_key("username") {
        return Err(AuthError::config(
            "MotherDuck profiles do not accept `user` or `username`",
        ));
    }

    let method = config
        .get_str_non_empty("method")
        .ok_or_else(|| AuthError::config("Missing required 'method' field in MotherDuck config"))?;

    let region = config.get_str_non_empty("region").ok_or_else(|| {
        AuthError::config("region is required: MotherDuck tokens are keyed by region")
    })?;
    if region.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(AuthError::config(format!(
            "region must be lowercase, got '{region}'"
        )));
    }

    // Mirrors the driver's region validation: dash-separated parts of lowercase
    // letters and digits, short enough for a DNS label with the cloud suffix.
    // https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L103-L122
    if region.len() > 63 - "-azure".len()
        || !region.split('-').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
    {
        return Err(AuthError::config(format!(
            "region '{region}' is not valid: expected a region such as us-east-1 or eastus2, of lowercase letters, digits and dashes"
        )));
    }

    // An explicit `host` is used as given. Without one, derive the hostname the
    // way the driver does for its region option: a region with a cloud suffix is
    // used as is, otherwise names with dashes are AWS and names without are Azure.
    // https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L136-L147
    // The driver accepts a region with both cloud suffixes, in either order.
    // That names two clouds at once, so reject it here.
    if region.split('-').any(|part| part == "aws") && region.split('-').any(|part| part == "azure")
    {
        return Err(AuthError::config(format!(
            "region '{region}' names both aws and azure. Use one cloud suffix."
        )));
    }

    let cloud = if region.ends_with("-aws") || region.ends_with("-azure") {
        ""
    } else if region.contains('-') {
        "-aws"
    } else {
        "-azure"
    };
    let region_host = format!("pg.{region}{cloud}.motherduck.com");

    // An explicit `host` must be the region's host, as in the driver, which
    // compares them lowercase and without a trailing dot.
    // https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L351-L362
    let location = match config.get_str_non_empty("host") {
        Some(host)
            if !host
                .trim_end_matches('.')
                .eq_ignore_ascii_case(&region_host) =>
        {
            return Err(AuthError::config(format!(
                "host '{host}' does not match region '{region}', whose host is '{region_host}'. Remove `host` or fix the region."
            )));
        }
        Some(host) => MotherDuckLocation::Host(host),
        None => MotherDuckLocation::Derived(region_host),
    };

    // https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L373
    let port = match config.get_string("port") {
        Some(port) => port.parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| {
            AuthError::config(format!(
                "port must be a number between 1 and 65535, got '{port}'"
            ))
        })?,
        None => DEFAULT_PORT,
    };

    match method {
        "token" => {
            let credential = match (
                config.get_str_non_empty("token"),
                std::env::var("MOTHERDUCK_TOKEN")
                    .ok()
                    .filter(|t| !t.is_empty()),
            ) {
                (Some(token), _) => MotherDuckCredential::Profile(token),
                (None, Some(token)) => MotherDuckCredential::Env(token),
                (None, None) => {
                    return Err(AuthError::config(
                        "MotherDuck token not found: set `token` in the profile or the MOTHERDUCK_TOKEN environment variable",
                    ));
                }
            };
            Ok(MotherDuckAuthIR {
                credential,
                location,
                port,
            })
        }
        "externalbrowser" => Err(AuthError::config(
            "MotherDuck authentication method 'externalbrowser' is not implemented yet. Use 'token'.",
        )),
        other => Err(AuthError::config(format!(
            "MotherDuck authentication method '{other}' is not supported. Use 'token'."
        ))),
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: database::Builder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<database::Builder, AuthError> {
    if let Some(uri) = builder.uri.as_mut() {
        // Required, but a connection argument, not part of authentication.
        uri.set_path(config.require_str("database")?);
    }
    let connect_timeout = match config.get_string("connect_timeout") {
        Some(timeout) => timeout
            .parse::<u64>()
            .ok()
            .filter(|t| *t > 0)
            .ok_or_else(|| {
                AuthError::config(format!(
                    "connect_timeout must be a positive number of seconds, got '{timeout}'"
                ))
            })?,
        None => DEFAULT_CONNECT_TIMEOUT,
    };
    builder.with_named_option(
        dbt_adbc::motherduck::CONNECT_TIMEOUT,
        connect_timeout.to_string(),
    )?;
    // The driver verifies TLS (verify-full) against the OS trust store by default.
    // https://github.com/adbc-drivers/motherduck/commit/8a35a7277de72f2a930f6b68ea4b300ee32e78a6
    if let Some(path) = config.get_str_non_empty("sslrootcert") {
        builder.with_named_option(dbt_adbc::motherduck::TLS_ROOT_CERT_PATH, path)?;
    }
    // Invalid values are rejected by the driver (INVALID_ARGUMENT).
    // https://github.com/adbc-drivers/motherduck/blob/80f8d1a82e0cb9cceaee320e3e61370845094cb9/src/motherduck/motherduck_database.cc#L43
    builder.with_named_option(
        dbt_adbc::motherduck::ATTACH_MODE,
        config.get_str("attach_mode").unwrap_or("single"),
    )?;
    if let Some(name) = config.get_str_non_empty("session_name") {
        builder.with_named_option(dbt_adbc::motherduck::SESSION_NAME, name)?;
    }
    Ok(builder)
}

pub struct MotherDuckAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl MotherDuckAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for MotherDuckAuth {
    fn backend(&self) -> Backend {
        Backend::MotherDuck
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::{other_option_value, uri_value};
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    fn config() -> AdapterConfig {
        AdapterConfig::new(Mapping::from_iter([
            ("method".into(), "token".into()),
            ("region".into(), "us-east-1".into()),
            ("token".into(), "secret/token".into()),
            ("database".into(), "analytics".into()),
            ("schema".into(), "main".into()),
        ]))
    }

    fn configure(mapping: Mapping) -> Result<database::Builder, AuthError> {
        MotherDuckAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(mapping))
    }

    #[test]
    fn builds_secure_endpoint_uri_with_defaults() {
        let builder = configure(config().repr().clone()).expect("configure");
        let uri = uri_value(&builder);

        assert_eq!(builder.username, None);
        assert_eq!(builder.password.as_deref(), Some("secret/token"));
        assert_eq!(
            uri,
            "motherduck://pg.us-east-1-aws.motherduck.com:5432/analytics"
        );
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::CONNECT_TIMEOUT),
            Some("240")
        );
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::TLS_ROOT_CERT_PATH),
            None
        );
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::ATTACH_MODE),
            Some("single")
        );
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::SESSION_NAME),
            None
        );
    }

    #[test]
    fn session_name_is_sent_as_a_driver_option() {
        let mut mapping = config().repr().clone();
        mapping.insert("session_name".into(), "dbt writer".into());
        let builder = configure(mapping).expect("configure");
        let uri = uri_value(&builder);

        assert_contains!(&uri, "motherduck.com:5432/analytics");
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::SESSION_NAME),
            Some("dbt writer")
        );
    }

    #[test]
    fn host_matching_the_region_is_accepted_as_given() {
        let mut mapping = config().repr().clone();
        mapping.insert("host".into(), "PG.us-east-1-aws.motherduck.com.".into());
        let uri = uri_value(&configure(mapping).expect("configure"));
        assert_contains!(&uri, ":5432/analytics");
    }

    #[test]
    fn host_not_matching_the_region_is_rejected() {
        for host in ["pg.private.example.com", "pg.us-west-2-aws.motherduck.com"] {
            let mut mapping = config().repr().clone();
            mapping.insert("host".into(), host.into());
            let err = configure(mapping).expect_err("mismatched host");
            assert_contains!(err.msg(), "does not match region 'us-east-1'");
        }
    }

    #[test]
    fn host_is_derived_from_the_region_like_the_driver() {
        for (region, host) in [
            ("us-east-1", "pg.us-east-1-aws.motherduck.com"),
            ("eastus2", "pg.eastus2-azure.motherduck.com"),
            ("us-east-1-aws", "pg.us-east-1-aws.motherduck.com"),
            ("eastus2-azure", "pg.eastus2-azure.motherduck.com"),
        ] {
            let mut mapping = config().repr().clone();
            mapping.insert("region".into(), region.into());
            let uri = uri_value(&configure(mapping).expect("configure"));
            assert_contains!(&uri, &format!("motherduck://{host}:5432/"));
        }
    }

    #[test]
    fn region_with_both_cloud_suffixes_is_rejected() {
        for region in ["us-east-1-aws-azure", "us-east-1-azure-aws", "aws-azure"] {
            let mut mapping = config().repr().clone();
            mapping.insert("region".into(), region.into());
            let err = configure(mapping).expect_err("both clouds");
            assert_contains!(err.msg(), "names both aws and azure");
        }
    }

    #[test]
    fn invalid_region_is_rejected() {
        for region in [
            "us--east-1",
            "-us",
            "us-",
            "us east",
            "us,east",
            "us_east_1",
        ] {
            let mut mapping = config().repr().clone();
            mapping.insert("region".into(), region.into());
            let err = configure(mapping).expect_err("invalid region");
            assert_contains!(err.msg(), "is not valid");
        }
    }

    #[test]
    fn region_is_required_even_with_a_host() {
        let mut mapping = config().repr().clone();
        mapping.remove("region");
        mapping.insert("host".into(), "pg.us-east-1-aws.motherduck.com".into());
        let err = configure(mapping).expect_err("no region");
        assert_contains!(err.msg(), "region is required");
    }

    #[test]
    fn user_and_username_are_rejected() {
        for key in ["user", "username"] {
            let mut mapping = config().repr().clone();
            mapping.insert(key.into(), "analyst".into());
            let err = configure(mapping).expect_err("user");
            assert_contains!(err.msg(), "do not accept `user` or `username`");
        }
    }

    #[test]
    fn uppercase_region_is_rejected() {
        let mut mapping = config().repr().clone();
        mapping.insert("region".into(), "US-EAST-1".into());
        let err = configure(mapping).expect_err("uppercase region");
        assert_contains!(err.msg(), "region must be lowercase");
    }

    /// Reads and writes the process environment, so it stays out of CI. Run it
    /// by hand with `cargo xtask test --llm --no-external-deps -p dbt-auth
    /// motherduck -- --ignored`.
    #[test]
    #[ignore = "mutates MOTHERDUCK_TOKEN in the process environment"]
    #[allow(clippy::disallowed_methods)]
    fn token_falls_back_to_the_environment() {
        let mut mapping = config().repr().clone();
        mapping.remove("token");

        // SAFETY: ignored test, run by hand on its own.
        unsafe { std::env::remove_var("MOTHERDUCK_TOKEN") };
        let err = configure(mapping.clone()).expect_err("no token anywhere");
        assert_contains!(err.msg(), "MOTHERDUCK_TOKEN");

        unsafe { std::env::set_var("MOTHERDUCK_TOKEN", "env-token") };
        let builder = configure(mapping.clone()).expect("env token");
        assert_eq!(builder.password.as_deref(), Some("env-token"));

        // An empty value counts as unset, as it does in the driver.
        unsafe { std::env::set_var("MOTHERDUCK_TOKEN", "") };
        configure(mapping).expect_err("empty env token");
        unsafe { std::env::remove_var("MOTHERDUCK_TOKEN") };
    }

    #[test]
    fn region_is_required() {
        let mut mapping = config().repr().clone();
        mapping.remove("region");
        let err = configure(mapping).expect_err("no region");
        assert_contains!(err.msg(), "region is required");
    }

    #[test]
    fn externalbrowser_is_not_implemented() {
        let mut mapping = config().repr().clone();
        mapping.insert("method".into(), "externalbrowser".into());
        let err = configure(mapping).expect_err("externalbrowser");
        assert_contains!(err.msg(), "not implemented yet");
    }

    #[test]
    fn accepts_numeric_and_string_port_and_timeout_values() {
        for (port, timeout) in [
            (dbt_yaml::Value::from(6543), dbt_yaml::Value::from(30)),
            (dbt_yaml::Value::from("6543"), dbt_yaml::Value::from("30")),
        ] {
            let mut mapping = config().repr().clone();
            mapping.insert("port".into(), port);
            mapping.insert("connect_timeout".into(), timeout);
            let builder = configure(mapping).expect("configure");
            assert_contains!(&uri_value(&builder), ":6543/");
            assert_eq!(
                other_option_value(&builder, dbt_adbc::motherduck::CONNECT_TIMEOUT),
                Some("30")
            );
        }
    }

    #[test]
    fn rejects_invalid_port_and_timeout_values() {
        for (field, value, expected) in [
            (
                "port",
                dbt_yaml::Value::from(0),
                "port must be a number between 1 and 65535, got '0'",
            ),
            (
                "connect_timeout",
                dbt_yaml::Value::from("soon"),
                "connect_timeout must be a positive number of seconds, got 'soon'",
            ),
        ] {
            let mut mapping = config().repr().clone();
            mapping.insert(field.into(), value);
            let err = configure(mapping).expect_err("invalid value");
            assert_eq!(err.msg(), expected);
        }
    }

    #[test]
    fn unknown_method_is_rejected() {
        let mut mapping = config().repr().clone();
        mapping.insert("method".into(), "oauth".into());
        let err = configure(mapping).expect_err("unknown method");
        assert_contains!(err.msg(), "not supported");
    }

    #[test]
    fn sslrootcert_is_sent_as_tls_root_cert_path_only_when_set() {
        for (value, expected) in [
            (None, None),
            (Some(""), None),
            (Some("/certs/root.crt"), Some("/certs/root.crt")),
        ] {
            let mut mapping = config().repr().clone();
            if let Some(value) = value {
                mapping.insert("sslrootcert".into(), value.into());
            }
            let builder = configure(mapping).expect("configure");
            assert_eq!(
                other_option_value(&builder, dbt_adbc::motherduck::TLS_ROOT_CERT_PATH),
                expected
            );
            assert!(!uri_value(&builder).contains('?'));
        }
    }

    #[test]
    fn attach_mode_defaults_to_single_and_passes_through() {
        let builder = configure(config().repr().clone()).expect("default");
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::ATTACH_MODE),
            Some("single")
        );

        let mut mapping = config().repr().clone();
        mapping.insert("attach_mode".into(), "workspace".into());
        let builder = configure(mapping).expect("workspace");
        assert_eq!(
            other_option_value(&builder, dbt_adbc::motherduck::ATTACH_MODE),
            Some("workspace")
        );
    }
}
