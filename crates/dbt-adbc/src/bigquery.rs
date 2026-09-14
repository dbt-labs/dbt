// from https://github.com/apache/arrow-adbc/blob/9a10e6791db6d54b813fde4df3925c354822192e/go/adbc/driver/bigquery/driver.go#L31

pub const AUTH_TYPE: &str = "adbc.bigquery.sql.auth_type";
pub const API_ENDPOINT: &str = "adbc.bigquery.sql.api_endpoint";
pub const LOCATION: &str = "adbc.bigquery.sql.location";
pub const PROJECT_ID: &str = "adbc.bigquery.sql.project_id";
pub const DATASET_ID: &str = "adbc.bigquery.sql.dataset_id";
pub const TABLE_ID: &str = "adbc.bigquery.sql.table_id";

// values
pub mod auth_type {
    pub const DEFAULT: &str = "adbc.bigquery.sql.auth_type.auth_bigquery";
    pub const USER_AUTHENTICATION: &str = "adbc.bigquery.sql.auth_type.user_authentication";
    pub const TEMPORARY_ACCESS_TOKEN: &str = "adbc.bigquery.sql.auth_type.temporary_access_token";
    pub const JSON_CREDENTIAL_FILE: &str = "adbc.bigquery.sql.auth_type.json_credential_file";
    pub const JSON_CREDENTIAL_STRING: &str = "adbc.bigquery.sql.auth_type.json_credential_string";
    pub const EXTERNAL_ACCOUNT: &str = "adbc.bigquery.sql.auth_type.external_account";
}

pub const AUTH_CREDENTIALS: &str = "adbc.bigquery.sql.auth_credentials";
// one-time access token, the kind that refresh token will generate incidentally
pub const AUTH_ACCESS_TOKEN: &str = "adbc.bigquery.sql.auth.access_token";
pub const AUTH_CLIENT_ID: &str = "adbc.bigquery.sql.auth.client_id";
pub const AUTH_CLIENT_SECRET: &str = "adbc.bigquery.sql.auth.client_secret";
pub const AUTH_REFRESH_TOKEN: &str = "adbc.bigquery.sql.auth.refresh_token";
pub const AUTH_ACCESS_TOKEN_ENDPOINT: &str = "adbc.bigquery.sql.auth.access_token_endpoint";
pub const AUTH_ACCESS_TOKEN_SERVER_NAME: &str = "adbc.bigquery.sql.auth.access_token_server_name";
pub const AUTH_QUOTA_PROJECT: &str = "adbc.bigquery.sql.auth.quota_project";

pub const AUTH_EXTERNAL_ACCOUNT_AUDIENCE: &str = "adbc.bigquery.sql.auth.external_account.audience";
pub const AUTH_EXTERNAL_ACCOUNT_IMPERSONATION_URL: &str =
    "adbc.bigquery.sql.auth.external_account.impersonation_url";
pub const AUTH_EXTERNAL_ACCOUNT_REQUEST_URL: &str =
    "adbc.bigquery.sql.auth.external_account.request_url";
pub const AUTH_EXTERNAL_ACCOUNT_REQUEST_DATA: &str =
    "adbc.bigquery.sql.auth.external_account.request_data";

pub const IMPERSONATE_TARGET_PRINCIPAL: &str = "adbc.bigquery.sql.impersonate.target_principal";
pub const IMPERSONATE_SCOPES: &str = "adbc.bigquery.sql.impersonate.scopes";

pub const IMPERSONATE_DEFAULT_SCOPES: [&str; 4] = [
    "https://www.googleapis.com/auth/bigquery",
    "https://www.googleapis.com/auth/cloud-platform",
    "https://www.googleapis.com/auth/drive",
    "https://www.googleapis.com/auth/userinfo.email",
];

// The parameter mode specifies if the query uses positional syntax ("?")
// or the named syntax ("@p"). It is illegal to mix positional and named syntax.
// Default is QUERY_PARAMETER_MODE_POSITIONAL.
pub const QUERY_PARAMETER_MODE: &str = "adbc.bigquery.sql.query.parameter_mode";
// values
pub const QUERY_PARAMETER_MODE_NAMED: &str = "adbc.bigquery.sql.query.parameter_mode_named";
pub const QUERY_PARAMETER_MODE_POSITIONAL: &str =
    "adbc.bigquery.sql.query.parameter_mode_positional";

pub const QUERY_DESTINATION_TABLE: &str = "adbc.bigquery.sql.query.destination_table";
pub const QUERY_DEFAULT_PROJECT_ID: &str = "adbc.bigquery.sql.query.default_project_id";
pub const QUERY_DEFAULT_DATASET_ID: &str = "adbc.bigquery.sql.query.default_dataset_id";
pub const QUERY_CREATE_DISPOSITION: &str = "adbc.bigquery.sql.query.create_disposition";
pub const QUERY_WRITE_DISPOSITION: &str = "adbc.bigquery.sql.query.write_disposition";
pub const QUERY_LABELS: &str = "adbc.bigquery.sql.query.labels";
pub const QUERY_DISABLE_QUERY_CACHE: &str = "adbc.bigquery.sql.query.disable_query_cache"; // bool
pub const DISABLE_FLATTENED_RESULTS: &str = "adbc.bigquery.sql.query.disable_flattened_results"; // bool
pub const QUERY_ALLOW_LARGE_RESULTS: &str = "adbc.bigquery.sql.query.allow_large_results"; // bool

