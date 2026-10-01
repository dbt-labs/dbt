use std::collections::HashMap;
use std::io::Read;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;
use ureq::http::header::{HeaderName, HeaderValue};
use ureq::{Agent, Body, Error as UreqError, http};

#[derive(Clone, Debug)]
struct CachedToken {
    value: String,
    expires_at: Instant,
}

static TOKEN_CACHE: Lazy<Arc<RwLock<Option<CachedToken>>>> =
    Lazy::new(|| Arc::new(RwLock::new(None)));

static TOKEN_FETCH_LOCK: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

#[derive(Debug, Error)]
pub enum TokenServiceError {
    #[error("Missing required key in token_endpoint: {0}")]
    MissingKey(String),

    #[error("Rate limit reached. Consider lowering concurrency or increasing IdP limits.")]
    RateLimited,

    #[error("HTTP request failed: {0}")]
    Http(UreqError),

    #[error("Invalid header name: {0}")]
    InvalidHeaderName(String),

    #[error("Invalid header value for '{0}': {1}")]
    InvalidHeaderValue(String, String),

    #[error("Unsupported identity provider type: {0}. Select 'okta' or 'entra'.")]
    UnsupportedProvider(String),

    #[error("Token not found in response body")]
    MissingToken,

    #[error("Token service synchronization failed: {0}")]
    Synchronization(String),
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

pub trait TokenService: Send + Sync {
    fn handle_request(&self) -> Result<String, TokenServiceError>;
    fn build_headers(&self) -> Result<HashMap<String, String>, TokenServiceError>;
}

pub struct BaseTokenService {
    agent: Agent,
    endpoint: TokenEndpoint,
}

#[derive(Debug, PartialEq, Eq)]
struct ParsedTokenResponse {
    token: String,
    expires_in: u64,
}

impl BaseTokenService {
    pub fn new(endpoint: TokenEndpoint) -> Result<Self, TokenServiceError> {
        endpoint.validate()?;
        let config = Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build();
        let agent = Agent::new_with_config(config);
        Ok(Self { agent, endpoint })
    }

    pub fn post(&self, headers: HashMap<String, String>) -> Result<String, TokenServiceError> {
        // --- Check cache first ---
        {
            let cache = TOKEN_CACHE.read().map_err(|_| {
                TokenServiceError::Synchronization("token cache read lock poisoned".into())
            })?;
            if let Some(ref token) = *cache
                && Instant::now() < token.expires_at
            {
                return Ok(token.value.clone());
            }
        }

        let _lock = TOKEN_FETCH_LOCK
            .lock()
            .map_err(|_| TokenServiceError::Synchronization("token fetch lock poisoned".into()))?;

        {
            let cache = TOKEN_CACHE.read().map_err(|_| {
                TokenServiceError::Synchronization("token cache read lock poisoned".into())
            })?;
            if let Some(ref token) = *cache
                && Instant::now() < token.expires_at
            {
                return Ok(token.value.clone());
            }
        }

        // --- Perform the request ---
        validate_headers(&headers)?;
        let mut request = self.agent.post(&self.endpoint.request_url);
        for (k, v) in &headers {
            request = request.header(k, v);
        }
        let response = request
            .send(self.endpoint.request_data.as_str())
            .map_err(TokenServiceError::Http)?;

        classify_response_status(response.status().as_u16())?;
        let bytes = Self::read_body(response)?;
        let parsed = parse_token_response(&bytes)?;

        let mut cache = TOKEN_CACHE.write().map_err(|_| {
            TokenServiceError::Synchronization("token cache write lock poisoned".into())
        })?;
        *cache = Some(CachedToken {
            value: parsed.token.clone(),
            expires_at: Instant::now() + Duration::from_secs(parsed.expires_in),
        });

        Ok(parsed.token)
    }

