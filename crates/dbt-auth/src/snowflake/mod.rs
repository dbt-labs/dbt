pub mod key_format;

use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter};
use database::Builder as DatabaseBuilder;
use dbt_adbc::database::LogLevel;
use dbt_adbc::{Backend, database, snowflake};

use std::fs;
use std::sync::Once;

const APP_NAME: &str = "dbt";

// WARNING: Still needs adjustment on what is considered must-have
const CONNECTION_PARAMS_STR: [&str; 9] = [
    "account",
    "role",
    "warehouse",
    "database",
    "schema",
    "host",
    "protocol",
    snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY,
    snowflake::QUERY_TAG_PARAM_KEY,
];

const CONNECTION_PARAMS: [&str; 2] = ["port", "client_session_keep_alive"];

const ACCEPTED_WORKLOAD_IDENTITY_PROVIDERS: [&str; 4] = ["OIDC", "AZURE", "GCP", "AWS"];

/// Configuration values that are needed for an auth method in a dbt-snowflake profile.
///
/// dbt snowflake only formalized `method` later in it's lifetime. For profiles without
/// `method`, we must naively copy over all these fields. These are mutually exclusive fields.
/// Username and password are handled separately because they do not drive legacy auth selection.
const AUTH_PARAMS_USED_FOR_LEGACY_CONFIG: [&str; 6] = [
    "private_key_path",
    "private_key",
    "private_key_passphrase",
    "oauth_client_id",
    "oauth_client_secret",
    "authenticator",
];

/// Overall deadline budget that gosnowflake uses for the login retry loop
/// (`context.WithTimeout(parent, LoginTimeout)`). Within this window, gosnowflake
/// performs up to `MaxRetryCount` (default 7) HTTP retries with exponential
/// backoff (1s base, 16s cap, jittered) — see `snowflakedb/gosnowflake::retry.go`
/// and `internal/config/dsn.go`.
///
/// 60s approximates Python's per-connector budget of 60s HTTP connect timeout ×
/// ~2 `requests`-internal retries (see `snowflake-connector-python::auth/by_plugin.py`
/// `DEFAULT_AUTH_CLASS_TIMEOUT = 120`). The dbt-adapter retry layer
/// ([engine/retry.rs](../../dbt-adapter/src/engine/retry.rs)) wraps with
/// `connect_retries=7` outer attempts to match Python's full ~480s budget.
///
/// Historical note: this used to be `"1s"` to keep gosnowflake from retrying
/// past the context deadline. That hang pathology was fixed by the cherry-pick
/// in `dbt-labs/arrow-adbc` PR #131 (`dbt-labs/gosnowflake`@337c068), which
/// makes gosnowflake bail on `ctx.Err() != nil` immediately — so we can safely
/// give it a real budget now.
///
/// The `DBT_SNOWFLAKE_LOGIN_TIMEOUT` env var overrides this default (parallel to
/// the existing `DBT_SNOWFLAKE_CLIENT_TIMEOUT` escape hatch below). Used by the
/// LSP e2e test runner to restore fast-fail behavior against unreachable
/// `snowflake.local` hosts.
const DEFAULT_REQUEST_TIMEOUT: &str = "600s";
const ADBC_STUB_PASSWORD: &str = "fs_pass";

/// dbt Core expects durations in seconds only so this utility appends that s
/// https://pkg.go.dev/time#ParseDuration for permitted units
fn postfix_seconds_unit(value: &str) -> String {
    format!("{value}s")
}

fn validate_warehouse_auth_fields(config: &AdapterConfig) -> Result<(), AuthError> {
    if config.get_str("user").is_none() {
        return Err(AuthError::config(
            "Snowflake warehouse authentication requires 'user'.",
        ));
    }
    if config.get_str("password").is_none() {
        return Err(AuthError::config(
            "Snowflake warehouse authentication requires 'password'.",
        ));
    }
    Ok(())
}

fn parse_workload_identity<'a>(
    config: &'a AdapterConfig,
) -> Result<SnowflakeAuthIR<'a>, AuthError> {
    let provider = config.get_str("workload_identity_provider");
    let provider_valid = provider.is_some_and(|p| {
        ACCEPTED_WORKLOAD_IDENTITY_PROVIDERS
            .iter()
            .any(|accepted| accepted.eq_ignore_ascii_case(p))
    });
    if !provider_valid {
        return Err(AuthError::config(format!(
            "workload_identity_provider must be set to one of the following values if authenticator='workload_identity'!:\n{}\n\nProvided workload_identity_provider was '{}'",
            ACCEPTED_WORKLOAD_IDENTITY_PROVIDERS.join(", "),
            provider.unwrap_or_default(),
        )));
    }
    let provider = provider.expect("validated above");

    let entra_resource = config.get_str("workload_identity_entra_resource");
    if entra_resource.is_some() && !provider.eq_ignore_ascii_case("AZURE") {
        return Err(AuthError::config(
            "workload_identity_entra_resource can only be set if workload_identity_provider is Azure",
        ));
    }

    Ok(SnowflakeAuthIR::WorkloadIdentity {
        provider,
        entra_resource,
        token: config.get_str("token"),
    })
}

static WARN_3DES_KEY_ONCE: Once = Once::new();

fn warn_ignored_auth_field(
    warning_printer: &dyn AuthWarningPrinter,
    auth_method: &str,
    field: &str,
) {
    warning_printer.warn(&format!(
        "For Snowflake {auth_method} authentication, '{field}' will be ignored and can be safely removed from your profile."
    ));
}

#[derive(Debug)]
enum SnowflakeAuthIR<'a> {
    Warehouse {
        user: &'a str,
        password: &'a str,
    },
    WarehouseMFA {
        user: &'a str,
        password: &'a str,
    },
    KeypairPath {
        user: &'a str,
        path: &'a str,
        passphrase: Option<&'a str>,
    },
    KeypairInline {
        user: &'a str,
        private_key: &'a str,
        passphrase: Option<&'a str>,
    },
    NativeOauth {
        client_id: &'a str,
        client_secret: &'a str,
        refresh_token: &'a str,
    },
    // Despite the name, this is an OAuth access token passthrough — not a self-signed
    // keypair JWT. The profile field is called `jwt_token` (method: snowflake_oauth_jwt)
    // or `token` (authenticator: jwt), but the value must be a valid OAuth access token
    // obtained from Snowflake's managed OAuth or an external IDP token exchange.
    NativeOauthJWT {
        user: Option<&'a str>,
        password: Option<&'a str>,
        access_token: &'a str,
    },
    Sso {
        user: &'a str,
    },
    Pat {
        user: &'a str,
        token: &'a str,
    },
    // AWS/GCP/AZURE fetch their own attestation from the local cloud metadata service.
    WorkloadIdentity {
        provider: &'a str,
        entra_resource: Option<&'a str>,
        token: Option<&'a str>,
    },
}

