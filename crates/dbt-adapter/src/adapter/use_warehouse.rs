//! This mod is not used currently, but we'll bring this back when we can let the guard to use
//! TypedBaseAdapter instead, then we can remove the adapter.restore_warehouse
//! This requires us to move the ConnectionGuard to TypedBaseAdapter
//! More details see https://github.com/dbt-labs/fs/pull/4039#discussion_r2159864154

use crate::Adapter;

pub struct UseWarehouseGuard<'a> {
    adapter: &'a Adapter,
    /// The original warehouse that we need to restore to when the guard is dropped.
    original_wh: Option<String>,
    node_id: String,
}

impl<'a> UseWarehouseGuard<'a> {
    pub fn new(adapter: &'a Adapter, original_wh: Option<String>, node_id: &str) -> Self {
        Self {
            adapter,
            original_wh,
            node_id: node_id.to_string(),
        }
    }
}

impl Drop for UseWarehouseGuard<'_> {
    fn drop(&mut self) {
        if let Some(original_wh) = &self.original_wh {
            // This is best effort
            let _ = self
                .adapter
                .use_warehouse(Some(original_wh.to_string()), &self.node_id);
        }
    }
}
