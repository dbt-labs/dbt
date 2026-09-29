// Names of Database options --------------------------------------------

pub const DATABASE: &str = "adbc.snowflake.sql.db";
pub const SCHEMA: &str = "adbc.snowflake.sql.schema";
pub const WAREHOUSE: &str = "adbc.snowflake.sql.warehouse";
pub const ROLE: &str = "adbc.snowflake.sql.role";
pub const REGION: &str = "adbc.snowflake.sql.region";
pub const ACCOUNT: &str = "adbc.snowflake.sql.account";
/// The protocol to use for the connection.
///
/// "http" or "https".
pub const PROTOCOL: &str = "adbc.snowflake.sql.uri.protocol";
pub const PORT: &str = "adbc.snowflake.sql.uri.port";
pub const HOST: &str = "adbc.snowflake.sql.uri.host";
/// The auth type to use for the connection.
///
/// Examples: auth_type::{DEFAULT, OAUTH, ...}.
pub const AUTH_TYPE: &str = "adbc.snowflake.sql.auth_type";
pub const LOGIN_TIMEOUT: &str = "adbc.snowflake.sql.client_option.login_timeout";
pub const REQUEST_TIMEOUT: &str = "adbc.snowflake.sql.client_option.request_timeout";
pub const JWT_EXPIRE_TIMEOUT: &str = "adbc.snowflake.sql.client_option.jwt_expire_timeout";
pub const CLIENT_TIMEOUT: &str = "adbc.snowflake.sql.client_option.client_timeout";
pub const AUTH_CLIENT_TIMEOUT: &str = "adbc.snowflake.sql.client_option.auth_client_timeout";
pub const USE_HIGH_PRECISION: &str = "adbc.snowflake.sql.client_option.use_high_precision";
pub const APPLICATION_NAME: &str = "adbc.snowflake.sql.client_option.app_name";
pub const SSL_SKIP_VERIFY: &str = "adbc.snowflake.sql.client_option.tls_skip_verify";
pub const OCSP_FAIL_OPEN_MODE: &str = "adbc.snowflake.sql.client_option.ocsp_fail_open_mode";
pub const AUTH_TOKEN: &str = "adbc.snowflake.sql.client_option.auth_token";
pub const AUTH_OKTA_URL: &str = "adbc.snowflake.sql.client_option.okta_url";
pub const KEEP_SESSION_ALIVE: &str = "adbc.snowflake.sql.client_option.keep_session_alive";
pub const JWT_PRIVATE_KEY: &str = "adbc.snowflake.sql.client_option.jwt_private_key";
pub const JWT_PRIVATE_KEY_PKCS8_VALUE: &str =
    "adbc.snowflake.sql.client_option.jwt_private_key_pkcs8_value";
pub const JWT_PRIVATE_KEY_PKCS8_PASSWORD: &str =
    "adbc.snowflake.sql.client_option.jwt_private_key_pkcs8_password";
pub const DISABLE_TELEMETRY: &str = "adbc.snowflake.sql.client_option.disable_telemetry";
pub const LOG_TRACING: &str = "adbc.snowflake.sql.client_option.tracing";
pub const CLIENT_CONFIG_FILE: &str = "adbc.snowflake.sql.client_option.config_file";

/// The identity provider to use for Workload Identity Federation (WIF) auth.
///
/// One of "OIDC", "AWS", "GCP", "AZURE". Must be set when auth_type is
/// auth_type::WORKLOAD_IDENTITY.
pub const WORKLOAD_IDENTITY_PROVIDER: &str = "adbc.snowflake.sql.client_option.identity_provider";
/// The Entra resource to request a token for. Only valid when
/// WORKLOAD_IDENTITY_PROVIDER is "AZURE"; Snowflake's default resource is used if unset.
pub const WORKLOAD_IDENTITY_ENTRA_RESOURCE: &str =
    "adbc.snowflake.sql.client_option.identity_provider_entra_resource";

// WARN: Do not set both of these for one runtime
// Turn on caching for username password MFA tokens
pub const CLIENT_CACHE_MFA_TOKEN: &str = "adbc.snowflake.sql.client_option.cache_mfa_token";
// Turns on caching for several methods: externalbrowser, (later) OAuth-related methods
pub const CLIENT_STORE_TEMP_CREDS: &str = "adbc.snowflake.sql.client_option.store_temp_creds";

// Session Param Keys
// https://docs.snowflake.com/en/sql-reference/parameters
pub const S3_STAGE_VPCE_DNS_NAME_PARAM_KEY: &str = "s3_stage_vpce_dns_name";
pub const QUERY_TAG_PARAM_KEY: &str = "query_tag";

pub mod auth_type {
    /// General username password authentication
    pub const DEFAULT: &str = "auth_snowflake";
    /// OAuth authentication
    pub const OAUTH: &str = "auth_oauth";
    /// Use a browser to access an FED and perform SSO authentication
    pub const EXTERNAL_BROWSER: &str = "auth_ext_browser";
    /// Native okta URL to perform SSO authentication on Okta
    pub const OKTA: &str = "auth_okta";
    /// Use Jwt to perform authentication
    pub const JWT: &str = "auth_jwt";
    /// Username and password with mfa
    pub const USERNAME_PASSWORD_MFA: &str = "auth_mfa";
    /// Snowflake Programmatic Access Token (PAT)
    pub const PROGRAMMATIC_ACCESS_TOKEN: &str = "auth_pat";
    /// Workload Identity Federation (WIF)
    pub const WORKLOAD_IDENTITY: &str = "auth_wif";
}

// Names of Connection options --------------------------------------------

// USE_HIGH_PRECISION can be a database option, but it is also a connection option.

/// dbt-specific fields that were added to support native Snowflake oauth
pub const CLIENT_ID: &str = "adbc.snowflake.sql.client_option.client_id";
pub const CLIENT_SECRET: &str = "adbc.snowflake.sql.client_option.client_secret";
pub const REFRESH_TOKEN: &str = "adbc.snowflake.sql.client_option.refresh_token";

// Names of schema metadata keys -----------------------------------------

pub mod schema_metadata {
    pub const QUERY_ID: &str = "SNOWFLAKE_QUERY_ID";
}
