use crate::{AdapterConfig, Auth, AuthError, AuthWarningPrinter, auth_configure_pipeline};
use database::Builder as DatabaseBuilder;

use dbt_adbc::{Backend, database, lake_compute};

#[derive(Debug)]
enum LakeComputeAuthIR<'a> {
    Token {
        token: &'a str,
    },
    // not yet verified
    ApiKey {
        api_key: &'a str,
    },
    OktaBrowser {
        auth_url: Option<&'a str>,
        token_url: Option<&'a str>,
        client_id: Option<&'a str>,
    },
    Fivetran {
        credential: &'a str,
        auth_url: Option<&'a str>,
    },
}

impl<'a> LakeComputeAuthIR<'a> {
    fn apply(
        self,
        mut builder: DatabaseBuilder,
        _warning_printer: &dyn AuthWarningPrinter,
    ) -> Result<DatabaseBuilder, AuthError> {
        match self {
            Self::ApiKey { api_key } => {
                builder
                    .with_named_option(lake_compute::AUTH_TYPE, lake_compute::auth_type::API_KEY)?;
                builder.with_named_option(lake_compute::AUTH_API_KEY, api_key)?;
            }
            Self::Token { token } => {
                builder
                    .with_named_option(lake_compute::AUTH_TYPE, lake_compute::auth_type::TOKEN)?;
                builder.with_named_option(lake_compute::AUTH_TOKEN, token)?;
            }
            Self::OktaBrowser {
                auth_url,
                token_url,
                client_id,
            } => {
                builder.with_named_option(
                    lake_compute::AUTH_TYPE,
                    lake_compute::auth_type::OKTA_BROWSER,
                )?;
                if let Some(v) = auth_url {
                    builder.with_named_option(lake_compute::OKTA_AUTH_URL, v)?;
                }
                if let Some(v) = token_url {
                    builder.with_named_option(lake_compute::OKTA_TOKEN_URL, v)?;
                }
                if let Some(v) = client_id {
                    builder.with_named_option(lake_compute::OKTA_CLIENT_ID, v)?;
                }
            }
            Self::Fivetran {
                credential,
                auth_url,
            } => {
                builder.with_named_option(
                    lake_compute::AUTH_TYPE,
                    lake_compute::auth_type::FIVETRAN,
                )?;
                builder.with_named_option(lake_compute::FIVETRAN_CREDENTIAL, credential)?;
                // Absent, the driver exchanges against production Fivetran.
                if let Some(v) = auth_url {
                    builder.with_named_option(lake_compute::FIVETRAN_API_URL, v)?;
                }
            }
        }
        Ok(builder)
    }
}

fn parse_auth<'a>(
    config: &'a AdapterConfig,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<LakeComputeAuthIR<'a>, AuthError> {
    // Absent `method` defaults to `'token'` -- the common case for a secondary
    // (non-default) `lake_compute` output, which otherwise needs no auth
    // config beyond a token. See fs#13653.
    let method = config
        .get_str("method")
        .unwrap_or(lake_compute::auth_type::TOKEN);
    match method {
        lake_compute::auth_type::API_KEY => Ok(LakeComputeAuthIR::ApiKey {
            api_key: config.require_str("api_key")?,
        }),
        lake_compute::auth_type::TOKEN => Ok(LakeComputeAuthIR::Token {
            token: config.require_str("token")?,
        }),
        lake_compute::auth_type::OKTA_BROWSER => Ok(LakeComputeAuthIR::OktaBrowser {
            auth_url: config.get_str("okta_auth_url"),
            token_url: config.get_str("okta_token_url"),
            client_id: config.get_str("okta_client_id"),
        }),
        lake_compute::auth_type::FIVETRAN => Ok(LakeComputeAuthIR::Fivetran {
            credential: config.require_str("fivetran_credential")?,
            auth_url: config.get_str("fivetran_auth_url"),
        }),
        other => Err(AuthError::config(format!(
            "unknown ALT auth method '{other}'; expected one of: '{}', '{}', '{}', '{}'",
            lake_compute::auth_type::API_KEY,
            lake_compute::auth_type::TOKEN,
            lake_compute::auth_type::OKTA_BROWSER,
            lake_compute::auth_type::FIVETRAN
        ))),
    }
}

