//! Run-phase `Object` impls — std-only types moved here from
//! `dbt-jinja-utils` so the typed run-phase ctx structs can hold them via
//! `JinjaObject<T>` rather than opaque `MinijinjaValue`.
//!
//! Currently moved: `HookConfig`, `LazyModelWrapper`.
//!
//! Pending later PRs (need handle traits or `dbt-common` decoupling to move):
//! * `RunConfig` → uses `dbt_schemas::schemas::project::ConfigKeys`.
//! * `WriteConfig` → uses `dbt_common::path::get_target_write_path` +
//!   `dbt_common::constants::DBT_RUN_DIR_NAME`.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::listener::RenderingEventListener;
use minijinja::value::{Enumerator, Object, Value as MinijinjaValue};
use minijinja::{Error as MinijinjaError, State};

use crate::ModelContextMap;

/// `{{ pre_hooks[i] }}` / `{{ post_hooks[i] }}` — single hook entry from
/// the node's YAML config. Wraps the SQL string and the per-hook
/// `transaction` flag.
#[derive(Clone)]
pub struct HookConfig {
    /// SQL template the hook executes.
    pub sql: String,
    /// Whether this hook participates in the surrounding transaction.
    pub transaction: bool,
}

impl Object for HookConfig {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        match key.as_str() {
            Some("sql") => Some(MinijinjaValue::from(self.sql.clone())),
            Some("transaction") => Some(MinijinjaValue::from(self.transaction)),
            _ => None,
        }
    }
    fn render(self: &Arc<Self>, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.sql)
    }
}

impl std::fmt::Debug for HookConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HookConfig {{ sql: {} }}", self.sql)
    }
}

/// `{{ model.* }}` / `{{ node.* }}` at run scope — wraps the serialized
/// node map and lazy-loads `compiled_code` / `compiled_sql` from the
/// on-disk compiled SQL file when accessed.
///
/// Files are read fresh on every attribute access (no caching) to keep
/// memory pressure low; users that read the field repeatedly in a single
/// render typically see identical bytes anyway.
#[derive(Debug)]
pub struct LazyModelWrapper {
    /// The original model data as a map. Shared with the sibling `node`
    /// wrapper so `model.update(...)` is visible through both slots.
    model_map: ModelContextMap,
    /// Path to the compiled SQL file.
    compiled_path: PathBuf,
}

impl LazyModelWrapper {
    /// Create a new lazy model wrapper over an already-built
    /// [`ModelContextMap`] (see [`crate::to_model_context_map`]).
    pub fn new(model_map: ModelContextMap, compiled_path: PathBuf) -> Self {
        Self {
            model_map,
            compiled_path,
        }
    }

    /// Load the compiled SQL content (no caching - read fresh each time).
    fn load_compiled_sql(&self) -> Option<String> {
        std::fs::read_to_string(&self.compiled_path).ok()
    }
}

