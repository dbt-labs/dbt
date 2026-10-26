//! Metadata SDK for the dbt platform.
//!
//! Resolves cached dbt authentication (via [`dbt_platform_auth`]) and exposes
//! higher-level operations scoped to an account: identity ([`PlatformClient::whoami`]),
//! projects ([`PlatformClient::list_projects`]), and catalog exploration through
//! the Discovery GraphQL API ([`PlatformClient::list_catalog_models`]).
//!
//! A [`PlatformClient`] is scoped to one account but refreshes its credential
//! automatically (an OAuth token is renewed as it nears expiry, and once more on a
//! `401`). Derive a client for another authenticated account with
//! [`PlatformClient::for_account`]; siblings share the underlying credential cache.
//!
//! ```no_run
//! # async fn run() -> dbt_platform::Result<()> {
//! use dbt_platform::PlatformClient;
//!
//! let client = PlatformClient::connect().await?;
//! let projects = client.list_projects().await?;
//!
//! // Pull metadata for another account the user is authenticated for.
//! let other = client.for_account(12345).await?;
//! let other_projects = other.list_projects().await?;
//! # Ok(())
//! # }
//! ```

mod auth;
mod catalog;
mod client;
mod error;
mod projects;

pub use catalog::CatalogModel;
pub use client::{PlatformClient, PlatformClientBuilder};
pub use error::{PlatformError, Result};

// Re-export the auth surface callers need to configure the client.
pub use dbt_platform_auth::{AuthError, Credential, OAuthSession, ResolverKind};

// Re-export the dbt Cloud Admin API response models returned by this SDK.
pub use dbt_cloud_api::models::{ProjectResponse, WhoamiResponse};
