use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum TokenServiceError {
    #[error("Missing required key in token_endpoint: {0}")]
    MissingKey(String),

    #[error("Unsupported identity provider type: {0}. Select 'okta' or 'entra'.")]
    UnsupportedProvider(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct TokenEndpoint {
    pub r#type: String,
    pub request_url: String,
    pub request_data: String,
    #[serde(flatten)]
    pub other_params: HashMap<String, String>,
}

impl TokenEndpoint {
    pub fn validate(&self) -> Result<(), TokenServiceError> {
        for key in ["type", "request_url", "request_data"] {
            if (key == "type" && self.r#type.is_empty())
                || (key == "request_url" && self.request_url.is_empty())
                || (key == "request_data" && self.request_data.is_empty())
            {
                return Err(TokenServiceError::MissingKey(key.to_string()));
            }
        }
        Ok(())
    }
}

/// Validates `endpoint` and returns the headers the driver should send when it
/// calls the endpoint's token URL, for the identity providers we support today.
pub fn token_endpoint_headers(
    endpoint: &TokenEndpoint,
) -> Result<HashMap<String, String>, TokenServiceError> {
    endpoint.validate()?;
    match endpoint.r#type.to_lowercase().as_str() {
        "okta" => okta_headers(endpoint),
        "entra" => Ok(entra_headers()),
        other => Err(TokenServiceError::UnsupportedProvider(other.to_string())),
    }
}

fn okta_headers(endpoint: &TokenEndpoint) -> Result<HashMap<String, String>, TokenServiceError> {
    let creds = endpoint
        .other_params
        .get("idp_auth_credentials")
        .ok_or_else(|| {
            TokenServiceError::MissingKey(
                "idp_auth_credentials (Base64 client_id:client_secret)".into(),
            )
        })?
        .trim();

    Ok(HashMap::from([
        ("accept".into(), "application/json".into()),
        ("authorization".into(), format!("Basic {}", creds)),
        (
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        ),
    ]))
}

fn entra_headers() -> HashMap<String, String> {
    HashMap::from([
        ("accept".into(), "application/json".into()),
        (
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        ),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn endpoint(request_url: String) -> TokenEndpoint {
        TokenEndpoint {
            r#type: "okta".into(),
            request_url,
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::from([(
                "idp_auth_credentials".into(),
                "Q2xpZW50SWQ6Q2xpZW50U2VjcmV0".into(),
            )]),
        }
    }

    #[test]
    fn token_endpoint_validate_requires_all_fields() {
        for missing_key in ["type", "request_url", "request_data"] {
            let mut endpoint = endpoint("http://127.0.0.1/token".into());
            match missing_key {
                "type" => endpoint.r#type.clear(),
                "request_url" => endpoint.request_url.clear(),
                "request_data" => endpoint.request_data.clear(),
                _ => unreachable!(),
            }

            let err = endpoint.validate().expect_err("validation should fail");
            assert!(matches!(err, TokenServiceError::MissingKey(ref key) if key == missing_key));
        }
    }

    #[test]
    fn token_endpoint_headers_rejects_unknown_provider() {
        let err = token_endpoint_headers(&TokenEndpoint {
            r#type: "github".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        })
        .expect_err("unknown provider should fail");

        assert!(matches!(err, TokenServiceError::UnsupportedProvider(ref ty) if ty == "github"));
    }

    #[test]
    fn okta_headers_sets_basic_auth_and_trims_credentials() {
        let headers = token_endpoint_headers(&TokenEndpoint {
            r#type: "okta".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::from([(
                "idp_auth_credentials".into(),
                "  Q2xpZW50SWQ6Q2xpZW50U2VjcmV0  ".into(),
            )]),
        })
        .expect("okta headers");

        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("authorization").map(String::as_str),
            Some("Basic Q2xpZW50SWQ6Q2xpZW50U2VjcmV0")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/x-www-form-urlencoded")
        );
    }

    #[test]
    fn okta_headers_requires_credentials() {
        let err = token_endpoint_headers(&TokenEndpoint {
            r#type: "okta".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        })
        .expect_err("missing credentials should fail");

        assert!(
            matches!(err, TokenServiceError::MissingKey(ref key) if key.contains("idp_auth_credentials"))
        );
    }

    #[test]
    fn entra_headers_sets_form_headers() {
        let headers = token_endpoint_headers(&TokenEndpoint {
            r#type: "entra".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        })
        .expect("entra headers");

        assert_eq!(
            headers.get("accept").map(String::as_str),
            Some("application/json")
        );
        assert_eq!(
            headers.get("content-type").map(String::as_str),
            Some("application/x-www-form-urlencoded")
        );
        assert!(!headers.contains_key("authorization"));
    }
}
