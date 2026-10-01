use async_trait::async_trait;
use dbt_common::FsResult;

#[async_trait]
pub trait LicenseFetcher: Send + Sync {
    async fn fetch_and_cache_license(&self) -> FsResult<()>;
}

pub struct NoOpLicenseFetcher;

#[async_trait]
impl LicenseFetcher for NoOpLicenseFetcher {
    async fn fetch_and_cache_license(&self) -> FsResult<()> {
        Ok(())
    }
}
