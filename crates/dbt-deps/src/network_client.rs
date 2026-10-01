use reqwest::Client;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{
    RetryTransientMiddleware, policies::ExponentialBackoff as RetryExponentialBackoff,
};

const MAX_CLIENT_RETRIES: u32 = 3;

pub(crate) fn retrying_http_client() -> ClientWithMiddleware {
    let retry_policy =
        RetryExponentialBackoff::builder().build_with_max_retries(MAX_CLIENT_RETRIES);
    ClientBuilder::new(Client::new())
        .with(RetryTransientMiddleware::new_with_policy(retry_policy))
        .build()
}
