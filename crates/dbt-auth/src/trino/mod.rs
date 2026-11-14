use crate::config::yml_value_to_string;
use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::{Backend, database, trino};
use dbt_yaml::Value as YmlValue;
use url::Url;

const SOURCE: &str = "dbt-trino-v2";
// dbt-trino v1 requires `port`; these defaults keep a profile without one working. The
// driver's own https default is 8443, so the port is always set on the URI.
const DEFAULT_HTTPS_PORT: u16 = 443;
const DEFAULT_HTTP_PORT: u16 = 8080;

/// Methods that exist in dbt-trino v1 but that this adapter does not support yet.
const UNSUPPORTED_METHODS: &[&str] = &["kerberos", "gssapi", "jwt"];

const SUPPORTED_METHODS: &str = "'none', 'ldap', 'oauth' and 'certificate'";

#[derive(Debug)]
enum HttpScheme {
    Http,
    Https,
}

/// The user of a session whose credential already names a principal.
#[derive(Debug)]
enum SessionUser<'a> {
    /// No `user`: the server takes the user from the token or the client certificate.
    Principal,
    /// `user`: the session runs as this user, if the principal may act as it.
    Named(&'a str),
}

/// `endpoint` is `trino://host:port`. dbt-trino v1 always uses https for every method but
/// `none`, so only `None` carries a scheme.
#[derive(Debug)]
enum TrinoAuthIR<'a> {
    /// No credentials: the server trusts `user`.
    None {
        endpoint: Url,
        scheme: HttpScheme,
        user: &'a str,
    },
    /// Username and password (`ldap` is the v1 name of the method).
    Ldap {
        endpoint: Url,
        user: &'a str,
        password: String,
    },
    /// The driver's external authentication: it gets a token through the server's OAuth 2.0
    /// flow, opening a browser for the login.
    OAuth {
        endpoint: Url,
        user: SessionUser<'a>,
    },
    /// Mutual TLS. The paths are PEM files the driver reads.
    Certificate {
        endpoint: Url,
        user: SessionUser<'a>,
        client_certificate: &'a str,
        client_private_key: &'a str,
    },
}

