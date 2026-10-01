//! Option keys for the lake compute ADBC driver.
//!
//! These mirror the keys understood by the `adbc_driver_dbt` driver crate (its
//! `options` module). They are kept in sync by convention — the same way the
//! other backend modules (e.g. [`crate::snowflake`]) mirror the keys expected by
//! their respective drivers — so the `dbt-auth` `lake_compute` module can configure a
//! [`crate::database::Builder`] without depending on the driver crate.

// Names of Database options --------------------------------------------

/// Lake compute API base URL (required).
pub const BASE_URL: &str = "adbc.dbt.base_url";
/// Organization to scope requests to.
pub const ORGANIZATION: &str = "adbc.dbt.organization";
/// Default warehouse alias for queries.
pub const WAREHOUSE: &str = "adbc.dbt.warehouse";
/// Default worker-pool affinity tag.
pub const AFFINITY: &str = "adbc.dbt.affinity";
/// Default per-query timeout, in seconds.
pub const TIMEOUT_SECONDS: &str = "adbc.dbt.timeout_seconds";
/// Default source SQL dialect for transpilation.
pub const DIALECT: &str = "adbc.dbt.dialect";

/// Authentication method: one of [`auth_type`].
pub const AUTH_TYPE: &str = "adbc.dbt.auth.method";
/// API key, sent as `X-API-Key`.
pub const AUTH_API_KEY: &str = "adbc.dbt.auth.api_key";
/// Static bearer token.
pub const AUTH_TOKEN: &str = "adbc.dbt.auth.token";

pub const CATALOG_BUNDLE: &str = "adbc.dbt.catalog_bundle";

/// Statement option. Read-only. Non-fatal warnings emitted by the backend for
/// the most recently executed statement (e.g. an export-limit truncation
/// notice), newline-joined. Mirrors `adbc_driver_dbt::options::LAST_WARNINGS`.
pub const LAST_WARNINGS: &str = "adbc.dbt.last_warnings";

/// Statement option. Read-only. The LakeCompute job id of the most recently
/// submitted statement, if any. Mirrors
/// `adbc_driver_dbt::options::LAST_QUERY_ID`.
pub const LAST_QUERY_ID: &str = "adbc.dbt.last_query_id";

/// Statement option, write-only, mutually exclusive with setting a SQL query
/// on the same statement. The LakeCompute query id of a previously completed
/// query whose result should be fetched directly instead of submitting new
/// SQL -- no worker/Temporal round trip, just a Postgres-backed status lookup
/// plus the same object-store fetch used for normal exports. Mirrors
/// `adbc_driver_dbt::options::RESULT_QUERY_ID`.
pub const RESULT_QUERY_ID: &str = "adbc.dbt.result_query_id";

/// Schema metadata keys used to convey per-statement, backend-reported
/// information that isn't part of the Arrow schema proper.
pub mod schema_metadata {
    /// Non-fatal warnings from the most recently executed statement (e.g. an
    /// export-limit truncation notice), newline-joined. Set by
    /// `adbc_execute_with_options` (from [`super::LAST_WARNINGS`]) so it can
    /// survive the trip through `adbc_execute_with_options`'s `RecordBatch`
    /// return value; read back by `AdapterResponse::from_record_batch`.
    pub const WARNINGS: &str = "LAKE_COMPUTE_WARNINGS";

    /// The LakeCompute job id of the executed statement. Set by
    /// `adbc_execute_with_options` (from [`super::LAST_QUERY_ID`]) so it can
    /// survive the trip through `adbc_execute_with_options`'s `RecordBatch`
    /// return value; read back by `AdapterResponse::from_record_batch` via
    /// `query_id_from_record_batch`.
    pub const QUERY_ID: &str = "LAKE_COMPUTE_QUERY_ID";
}

/// Okta authorization endpoint.
pub const OKTA_AUTH_URL: &str = "adbc.dbt.auth.okta.auth_url";
/// Okta token endpoint.
pub const OKTA_TOKEN_URL: &str = "adbc.dbt.auth.okta.token_url";
/// Okta OAuth client id.
pub const OKTA_CLIENT_ID: &str = "adbc.dbt.auth.okta.client_id";

/// Long-lived Fivetran personal access token (`dct_…`).
pub const FIVETRAN_CREDENTIAL: &str = "adbc.dbt.auth.fivetran.credential";
/// Fivetran public API base URL the credential is exchanged against.
pub const FIVETRAN_API_URL: &str = "adbc.dbt.auth.fivetran.api_url";

/// Accepted values for [`AUTH_TYPE`].
pub mod auth_type {
    /// Send an `X-API-Key` header.
    pub const API_KEY: &str = "api_key";
    /// Send a static bearer token.
    pub const TOKEN: &str = "token";
    /// Run the interactive Okta PKCE browser flow.
    pub const OKTA_BROWSER: &str = "okta_browser";
    /// Exchange a Fivetran PAT for a short-lived bearer token.
    pub const FIVETRAN: &str = "fivetran";
}
