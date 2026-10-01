use std::sync::Arc;

use dbt_jinja_utils::JinjaFactory;

/// Jinja-related services. Exposes the [`JinjaFactory`] used to create rendering
/// environments.
pub struct JinjaFeature {
    pub factory: Arc<dyn JinjaFactory>,
}