impl<'a> TrinoAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        // The driver picks https unless `SSL=false`, so the scheme is always explicit.
        match self {
            Self::None {
                mut endpoint,
                scheme,
                user,
            } => {
                let ssl = match scheme {
                    HttpScheme::Http => "false",
                    HttpScheme::Https => "true",
                };
                endpoint.query_pairs_mut().append_pair("SSL", ssl);
                builder.with_uri(endpoint);
                builder.with_username(user);
            }
            Self::Ldap {
                mut endpoint,
                user,
                password,
            } => {
                endpoint.query_pairs_mut().append_pair("SSL", "true");
                builder.with_uri(endpoint);
                builder.with_username(user);
                builder.with_password(password);
            }
            Self::OAuth { mut endpoint, user } => {
                // The driver's default login timeout applies; dbt-trino v1 has no setting for it.
                endpoint
                    .query_pairs_mut()
                    .append_pair("SSL", "true")
                    .append_pair("externalAuthentication", "true");
                builder.with_uri(endpoint);
                match user {
                    SessionUser::Principal => {}
                    SessionUser::Named(user) => {
                        builder.with_username(user);
                    }
                }
            }
            Self::Certificate {
                mut endpoint,
                user,
                client_certificate,
                client_private_key,
            } => {
                endpoint
                    .query_pairs_mut()
                    .append_pair("SSL", "true")
                    .append_pair("SSLClientCertPath", client_certificate)
                    .append_pair("SSLClientKeyPath", client_private_key);
                builder.with_uri(endpoint);
                match user {
                    SessionUser::Principal => {}
                    SessionUser::Named(user) => {
                        builder.with_username(user);
                    }
                }
            }
        }
        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<TrinoAuthIR<'a>, AuthError> {
    let method = config.get_str_non_empty("method").unwrap_or("none");

    if UNSUPPORTED_METHODS.contains(&method) {
        return Err(AuthError::config(format!(
            "Trino authentication method '{method}' is not yet supported; \
             supported methods are {SUPPORTED_METHODS}",
        )));
    }

    let scheme = match config.get_str_non_empty("http_scheme") {
        None => None,
        Some(s) if s.eq_ignore_ascii_case("https") => Some(HttpScheme::Https),
        Some(s) if s.eq_ignore_ascii_case("http") => Some(HttpScheme::Http),
        Some(other) => {
            return Err(AuthError::config(format!(
                "Trino 'http_scheme' must be 'http' or 'https', got '{other}'"
            )));
        }
    };
    let https_only = || match scheme {
        Some(HttpScheme::Http) => Err(AuthError::config(format!(
            "Trino method '{method}' requires 'http_scheme: https'"
        ))),
        _ => Ok(()),
    };
    let endpoint = |scheme: &HttpScheme| -> Result<Url, AuthError> {
        let host = config
            .get_str_non_empty("host")
            .ok_or_else(|| AuthError::config("Trino requires 'host' in profile configuration"))?;
        let port = match config.get_non_null("port") {
            None => match scheme {
                HttpScheme::Http => DEFAULT_HTTP_PORT,
                HttpScheme::Https => DEFAULT_HTTPS_PORT,
            },
            Some(port) => {
                let port = yml_value_to_string(port);
                port.parse().map_err(|_| {
                    AuthError::config(format!("Trino 'port' must be a port number, got '{port}'"))
                })?
            }
        };
        let invalid_host = |e: &dyn std::fmt::Display| {
            AuthError::config(format!("Invalid Trino 'host' '{host}': {e}"))
        };
        let mut endpoint = Url::parse("trino://").map_err(|e| invalid_host(&e))?;
        endpoint
            .set_host(Some(host))
            .map_err(|e| invalid_host(&e))?;
        endpoint
            .set_port(Some(port))
            .map_err(|()| invalid_host(&format!("cannot set port {port}")))?;
        Ok(endpoint)
    };
    let user = || {
        config
            .get_str_non_empty("user")
            .ok_or_else(|| AuthError::config("Trino requires 'user' in profile configuration"))
    };
    let session_user = || match config.get_str_non_empty("user") {
        Some(user) => SessionUser::Named(user),
        None => SessionUser::Principal,
    };

    // A `password` in the profile is only read by `ldap`: the other methods authenticate
    // with no credentials, a token or the client certificate.
    match method {
        "none" => {
            let scheme = scheme.unwrap_or(HttpScheme::Http);
            Ok(TrinoAuthIR::None {
                user: user()?,
                endpoint: endpoint(&scheme)?,
                scheme,
            })
        }
        "ldap" => {
            https_only()?;
            let password = config
                .get_non_null("password")
                .map(|password| yml_value_to_string(password).into_owned())
                .ok_or_else(|| {
                    AuthError::config("Trino method 'ldap' requires 'password' in profile")
                })?;
            Ok(TrinoAuthIR::Ldap {
                endpoint: endpoint(&HttpScheme::Https)?,
                user: user()?,
                password,
            })
        }
        "oauth" => {
            https_only()?;
            Ok(TrinoAuthIR::OAuth {
                endpoint: endpoint(&HttpScheme::Https)?,
                user: session_user(),
            })
        }
        // dbt-trino's `TrinoCertificateCredentials` requires both paths.
        "certificate" => {
            https_only()?;
            let path = |field: &str| {
                config.get_str_non_empty(field).ok_or_else(|| {
                    AuthError::config(format!(
                        "Trino method 'certificate' requires '{field}' in profile"
                    ))
                })
            };
            Ok(TrinoAuthIR::Certificate {
                endpoint: endpoint(&HttpScheme::Https)?,
                user: session_user(),
                client_certificate: path("client_certificate")?,
                client_private_key: path("client_private_key")?,
            })
        }
        "oauth_console" => Err(AuthError::config(
            "Trino method 'oauth_console' is not supported yet: the Trino ADBC driver \
             cannot print the login URL instead of opening a browser; use 'oauth'",
        )),
        other => Err(AuthError::config(format!(
            "Unknown Trino authentication method '{other}'; \
             supported methods are {SUPPORTED_METHODS}",
        ))),
    }
}

