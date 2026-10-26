use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::client::PlatformClient;
use crate::error::{PlatformError, Result};

/// Envelope returned by the Discovery GraphQL API.
#[derive(Debug, serde::Deserialize)]
struct GraphQlResponse<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<GraphQlError>,
}

#[derive(Debug, serde::Deserialize)]
struct GraphQlError {
    message: String,
}

/// A model as exposed by the applied state of a Discovery environment.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub unique_id: String,
    pub name: Option<String>,
    pub database: Option<String>,
    pub schema: Option<String>,
    pub alias: Option<String>,
    pub description: Option<String>,
}

// --- Response shapes for the models query below. ---

#[derive(Debug, serde::Deserialize)]
struct ModelsQueryData {
    environment: Option<EnvironmentApplied>,
}

#[derive(Debug, serde::Deserialize)]
struct EnvironmentApplied {
    applied: Option<AppliedState>,
}

#[derive(Debug, serde::Deserialize)]
struct AppliedState {
    models: ModelConnection,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModelConnection {
    page_info: PageInfo,
    edges: Vec<ModelEdge>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
    has_next_page: bool,
    end_cursor: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ModelEdge {
    node: CatalogModel,
}

/// GraphQL query listing applied models for an environment, one page at a time.
const MODELS_QUERY: &str = r#"
query Models($environmentId: BigInt!, $first: Int!, $after: String) {
  environment(id: $environmentId) {
    applied {
      models(first: $first, after: $after) {
        pageInfo { hasNextPage endCursor }
        edges {
          node {
            uniqueId
            name
            database
            schema
            alias
            description
          }
        }
      }
    }
  }
}
"#;

/// Page size requested from the Discovery API when paginating catalog results.
const CATALOG_PAGE_SIZE: i64 = 100;

impl PlatformClient {
    /// POST a GraphQL request body to the Discovery endpoint with a bearer token,
    /// force-refreshing the token first when `force_refresh` is set.
    async fn discovery_post(
        &self,
        body: &serde_json::Value,
        force_refresh: bool,
    ) -> Result<reqwest::Response> {
        Ok(self
            .http()
            .post(self.discovery_graphql_url())
            .header(reqwest::header::USER_AGENT, self.user_agent())
            .bearer_auth(self.token(force_refresh).await?)
            .json(body)
            .send()
            .await?)
    }

    /// Execute an arbitrary query against the internal Discovery GraphQL API and
    /// deserialize its `data` field into `T`.
    ///
    /// This is the low-level escape hatch; prefer the typed catalog helpers when
    /// they cover your need. Returns [`PlatformError::Discovery`] if the response
    /// carries GraphQL errors or an empty `data` field.
    pub async fn discovery_query<T: DeserializeOwned>(
        &self,
        query: &str,
        variables: impl Serialize,
    ) -> Result<T> {
        let body = serde_json::json!({
            "query": query,
            "variables": variables,
        });

        // Fetch a fresh token, and if the request is rejected as unauthorized,
        // force-refresh once and retry — mirroring the Admin API path.
        let mut response = self.discovery_post(&body, false).await?;
        if matches!(
            response.status(),
            reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN
        ) {
            response = self.discovery_post(&body, true).await?;
        }

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await?;
            return Err(PlatformError::Discovery(format!("status {status}: {text}")));
        }

        let bytes = response.bytes().await?;
        let parsed: GraphQlResponse<T> = serde_json::from_slice(&bytes)?;
        if !parsed.errors.is_empty() {
            let joined = parsed
                .errors
                .iter()
                .map(|e| e.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(PlatformError::Discovery(joined));
        }
        parsed
            .data
            .ok_or_else(|| PlatformError::Discovery("response contained no data".to_owned()))
    }

    /// List every applied model in a Discovery environment, transparently
    /// paginating through the connection.
    pub async fn list_catalog_models(&self, environment_id: i64) -> Result<Vec<CatalogModel>> {
        let mut all = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let variables = serde_json::json!({
                "environmentId": environment_id,
                "first": CATALOG_PAGE_SIZE,
                "after": after,
            });
            let data: ModelsQueryData = self.discovery_query(MODELS_QUERY, variables).await?;
            let Some(connection) = data.environment.and_then(|e| e.applied).map(|a| a.models)
            else {
                break;
            };
            all.extend(connection.edges.into_iter().map(|e| e.node));
            match connection.page_info {
                PageInfo {
                    has_next_page: true,
                    end_cursor: Some(cursor),
                } => after = Some(cursor),
                _ => break,
            }
        }
        Ok(all)
    }
}
