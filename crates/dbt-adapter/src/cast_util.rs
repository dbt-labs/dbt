//! A set of util functions for casting from/to Value
use crate::relation::RelationObject;

use dbt_schemas::schemas::relations::base::BaseRelation;

use std::file;
use std::sync::Arc;

pub const THIS_RELATION_KEY: &str = "__this__";

pub fn downcast_value_to_dyn_base_relation(
    value: &minijinja::Value,
) -> Result<Arc<dyn BaseRelation>, minijinja::Error> {
    if let Some(relation_object) = value.downcast_object::<RelationObject>() {
        Ok(relation_object.inner())
    } else if let Some(relation_object) = value
        .as_object()
        .ok_or_else(|| {
            minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "relation must be an object",
            )
        })?
        .get_value(&minijinja::Value::from(THIS_RELATION_KEY))
    {
        Ok(relation_object
            .downcast_object::<RelationObject>()
            .ok_or_else(|| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "this must be a BaseRelation object",
                )
            })?
            .inner())
    } else {
        Err(minijinja::Error::new(
            minijinja::ErrorKind::InvalidOperation,
            format!(
                "Unsupported relation type ({}) in {}:{}",
                value,
                file!(),
                line!()
            ),
        ))
    }
}