/// The profile's `client_tags`, trimmed and de-duplicated in first-seen order, exactly as they
/// go on the connection URI. A single string is one tag (not split on commas). Null, empty and
/// comma-containing tags are config errors.
///
/// The engine also calls this so per-statement client tags start from the same list.
pub fn normalized_client_tags(config: &AdapterConfig) -> Result<Vec<String>, AuthError> {
    let mut tags: Vec<String> = Vec::new();
    let mut push = |raw: &str| -> Result<(), AuthError> {
        let tag = raw.trim();
        if tag.is_empty() || tag.contains(',') {
            return Err(AuthError::config(format!(
                "Invalid client tag '{tag}' in profile client_tags: \
                 tags must be non-empty and must not contain ','"
            )));
        }
        if !tags.iter().any(|t| t == tag) {
            tags.push(tag.to_string());
        }
        Ok(())
    };
    match config.get_non_null("client_tags") {
        Some(YmlValue::Sequence(items, _)) => {
            for item in items {
                if item.is_null() {
                    return Err(AuthError::config(
                        "Invalid client tag null in profile client_tags: \
                         tags must be non-empty and must not contain ','",
                    ));
                }
                push(&yml_value_to_string(item))?;
            }
        }
        // A single string is one tag; it is not split on commas.
        Some(other) => push(&yml_value_to_string(other))?,
        None => {}
    }
    Ok(tags)
}

#[derive(Debug)]
enum Cert<'a> {
    /// Verify the server certificate with the system roots.
    Default,
    /// `cert: false`
    SkipVerification,
    /// `cert: <path>`: a CA bundle to verify the server certificate with.
    Path(&'a str),
}

/// The profile fields that do not depend on the auth method. All of them are optional.
#[derive(Debug)]
struct TrinoConnectionArgs<'a> {
    catalog: Option<&'a str>,
    /// Only set with `catalog`, as the URI path is `/catalog/schema`.
    schema: Option<&'a str>,
    client_tags: Vec<String>,
    /// `key:value` pairs.
    session_properties: Vec<String>,
    /// `catalog:role` pairs.
    roles: Vec<String>,
    timezone: Option<&'a str>,
    /// `retries`: how many times the driver sends one HTTP request. `None` keeps the driver
    /// default.
    request_retry_max_attempts: Option<i64>,
    /// `validate_connection: false` skips the credentials check when connecting.
    validate_connection: bool,
    cert: Cert<'a>,
    /// Header name and value.
    http_headers: Vec<(String, String)>,
    impersonation_user: Option<&'a str>,
}

