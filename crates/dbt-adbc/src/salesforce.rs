// Names of Database options for Salesforce Data Cloud ADBC Driver
// Based on the Go implementation at arrow-adbc/go/adbc/driver/salesforce/driver.go

/// Login URL (default: [DEFAULT_LOGIN_URL])
pub const LOGIN_URL: &str = "adbc.salesforce.dc.login_url";
/// Username for authentication
pub const USERNAME: &str = "adbc.salesforce.dc.username";
/// Client ID for OAuth/JWT authentication (Connected App Consumer Key)
pub const CLIENT_ID: &str = "adbc.salesforce.dc.client_id";

/// JWT Bearer Authentication options
pub const JWT_PRIVATE_KEY: &str = "adbc.salesforce.dc.private_key";

/// Username/Password Authentication options
pub const PASSWORD: &str = "adbc.salesforce.dc.password";

/// Client Secret for OAuth authentication (Connected App Consumer Secret)
pub const CLIENT_SECRET: &str = "adbc.salesforce.dc.client_secret";

/// The Salesforce instance URL (e.g., https://myorg.my.salesforce.com)
pub const INSTANCE_URL: &str = "adbc.salesforce.dc.instance_url";

/// QueryOptions: Timeout for HTTP requests
pub const QUERY_TIMEOUT: &str = "adbc.salesforce.dc.query.timeout";
/// QueryOptions: Row limit
pub const QUERY_ROW_LIMIT: &str = "adbc.salesforce.dc.query.row_limit";

/// Data Transform Run Timeout
pub const DATA_TRANSFORM_RUN_TIMEOUT: &str = "adbc.salesforce.dc.data_transform_run_timeout";

/// The authentication type to use for the connection
pub const AUTH_TYPE: &str = "adbc.salesforce.dc.auth_type";
/// Authentication types
pub mod auth_type {
    /// JWT Bearer flow (recommended for server-to-server)
    pub const JWT: &str = "adbc.salesforce.dc.auth_type.jwt_bearer";
    /// Username/Password flow
    pub const USERNAME_PASSWORD: &str = "adbc.salesforce.dc.auth_type.username_password";
}

// Default values
pub const DEFAULT_LOGIN_URL: &str = "https://login.salesforce.com";
pub const DEFAULT_API_VERSION: &str = "65.0";
pub const DEFAULT_REQUEST_TIMEOUT: &str = "30s";
