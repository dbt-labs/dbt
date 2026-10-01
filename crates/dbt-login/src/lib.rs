use std::sync::Arc;

use dbt_common::FsResult;

mod execute;
mod execute_status;
mod state_guidance;

pub use execute::execute_login;
pub use execute_status::execute_login_status;

#[async_trait::async_trait]
pub trait LoginHooks: Send + Sync {
    /// Called after a successful login to dispatch async post-login actions.
    async fn did_login(self: Arc<Self>) -> FsResult<()>;
}

pub struct DefaultLoginHooks;

#[async_trait::async_trait]
impl LoginHooks for DefaultLoginHooks {
    async fn did_login(self: Arc<Self>) -> FsResult<()> {
        Ok(())
    }
}