impl<'a> SnowflakeAuthIR<'a> {
    pub fn apply(
        self,
        mut builder: DatabaseBuilder,
        warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::NativeOauth {
                client_id,
                client_secret,
                refresh_token,
            } => {
                builder.with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH)?;
                builder.with_named_option(snowflake::CLIENT_ID, client_id)?;
                builder.with_named_option(snowflake::CLIENT_SECRET, client_secret)?;
                builder.with_named_option(snowflake::REFRESH_TOKEN, refresh_token)?;
                builder.with_named_option(snowflake::CLIENT_STORE_TEMP_CREDS, "true")?;
            }
            Self::NativeOauthJWT {
                user,
                password,
                access_token,
            } => {
                if let Some(user) = user {
                    builder.with_username(user);
                }
                if let Some(password) = password {
                    builder.with_password(password);
                }
                builder.with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH)?;
                builder.with_named_option(snowflake::AUTH_TOKEN, access_token)?;
                builder.with_named_option(snowflake::CLIENT_STORE_TEMP_CREDS, "true")?;
            }
            Self::Sso { user } => {
                builder.with_username(user);
                builder.with_password(ADBC_STUB_PASSWORD);
                builder.with_named_option(
                    snowflake::AUTH_TYPE,
                    snowflake::auth_type::EXTERNAL_BROWSER,
                )?;
                builder.with_named_option(snowflake::CLIENT_STORE_TEMP_CREDS, "true")?;
            }
            Self::KeypairPath {
                user,
                path,
                passphrase,
            } => {
                builder.with_username(user);
                builder.with_password(ADBC_STUB_PASSWORD);
                builder.with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::JWT)?;
                fs::metadata(path).map_err(|_| {
                    AuthError::config(format!("Private key file not found: '{path}'"))
                })?;
                if let Some(pass) = passphrase {
                    let key_content = fs::read_to_string(path).map_err(|_| {
                        AuthError::config(format!("Could not read from key file: '{path}'"))
                    })?;
                    let (normalized, status) = key_format::normalize_key(&key_content)?;
                    if let key_format::SnowflakeKeypairStatus::Warn3Des(msg) = status {
                        WARN_3DES_KEY_ONCE.call_once(|| warning_printer.warn(msg));
                    }
                    builder
                        .with_named_option(snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE, normalized)?;
                    builder.with_named_option(snowflake::JWT_PRIVATE_KEY_PKCS8_PASSWORD, pass)?;
                } else {
                    builder.with_named_option(snowflake::JWT_PRIVATE_KEY, path)?;
                }
            }
            Self::KeypairInline {
                user,
                private_key,
                passphrase,
            } => {
                builder.with_username(user);
                builder.with_password(ADBC_STUB_PASSWORD);
                builder.with_named_option(snowflake::AUTH_TYPE, snowflake::auth_type::JWT)?;
                let (normalized, status) = key_format::normalize_key(private_key)?;
                if let key_format::SnowflakeKeypairStatus::Warn3Des(msg) = status {
                    WARN_3DES_KEY_ONCE.call_once(|| warning_printer.warn(msg));
                }
                builder.with_named_option(snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE, normalized)?;
                if let Some(pass) = passphrase {
                    builder.with_named_option(snowflake::JWT_PRIVATE_KEY_PKCS8_PASSWORD, pass)?;
                }
            }
            Self::WarehouseMFA { user, password } => {
                builder.with_username(user);
                builder.with_password(password);
                builder.with_named_option(
                    snowflake::AUTH_TYPE,
                    snowflake::auth_type::USERNAME_PASSWORD_MFA,
                )?;
                builder.with_named_option(snowflake::CLIENT_CACHE_MFA_TOKEN, "true")?;
            }
            Self::Warehouse { user, password } => {
                builder.with_username(user);
                builder.with_password(password);
            }
            Self::Pat { user, token } => {
                // PAT auth uses User + Token only when building the auth request body
                // https://github.com/snowflakedb/gosnowflake/blob/v1.17.1/auth.go#L53
                builder.with_username(user);
                builder.with_named_option(
                    snowflake::AUTH_TYPE,
                    snowflake::auth_type::PROGRAMMATIC_ACCESS_TOKEN,
                )?;
                builder.with_named_option(snowflake::AUTH_TOKEN, token)?;
            }
            Self::WorkloadIdentity {
                provider,
                entra_resource,
                token,
            } => {
                builder.with_named_option(
                    snowflake::AUTH_TYPE,
                    snowflake::auth_type::WORKLOAD_IDENTITY,
                )?;
                builder.with_named_option(snowflake::WORKLOAD_IDENTITY_PROVIDER, provider)?;
                if let Some(entra_resource) = entra_resource {
                    builder.with_named_option(
                        snowflake::WORKLOAD_IDENTITY_ENTRA_RESOURCE,
                        entra_resource,
                    )?;
                }
                if let Some(token) = token {
                    builder.with_named_option(snowflake::AUTH_TOKEN, token)?;
                }
            }
        }

        Ok(builder)
    }
}