impl<'a> TrinoConnectionArgs<'a> {
    fn parse(config: &'a AdapterConfig) -> Result<Self, AuthError> {
        let key_value_pairs = |field: &str| match config.get_non_null(field) {
            Some(YmlValue::Mapping(map, _)) => Ok(map
                .iter()
                .map(|(k, v)| format!("{}:{}", yml_value_to_string(k), yml_value_to_string(v)))
                .collect()),
            Some(_) => Err(AuthError::config(format!(
                "Trino '{field}' must be a mapping"
            ))),
            None => Ok(Vec::new()),
        };

        // dbt-trino v1 passes `retries` to its client as `max_attempts`: how many times one
        // HTTP request is sent on connection errors and 429/502/503/504 responses. A failed
        // query is never re-run. The driver's `request_retry_max_attempts` is the same setting
        // and rejects values below 1 (with 0, v1 fails every request).
        let request_retry_max_attempts = match config.get_non_null("retries") {
            None => None,
            Some(retries) => {
                // A templated value arrives as a string.
                let attempts = retries
                    .as_i64()
                    .or_else(|| retries.as_str().and_then(|s| s.parse().ok()));
                match attempts {
                    Some(n) if n > 0 => Some(n),
                    _ => {
                        return Err(AuthError::config(format!(
                            "Trino 'retries' must be a positive integer, got '{}'",
                            yml_value_to_string(retries)
                        )));
                    }
                }
            }
        };

        // A templated value arrives as a string.
        let validate_connection = match config.get_non_null("validate_connection") {
            None => true,
            Some(v) => match v.as_bool() {
                Some(b) => b,
                None => match v.as_str().map(str::to_ascii_lowercase).as_deref() {
                    Some("true") => true,
                    Some("false") => false,
                    _ => {
                        return Err(AuthError::config(
                            "Trino 'validate_connection' must be a boolean",
                        ));
                    }
                },
            },
        };

        let cert = match config.get_non_null("cert") {
            None => Cert::Default,
            // `cert: true` means "verify with the system roots", the default.
            Some(v) => match (v.as_bool(), v.as_str()) {
                (Some(true), _) | (_, Some("true" | "")) => Cert::Default,
                (Some(false), _) | (_, Some("false")) => Cert::SkipVerification,
                (_, Some(path)) => Cert::Path(path),
                (None, None) => {
                    return Err(AuthError::config(
                        "Trino 'cert' must be a boolean or a path to a certificate",
                    ));
                }
            },
        };

        let http_headers = match config.get_non_null("http_headers") {
            Some(YmlValue::Mapping(headers, _)) => headers
                .iter()
                .map(|(name, value)| {
                    (
                        yml_value_to_string(name).into_owned(),
                        yml_value_to_string(value).into_owned(),
                    )
                })
                .collect(),
            _ => Vec::new(),
        };

        // `catalog` is an alias of `database` in the profile.
        let catalog = config
            .get_str_non_empty("database")
            .or_else(|| config.get_str_non_empty("catalog"));

        // `connect_timeout` (injected by `dbt debug`) is accepted and ignored.
        Ok(Self {
            catalog,
            schema: catalog.and_then(|_| config.get_str_non_empty("schema")),
            client_tags: normalized_client_tags(config)?,
            session_properties: key_value_pairs("session_properties")?,
            roles: key_value_pairs("roles")?,
            timezone: config.get_str_non_empty("timezone"),
            request_retry_max_attempts,
            validate_connection,
            cert,
            http_headers,
            impersonation_user: config.get_str_non_empty("impersonation_user"),
        })
    }

