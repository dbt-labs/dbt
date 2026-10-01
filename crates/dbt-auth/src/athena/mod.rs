use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;
use dbt_adbc::{Backend, athena, database};

const DEFAULT_CATALOG: &str = "awsdatacatalog";
const DEFAULT_WORK_GROUP: &str = "primary";

#[derive(Debug)]
enum AthenaAuthIR<'a> {
    Iam,
    AccessKey {
        access_key_id: &'a str,
        secret_access_key: &'a str,
    },
    /// Static keys plus a session token (STS-issued temporary credentials).
    TemporaryCredentials {
        access_key_id: &'a str,
        secret_access_key: &'a str,
        session_token: &'a str,
    },
    Profile {
        profile_name: &'a str,
    },
}

impl<'a> AthenaAuthIR<'a> {
    fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::Iam => {
                builder.with_named_option(athena::AUTH_TYPE, athena::auth_type::IAM)?;
            }
            Self::AccessKey {
                access_key_id,
                secret_access_key,
            } => {
                builder.with_named_option(athena::AUTH_TYPE, athena::auth_type::ACCESS_KEY)?;
                builder.with_named_option(athena::ACCESS_KEY_ID, access_key_id)?;
                builder.with_named_option(athena::SECRET_ACCESS_KEY, secret_access_key)?;
            }
            Self::TemporaryCredentials {
                access_key_id,
                secret_access_key,
                session_token,
            } => {
                builder.with_named_option(athena::AUTH_TYPE, athena::auth_type::ACCESS_KEY)?;
                builder.with_named_option(athena::ACCESS_KEY_ID, access_key_id)?;
                builder.with_named_option(athena::SECRET_ACCESS_KEY, secret_access_key)?;
                builder.with_named_option(athena::SESSION_TOKEN, session_token)?;
            }
            Self::Profile { profile_name } => {
                builder.with_named_option(athena::AUTH_TYPE, athena::auth_type::PROFILE)?;
                builder.with_named_option(athena::PROFILE_NAME, profile_name)?;
            }
        }
        Ok(builder)
    }
}

/// dbt-athena Python profile fields the dbt Athena backend doesn't yet
/// honor. Both lists are kept separate so the policy split (auth-affecting vs
/// non-auth) is easy to revisit, but during initial rollout both are rejected.
/// Tracked alongside Part 6 (#9460): full Glue/Iceberg/STS support.
const HARD_ERROR_UNSUPPORTED_FIELDS: &[&str] = &[
    "assume_role_arn",
    "assume_role_external_id",
    "assume_role_session_name",
    "assume_role_duration_seconds",
];

const NOT_YET_SUPPORTED_FIELDS: &[&str] = &[
    "endpoint_url",
    "skip_workgroup_check",
    "s3_data_dir",
    "s3_data_naming",
    "s3_tmp_table_dir",
    "poll_interval",
    "debug_query_state",
    "num_retries",
    "num_boto3_retries",
    "num_iceberg_retries",
    "spark_work_group",
    "seed_s3_upload_args",
    "lf_tags_database",
];

