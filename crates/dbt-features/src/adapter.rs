use std::sync::Arc;

use dbt_adapter::adapter::AdapterFactory;
use dbt_adapter::sql_types::TypeOpsFactory;

pub struct AdapterFeature {
    pub type_ops_factory: Arc<dyn TypeOpsFactory>,
    pub adapter_factory: Arc<dyn AdapterFactory>,
}