#[allow(clippy::cognitive_complexity)]
fn parse_auth<'a>(
    config: &'a AdapterConfig,
    warning_printer: &dyn AuthWarningPrinter,
) -> Result<SnowflakeAuthIR<'a>, AuthError> {
    // Case 1: Profile has `method`. We can do strict evaluation of their profiles.yml
    if let Some(method) = config.get_str("method") {
        if config.get_str("authenticator").is_some() {
            return Err(AuthError::config(
                "Using 'method' in your Snowflake profile subsumes 'authenticator' field. Please remove authenticator.",
            ));
        }

        match method {
            "keypair" => {
                if config.get_str("user").is_none() {
                    return Err(AuthError::config(
                        "Snowflake keypair authentication requires 'user'.",
                    ));
                }
                if config.contains_key("password") {
                    warn_ignored_auth_field(warning_printer, "keypair", "password");
                }

                let pk_path = config.get_str("private_key_path");
                let pk_raw = config.get_str("private_key");
                let pk_pass = config.get_str("private_key_passphrase");

                let user = config.get_str("user").expect("validated above");
                match (pk_path, pk_raw) {
                    (Some(_), Some(_)) => Err(AuthError::config(
                        "Cannot specify both 'private_key' and 'private_key_path'",
                    )),
                    (Some(path), None) => Ok(SnowflakeAuthIR::KeypairPath {
                        user,
                        path,
                        passphrase: pk_pass,
                    }),
                    (None, Some(raw)) => Ok(SnowflakeAuthIR::KeypairInline {
                        user,
                        private_key: raw,
                        passphrase: pk_pass,
                    }),
                    (None, None) => Err(AuthError::config(
                        "Keypair authentication requires exactly one of 'private_key' or 'private_key_path'",
                    )),
                }
            }
            "sso" => {
                if config.contains_key("password") {
                    warn_ignored_auth_field(warning_printer, "SSO", "password");
                }

                config
                    .get_str("user")
                    .map(|user| SnowflakeAuthIR::Sso { user })
                    .ok_or_else(|| {
                        AuthError::config("Snowflake SSO authentication requires 'user'.")
                    })
            }
            "snowflake_oauth" => {
                if config.contains_key("user") {
                    warn_ignored_auth_field(warning_printer, "OAuth", "user");
                }
                if config.contains_key("password") {
                    warn_ignored_auth_field(warning_printer, "OAuth", "password");
                }

                // TODO(versusfacit): update upstream to allow for refresh_token
                // if config.contains_key("token") {
                //     return Err(AuthError::config(
                //         "Rename 'token' to 'refresh_token' in profile for 'method: snowflake_oauth'.",
                //     ));
                // };

                let cid = config.get_str("oauth_client_id");
                let sec = config.get_str("oauth_client_secret");
                let tok = config.get_str("token");

                match (cid, sec, tok) {
                    (Some(client_id), Some(client_secret), Some(refresh_token)) => {
                        Ok(SnowflakeAuthIR::NativeOauth {
                            client_id,
                            client_secret,
                            refresh_token,
                        })
                    }
                    _ => Err(AuthError::config(
                        "Profile requires 'oauth_client_id', 'oauth_client_secret', and 'token' for method: snowflake_oauth.",
                    )),
                }
            }
            "snowflake_oauth_jwt" => {
                if let Some(access_token) = config.get_str("jwt_token") {
                    Ok(SnowflakeAuthIR::NativeOauthJWT {
                        user: config.get_str("user"),
                        password: config.get_str("password"),
                        access_token,
                    })
                } else {
                    Err(AuthError::config(
                        "Profile requires 'jwt_token' for 'method: snowflake_oauth_jwt'.",
                    ))
                }
            }
            "warehouse" => {
                validate_warehouse_auth_fields(config)?;
                Ok(SnowflakeAuthIR::Warehouse {
                    user: config.get_str("user").expect("validated above"),
                    password: config.get_str("password").expect("validated above"),
                })
            }
            "warehouse_mfa" => {
                validate_warehouse_auth_fields(config)?;
                Ok(SnowflakeAuthIR::WarehouseMFA {
                    user: config.get_str("user").expect("validated above"),
                    password: config.get_str("password").expect("validated above"),
                })
            }
            "programmatic_access_token" => {
                let user = config.get_str("user").ok_or_else(|| {
                    AuthError::config("Snowflake PAT authentication requires 'user'.")
                })?;
                let token = config.get_str("token").ok_or_else(|| {
                    AuthError::config("Snowflake PAT authentication requires 'token'.")
                })?;
                if config.contains_key("password") {
                    warn_ignored_auth_field(warning_printer, "PAT", "password");
                }
                Ok(SnowflakeAuthIR::Pat { user, token })
            }
            "workload_identity" => parse_workload_identity(config),
            unsupported_method => Err(AuthError::config(format!(
                "Profile has unsupported authentication method {unsupported_method}"
            ))),
        }

    // Case 2: User has no `method` and we must rely on simple passthrough.
    //
    // For backwards compatibility with dbt-snowflake in core. By default,
    // there is no `method` parameter. `method` is the standard authentication
    // method option for other adapters, however.
    // FIXME: I made this better but Felipe has a long-term iterator-based solution to make this exact
    } else {
        // Reduce ambiguity in loop
        if config.contains_key("private_key_path") && config.contains_key("private_key") {
            return Err(AuthError::config(
                "Cannot specify both `private_key` and `private_key_path`.".to_owned(),
            ));
        }

        // Take first matching, even if this isn't strictly correct right, fallback to user/pass
        for key in AUTH_PARAMS_USED_FOR_LEGACY_CONFIG.iter() {
            if let Some(value) = config.get_str(key) {
                return match *key {
                    "private_key_path" => {
                        if config.get_str("user").is_none() {
                            return Err(AuthError::config(
                                "Snowflake keypair authentication requires 'user'.",
                            ));
                        }
                        if config.contains_key("password") {
                            warn_ignored_auth_field(warning_printer, "keypair", "password");
                        }

                        Ok(SnowflakeAuthIR::KeypairPath {
                            user: config.get_str("user").expect("validated above"),
                            path: value,
                            passphrase: config.get_str("private_key_passphrase"),
                        })
                    }
                    "private_key" => {
                        if config.get_str("user").is_none() {
                            return Err(AuthError::config(
                                "Snowflake keypair authentication requires 'user'.",
                            ));
                        }
                        if config.contains_key("password") {
                            warn_ignored_auth_field(warning_printer, "keypair", "password");
                        }

                        Ok(SnowflakeAuthIR::KeypairInline {
                            user: config.get_str("user").expect("validated above"),
                            private_key: value,
                            passphrase: config.get_str("private_key_passphrase"),
                        })
                    }
                    "private_key_passphrase" => {
                        if config.get_str("user").is_none() {
                            return Err(AuthError::config(
                                "Snowflake keypair authentication requires 'user'.",
                            ));
                        }
                        if config.contains_key("password") {
                            warn_ignored_auth_field(warning_printer, "keypair", "password");
                        }

                        // We found a passphrase, so we MUST find a key source to go with it
                        let path = config.get_str("private_key_path");
                        let raw = config.get_str("private_key");

                        match (path, raw) {
                            (Some(p), _) => Ok(SnowflakeAuthIR::KeypairPath {
                                user: config.get_str("user").expect("validated above"),
                                path: p,
                                passphrase: Some(value),
                            }),
                            (None, Some(r)) => Ok(SnowflakeAuthIR::KeypairInline {
                                user: config.get_str("user").expect("validated above"),
                                private_key: r,
                                passphrase: Some(value),
                            }),
                            (None, None) => Err(AuthError::config(
                                "Found 'private_key_passphrase' but missing 'private_key_path' or 'private_key'",
                            )),
                        }
                    }
                    "oauth_client_id" | "oauth_client_secret" => {
                        // TODO(versusfacit): reenable warnings when possible
                        // if config.contains_key("user") {
                        //     warn_ignored_auth_field(warning_printer, "OAuth", "user");
                        // }
                        // if config.contains_key("password") {
                        //     warn_ignored_auth_field(warning_printer, "OAuth", "password");
                        // }

                        let cid = config.get_str("oauth_client_id");
                        let sec = config.get_str("oauth_client_secret");
                        // backwards compatibility; we'd prefer this to be named refresh_token but this
                        let tok = config.get_str("token");
                        match (cid, sec, tok) {
                            (Some(client_id), Some(client_secret), Some(refresh_token)) => {
                                Ok(SnowflakeAuthIR::NativeOauth {
                                    // Unlike other auth methods, oauth does NOT want to be stubbed
                                    // with user/password
                                    client_id,
                                    client_secret,
                                    refresh_token,
                                })
                            }
                            _ => Err(AuthError::config(
                                "Legacy OAuth requires `oauth_client_id`, `oauth_client_secret`, and `token`.",
                            )),
                        }
                    }
                    "authenticator" => {
                        if value == "externalbrowser" {
                            if config.contains_key("password") {
                                warn_ignored_auth_field(warning_printer, "SSO", "password");
                            }

                            config
                                .get_str("user")
                                .map(|user| SnowflakeAuthIR::Sso { user })
                                .ok_or_else(|| {
                                    AuthError::config(
                                        "Snowflake SSO authentication requires 'user'.",
                                    )
                                })
                        } else if value == "oauth" {
                            if config.contains_key("user") {
                                warn_ignored_auth_field(warning_printer, "OAuth", "user");
                            }
                            if config.contains_key("password") {
                                warn_ignored_auth_field(warning_printer, "OAuth", "password");
                            }

                            let cid = config.get_str("oauth_client_id");
                            let sec = config.get_str("oauth_client_secret");
                            let tok = config.get_str("refresh_token");

                            match (cid, sec, tok) {
                                (Some(client_id), Some(client_secret), Some(refresh_token)) => {
                                    Ok(SnowflakeAuthIR::NativeOauth {
                                        client_id,
                                        client_secret,
                                        refresh_token,
                                    })
                                }
                                _ => Err(AuthError::config(
                                    "Legacy 'authenticator: oauth' requires oauth_client_id, oauth_client_secret, and refresh_token",
                                )),
                            }
                        } else if value == "jwt" {
                            config
                                .get_str("token")
                                .map(|access_token| SnowflakeAuthIR::NativeOauthJWT {
                                    user: config.get_str("user"),
                                    password: config.get_str("password"),
                                    access_token,
                                })
                                .ok_or_else(|| {
                                    AuthError::config("Legacy 'authenticator: jwt' requires token")
                                })
                        } else if value == "username_password_mfa" {
                            validate_warehouse_auth_fields(config)?;
                            Ok(SnowflakeAuthIR::WarehouseMFA {
                                user: config.get_str("user").expect("validated above"),
                                password: config.get_str("password").expect("validated above"),
                            })
                        } else if value == "programmatic_access_token" {
                            let user = config.get_str("user").ok_or_else(|| {
                                AuthError::config("Snowflake PAT authentication requires 'user'.")
                            })?;
                            let token = config.get_str("token").ok_or_else(|| {
                                AuthError::config("Snowflake PAT authentication requires 'token'.")
                            })?;
                            if config.contains_key("password") {
                                warn_ignored_auth_field(warning_printer, "PAT", "password");
                            }
                            Ok(SnowflakeAuthIR::Pat { user, token })
                        } else if value == "workload_identity" {
                            parse_workload_identity(config)
                        } else {
                            Err(AuthError::config(format!(
                                "Unsupported authenticator: {value}"
                            )))
                        }
                    }
                    _ => panic!("unexpected key: {key}"),
                };
            }
        }

        // Fallback to username and password when no other auth is possible
        validate_warehouse_auth_fields(config)?;
        Ok(SnowflakeAuthIR::Warehouse {
            user: config.get_str("user").expect("validated above"),
            password: config.get_str("password").expect("validated above"),
        })
    }
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    for key in CONNECTION_PARAMS_STR {
        if let Some(value) = config.get_str(key) {
            match key {
                "account" => builder.with_named_option(snowflake::ACCOUNT, value),
                "database" => builder.with_named_option(snowflake::DATABASE, value),
                "schema" => builder.with_named_option(snowflake::SCHEMA, value),
                "role" => builder.with_named_option(snowflake::ROLE, value),
                "warehouse" => builder.with_named_option(snowflake::WAREHOUSE, value),
                "host" => builder.with_named_option(snowflake::HOST, value),
                "protocol" => builder.with_named_option(snowflake::PROTOCOL, value),
                snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY => {
                    builder.with_named_option(snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY, value)
                }
                snowflake::QUERY_TAG_PARAM_KEY => {
                    builder.with_named_option(snowflake::QUERY_TAG_PARAM_KEY, value)
                }
                _ => panic!("unexpected key: {key}"),
            }?;
        }
    }

    for key in CONNECTION_PARAMS {
        if let Some(value) = config.get_string(key) {
            match key {
                "port" => builder.with_named_option(snowflake::PORT, value.as_ref()),
                "client_session_keep_alive" => {
                    builder.with_named_option(snowflake::KEEP_SESSION_ALIVE, value.as_ref())
                }
                _ => panic!("unexpected key: {key}"),
            }?;
        }
    }
    let application_name = config.get_str("application_name").unwrap_or(APP_NAME);
    builder.with_named_option(snowflake::APPLICATION_NAME, application_name)?;

    // LOGIN_TIMEOUT defaults to 300s,
    // see https://github.com/dbt-labs/gosnowflake/blob/c1d9c4ea1fde32184cbce1f728a4db2ea0cec048/dsn.go         = 300 * time.Second // Timeout for retry for login EXCLUDING clientTimeout
    // but is configurable via an env var to fail fast in certain cases
    if let Ok(login_timeout) = std::env::var("DBT_SNOWFLAKE_LOGIN_TIMEOUT") {
        builder.with_named_option(snowflake::LOGIN_TIMEOUT, login_timeout)?;
    }

    let request_timeout = config
        .get_string("request_timeout")
        .map(|v| postfix_seconds_unit(v.as_ref()))
        .unwrap_or_else(|| DEFAULT_REQUEST_TIMEOUT.to_string());
    builder.with_named_option(snowflake::REQUEST_TIMEOUT, request_timeout)?;

    if let Ok(client_timeout) = std::env::var("DBT_SNOWFLAKE_CLIENT_TIMEOUT") {
        builder.with_named_option(snowflake::CLIENT_TIMEOUT, client_timeout)?;
    }
    if let Ok(auth_client_timeout) = std::env::var("DBT_SNOWFLAKE_AUTH_CLIENT_TIMEOUT") {
        builder.with_named_option(snowflake::AUTH_CLIENT_TIMEOUT, auth_client_timeout)?;
    }

    // disable any logging from Gosnowflake that's not a fatal/panic by default;
    // can be overridden via the `driver_log_level` profile field for debugging.
    let log_tracing = match config.get_str("driver_log_level") {
        Some(value) => value
            .parse::<LogLevel>()
            .map_err(|e| AuthError::config(e.to_string()))?
            .to_string(),
        None => LogLevel::Fatal.to_string(),
    };
    builder.with_named_option(snowflake::LOG_TRACING, log_tracing)?;

    Ok(builder)
}