fn reject_unsupported_fields(config: &AdapterConfig) -> Result<(), AuthError> {
    // `contains_key` (vs string-only checks) so unsupported numeric/boolean
    // YAML values (e.g. `assume_role_duration_seconds: 3600`,
    // `skip_workgroup_check: true`) are still caught.
    for field in HARD_ERROR_UNSUPPORTED_FIELDS
        .iter()
        .chain(NOT_YET_SUPPORTED_FIELDS.iter())
    {
        if config.contains_key(field) {
            return Err(AuthError::config(format!(
                "Athena profile field '{field}' is not yet supported by the dbt Athena \
                 backend; please remove it from your profile.",
            )));
        }
    }
    Ok(())
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<AthenaAuthIR<'a>, AuthError> {
    reject_unsupported_fields(config)?;

    // Auth path is inferred from which credential fields are set, matching
    // dbt-athena Python (which has no explicit `method` field):
    //   aws_access_key_id  + aws_session_token  -> TemporaryCredentials
    //   aws_access_key_id  alone                -> AccessKey
    //   aws_profile_name                        -> Profile
    //   none                                    -> Iam (default chain)
    let access_key_id = config.get_str("aws_access_key_id");
    let session_token = config.get_str("aws_session_token");
    let profile_name = config.get_str("aws_profile_name");

    if let Some(access_key_id) = access_key_id {
        if access_key_id.is_empty() {
            return Err(AuthError::config(
                "Athena 'aws_access_key_id' must not be empty",
            ));
        }
        let secret_access_key = config.require_str("aws_secret_access_key").map_err(|_| {
            AuthError::config(
                "Athena auth requires 'aws_secret_access_key' when 'aws_access_key_id' is set",
            )
        })?;
        if secret_access_key.is_empty() {
            return Err(AuthError::config(
                "Athena 'aws_secret_access_key' must not be empty",
            ));
        }
        let session_token = session_token.filter(|t| !t.is_empty());
        return Ok(if let Some(session_token) = session_token {
            AthenaAuthIR::TemporaryCredentials {
                access_key_id,
                secret_access_key,
                session_token,
            }
        } else {
            AthenaAuthIR::AccessKey {
                access_key_id,
                secret_access_key,
            }
        });
    }

    if let Some(profile_name) = profile_name {
        if profile_name.is_empty() {
            return Err(AuthError::config(
                "Athena 'aws_profile_name' must not be empty",
            ));
        }
        return Ok(AthenaAuthIR::Profile { profile_name });
    }

    Ok(AthenaAuthIR::Iam)
}

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    let region = config
        .require_str("region_name")
        .map_err(|_| AuthError::config("Athena requires 'region_name' in profile configuration"))?;
    builder.with_named_option(athena::REGION, region)?;

    let s3_staging_dir = config.require_str("s3_staging_dir").map_err(|_| {
        AuthError::config("Athena requires 's3_staging_dir' in profile configuration")
    })?;
    builder.with_named_option(athena::S3_STAGING_DIR, s3_staging_dir)?;

    // dbt-athena's `database` profile field maps to the Athena/Glue catalog name
    // (not a Postgres-style database). `catalog` is accepted as an alias to match
    // dbt-athena Python's `_ALIASES = {"catalog": "database"}`; `database` wins
    // when both are set.
    let catalog = config
        .get_str("database")
        .or_else(|| config.get_str("catalog"))
        .unwrap_or(DEFAULT_CATALOG);
    builder.with_named_option(athena::CATALOG, catalog)?;

    let schema = config
        .require_str("schema")
        .map_err(|_| AuthError::config("Athena requires 'schema' in profile configuration"))?;
    builder.with_named_option(athena::SCHEMA, schema)?;

    let work_group = config.get_str("work_group").unwrap_or(DEFAULT_WORK_GROUP);
    builder.with_named_option(athena::WORK_GROUP, work_group)?;

    Ok(builder)
}

pub struct AthenaAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl AthenaAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for AthenaAuth {
    fn backend(&self) -> Backend {
        Backend::Athena
    }

    fn configure(&self, config: &AdapterConfig) -> Result<database::Builder, AuthError> {
        auth_configure_pipeline!(self, &config, parse_auth, apply_connection_args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_options::other_option_value;
    use dbt_yaml::Mapping;

    fn base_required() -> Mapping {
        Mapping::from_iter([
            ("region_name".into(), "us-east-1".into()),
            ("s3_staging_dir".into(), "s3://my-bucket/athena/".into()),
            ("schema".into(), "analytics".into()),
        ])
    }

    #[test]
    fn test_iam_default_method() {
        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(base_required()))
            .expect("configure");

        assert_eq!(other_option_value(&builder, athena::AUTH_TYPE), Some("iam"));
        assert_eq!(
            other_option_value(&builder, athena::REGION),
            Some("us-east-1")
        );
        assert_eq!(
            other_option_value(&builder, athena::S3_STAGING_DIR),
            Some("s3://my-bucket/athena/")
        );
        assert_eq!(
            other_option_value(&builder, athena::SCHEMA),
            Some("analytics")
        );
        assert_eq!(
            other_option_value(&builder, athena::CATALOG),
            Some(DEFAULT_CATALOG)
        );
        assert_eq!(
            other_option_value(&builder, athena::WORK_GROUP),
            Some(DEFAULT_WORK_GROUP)
        );
    }

    #[test]
    fn test_access_key_inferred() {
        let mut config = base_required();
        config.insert("aws_access_key_id".into(), "AKIAEXAMPLE".into());
        config.insert("aws_secret_access_key".into(), "secret".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::AUTH_TYPE),
            Some("access_key")
        );
        assert_eq!(
            other_option_value(&builder, athena::ACCESS_KEY_ID),
            Some("AKIAEXAMPLE")
        );
        assert_eq!(
            other_option_value(&builder, athena::SECRET_ACCESS_KEY),
            Some("secret")
        );
        assert!(other_option_value(&builder, athena::SESSION_TOKEN).is_none());
    }

