//! Small utilities for pre-processing raw `yaml::Value` before Jinja rendering.
//!
//! Used by resolvers that need to keep specific fields out of
//! [`into_typed_with_jinja`](dbt_jinja_utils::serde::into_typed_with_jinja)'s
//! field-transformer (the recursive Jinja renderer). Documentation text is
//! the canonical example: a `description:` may legitimately contain
//! `{% raw %}{{ ref(...) }}{% endraw %}` doc snippets that must not be
//! evaluated at parse time. dbt-core never renders descriptions; Fusion
//! mirrors that by detaching the field before the renderer sees it and
//! reattaching the raw string afterwards.
//!
//! The helpers operate on `yaml::Value` rather than the typed struct so
//! the field is removed from the renderer's input — wrapping the typed
//! field in `Verbatim` would not help once `into_typed_with_jinja` has
//! already walked the string.

use dbt_yaml::Value as YmlValue;

/// Removes `field` from a mapping `value`. Returns the removed value if
/// `value` was a mapping and the key was present; otherwise returns
/// `None`. The caller is expected to reattach the raw value to the typed
/// struct after `into_typed_with_jinja`, if it needs to be preserved.
pub(crate) fn detach_field_from_mapping(value: &mut YmlValue, field: &str) -> Option<YmlValue> {
    value.as_mapping_mut().and_then(|m| m.remove(field))
}