impl Object for LazyModelWrapper {
    fn get_value(self: &Arc<Self>, key: &MinijinjaValue) -> Option<MinijinjaValue> {
        let key_str = key.as_str()?;

        match key_str {
            "compiled_code" | "compiled_sql" => {
                // Both fields return the same compiled SQL content
                self.load_compiled_sql().map(MinijinjaValue::from)
            }
            _ => self.model_map.get(key),
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        // Only enumerate fields from model_map (not lazy-loaded fields) so
        // serialization includes all stable fields (e.g. `resource_type`)
        // but doesn't trigger a disk read at enumerate time.
        let keys = self.model_map.keys();

        Enumerator::Iter(Box::new(keys.into_iter()))
    }

    /// Forward dict methods (`update`, `pop`, `get`, `items`, …) to the
    /// backing map. Not redundant with the default `Object::call_method`:
    /// that one only looks the method name up as a *key* and calls the value
    /// it finds, so `model.update({...})` fails with `UnknownMethod`.
    /// Only `MutableMap`'s own `call_method` implements the dict protocol.
    /// Covered by `test_model_update_through_wrapper`.
    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[MinijinjaValue],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, MinijinjaError> {
        Object::call_method(&self.model_map, state, method, args, listeners)
    }
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;

    use super::*;
    use crate::to_model_context_map;

    #[test]
    fn test_lazy_model_wrapper_basic() {
        let mut model_map = IndexMap::new();
        model_map.insert("name".to_string(), MinijinjaValue::from("test_model"));
        model_map.insert("version".to_string(), MinijinjaValue::from(2));

        let wrapper = Arc::new(LazyModelWrapper::new(
            to_model_context_map(model_map),
            PathBuf::from("/non/existent/path.sql"),
        ));

        assert_eq!(
            wrapper.get_value(&MinijinjaValue::from("name")),
            Some(MinijinjaValue::from("test_model"))
        );
        assert_eq!(
            wrapper.get_value(&MinijinjaValue::from("version")),
            Some(MinijinjaValue::from(2))
        );

        // Missing file → compiled_code/compiled_sql resolve to None.
        assert!(
            wrapper
                .get_value(&MinijinjaValue::from("compiled_code"))
                .is_none()
        );
        assert!(
            wrapper
                .get_value(&MinijinjaValue::from("compiled_sql"))
                .is_none()
        );
    }

    #[test]
    fn test_lazy_compiled_fields() {
        let mut model_map = IndexMap::new();
        model_map.insert("name".to_string(), MinijinjaValue::from("test_model"));

        let temp_dir = std::env::temp_dir();
        let test_file = temp_dir.join("test_compiled_pr9.sql");
        std::fs::write(&test_file, "SELECT * FROM table").unwrap();

        let wrapper = Arc::new(LazyModelWrapper::new(
            to_model_context_map(model_map),
            test_file.clone(),
        ));

        let compiled_code = wrapper.get_value(&MinijinjaValue::from("compiled_code"));
        assert_eq!(
            compiled_code.and_then(|v| v.as_str().map(String::from)),
            Some("SELECT * FROM table".to_string())
        );

        let compiled_sql = wrapper.get_value(&MinijinjaValue::from("compiled_sql"));
        assert_eq!(
            compiled_sql.and_then(|v| v.as_str().map(String::from)),
            Some("SELECT * FROM table".to_string())
        );

        std::fs::remove_file(&test_file).ok();
    }

    /// Run-phase counterpart of the `model_update.slt` parse/compile test:
    /// at run scope `model` is a `LazyModelWrapper`, not a bare map, so the
    /// dict protocol only reaches the backing map through the wrapper's
    /// `call_method` forward. Without it this render fails with
    /// `UnknownMethod`.
    #[test]
    fn test_model_update_through_wrapper() {
        let mut model_map = IndexMap::new();
        model_map.insert("description".to_string(), MinijinjaValue::from("original"));

        let wrapper = LazyModelWrapper::new(
            to_model_context_map(model_map),
            PathBuf::from("/non/existent/path.sql"),
        );

        let env = minijinja::Environment::new();
        let ctx = std::collections::BTreeMap::from([(
            "model".to_string(),
            MinijinjaValue::from_object(wrapper),
        )]);

        let rendered = env
            .render_str(
                r#"{% do model.update({"description": "runtime description"}) %}{{ model.description }}"#,
                ctx,
                &[],
            )
            .expect("model.update() must be callable at run scope");

        assert_eq!(rendered, "runtime description");
    }

    #[test]
    fn test_model_wrapper_value_matrix() {
        let mut model_map = IndexMap::new();
        model_map.insert("name".to_string(), MinijinjaValue::from("test_model"));
        model_map.insert("version".to_string(), MinijinjaValue::from(2));
        let wrapper = MinijinjaValue::from_object(LazyModelWrapper::new(
            to_model_context_map(model_map),
            PathBuf::from("/non/existent/path.sql"),
        ));

        let hook = MinijinjaValue::from_object(HookConfig {
            sql: "select 1".to_string(),
            transaction: true,
        });
        assert_eq!(wrapper.kind(), minijinja::value::ValueKind::Map);
        assert_eq!(hook.kind(), minijinja::value::ValueKind::Map);
        let env = minijinja::Environment::new();
        assert_eq!(
            env.render_str(
                "{{ value | list | length }}|{{ value.name }}|{{ value.version }}|{{ hook.sql }}|{{ hook.transaction }}",
                std::collections::BTreeMap::from([("value", wrapper), ("hook", hook)]),
                &[],
            )
            .unwrap(),
            "2|test_model|2|select 1|True"
        );
    }

    /// `model` and `node` wrap the same node, so a mutation through one
    /// (what `{% do model.update(...) %}` ends up doing) is visible via the
    /// other — matching dbt Core, where both names refer to one dict.
    #[test]
    fn test_model_and_node_share_backing_map() {
        let mut model_map = IndexMap::new();
        model_map.insert("name".to_string(), MinijinjaValue::from("test_model"));
        let shared = to_model_context_map(model_map);

        let path = PathBuf::from("/non/existent/path.sql");
        let model = Arc::new(LazyModelWrapper::new(shared.clone(), path.clone()));
        let node = Arc::new(LazyModelWrapper::new(shared.clone(), path));

        shared.insert(
            MinijinjaValue::from("description"),
            MinijinjaValue::from("runtime description"),
        );

        for wrapper in [&model, &node] {
            assert_eq!(
                wrapper.get_value(&MinijinjaValue::from("description")),
                Some(MinijinjaValue::from("runtime description"))
            );
        }
    }
}
