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

/// ADBC statement option naming a catalog or storage operation the driver runs in
/// place of the SQL text (`glue.get_table`, `s3.put_object`, ...). The response comes
/// back as one row with a utf8 `result` column holding the AWS API output as JSON.
/// See `operations.go` in github.com/dbt-labs/athena.
pub const OPERATION: &str = "athena.operation";

/// ADBC statement option carrying the operation's input, the AWS API input as JSON.
pub const OPERATION_PAYLOAD: &str = "athena.operation.payload";

/// Keys of the query statistics the driver puts in the result schema metadata.
pub mod schema_metadata {
    pub const QUERY_ID: &str = "ATHENA:query_id";
    pub const DATA_SCANNED_IN_BYTES: &str = "ATHENA:Statistics:DataScannedInBytes";
    /// Rows written by a CTAS, INSERT, MERGE or DELETE; absent for DDL.
    pub const UPDATE_COUNT: &str = "ATHENA:UpdateCount";
}
