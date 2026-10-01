use std::collections::BTreeSet;
use std::sync::Arc;

use crate::value::{Enumerator, Object, Value};

/// Utility struct used by [`context!`](crate::context) to merge
/// multiple values.
#[derive(Clone, Debug)]
pub struct MergeObject(pub Vec<Value>);

impl Object for MergeObject {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.0
            .iter()
            .filter_map(|x| x.get_item_opt(key))
            .find(|x| !x.is_undefined())
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        // we collect here the whole internal object once on iteration so that
        // we have an enumerator with a known length.
        let items = self
            .0
            .iter()
            .flat_map(|v| v.try_iter().ok())
            .flatten()
            .collect::<BTreeSet<_>>();
        Enumerator::Iter(Box::new(items.into_iter()))
    }
}
