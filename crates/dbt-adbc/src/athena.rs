// Option key constants for the Athena ADBC driver (`adbc_driver_athena`).
//
// Unlike the Apache Arrow ADBC drivers (Snowflake, BigQuery, Salesforce) which use an
// `adbc.<driver>.sql.*` prefix, the Athena driver uses a plain `athena.*` prefix.
// Keys sourced from: https://github.com/dbt-labs/athena/blob/main/go/driver.go

/// ADBC option key for the AWS region (e.g. `"us-east-1"`).
pub const REGION: &str = "athena.region";

/// ADBC option key for the Glue/Athena catalog name.
/// Maps to the dbt profile field `database`.
pub const CATALOG: &str = "athena.catalog";

/// ADBC option key for the default schema (Glue database).
/// Maps to the dbt profile field `schema`.
pub const SCHEMA: &str = "athena.schema";

/// ADBC option key for the S3 output location for query results.
/// Must be an S3 URI, e.g. `"s3://my-bucket/prefix/"`.
/// Required.
pub const S3_STAGING_DIR: &str = "athena.s3_staging_dir";

/// ADBC option key for the Athena workgroup name (default: `"primary"`).
pub const WORK_GROUP: &str = "athena.work_group";

/// ADBC option key selecting the authentication method.
/// See [`auth_type`] for valid values.
pub const AUTH_TYPE: &str = "athena.auth_type";

pub mod auth_type {
    /// Use the default AWS credential chain (env vars, instance profile, IAM role, etc.).
    pub const IAM: &str = "iam";
    /// Authenticate with a static access key + secret.
    pub const ACCESS_KEY: &str = "access_key";
    /// Authenticate using a named AWS CLI profile.
    pub const PROFILE: &str = "profile";
}

/// ADBC option key for an IAM role assumed on top of the credentials of
/// [`AUTH_TYPE`]; every AWS call of the driver then uses the role.
/// Maps to the dbt profile field `assume_role_arn`.
pub const ROLE_ARN: &str = "athena.aws.role_arn";

/// ADBC option key for the AssumeRole external ID (`assume_role_external_id`).
pub const ROLE_EXTERNAL_ID: &str = "athena.aws.role_external_id";

/// ADBC option key for the AssumeRole session name (`assume_role_session_name`).
pub const ROLE_SESSION_NAME: &str = "athena.aws.role_session_name";

/// ADBC option key for the lifetime of the assumed credentials, as a Go duration
/// (`"3600s"`). Maps to `assume_role_duration_seconds`.
pub const ROLE_DURATION: &str = "athena.aws.role_duration";

/// ADBC option key for the attempts per AWS API request, the first one included.
pub const MAX_ATTEMPTS: &str = "athena.aws.max_attempts";

/// ADBC option key for the Athena API endpoint (`endpoint_url`).
pub const ENDPOINT_URL: &str = "athena.endpoint_url";

/// ADBC option key for the interval between query status checks, as a Go
/// duration (`"1s"`). Maps to `poll_interval`.
pub const POLL_INTERVAL: &str = "athena.poll_interval";

/// ADBC option key for the reruns of a query that fails with
/// `ICEBERG_COMMIT_ERROR` (`num_iceberg_retries`).
pub const ICEBERG_COMMIT_RETRIES: &str = "athena.iceberg_commit_retries";

/// ADBC option key for the AWS access key ID.
/// Only used when [`AUTH_TYPE`] = [`auth_type::ACCESS_KEY`].
pub const ACCESS_KEY_ID: &str = "athena.aws.access_key_id";

/// ADBC option key for the AWS secret access key.
/// Only used when [`AUTH_TYPE`] = [`auth_type::ACCESS_KEY`].
pub const SECRET_ACCESS_KEY: &str = "athena.aws.secret_access_key";

/// ADBC option key for the AWS session token.
/// Optional; used alongside [`ACCESS_KEY_ID`] and [`SECRET_ACCESS_KEY`]
/// when using temporary credentials.
pub const SESSION_TOKEN: &str = "athena.aws.session_token";

/// ADBC option key for the named AWS CLI profile.
/// Only used when [`AUTH_TYPE`] = [`auth_type::PROFILE`].
pub const PROFILE_NAME: &str = "athena.aws.profile";
