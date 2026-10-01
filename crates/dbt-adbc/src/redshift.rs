/// The auth provider.
pub const AUTH_PROVIDER: &str = "redshift.auth.provider";
/// If specified, driver uses AWS SDK to fetch credentials.
pub const CLUSTER_TYPE: &str = "redshift.cluster_type";

pub mod cluster_type {
    pub const REDSHIFT: &str = "redshift";
    pub const REDSHIFT_IAM: &str = "redshift-iam";
    pub const SERVERLESS: &str = "redshift-serverless";
}

/// Option to automatically create user if it does not exist.
pub const AUTO_CREATE: &str = "redshift.auto_create_user";

/// Name of cluster containing the database.
pub const CLUSTER_IDENTIFIER: &str = "redshift.cluster_identifier";

/// Custom domain name associated with workgroup
pub const CUSTOM_DOMAIN_NAME: &str = "redshift.custom_domain_name";
/// Workgroup name associated with database
pub const WORK_GROUP_NAME: &str = "redshift.workgroup_name";

/// JSON string list of groups the user should join
pub const DB_GROUPS: &str = "redshift.db_groups";

pub const DB_NAME: &str = "redshift.db_name";
pub const CONNECTION_URI: &str = "redshift.connection_uri";

pub const AUTH_IDC_REGION: &str = "redshift.auth.idc_region";
pub const AUTH_ISSUER_URL: &str = "redshift.auth.issuer_url";

pub const AUTH_TOKEN_TYPE: &str = "redshift.auth.token_type";
pub const AUTH_TOKEN: &str = "redshift.auth.token";
pub const AUTH_IDP_LISTEN_PORT: &str = "redshift.auth.listen_port";
pub const AUTH_IDP_RESPONSE_TIMEOUT: &str = "redshift.auth.idp_response_timeout_seconds";
pub const AUTH_IDC_CLIENT_DISPLAY_NAME: &str = "redshift.auth.idc_client_display_name";

/// Whether TLS encryption is required
pub const SSL_MODE: &str = "redshift.ssl_mode";
pub const SSL_CERT: &str = "redshift.ssl_cert";
pub const SSL_KEY: &str = "redshift.ssl_key";
pub const SSL_ROOT_CERT: &str = "redshift.ssl_root_key";

pub const CONNECT_TIMEOUT_MS: &str = "redshift.connect_timeout_ms";
pub const CONNECT_TIMEOUT: &str = "redshift.connect_timeout";

pub const APPLICATION_NAME: &str = "redshift.application_name";

pub const AWS_REGION: &str = "redshift.aws.region";
pub const AWS_PROFILE: &str = "redshift.aws.profile";
pub const AWS_ACCESS_KEY_ID: &str = "redshift.aws.access_key_id";
pub const AWS_SECRET_ACCESS_KEY: &str = "redshift.aws.secret_access_key";
pub const AWS_SESSION_TOKEN: &str = "redshift.aws.session_token";

/// S3 bucket to use when ingesting data
pub const INGEST_BUCKET: &str = "redshift.ingest.bucket";

pub const AUTH_PROVIDER_USER_PASS: &str = "userpass";
pub const AUTH_PROVIDER_BROWSER_IDC: &str = "BrowserIdcAuthPlugin";
pub const AUTH_PROVIDER_IDP_TOKEN: &str = "IdpTokenAuthPlugin";
