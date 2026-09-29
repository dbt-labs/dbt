use dbt_cloud_api::apis::{projects_api, whoami_api};
use dbt_cloud_api::models::{ProjectResponse, WhoamiResponse};

use crate::client::PlatformClient;
use crate::error::Result;

/// The maximum page size accepted by the dbt Cloud Admin API list endpoints.
const MAX_PAGE_SIZE: i32 = 100;

impl PlatformClient {
    /// Fetch information about the authenticated user.
    pub async fn whoami(&self) -> Result<WhoamiResponse> {
        self.with_config(|cfg| async move { Ok(whoami_api::whoami(&cfg).await?) })
            .await
    }

    /// Fetch a single page of projects for the connected account.
    ///
    /// Prefer [`PlatformClient::list_projects`] unless you need to search by
    /// name or drive pagination yourself.
    pub async fn list_projects_page(
        &self,
        limit: Option<i32>,
        offset: Option<i32>,
        name_contains: Option<&str>,
    ) -> Result<Vec<ProjectResponse>> {
        let account_id = self.account_id_i64();
        self.with_config(|cfg| async move {
            let enveloped =
                projects_api::list_projects(&cfg, account_id, limit, offset, name_contains, None)
                    .await?;
            Ok(enveloped.data)
        })
        .await
    }

    /// List every project in the connected account, transparently paginating.
    pub async fn list_projects(&self) -> Result<Vec<ProjectResponse>> {
        let mut all = Vec::new();
        let mut offset = 0;
        loop {
            let page = self
                .list_projects_page(Some(MAX_PAGE_SIZE), Some(offset), None)
                .await?;
            let page_len = page.len() as i32;
            all.extend(page);
            if page_len < MAX_PAGE_SIZE {
                break;
            }
            offset += page_len;
        }
        Ok(all)
    }

    /// Fetch a single project by id.
    pub async fn get_project(&self, project_id: i64) -> Result<ProjectResponse> {
        let account_id = self.account_id_i64();
        self.with_config(|cfg| async move {
            let enveloped = projects_api::retrieve_project(&cfg, account_id, project_id).await?;
            Ok(*enveloped.data)
        })
        .await
    }
}