    fn read_body(response: http::Response<Body>) -> Result<Vec<u8>, TokenServiceError> {
        let mut reader = response.into_body().into_reader();
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .map_err(|err| TokenServiceError::Http(UreqError::Io(err)))?;
        Ok(bytes)
    }
}

fn validate_headers(headers: &HashMap<String, String>) -> Result<(), TokenServiceError> {
    for (k, v) in headers {
        HeaderName::from_bytes(k.as_bytes())
            .map_err(|_| TokenServiceError::InvalidHeaderName(k.clone()))?;
        HeaderValue::from_str(v)
            .map_err(|_| TokenServiceError::InvalidHeaderValue(k.clone(), v.clone()))?;
    }

    Ok(())
}

fn classify_response_status(status: u16) -> Result<(), TokenServiceError> {
    if status == 429 {
        return Err(TokenServiceError::RateLimited);
    }
    if status >= 400 {
        return Err(TokenServiceError::Http(UreqError::StatusCode(status)));
    }

    Ok(())
}

fn parse_token_response(bytes: &[u8]) -> Result<ParsedTokenResponse, TokenServiceError> {
    let json: Value = serde_json::from_slice(bytes).map_err(|_| TokenServiceError::MissingToken)?;

    let token = json
        .get("access_token")
        .and_then(|v| v.as_str())
        .ok_or(TokenServiceError::MissingToken)?
        .to_string();

    let expires_in = json
        .get("expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);

    Ok(ParsedTokenResponse { token, expires_in })
}

pub struct OktaIdpTokenService {
    base: BaseTokenService,
}

impl TokenService for OktaIdpTokenService {
    fn handle_request(&self) -> Result<String, TokenServiceError> {
        let headers = self.build_headers()?;
        self.base.post(headers)
    }

    fn build_headers(&self) -> Result<HashMap<String, String>, TokenServiceError> {
        let creds = self
            .base
            .endpoint
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
}

pub struct EntraIdpTokenService {
    base: BaseTokenService,
}

impl TokenService for EntraIdpTokenService {
    fn handle_request(&self) -> Result<String, TokenServiceError> {
        let headers = self.build_headers()?;
        self.base.post(headers)
    }

    fn build_headers(&self) -> Result<HashMap<String, String>, TokenServiceError> {
        Ok(HashMap::from([
            ("accept".into(), "application/json".into()),
            (
                "content-type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
        ]))
    }
}

pub fn create_token_service_client(
    endpoint: TokenEndpoint,
) -> Result<Box<dyn TokenService + Send + Sync>, TokenServiceError> {
    match endpoint.r#type.to_lowercase().as_str() {
        "okta" => Ok(Box::new(OktaIdpTokenService {
            base: BaseTokenService::new(endpoint)?,
        })),
        "entra" => Ok(Box::new(EntraIdpTokenService {
            base: BaseTokenService::new(endpoint)?,
        })),
        _ => Err(TokenServiceError::UnsupportedProvider(endpoint.r#type)),
    }
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
    fn create_token_service_client_rejects_unknown_provider() {
        let result = create_token_service_client(TokenEndpoint {
            r#type: "github".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        });

        let err = match result {
            Ok(_) => panic!("unknown provider should fail"),
            Err(err) => err,
        };

        assert!(matches!(err, TokenServiceError::UnsupportedProvider(ref ty) if ty == "github"));
    }

    #[test]
    fn okta_build_headers_sets_basic_auth_and_trims_credentials() {
        let client = create_token_service_client(TokenEndpoint {
            r#type: "okta".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::from([(
                "idp_auth_credentials".into(),
                "  Q2xpZW50SWQ6Q2xpZW50U2VjcmV0  ".into(),
            )]),
        })
        .expect("okta client");

        let headers = client.build_headers().expect("okta headers");
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
    fn okta_build_headers_requires_credentials() {
        let client = create_token_service_client(TokenEndpoint {
            r#type: "okta".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        })
        .expect("okta client");

        let err = client
            .build_headers()
            .expect_err("missing credentials should fail");
        assert!(
            matches!(err, TokenServiceError::MissingKey(ref key) if key.contains("idp_auth_credentials"))
        );
    }

    #[test]
    fn entra_build_headers_sets_form_headers() {
        let client = create_token_service_client(TokenEndpoint {
            r#type: "entra".into(),
            request_url: "http://127.0.0.1/token".into(),
            request_data: "grant_type=refresh_token".into(),
            other_params: HashMap::new(),
        })
        .expect("entra client");

        let headers = client.build_headers().expect("entra headers");
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

    #[test]
    fn validate_headers_rejects_invalid_header_name() {
        let err = validate_headers(&HashMap::from([("bad header".into(), "value".into())]))
            .expect_err("invalid header name should fail");

        assert!(
            matches!(err, TokenServiceError::InvalidHeaderName(ref name) if name == "bad header")
        );
    }

    #[test]
    fn validate_headers_rejects_invalid_header_value() {
        let err = validate_headers(&HashMap::from([(
            "authorization".into(),
            "Basic line1\r\nline2".into(),
        )]))
        .expect_err("invalid header value should fail");

        assert!(
            matches!(err, TokenServiceError::InvalidHeaderValue(ref name, _) if name == "authorization")
        );
    }

    #[test]
    fn classify_response_status_allows_success() {
        assert!(classify_response_status(200).is_ok());
        assert!(classify_response_status(399).is_ok());
    }

    #[test]
    fn classify_response_status_returns_rate_limited() {
        let err = classify_response_status(429).expect_err("429 should fail");
        assert!(matches!(err, TokenServiceError::RateLimited));
    }

    #[test]
    fn classify_response_status_returns_http_error() {
        let err = classify_response_status(500).expect_err("500 should fail");
        assert!(matches!(
            err,
            TokenServiceError::Http(UreqError::StatusCode(500))
        ));
    }

    #[test]
    fn parse_token_response_extracts_token_and_expiry() {
        let parsed = parse_token_response(br#"{"access_token":"token-1","expires_in":120}"#)
            .expect("valid token response");

        assert_eq!(
            parsed,
            ParsedTokenResponse {
                token: "token-1".into(),
                expires_in: 120
            }
        );
    }

    #[test]
    fn parse_token_response_defaults_expiry() {
        let parsed =
            parse_token_response(br#"{"access_token":"token-1"}"#).expect("valid token response");

        assert_eq!(
            parsed,
            ParsedTokenResponse {
                token: "token-1".into(),
                expires_in: 3600
            }
        );
    }

    #[test]
    fn parse_token_response_requires_access_token() {
        let err =
            parse_token_response(br#"{"expires_in":120}"#).expect_err("missing token should fail");
        assert!(matches!(err, TokenServiceError::MissingToken));
    }

    #[test]
    fn parse_token_response_rejects_malformed_json() {
        let err = parse_token_response(br#"{"access_token":"token-1""#)
            .expect_err("malformed json should fail");
        assert!(matches!(err, TokenServiceError::MissingToken));
    }
}