    #[test]
    fn test_temporary_credentials_inferred_from_session_token() {
        let mut config = base_required();
        config.insert("aws_access_key_id".into(), "AKIAEXAMPLE".into());
        config.insert("aws_secret_access_key".into(), "secret".into());
        config.insert("aws_session_token".into(), "token-xyz".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::SESSION_TOKEN),
            Some("token-xyz")
        );
    }

    #[test]
    fn test_profile_inferred() {
        let mut config = base_required();
        config.insert("aws_profile_name".into(), "my-profile".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::AUTH_TYPE),
            Some("profile")
        );
        assert_eq!(
            other_option_value(&builder, athena::PROFILE_NAME),
            Some("my-profile")
        );
    }

    #[test]
    fn test_custom_catalog_and_work_group() {
        let mut config = base_required();
        config.insert("database".into(), "my_catalog".into());
        config.insert("work_group".into(), "analytics-wg".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::CATALOG),
            Some("my_catalog")
        );
        assert_eq!(
            other_option_value(&builder, athena::WORK_GROUP),
            Some("analytics-wg")
        );
    }

    #[test]
    fn test_catalog_alias_maps_to_database() {
        let mut config = base_required();
        config.insert("catalog".into(), "my_catalog".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::CATALOG),
            Some("my_catalog")
        );
    }

    #[test]
    fn test_database_takes_precedence_over_catalog_alias() {
        let mut config = base_required();
        config.insert("database".into(), "canonical".into());
        config.insert("catalog".into(), "alias_value".into());

        let builder = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure");

        assert_eq!(
            other_option_value(&builder, athena::CATALOG),
            Some("canonical")
        );
    }

    #[test]
    fn test_missing_region_returns_error() {
        let mut config = base_required();
        config.remove("region_name");

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("should require region_name");
        assert!(err.msg().contains("region_name"), "got: {}", err.msg());
    }

    #[test]
    fn test_missing_s3_staging_dir_returns_error() {
        let mut config = base_required();
        config.remove("s3_staging_dir");

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("should require s3_staging_dir");
        assert!(err.msg().contains("s3_staging_dir"), "got: {}", err.msg());
    }

    #[test]
    fn test_missing_schema_returns_error() {
        let mut config = base_required();
        config.remove("schema");

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("should require schema");
        assert!(err.msg().contains("schema"), "got: {}", err.msg());
    }

    #[test]
    fn test_access_key_id_without_secret_returns_error() {
        let mut config = base_required();
        config.insert("aws_access_key_id".into(), "AKIAEXAMPLE".into());

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("should require aws_secret_access_key");
        assert!(
            err.msg().contains("aws_secret_access_key"),
            "got: {}",
            err.msg()
        );
    }

    #[test]
    fn test_empty_access_key_id_returns_error() {
        let mut config = base_required();
        config.insert("aws_access_key_id".into(), "".into());
        config.insert("aws_secret_access_key".into(), "secret".into());

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("empty access_key_id should error");
        assert!(
            err.msg().contains("aws_access_key_id"),
            "got: {}",
            err.msg()
        );
    }

    #[test]
    fn test_empty_profile_name_returns_error() {
        let mut config = base_required();
        config.insert("aws_profile_name".into(), "".into());

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("empty profile name should error");
        assert!(err.msg().contains("aws_profile_name"), "got: {}", err.msg());
    }

    #[test]
    fn test_assume_role_arn_returns_error() {
        let mut config = base_required();
        config.insert(
            "assume_role_arn".into(),
            "arn:aws:iam::123456789012:role/MyRole".into(),
        );

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("assume_role_arn should be rejected");
        assert!(err.msg().contains("assume_role_arn"), "got: {}", err.msg());
    }

    #[test]
    fn test_numeric_assume_role_duration_returns_error() {
        // Ensures non-string YAML values for unsupported fields are still rejected.
        let mut config = base_required();
        config.insert(
            "assume_role_duration_seconds".into(),
            dbt_yaml::Value::number(3600i64.into()),
        );

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("numeric assume_role_duration_seconds should be rejected");
        assert!(
            err.msg().contains("assume_role_duration_seconds"),
            "got: {}",
            err.msg()
        );
    }

    #[test]
    fn test_boolean_skip_workgroup_check_returns_error() {
        let mut config = base_required();
        config.insert("skip_workgroup_check".into(), dbt_yaml::Value::bool(true));

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("boolean skip_workgroup_check should be rejected");
        assert!(
            err.msg().contains("skip_workgroup_check"),
            "got: {}",
            err.msg()
        );
    }

    #[test]
    fn test_s3_data_dir_returns_error() {
        let mut config = base_required();
        config.insert("s3_data_dir".into(), "s3://mybucket/data/".into());

        let err = AthenaAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect_err("s3_data_dir should be rejected");
        assert!(err.msg().contains("s3_data_dir"), "got: {}", err.msg());
    }
}