pub struct SnowflakeAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl SnowflakeAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for SnowflakeAuth {
    fn backend(&self) -> Backend {
        Backend::Snowflake
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        crate::auth_configure_pipeline!(self, config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoopAuthWarningPrinter;
    use crate::test_options::option_str_value;
    use adbc_core::options::OptionDatabase;
    use dbt_yaml::Mapping;
    use dbt_yaml::Value as YmlValue;
    use key_format::{
        PEM_ENCRYPTED_END, PEM_ENCRYPTED_START, PEM_UNENCRYPTED_END, PEM_UNENCRYPTED_START,
    };

    fn base_config() -> Mapping {
        Mapping::from_iter([
            ("user".into(), "U".into()),
            ("password".into(), "P".into()),
            ("account".into(), "A".into()),
            ("role".into(), "role".into()),
            ("warehouse".into(), "warehouse".into()),
        ])
    }

    fn base_config_without_user() -> Mapping {
        Mapping::from_iter([
            ("password".into(), "P".into()),
            ("account".into(), "A".into()),
            ("role".into(), "role".into()),
            ("warehouse".into(), "warehouse".into()),
        ])
    }

    fn base_config_without_password() -> Mapping {
        Mapping::from_iter([
            ("user".into(), "U".into()),
            ("account".into(), "A".into()),
            ("role".into(), "role".into()),
            ("warehouse".into(), "warehouse".into()),
        ])
    }

    fn base_config_without_user_password() -> Mapping {
        Mapping::from_iter([
            ("account".into(), "A".into()),
            ("role".into(), "role".into()),
            ("warehouse".into(), "warehouse".into()),
        ])
    }

    fn assert_parse_auth_config_error(config: Mapping, expected_msg: &str) {
        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);
        match result {
            Err(AuthError::Config(msg)) => assert_eq!(msg, expected_msg),
            other => panic!("Expected AuthError::Config({expected_msg:?}), got {other:?}"),
        }
    }

    fn run_config_test(config: Mapping, expected: &[(&str, &str)]) {
        let auth = SnowflakeAuth {
            warning_printer: Box::new(NoopAuthWarningPrinter),
        };
        let auth_result = auth
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        let mut results = Mapping::default();

        for (k, v) in auth_result.into_iter() {
            let key = match k {
                OptionDatabase::Username => "user".to_owned(),
                OptionDatabase::Password => "password".to_owned(),
                OptionDatabase::Other(name) => name.to_owned(),
                _ => continue,
            };
            if key == snowflake::CLIENT_TIMEOUT || key == snowflake::LOGIN_TIMEOUT {
                continue;
            }
            results.insert(key.into(), option_str_value(&v).into());
        }

        for &(key, expected_val) in expected {
            assert_eq!(
                results
                    .get(key)
                    .unwrap_or_else(|| panic!("Missing key: {key}")),
                &expected_val,
                "Value mismatch for key: {key}"
            );
        }

        assert_eq!(
            results.len(),
            expected.len(),
            "Unexpected extra keys:
    left: {results:?}
    right: {expected:?}",
        );
    }

    fn wrap_pem_64(begin: &str, body_b64: &str, end: &str) -> String {
        let mut out = String::new();
        out.push_str(begin);
        out.push('\n');
        let bytes = body_b64.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            let j = (i + 64).min(bytes.len());
            // body_b64 is ASCII, so this is safe
            out.push_str(std::str::from_utf8(&bytes[i..j]).unwrap());
            out.push('\n');
            i = j;
        }
        out.push_str(end);
        out
    }