/// Production dbt Compute service, used when `base_url` is absent from both
/// the profile and the `DBT_COMPUTE_BASE_URL` environment variable.
const DEFAULT_BASE_URL: &str = "https://api.lake-compute.fivetran.com/";

fn apply_connection_args(
    config: &AdapterConfig,
    mut builder: DatabaseBuilder,
    _warning_printer: &dyn AuthWarningPrinter,
) -> Result<DatabaseBuilder, AuthError> {
    // Each branch hands its own natively-typed value (borrowed profile
    // string, owned env var, or `'static` default) straight to
    // `with_named_option`, which is the actual `impl Into<String>` boundary
    // -- avoids unifying into an owned `String` any earlier than that.
    if let Some(base_url) = config.get_str("base_url") {
        builder.with_named_option(lake_compute::BASE_URL, base_url)?;
    } else if let Ok(base_url) = std::env::var("DBT_COMPUTE_BASE_URL") {
        builder.with_named_option(lake_compute::BASE_URL, base_url)?;
    } else {
        builder.with_named_option(lake_compute::BASE_URL, DEFAULT_BASE_URL)?;
    }

    if let Some(bundle) = config.get_str("catalog_bundle") {
        builder.with_named_option(lake_compute::CATALOG_BUNDLE, bundle)?;
    }

    Ok(builder)
}

pub struct LakeComputeAuth {
    pub warning_printer: Box<dyn AuthWarningPrinter>,
}

impl LakeComputeAuth {
    pub fn new(warning_printer: Box<dyn AuthWarningPrinter>) -> Self {
        Self { warning_printer }
    }
}

impl Auth for LakeComputeAuth {
    fn backend(&self) -> Backend {
        Backend::LakeCompute
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

    fn configure(config: Mapping) -> DatabaseBuilder {
        LakeComputeAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(config))
            .expect("configure")
    }

    #[test]
    fn base_url_and_api_key() {
        let builder = configure(Mapping::from_iter([
            ("base_url".into(), "https://compute.example".into()),
            ("method".into(), "api_key".into()),
            ("api_key".into(), "secret-key".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, lake_compute::BASE_URL),
            Some("https://compute.example")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_TYPE),
            Some("api_key")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_API_KEY),
            Some("secret-key")
        );
    }

    #[test]
    fn missing_method_defaults_to_token() {
        let builder = configure(Mapping::from_iter([
            ("base_url".into(), "https://compute.example".into()),
            ("token".into(), "secret-token".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_TYPE),
            Some("token")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_TOKEN),
            Some("secret-token")
        );
    }

    #[test]
    fn okta_browser_method() {
        let builder = configure(Mapping::from_iter([
            ("base_url".into(), "https://compute.example".into()),
            ("method".into(), "okta_browser".into()),
            ("okta_client_id".into(), "client-123".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_TYPE),
            Some("okta_browser")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::OKTA_CLIENT_ID),
            Some("client-123")
        );
    }

    #[test]
    fn fivetran_method() {
        let builder = configure(Mapping::from_iter([
            ("base_url".into(), "https://compute.example".into()),
            ("method".into(), "fivetran".into()),
            ("fivetran_credential".into(), "dct_secret".into()),
        ]));
        assert_eq!(
            other_option_value(&builder, lake_compute::AUTH_TYPE),
            Some("fivetran")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::FIVETRAN_CREDENTIAL),
            Some("dct_secret")
        );
        assert_eq!(
            other_option_value(&builder, lake_compute::FIVETRAN_API_URL),
            None
        );
    }

    #[test]
    fn fivetran_method_requires_a_credential() {
        let err = LakeComputeAuth::new(Box::new(crate::NoopAuthWarningPrinter))
            .configure(&AdapterConfig::new(Mapping::from_iter([(
                "method".into(),
                "fivetran".into(),
            )])))
            .expect_err("missing credential");
        assert!(
            format!("{err:?}").contains("fivetran_credential"),
            "{err:?}"
        );
    }
}