pub const QUERY_PRIORITY: &str = "adbc.bigquery.sql.query.priority"; // string
pub const QUERY_MAX_BILLING_TIER: &str = "adbc.bigquery.sql.query.max_billing_tier"; // i64
pub const QUERY_MAX_BYTES_BILLED: &str = "adbc.bigquery.sql.query.max_bytes_billed"; // i64
pub const QUERY_USE_LEGACY_SQL: &str = "adbc.bigquery.sql.query.use_legacy_sql"; // bool
pub const QUERY_DRY_RUN: &str = "adbc.bigquery.sql.query.dry_run"; // bool
pub const QUERY_CREATE_SESSION: &str = "adbc.bigquery.sql.query.create_session"; // bool
pub const QUERY_JOB_TIMEOUT: &str = "adbc.bigquery.sql.query.job_timeout"; // i64

pub const QUERY_RESULT_BUFFER_SIZE: &str = "adbc.bigquery.sql.query.result_buffer_size"; // i64
pub const QUERY_PREFETCH_CONCURRENCY: &str = "adbc.bigquery.sql.query.prefetch_concurrency"; // i64

pub const QUERY_LINK_FAILED_JOB: &str = "adbc.bigquery.sql.query.link_failed_job";

// values
pub const DEFAULT_QUERY_RESULT_BUFFER_SIZE: i64 = 200;
pub const DEFAULT_QUERY_PREFETCH_CONCURRENCY: i64 = 10;

pub const DEFAULT_CCESS_TOKEN_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/token";
pub const DEFAULT_ACCESS_TOKEN_SERVER_NAME: &str = "google.com";
pub const INGEST_FILE_DELIMITER: &str = "adbc.bigquery.ingest.csv_delimiter";
pub const INGEST_PATH: &str = "adbc.bigquery.ingest.csv_filepath";
pub const INGEST_SCHEMA: &str = "adbc.bigquery.ingest.csv_schema";
pub const UPDATE_TABLE_COLUMNS_DESCRIPTION: &str = "adbc.bigquery.table.update_columns_description";
pub const UPDATE_TABLE_COLUMNS_POLICY_TAGS: &str = "adbc.bigquery.table.update_columns_policy_tags";
pub const UPDATE_TABLE_DESCRIPTION: &str = "adbc.bigquery.table.update_description";
pub const UPDATE_DATASET_AUTHORIZE_VIEW_TO_DATASETS: &str =
    "adbc.bigquery.dataset.authorize_view_to_datasets";

pub const DATAPROC_REGION: &str = "adbc.bigquery.dataproc.compute_region";
pub const DATAPROC_PROJECT: &str = "adbc.bigquery.dataproc.project";
pub const DATAPROC_POOLING_TIMEOUT: &str = "adbc.bigquery.dataproc.pooling_timeout"; // i64
pub const CREATE_BATCH_REQ_PARENT: &str = "adbc.bigquery.create_batch.parent";
pub const CREATE_BATCH_REQ_BATCH_YML: &str = "adbc.bigquery.create_batch.batch_yml";
pub const CREATE_BATCH_REQ_BATCH_ID: &str = "adbc.bigquery.create_batch.batch_id";
pub const DATAPROC_SUBMIT_JOB_REQ_CLUSTER_NAME: &str =
    "adbc.bigquery.dataproc.submit_job.cluster_name";
pub const DATAPROC_SUBMIT_JOB_REQ_GCS_PATH: &str = "adbc.bigquery.dataproc.submit_job.gcs_path";
pub const WRITE_GCS_BUCKET: &str = "adbc.bigquery.write_gcs.bucket";
pub const WRITE_GCS_OBJECT_NAME: &str = "adbc.bigquery.write_gcs.object_name";
pub const WRITE_GCS_CONTENT: &str = "adbc.bigquery.write_gcs.content";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_PATH: &str =
    "adbc.bigquery.notebook_execute_job.gsc_path";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_FILE_NAME: &str =
    "adbc.bigquery.notebook_execute_job.model_file_name";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_MODEL_NAME: &str =
    "adbc.bigquery.notebook_execute_job.model_name";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_GSC_BUCKET: &str =
    "adbc.bigquery.notebook_execute_job.gsc_bucket";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_TEMPLATE_ID: &str =
    "adbc.bigquery.notebook_execute_job.template_id";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PARENT: &str =
    "adbc.bigquery.notebook_execute_job.parent";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_PROJECT: &str =
    "adbc.bigquery.notebook_execute_job.project";
pub const CREATE_NOTEBOOK_EXECUTE_JOB_REQ_REGION: &str =
    "adbc.bigquery.notebook_execute_job.region";
pub const COPY_TABLE_SOURCE: &str = "adbc.bigquery.copy_table.source";
pub const COPY_TABLE_DESTINATION: &str = "adbc.bigquery.copy_table.destination";
pub const COPY_TABLE_WRITE_DISPOSITION: &str = "adbc.bigquery.copy_table.write_disposition";

pub mod schema_metadata {
    pub const QUERY_ID: &str = "BIGQUERY:query_id";
    pub const STATEMENT_TYPE: &str = "BIGQUERY:Statistics:Query:StatementType";
    pub const TOTAL_BYTES_BILLED: &str = "BIGQUERY:Statistics:Query:TotalBytesBilled";
    pub const QUERY_TOTAL_BYTES_PROCESSED: &str = "BIGQUERY:Statistics:Query:TotalBytesProcessed";
    pub const JOB_TOTAL_BYTES_PROCESSED: &str = "BIGQUERY:Statistics:TotalBytesProcessed";
    pub const SLOT_MILLIS: &str = "BIGQUERY:Statistics:Query:SlotMillis";
    pub const NUM_DML_AFFECTED_ROWS: &str = "BIGQUERY:Statistics:Query:NumDMLAffectedRows";
}