    #[test]
    fn test_simple_pass() {
        let config = base_config();
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_application_name_override() {
        let mut config = base_config();
        config.insert("application_name".into(), "custom_app".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "custom_app"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_simple_pass_with_driver_log_level_override() {
        let mut config = base_config();
        config.insert("driver_log_level".into(), "debug".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "debug"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_simple_pass_with_invalid_driver_log_level() {
        let mut config = base_config();
        config.insert("driver_log_level".into(), "bogus".into());
        let auth = SnowflakeAuth {
            warning_printer: Box::new(NoopAuthWarningPrinter),
        };
        let result = auth.configure(&AdapterConfig::new(config));
        match result {
            Err(AuthError::Config(msg)) => {
                assert!(
                    msg.contains("invalid log level"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("Expected AuthError::Config(...), got {other:?}"),
        }
    }

    #[test]
    fn test_simple_pass_with_custom_connect_timeout_a() {
        let mut config = base_config();
        config.insert("connect_timeout".into(), "100".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_simple_pass_with_custom_connect_timeout_b() {
        let mut config = base_config();
        config.insert("connect_timeout".into(), "0".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_simple_pass_with_custom_request_timeout_a() {
        let mut config = base_config();
        config.insert("request_timeout".into(), "100".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, "100s"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_simple_pass_with_custom_request_timeout_b() {
        let mut config = base_config();
        config.insert("request_timeout".into(), "0".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, "0s"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_numeric_timeouts_are_respected() {
        let mut config = base_config();
        config.insert("connect_timeout".into(), YmlValue::number(100i64.into()));
        config.insert("request_timeout".into(), YmlValue::number(0i64.into()));
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, "0s"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_numeric_port_is_respected() {
        let mut config = base_config();
        config.insert("port".into(), YmlValue::number(443u64.into()));
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::PORT, "443"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_pass_with_method() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_warehouse_method_requires_user() {
        let mut config = base_config();
        config.remove("user");
        config.insert("method".into(), "warehouse".into());
        assert_parse_auth_config_error(
            config,
            "Snowflake warehouse authentication requires 'user'.",
        );
    }

    #[test]
    fn test_warehouse_method_requires_password() {
        let mut config = base_config();
        config.remove("password");
        config.insert("method".into(), "warehouse".into());
        assert_parse_auth_config_error(
            config,
            "Snowflake warehouse authentication requires 'password'.",
        );
    }

    #[test]
    fn test_keypair_method_requires_user() {
        let mut config = base_config_without_user();
        config.remove("password");
        config.insert("method".into(), "keypair".into());
        config.insert("private_key".into(), "private-key".into());
        assert_parse_auth_config_error(config, "Snowflake keypair authentication requires 'user'.");
    }

    #[test]
    fn test_keypair_method_ignores_password_and_uses_stub_password() {
        let mut config = base_config();
        config.insert("method".into(), "keypair".into());
        let expected_pem = format!(
            "{}\n{}\n{}",
            PEM_UNENCRYPTED_START, "private_key", PEM_UNENCRYPTED_END
        );
        config.insert("private_key".into(), expected_pem.clone().into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (
                snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE,
                expected_pem.as_str(),
            ),
            (snowflake::AUTH_TYPE, "auth_jwt"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_keypair_value_with_method_param() {
        let mut config = base_config_without_password();
        config.insert("method".into(), "keypair".into());
        let expected_pem = format!(
            "{}\n{}\n{}",
            PEM_UNENCRYPTED_START, "private_key", PEM_UNENCRYPTED_END
        );
        config.insert("private_key".into(), expected_pem.clone().into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (
                snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE,
                expected_pem.as_str(),
            ),
            (snowflake::AUTH_TYPE, "auth_jwt"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_keypair_path_with_method_param() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut tmp_file = NamedTempFile::new().expect("Failed to create temp file");
        writeln!(tmp_file, "fake-private-key-data").unwrap();
        let temp_path = tmp_file.path().to_str().expect("Valid UTF-8 path");

        let mut config = base_config_without_password();
        config.insert("method".into(), "keypair".into());
        config.insert("private_key_path".into(), temp_path.into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::JWT_PRIVATE_KEY, temp_path),
            (snowflake::AUTH_TYPE, "auth_jwt"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    // No library function to generate an encrypted key; made manually from
    // openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 | \
    // openssl pkcs8 -topk8 -v2 aes-256-cbc -passout pass:private_key_passphrase -inform PEM -outform DER | \
    // base64 -w0
    const ENCRYPTED_PKCS8_DER_B64: &str = "MIIFNTBfBgkqhkiG9w0BBQ0wUjAxBgkqhkiG9w0BBQwwJAQQTicT7AlFo6LN0RdUzkuo4AICCAAwDAYIKoZIhvcNAgkFADAdBglghkgBZQMEASoEEOnNZh3Day9astKrOi93uxgEggTQp2Z0RUN8e9pMhU3OUt+Jjz1HVVIILogdkDKktKbY4KOB/dT7qYDBa3pHqcHbIQm8frhpzKH4wDLptEblasFPcA0kaLHaDE8wQj6YalnMGWxF5T1aGKXqIRXr9xQFDzpllXrf2b5LIHKw1SzFX/qy8jv5KtXG6910fDVRM7h02eJFWYmm0uqbS9WHcU7IeSEgdiiER2Zvx0fsEZ3oM+gDnhg4/eW9QTRqqAU3oISSEstl+BXBYWYQFUf7wl2SEiKyDdQRzBhzSO8h00EQtiGcXviJUUoksktmQkJfIjjZBz/nHHjtNpQpTKa+uev/IY6/E2adxX3qkroSvdsK1phLq8a/JUhvVDTDxAOSNzaNQndnXJnhbpNAnnq32TilhnZhRXYjMJVXNlutTkoV90yyXara9WJ9Es2zZntuGathTYSre8VR0JAIgYvpqPP7DzD1hcbDzVES6q75gtaI+KD+af3QUnlReLP/c8roXsm27BGE2z5eo1j+gjzbOLqF/6EmkKzuLrJGl9pitSXVZBDeOzXOEIlvFytmhz+HjIGMGgBiPpBcOv73Whb91KF4PuCciXVGBhAlHlXNG5nvhL2NdfXxxHHTIGgGe9dQMAP5ap7z6sfjcLv/osp+jPqaizPZtUF3V/4OdiGFtJMRcD8Rnw/CTv/wWZksIpQ+PCJYR82dRY9Bu7F4v77ts1096otHI7dwA0SetZ2xeDngNiGlMVls3mygXknp5x8Tq737uyXId6vD/6fSBrI14gtJB6yFhbc5oc77UcWJQdvi+gOu4daLNuXdj7qlLFbQvWMNR5+LeJDsoW8jiULYX1vN+TKwzlszTBpi2+788LXWUtOC6wFxSk8SM9nVhXM4i8ONH3lioFy+N5MG9q4BGbvBiTLFfvn/MEp6fpVD1xrE9qfTfDqJjaNo3WBuSvFruLSS1Ih+ikPFHt8KV3chakByLGunOZKhkJV0B+Eh7HOD/TRoo0bf6EJ+I/WruQ/FvMRnKahuHX8Lr7nGFIg+VbNz/pMHevw1Tg9bD3koyVNbG3hpe4DFBd2gk8edIauCSAVJjt+JpJyiCfsYZw7RaCdbmjgw9Q8n43H5nAaiIfAU0hjya5RWA4HPH4e5RuZYQfvVsNUxcVTCE1BeZwZy+lFQFzd/DHW0EJQmhQwCBiy72xgn72Yv6XEkQDZOqNipcc7kja3JYSujSeXRPuWgmiQHyMQlDaz0qdJjmd5vUbFjoVFWsT3xAynddEl5hn7KCyOGDEvwdMLQI0CWP9MG+ZK8dXTE24u0oULZkWo2m2Zsqey05Erl0iKppu0d24HsJz8q9ueE5rWHOLV4L01fB5wiUvLBSkm3K9TLUeMdl/pw/3qxYe709ggQgqrM3UBcBzckEQ0sO8vBhDfbTZzKSquBS1ve29u/PUAM/g78AgcMwmiJpNrRVF5LNyLbBukSNxBigJkG61Tsqe9hfY9GsjKEefi6P0FTmaAmsw1vROCJSwqceWO+ldrYbOov0ViDYM1UfDO1lS7AItii8U1JCeuZkrMjcCZdoyhET3LTHM+NOHwLqce2RwVvoQMPk4kYftRohjR+M7/4WC9vwt5GmoK4NeNCBNdwphHLM/k5Dogu9/OOe8xrNRvunYunrU8w6ZOKR+s=";

    #[test]
    fn test_encrypted_keypair_without_method_param() {
        let mut config = base_config();
        let expected_pem = wrap_pem_64(
            PEM_ENCRYPTED_START,
            ENCRYPTED_PKCS8_DER_B64,
            PEM_ENCRYPTED_END,
        );
        let passphrase = "private_key_passphrase";
        config.insert("private_key".into(), ENCRYPTED_PKCS8_DER_B64.into());
        config.insert("private_key_passphrase".into(), passphrase.into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (
                snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE,
                expected_pem.as_str(),
            ),
            (snowflake::JWT_PRIVATE_KEY_PKCS8_PASSWORD, passphrase),
            (snowflake::AUTH_TYPE, "auth_jwt"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_encrypted_keypair_with_method_param() {
        let mut config = base_config_without_password();
        config.insert("method".into(), "keypair".into());

        let passphrase = "private_key_passphrase";
        let expected_pem = format!(
            "{}\n{}\n{}",
            PEM_ENCRYPTED_START, "private_key", PEM_ENCRYPTED_END
        );

        config.insert("private_key".into(), expected_pem.clone().into());
        config.insert("private_key_passphrase".into(), passphrase.into());

        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::JWT_PRIVATE_KEY_PKCS8_VALUE,
                expected_pem.as_str(),
            ),
            (snowflake::JWT_PRIVATE_KEY_PKCS8_PASSWORD, passphrase),
            (snowflake::AUTH_TYPE, "auth_jwt"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_external_browser_authentication() {
        let mut config = base_config();
        config.insert("authenticator".into(), "externalbrowser".into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::EXTERNAL_BROWSER),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_external_browser_authentication_requires_user() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "externalbrowser".into());
        assert_parse_auth_config_error(config, "Snowflake SSO authentication requires 'user'.");
    }

    #[test]
    fn test_external_browser_authentication_uses_stub_password() {
        let mut config = base_config();
        config.remove("password");
        config.insert("authenticator".into(), "externalbrowser".into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::EXTERNAL_BROWSER),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_external_browser_authentication_with_method_param() {
        let mut config = base_config();
        config.insert("method".into(), "sso".into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::EXTERNAL_BROWSER),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_external_browser_authentication_with_method_param_requires_user() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "sso".into());
        assert_parse_auth_config_error(config, "Snowflake SSO authentication requires 'user'.");
    }

    #[test]
    fn test_external_browser_authentication_with_method_param_uses_stub_password() {
        let mut config = base_config();
        config.remove("password");
        config.insert("method".into(), "sso".into());
        let expected = [
            ("user", "U"),
            ("password", ADBC_STUB_PASSWORD),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::EXTERNAL_BROWSER),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_native_oauth() {
        let mut config = base_config();
        config.insert("authenticator".into(), "oauth".into());
        config.insert("oauth_client_id".into(), "C".into());
        config.insert("oauth_client_secret".into(), "S".into());
        config.insert("token".into(), "R".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (snowflake::CLIENT_ID, "C"),
            (snowflake::CLIENT_SECRET, "S"),
            (snowflake::REFRESH_TOKEN, "R"),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_native_oauth_with_method_param() {
        let mut config = base_config();
        config.insert("method".into(), "snowflake_oauth".into());
        config.insert("oauth_client_id".into(), "C".into());
        config.insert("oauth_client_secret".into(), "S".into());
        config.insert("token".into(), "R".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (snowflake::CLIENT_ID, "C"),
            (snowflake::CLIENT_SECRET, "S"),
            (snowflake::REFRESH_TOKEN, "R"),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    #[ignore]
    fn test_oauth_fails_with_token_instead_of_refresh_token() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "snowflake_oauth".into());
        config.insert("oauth_client_id".into(), "client_id".into());
        config.insert("oauth_client_secret".into(), "secret".into());
        config.insert("token".into(), "should_be_refresh_token".into());

        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);

        assert!(
            matches!(result, Err(ref e) if matches!(e, AuthError::Config(_))),
            "Expected configuration error, got: {result:?}"
        );

        if let Err(e) = result {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("Rename") && msg.contains("token"),
                "Unexpected error message: {msg}"
            );
        }
    }

    #[test]
    fn test_oauth_method_ignores_user_and_password() {
        let mut config = base_config();
        config.insert("method".into(), "snowflake_oauth".into());
        config.insert("oauth_client_id".into(), "C".into());
        config.insert("oauth_client_secret".into(), "S".into());
        config.insert("token".into(), "R".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (snowflake::CLIENT_ID, "C"),
            (snowflake::CLIENT_SECRET, "S"),
            (snowflake::REFRESH_TOKEN, "R"),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_invalid_private_key_path() {
        let auth = SnowflakeAuth {
            warning_printer: Box::new(NoopAuthWarningPrinter),
        };
        let bad_path = "this_file_does_not_exist.p8";

        macro_rules! assert_file_not_found {
            ($config_map:expr, $label:expr) => {
                let result = auth.configure(&AdapterConfig::new($config_map));
                assert!(
                    matches!(result, Err(AuthError::Config(ref msg)) if msg.contains("Private key file not found")),
                    "{} path failed: expected 'Private key file not found' error, got {:?}",
                    $label, result
                );
            };
        }

        // 1. Test Legacy Case
        let mut legacy_map = base_config_without_password();
        legacy_map.insert("private_key_path".into(), bad_path.into());
        assert_file_not_found!(legacy_map, "Legacy");

        // 2. Test Modern Case
        let mut modern_map = base_config_without_password();
        modern_map.insert("method".into(), "keypair".into());
        modern_map.insert("private_key_path".into(), bad_path.into());
        assert_file_not_found!(modern_map, "Modern");
    }

    #[test]
    fn test_oauth_fails_with_missing_required_fields() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "snowflake_oauth".into());
        config.insert("oauth_client_id".into(), "client_id".into());
        // oauth_client_secret OMITTED ON PURPOSE
        config.insert("token".into(), "token".into());

        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);

        assert!(
            matches!(result, Err(ref e) if matches!(e, AuthError::Config(_))),
            "Expected configuration error due to missing OAuth fields, got: {result:?}"
        );

        if let Err(e) = result {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("oauth_client_id")
                    && msg.contains("oauth_client_secret")
                    && msg.contains("token"),
                "Unexpected error message: {msg}"
            );
        }
    }

    #[test]
    fn test_userpass_mfa() {
        let mut config = base_config();
        config.insert("authenticator".into(), "username_password_mfa".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::USERNAME_PASSWORD_MFA,
            ),
            (snowflake::CLIENT_CACHE_MFA_TOKEN, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_userpass_mfa_requires_user() {
        let mut config = base_config();
        config.remove("user");
        config.insert("authenticator".into(), "username_password_mfa".into());
        assert_parse_auth_config_error(
            config,
            "Snowflake warehouse authentication requires 'user'.",
        );
    }

    #[test]
    fn test_userpass_mfa_requires_password() {
        let mut config = base_config();
        config.remove("password");
        config.insert("authenticator".into(), "username_password_mfa".into());
        assert_parse_auth_config_error(
            config,
            "Snowflake warehouse authentication requires 'password'.",
        );
    }

    #[test]
    fn test_userpass_mfa_with_method_param() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse_mfa".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::USERNAME_PASSWORD_MFA,
            ),
            (snowflake::CLIENT_CACHE_MFA_TOKEN, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_catch_unneeded_authenticator() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse".into());
        config.insert("authenticator".into(), "wrong".into());

        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);

        assert!(
            matches!(result, Err(ref e) if matches!(e, AuthError::Config(_))),
            "Expected configuration error, got: {result:?}"
        );

        if let Err(e) = result {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("Using 'method' in your Snowflake profile subsumes"),
                "Unexpected error message: {msg}"
            );
        }
    }

    #[test]
    fn test_jwt_oauth() {
        let mut config = base_config();
        config.insert("authenticator".into(), "jwt".into());
        config.insert(
            "token".into(),
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9".into(),
        );

        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (
                snowflake::AUTH_TOKEN,
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            ),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];

        run_config_test(config, &expected);
    }

    #[test]
    fn test_jwt_oauth_without_user_and_password() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "jwt".into());
        config.insert(
            "token".into(),
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9".into(),
        );

        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (
                snowflake::AUTH_TOKEN,
                "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            ),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];

        run_config_test(config, &expected);
    }

    #[test]
    fn test_jwt_oauth_fails_with_token_instead_of_jwt() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "snowflake_oauth_jwt".into());
        config.insert("token".into(), "wrong_field".into());

        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);

        assert!(
            matches!(result, Err(ref e) if matches!(e, AuthError::Config(_))),
            "Expected configuration error, got: {result:?}"
        );

        if let Err(e) = result {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("Profile") && msg.contains("'jwt_token'"),
                "Unexpected error message: {msg}"
            );
        }
    }

    #[test]
    fn test_jwt_oauth_fails_with_missing_jwt() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "snowflake_oauth_jwt".into());
        // jwt intentionally missing

        let cfg = AdapterConfig::new(config);
        let result = parse_auth(&cfg, &NoopAuthWarningPrinter);

        assert!(
            matches!(result, Err(ref e) if matches!(e, AuthError::Config(_))),
            "Expected configuration error for missing jwt, got: {result:?}"
        );

        if let Err(e) = result {
            let msg = format!("{e:?}");
            assert!(
                msg.contains("jwt_token") && msg.contains("snowflake_oauth_jwt"),
                "Unexpected error message: {msg}"
            );
        }
    }

    #[test]
    fn test_oauth_jwt_method_uses_user_and_password_if_present() {
        let mut config = base_config();
        config.insert("method".into(), "snowflake_oauth_jwt".into());
        config.insert("jwt_token".into(), "jwt".into());
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (snowflake::AUTH_TOKEN, "jwt"),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_oauth_jwt_method_works_without_user_and_password() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "snowflake_oauth_jwt".into());
        config.insert("jwt_token".into(), "jwt".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (snowflake::AUTH_TYPE, snowflake::auth_type::OAUTH),
            (snowflake::AUTH_TOKEN, "jwt"),
            (snowflake::CLIENT_STORE_TEMP_CREDS, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_pat_authentication() {
        let mut config = base_config_without_password();
        config.insert("authenticator".into(), "programmatic_access_token".into());
        config.insert("token".into(), "my-pat-token".into());

        // PAT auth: gosnowflake uses only User + Token for AuthTypePat; Password
        // is never read or sent to Snowflake. The ADBC password field must not
        // be set — neither real nor stubbed.
        let expected = [
            ("user", "U"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::PROGRAMMATIC_ACCESS_TOKEN,
            ),
            (snowflake::AUTH_TOKEN, "my-pat-token"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_pat_authentication_with_method_param() {
        let mut config = base_config_without_password();
        config.insert("method".into(), "programmatic_access_token".into());
        config.insert("token".into(), "my-pat-token".into());

        // PAT auth: no password field should be present.
        let expected = [
            ("user", "U"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::PROGRAMMATIC_ACCESS_TOKEN,
            ),
            (snowflake::AUTH_TOKEN, "my-pat-token"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_pat_authentication_ignores_password() {
        // When user mistakenly includes a password in the profile alongside PAT,
        // it must be dropped entirely (not replaced with a stub) — gosnowflake
        // ignores cfg.Password for AuthTypePat, so leaking it would only add
        // noise to logs.
        let mut config = base_config();
        config.insert("authenticator".into(), "programmatic_access_token".into());
        config.insert("token".into(), "my-pat-token".into());

        let expected = [
            ("user", "U"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::PROGRAMMATIC_ACCESS_TOKEN,
            ),
            (snowflake::AUTH_TOKEN, "my-pat-token"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_pat_authentication_requires_user() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "programmatic_access_token".into());
        config.insert("token".into(), "my-pat-token".into());
        assert_parse_auth_config_error(config, "Snowflake PAT authentication requires 'user'.");
    }

    #[test]
    fn test_pat_authentication_requires_token() {
        let mut config = base_config_without_password();
        config.insert("authenticator".into(), "programmatic_access_token".into());
        assert_parse_auth_config_error(config, "Snowflake PAT authentication requires 'token'.");
    }

    #[test]
    fn test_pat_method_requires_user() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "programmatic_access_token".into());
        config.insert("token".into(), "my-pat-token".into());
        assert_parse_auth_config_error(config, "Snowflake PAT authentication requires 'user'.");
    }

    #[test]
    fn test_pat_method_requires_token() {
        let mut config = base_config_without_password();
        config.insert("method".into(), "programmatic_access_token".into());
        assert_parse_auth_config_error(config, "Snowflake PAT authentication requires 'token'.");
    }

    #[test]
    fn test_s3_stage_vpce_dns_name() {
        let mut config = base_config();
        config.insert(
            snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY.into(),
            "my-vpce-endpoint.s3.region.vpce.amazonaws.com".into(),
        );
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "dbt"),
            (
                snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY,
                "my-vpce-endpoint.s3.region.vpce.amazonaws.com",
            ),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_s3_stage_vpce_dns_name_with_method() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse".into());
        config.insert(
            snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY.into(),
            "my-vpce-endpoint.s3.region.vpce.amazonaws.com".into(),
        );
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::S3_STAGE_VPCE_DNS_NAME_PARAM_KEY,
                "my-vpce-endpoint.s3.region.vpce.amazonaws.com",
            ),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_query_tag() {
        let mut config = base_config();
        config.insert(
            snowflake::QUERY_TAG_PARAM_KEY.into(),
            "custom-query-tag".into(),
        );
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "dbt"),
            (snowflake::QUERY_TAG_PARAM_KEY, "custom-query-tag"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_query_tag_with_method() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse".into());

        config.insert(
            snowflake::QUERY_TAG_PARAM_KEY.into(),
            "custom-query-tag".into(),
        );
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "dbt"),
            (snowflake::QUERY_TAG_PARAM_KEY, "custom-query-tag"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_keep_session_alive_with_method() {
        let mut config = base_config();
        config.insert("method".into(), "warehouse".into());
        config.insert("client_session_keep_alive".into(), YmlValue::bool(true));
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "dbt"),
            (snowflake::KEEP_SESSION_ALIVE, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_workload_identity_authenticator() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "workload_identity".into());
        config.insert("workload_identity_provider".into(), "azure".into());
        config.insert(
            "workload_identity_entra_resource".into(),
            "app://123".into(),
        );
        config.insert("token".into(), "test_token".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::WORKLOAD_IDENTITY,
            ),
            (snowflake::WORKLOAD_IDENTITY_PROVIDER, "azure"),
            (snowflake::WORKLOAD_IDENTITY_ENTRA_RESOURCE, "app://123"),
            (snowflake::AUTH_TOKEN, "test_token"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_workload_identity_method() {
        let mut config = base_config_without_user_password();
        config.insert("method".into(), "workload_identity".into());
        config.insert("workload_identity_provider".into(), "OIDC".into());
        let expected = [
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, APP_NAME),
            (
                snowflake::AUTH_TYPE,
                snowflake::auth_type::WORKLOAD_IDENTITY,
            ),
            (snowflake::WORKLOAD_IDENTITY_PROVIDER, "OIDC"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }

    #[test]
    fn test_workload_identity_fails_without_provider() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "workload_identity".into());
        assert_parse_auth_config_error(
            config,
            "workload_identity_provider must be set to one of the following values if authenticator='workload_identity'!:\nOIDC, AZURE, GCP, AWS\n\nProvided workload_identity_provider was ''",
        );
    }

    #[test]
    fn test_workload_identity_fails_with_invalid_provider() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "workload_identity".into());
        config.insert(
            "workload_identity_provider".into(),
            "some_non_existent_cloud_provider".into(),
        );
        assert_parse_auth_config_error(
            config,
            "workload_identity_provider must be set to one of the following values if authenticator='workload_identity'!:\nOIDC, AZURE, GCP, AWS\n\nProvided workload_identity_provider was 'some_non_existent_cloud_provider'",
        );
    }

    #[test]
    fn test_workload_identity_fails_with_entra_resource_and_non_azure_provider() {
        let mut config = base_config_without_user_password();
        config.insert("authenticator".into(), "workload_identity".into());
        config.insert("workload_identity_provider".into(), "aws".into());
        config.insert(
            "workload_identity_entra_resource".into(),
            "app://123".into(),
        );
        assert_parse_auth_config_error(
            config,
            "workload_identity_entra_resource can only be set if workload_identity_provider is Azure",
        );
    }

    #[test]
    fn test_keep_session_alive_string_value() {
        let mut config = base_config();
        config.insert(
            "client_session_keep_alive".into(),
            YmlValue::string("true".to_owned()),
        );
        let expected = [
            ("user", "U"),
            ("password", "P"),
            (snowflake::ACCOUNT, "A"),
            (snowflake::ROLE, "role"),
            (snowflake::WAREHOUSE, "warehouse"),
            (snowflake::APPLICATION_NAME, "dbt"),
            (snowflake::KEEP_SESSION_ALIVE, "true"),
            (snowflake::LOG_TRACING, "fatal"),
            (snowflake::REQUEST_TIMEOUT, DEFAULT_REQUEST_TIMEOUT),
        ];
        run_config_test(config, &expected);
    }
}
