use std::sync::Arc;

use dbt_parser::resolver_hooks::ResolverHooks;

pub struct ResolverFeature {
    pub hooks: Arc<dyn ResolverHooks>,
}
