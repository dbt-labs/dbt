use std::sync::Arc;

use dbt_login::LoginHooks;

pub struct LoginFeature {
    pub hooks: Arc<dyn LoginHooks>,
}