    fn apply(self, mut builder: DatabaseBuilder) -> Result<DatabaseBuilder, AuthError> {
        let Some(uri) = builder.uri.as_mut() else {
            return Err(AuthError::config(
                "Trino connection URI must be set before connection arguments",
            ));
        };
        uri.path_segments_mut()
            .map_err(|()| {
                AuthError::config("Trino connection URI cannot hold a catalog or schema")
            })?
            .extend(self.catalog)
            .extend(self.schema);
        {
            let mut query = uri.query_pairs_mut();
            if !self.validate_connection {
                query.append_pair("validateConnection", "false");
            }
            if !self.client_tags.is_empty() {
                query.append_pair("clientTags", &self.client_tags.join(","));
            }
            if !self.session_properties.is_empty() {
                query.append_pair("session_properties", &self.session_properties.join(";"));
            }
            if !self.roles.is_empty() {
                query.append_pair("roles", &self.roles.join(";"));
            }
            if let Some(timezone) = self.timezone {
                query.append_pair("timezone", timezone);
            }
            if let Some(attempts) = self.request_retry_max_attempts {
                query.append_pair("request_retry_max_attempts", &attempts.to_string());
            }
            query.append_pair("source", SOURCE);
            match self.cert {
                Cert::Default => {}
                Cert::SkipVerification => {
                    query.append_pair("SSLVerification", "NONE");
                }
                Cert::Path(path) => {
                    query.append_pair("SSLCertPath", path);
                }
            }
        }

        for (name, value) in self.http_headers {
            builder.with_named_option(format!("{}{name}", trino::HTTP_HEADER_PREFIX), value)?;
        }
        if let Some(user) = self.impersonation_user {
            builder.with_named_option(trino::IMPERSONATION_USER, user)?;
        }
        Ok(builder)
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    TrinoConnectionArgs::parse(config)?.apply(builder)
}

pub struct TrinoAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl TrinoAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for TrinoAuth {
    fn backend(&self) -> Backend {
        Backend::Trino
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::{other_option_value, uri_value};
    use dbt_test_primitives::assert_contains;
    use dbt_yaml::Mapping;

    fn configure(yaml: &str) -> Result<database::Builder, AuthError> {
        let config: Mapping = dbt_yaml::from_str(yaml).expect("parse yaml");
        TrinoAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
    }

    fn assert_error_contains(yaml: &str, needle: &str) {
        match configure(yaml) {
            Err(e) => assert_contains!(e.msg(), needle),
            Ok(_) => panic!("Expected error containing {needle:?} for {yaml:?}"),
        }
    }

    /// The connection URI, with its query parameters decoded.
    fn uri(builder: &database::Builder) -> (String, Vec<(String, String)>) {
        let uri = Url::parse(&uri_value(builder)).expect("valid URI");
        let params = uri.query_pairs().into_owned().collect();
        let mut base = uri;
        base.set_query(None);
        (base.to_string(), params)
    }

    fn param(builder: &database::Builder, name: &str) -> Option<String> {
        uri(builder)
            .1
            .into_iter()
            .find_map(|(k, v)| (k == name).then_some(v))
    }

    const LDAP: &str = "method: ldap\nhost: h\nuser: u\npassword: p\n";
    const CERTIFICATE: &str = "method: certificate\nhost: h\nclient_certificate: /c/client.pem\nclient_private_key: /c/client.key\n";

    #[test]
    fn none_defaults_to_http() {
        // `method` omitted means `none`.
        for yaml in [
            "method: none\nhost: h\nuser: u\npassword: p\n",
            "host: h\nuser: u\n",
        ] {
            let builder = configure(yaml).expect("configure");
            let (base, params) = uri(&builder);
            assert_eq!(base, "trino://h:8080");
            assert_eq!(
                params,
                [("SSL", "false"), ("source", "dbt-trino-v2")].map(|(k, v)| (k.into(), v.into()))
            );
            assert_eq!(builder.username.as_deref(), Some("u"));
            assert_eq!(builder.password, None);
        }

        let builder = configure("host: h\nuser: u\nhttp_scheme: HTTPS\n").expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:443");
        assert_eq!(param(&builder, "SSL").as_deref(), Some("true"));
    }

    #[test]
    fn ldap_user_and_password() {
        let builder = configure(&format!("{LDAP}port: '8443'\ncatalog: cat\nschema: sch\n"))
            .expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:8443/cat/sch");
        assert_eq!(param(&builder, "SSL").as_deref(), Some("true"));
        assert_eq!(builder.username.as_deref(), Some("u"));
        assert_eq!(builder.password.as_deref(), Some("p"));

        // A number is a valid password.
        let builder =
            configure("method: ldap\nhost: h\nuser: u\npassword: 1234\n").expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:443");
        assert_eq!(builder.password.as_deref(), Some("1234"));
    }

    #[test]
    fn oauth_external_authentication() {
        // The password is not sent: the driver rejects one with external authentication.
        let builder =
            configure("method: oauth\nhost: h\nuser: u\npassword: p\n").expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:443");
        assert_eq!(param(&builder, "SSL").as_deref(), Some("true"));
        assert_eq!(
            param(&builder, "externalAuthentication").as_deref(),
            Some("true")
        );
        assert_eq!(builder.username.as_deref(), Some("u"));
        assert_eq!(builder.password, None);

        // Without `user`, the server takes it from the token.
        let builder = configure("method: oauth\nhost: h\n").expect("configure");
        assert_eq!(builder.username, None);
    }

    #[test]
    fn certificate_mutual_tls() {
        let builder =
            configure(&format!("{CERTIFICATE}password: p\ncert: /c/ca.pem\n")).expect("configure");
        let (base, params) = uri(&builder);
        assert_eq!(base, "trino://h:443");
        for (name, value) in [
            ("SSL", "true"),
            ("SSLClientCertPath", "/c/client.pem"),
            ("SSLClientKeyPath", "/c/client.key"),
            // `cert` still sets how the server certificate is verified.
            ("SSLCertPath", "/c/ca.pem"),
        ] {
            assert!(
                params.contains(&(name.into(), value.into())),
                "{name} in {params:?}"
            );
        }
        // Without `user`, the server takes it from the client certificate.
        assert_eq!(builder.username, None);
        assert_eq!(builder.password, None);

        let builder = configure(&format!("{CERTIFICATE}user: u\n")).expect("configure");
        assert_eq!(builder.username.as_deref(), Some("u"));

        // Other methods ignore the certificate fields.
        let builder =
            configure(&format!("{LDAP}client_certificate: /c/client.pem\n")).expect("configure");
        assert_eq!(param(&builder, "SSLClientCertPath"), None);
    }

    #[test]
    fn required_fields_per_method() {
        for (yaml, needle) in [
            ("method: none\nhost: h\n", "'user'"),
            ("method: none\nhost: null\nuser: u\n", "'host'"),
            ("method: ldap\nhost: h\npassword: p\n", "'user'"),
            (
                "method: ldap\nhost: h\nuser: u\npassword: null\n",
                "'password'",
            ),
            ("method: oauth\nuser: u\n", "'host'"),
            (
                "method: certificate\nhost: h\nclient_private_key: /c/client.key\n",
                "requires 'client_certificate'",
            ),
            (
                "method: certificate\nhost: h\nclient_certificate: /c/client.pem\nclient_private_key: null\n",
                "requires 'client_private_key'",
            ),
        ] {
            assert_error_contains(yaml, needle);
        }
    }

    #[test]
    fn http_scheme_and_port() {
        // dbt-trino v1 always uses https for every method but `none`.
        for yaml in [LDAP, "method: oauth\nhost: h\n", CERTIFICATE] {
            assert_error_contains(
                &format!("{yaml}http_scheme: http\n"),
                "requires 'http_scheme: https'",
            );
        }
        assert_error_contains(
            "host: h\nuser: u\nhttp_scheme: ftp\n",
            "'http_scheme' must be 'http' or 'https', got 'ftp'",
        );

        let builder = configure("host: h\nuser: u\nport: null\n").expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:8080");
        let builder = configure("host: h\nuser: u\nport: 8081\n").expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:8081");
        assert_error_contains(
            "host: h\nuser: u\nport: abc\n",
            "'port' must be a port number",
        );
        assert_error_contains("host: 'h/x'\nuser: u\n", "Invalid Trino 'host'");
    }

    #[test]
    fn unsupported_and_unknown_methods() {
        assert_error_contains(
            "method: kerberos\nhost: h\nuser: u\n",
            "'kerberos' is not yet supported",
        );
        assert_error_contains(
            "method: oauth_console\nhost: h\n",
            "cannot print the login URL",
        );
        assert_error_contains(
            "method: magic\nhost: h\nuser: u\n",
            "supported methods are 'none', 'ldap', 'oauth' and 'certificate'",
        );
    }

    #[test]
    fn client_tags() {
        let yaml = format!("{LDAP}client_tags: [' b ', a, b, 1]\n");
        let config: Mapping = dbt_yaml::from_str(&yaml).expect("parse yaml");
        assert_eq!(
            normalized_client_tags(&AdapterConfig::new(config)).unwrap(),
            ["b", "a", "1"]
        );
        let builder = configure(&yaml).expect("configure");
        assert_eq!(param(&builder, "clientTags").as_deref(), Some("b,a,1"));

        assert_eq!(
            param(&configure(LDAP).expect("configure"), "clientTags"),
            None
        );

        for (tags, needle) in [
            ("[\"a,b\"]", "Invalid client tag 'a,b'"),
            // A single string is one tag; it is not split on commas.
            ("\"a,b\"", "must not contain ','"),
            ("[\"  \"]", "must be non-empty"),
            ("[a, null]", "Invalid client tag null"),
        ] {
            assert_error_contains(&format!("{LDAP}client_tags: {tags}\n"), needle);
        }
    }

    #[test]
    fn session_properties_roles_and_timezone() {
        let builder = configure(&format!(
            "{LDAP}session_properties:\n  query_max_run_time: 10m\n  x: 1\nroles:\n  hive: admin\n  system: analyst\ntimezone: UTC\n"
        ))
        .expect("configure");
        assert_eq!(
            param(&builder, "session_properties").as_deref(),
            Some("query_max_run_time:10m;x:1")
        );
        assert_eq!(
            param(&builder, "roles").as_deref(),
            Some("hive:admin;system:analyst")
        );
        assert_eq!(param(&builder, "timezone").as_deref(), Some("UTC"));

        assert_error_contains(
            &format!("{LDAP}roles: admin\n"),
            "'roles' must be a mapping",
        );
    }

    #[test]
    fn catalog_and_schema() {
        let builder = configure(&format!("{LDAP}database: d\nschema: s\n")).expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:443/d/s");
        // The schema is part of the path only after a catalog.
        let builder = configure(&format!("{LDAP}schema: s\n")).expect("configure");
        assert_eq!(uri(&builder).0, "trino://h:443");
    }

    #[test]
    fn retries() {
        assert_eq!(
            param(
                &configure(LDAP).expect("configure"),
                "request_retry_max_attempts"
            ),
            None
        );
        // A templated value arrives as a string.
        for (value, expected) in [("3", "3"), ("1", "1"), ("\"5\"", "5")] {
            let builder = configure(&format!("{LDAP}retries: {value}\n")).expect("configure");
            assert_eq!(
                param(&builder, "request_retry_max_attempts").as_deref(),
                Some(expected)
            );
        }
        for value in ["0", "-1", "\"0\"", "two", "1.5", "true"] {
            assert_error_contains(
                &format!("{LDAP}retries: {value}\n"),
                "'retries' must be a positive integer",
            );
        }
    }

    #[test]
    fn validate_connection() {
        // Unset and true keep the driver default; a templated value arrives as a string.
        for (value, expected) in [
            ("null", None),
            ("true", None),
            ("false", Some("false")),
            ("\"False\"", Some("false")),
        ] {
            let builder =
                configure(&format!("{LDAP}validate_connection: {value}\n")).expect("configure");
            assert_eq!(
                param(&builder, "validateConnection").as_deref(),
                expected,
                "{value}"
            );
        }
        assert_error_contains(
            &format!("{LDAP}validate_connection: maybe\n"),
            "'validate_connection' must be a boolean",
        );
    }

    #[test]
    fn cert() {
        for (value, verification, path) in [
            ("true", None, None),
            ("false", Some("NONE"), None),
            ("\"false\"", Some("NONE"), None),
            ("/etc/ca.pem", None, Some("/etc/ca.pem")),
        ] {
            let builder = configure(&format!("{LDAP}cert: {value}\n")).expect("configure");
            assert_eq!(
                param(&builder, "SSLVerification").as_deref(),
                verification,
                "{value}"
            );
            assert_eq!(param(&builder, "SSLCertPath").as_deref(), path, "{value}");
        }
        assert_error_contains(
            &format!("{LDAP}cert: [a]\n"),
            "'cert' must be a boolean or a path",
        );
    }

    #[test]
    fn http_headers_and_impersonation_user() {
        let builder = configure(&format!(
            "{LDAP}http_headers:\n  X-Foo: bar\nimpersonation_user: imp\n"
        ))
        .expect("configure");
        assert_eq!(
            other_option_value(&builder, "trino.http_header.X-Foo"),
            Some("bar")
        );
        assert_eq!(
            other_option_value(&builder, trino::IMPERSONATION_USER),
            Some("imp")
        );
    }
}
