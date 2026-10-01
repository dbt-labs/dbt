use dbt_common::{ErrorCode, FsResult, fs_err};
use reqwest::header::{ACCEPT, AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{
    RetryTransientMiddleware, policies::ExponentialBackoff as RetryExponentialBackoff,
};

pub const MAX_CLIENT_RETRIES: u32 = 3;

/// Builds a URL under the dbt Cloud private API base path:
/// `https://{host}/api/private/accounts/{account_id}/{path}`
pub fn build_private_api_url(host: &str, account_id: &str, path: &str) -> String {
    format!("https://{host}/api/private/accounts/{account_id}/{path}")
}

pub enum CloudAuthScheme {
    Bearer,
    Token,
}

pub fn build_retry_client(base_client: reqwest::Client) -> ClientWithMiddleware {
    let retry_policy =
        RetryExponentialBackoff::builder().build_with_max_retries(MAX_CLIENT_RETRIES);
    ClientBuilder::new(base_client)
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}

pub fn build_cloud_api_client(
    token: &str,
    auth_scheme: CloudAuthScheme,
    invocation_id: Option<&str>,
) -> FsResult<ClientWithMiddleware> {
    let mut default_headers = HeaderMap::new();
    default_headers.insert(ACCEPT, HeaderValue::from_static("application/json"));

    let auth_value = match auth_scheme {
        CloudAuthScheme::Bearer => format!("Bearer {token}"),
        CloudAuthScheme::Token => format!("Token {token}"),
    };
    let authorization = HeaderValue::from_str(&auth_value).map_err(|err| {
        fs_err!(
            ErrorCode::InvalidArgument,
            "Invalid Authorization header value: {}",
            err
        )
    })?;
    default_headers.insert(AUTHORIZATION, authorization);

    if let Some(invocation_id) = invocation_id {
        let invocation_header = HeaderValue::from_str(invocation_id).map_err(|err| {
            fs_err!(
                ErrorCode::InvalidArgument,
                "Invalid invocation id header value: {}",
                err
            )
        })?;
        default_headers.insert(
            HeaderName::from_static("x-invocation-id"),
            invocation_header,
        );
    }

    let base_client = reqwest::Client::builder()
        .default_headers(default_headers)
        .build()
        .map_err(|err| {
            fs_err!(
                ErrorCode::NetworkError,
                "Failed to build cloud HTTP client: {}",
                err
            )
        })?;

    Ok(build_retry_client(base_client))
}
