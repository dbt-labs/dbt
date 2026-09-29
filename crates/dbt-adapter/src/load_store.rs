use minijinja::arg_utils::ArgsIter;
use minijinja::value::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::response::{AdapterResponse, ResultObject};
use crate::value::none_value;
use dbt_agate::AgateTable;
use dbt_common::tracing::span_info::find_and_update_span_attrs;
use dbt_telemetry::NodeEvaluated;

/// A store for DBT query results that provides callable functions to access the store
#[derive(Clone, Default)]
pub struct ResultStore {
    results: Arc<Mutex<HashMap<String, Value>>>,
}

impl ResultStore {
    /// Clear all results from the store
    pub fn clear(&self) {
        let mut results = self.results.lock().unwrap();
        results.clear();
    }

    /// Read the `main` statement's [`AdapterResponse`] without consuming it.
    ///
    /// `main` is exempt from the load-once rule in [`Self::load_result`], so
    /// reading it here cannot make a later `load_result('main')` fail.
    pub fn main_adapter_response(&self) -> Option<AdapterResponse> {
        let results = self.results.lock().unwrap();
        let result = results.get("main")?.downcast_object::<ResultObject>()?;
        Some(result.response.clone())
    }

    /// https://github.com/dbt-labs/dbt-core/blob/34bb3f94dde716a3f9c36481d2ead85c211075dd/core/dbt/context/providers.py#L1043
    pub fn store_result(
        &self,
    ) -> impl Fn(&[Value]) -> Result<Value, minijinja::Error> + Clone + use<> {
        let store = self.clone();
        move |args: &[Value]| {
            // name: str,
            // response: Any,
            // agate_table: Optional["agate.Table"] = None
            let iter = ArgsIter::new("store_result", &["name", "response"], args);
            let name: String = iter.next_arg::<&str>()?.to_string();
            let response_value = iter.next_arg::<Value>()?;
            let response = if response_value.is_introspective_stub() {
                // A tainted value's real shape is unknowable and, even when
                // its fabricated inner value happens to be a real
                // `AdapterResponse` (see `IntrospectiveValue::unpack`), it
                // never downcasts to one -- the taint wrapper itself is
                // what's stored, not its contents. Degrade to a default
                // response instead of erroring via `AdapterResponse::
                // try_from`'s downcast/string fallback below.
                AdapterResponse::default()
            } else {
                AdapterResponse::try_from(response_value)?
            };

            let table: Option<Value> = iter.next_kwarg::<Option<Value>>("agate_table")?;
            let table = if let Some(t) = table {
                if t.is_introspective_stub() {
                    // Same rationale as `response` above: `.expect(
                    // "agate_table")` below would otherwise hard-panic
                    // instead of degrading gracefully, since a tainted
                    // value can never downcast to a concrete `AgateTable`.
                    Some(AgateTable::default())
                } else if !t.is_none() {
                    Some((*t.downcast_object::<AgateTable>().expect("agate_table")).clone())
                } else {
                    Some(AgateTable::default())
                }
            } else {
                Some(AgateTable::default())
            };

            // Record rows_affected on the NodeEvaluated span if non-negative.
            // dbt-core uses -1 to indicate unknown rows affected. Telemetry uses `None` for unknown.
            let rows_affected = response.rows_affected_i64();
            if rows_affected >= 0 {
                find_and_update_span_attrs::<_, NodeEvaluated>(|attrs| {
                    attrs.rows_affected = Some(rows_affected as u64);
                });
            }

            let value = Value::from_object(ResultObject::new(response, table));
            iter.finish()?;

            let mut results = store.results.lock().unwrap();
            results.insert(name, value);

            Ok(Value::from(""))
        }
    }

    /// https://github.com/dbt-labs/dbt-core/blob/34bb3f94dde716a3f9c36481d2ead85c211075dd/core/dbt/context/providers.py#L1022
    pub fn load_result(
        &self,
    ) -> impl Fn(&[Value]) -> Result<Value, minijinja::Error> + Clone + use<> {
        let store = self.clone();
        move |args: &[Value]| {
            // name: str,
            let iter = ArgsIter::new("load_result", &["name"], args);
            let name: String = iter.next_arg::<&str>()?.to_string();
            iter.finish()?;

            let mut results = store.results.lock().unwrap();

            if let Some(value) = results.get_mut(&name) {
                if name == "main" {
                    Ok(value.clone())
                } else if *value == none_value() {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::MacroResultAlreadyLoadedError,
                        format!(
                            "The 'statement' result named '{name}' has already been loaded into a variable"
                        ),
                    ))
                } else {
                    let result = value.clone();
                    *value = none_value();
                    Ok(result)
                }
            } else {
                Ok(none_value())
            }
        }
    }

    /// https://github.com/dbt-labs/dbt-core/blob/34bb3f94dde716a3f9c36481d2ead85c211075dd/core/dbt/context/providers.py#L1043
    pub fn store_raw_result(
        &self,
    ) -> impl Fn(&[Value]) -> Result<Value, minijinja::Error> + Clone + use<> {
        let store = self.clone();
        move |args: &[Value]| {
            // name: str,
            // message=Optional[str],
            // code=Optional[str],
            // rows_affected=Optional[str],
            // agate_table: Optional["agate.Table"] = None,
            let iter = ArgsIter::new("store_raw_result", &[], args);
            let name: String = iter.next_kwarg::<String>("name")?;
            let message: Option<String> = iter.next_kwarg::<Option<String>>("message")?;
            let code: Option<String> = iter.next_kwarg::<Option<String>>("code")?;
            let rows_affected: Option<String> =
                iter.next_kwarg::<Option<String>>("rows_affected")?;
            let agate_table: Option<Value> = iter.next_kwarg::<Option<Value>>("agate_table")?;

            // Parse rows_affected only if string value is present and valid
            let rows_affected = if let Some(rows_affected) = rows_affected
                && let Some(rows) = rows_affected.parse::<i64>().ok()
            {
                // Record rows_affected on the NodeEvaluated span only if value was present and non-negative.
                // dbt-core uses -1 to indicate unknown rows affected. Telemetry uses `None` for unknown.
                if rows >= 0 {
                    find_and_update_span_attrs::<_, NodeEvaluated>(|attrs| {
                        attrs.rows_affected = Some(rows as u64);
                    });
                };

                rows
            } else {
                0
            };

            // Create adapter response (keep original semantics: default to 0 if not present)
            let response = AdapterResponse::new()
                .with_message(message.unwrap_or_default())
                .with_code(code.unwrap_or_default())
                .with_rows_affected(rows_affected);
            let mut results = store.results.lock().unwrap();
            let value = Value::from_object(ResultObject::new(
                response,
                agate_table
                    .map(|t| {
                        if t.is_introspective_stub() {
                            // See `store_result`'s identical check: a
                            // tainted value never downcasts to a concrete
                            // `AgateTable`, so `.expect("agate_table")`
                            // below would otherwise hard-panic instead of
                            // degrading gracefully.
                            AgateTable::default()
                        } else if !t.is_none() {
                            (*t.downcast_object::<AgateTable>().expect("agate_table")).clone()
                        } else {
                            AgateTable::default()
                        }
                    })
                    .or(Some(AgateTable::default())),
            ));

            results.insert(name, value);
            Ok(Value::from(true))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::value::Kwargs;

    fn store_named(store: &ResultStore, name: &str) -> Result<Value, minijinja::Error> {
        let store_raw = store.store_raw_result();
        store_raw(&[Value::from(Kwargs::from_iter([(
            "name",
            Value::from(name),
        )]))])
    }

    fn load_named(store: &ResultStore, name: &str) -> Result<Value, minijinja::Error> {
        let load = store.load_result();
        load(&[Value::from(name)])
    }

    /// The `statement(...)` macro pattern stores a named result and then loads
    /// (consumes) it. Two microbatch batches that share one registry interleave
    /// as store(A)/store(B)/load(A)/load(B); B's load then sees the consumed
    /// sentinel and raises `MacroResultAlreadyLoadedError`. This reproduces the
    /// `concurrent_batches=true` microbatch bug (fs#11019 follow-up), where the
    /// collision is on `get_columns_in_relation` (Postgres) or
    /// `run_query_statement` (Snowflake).
    #[test]
    fn shared_result_store_collides_across_batches() {
        let shared = ResultStore::default();
        store_named(&shared, "get_columns_in_relation").unwrap(); // batch A stores
        store_named(&shared, "get_columns_in_relation").unwrap(); // batch B overwrites
        load_named(&shared, "get_columns_in_relation").unwrap(); // batch A consumes
        let err = load_named(&shared, "get_columns_in_relation").unwrap_err(); // batch B
        assert_eq!(
            err.kind(),
            minijinja::ErrorKind::MacroResultAlreadyLoadedError
        );
    }

    /// The fix gives each batch its own `ResultStore` (see
    /// `reset_result_store`), so the same interleaving no longer collides.
    #[test]
    fn isolated_result_stores_do_not_collide_across_batches() {
        let batch_a = ResultStore::default();
        let batch_b = ResultStore::default();
        store_named(&batch_a, "get_columns_in_relation").unwrap();
        store_named(&batch_b, "get_columns_in_relation").unwrap();
        load_named(&batch_a, "get_columns_in_relation").unwrap();
        // batch B loads its own (still-present) result, not A's consumed one.
        let v = load_named(&batch_b, "get_columns_in_relation").unwrap();
        assert!(!v.is_none());
    }

    /// Regression test: under `JinjaRenderMode::Symbolic`, `{% set res, table
    /// = adapter.execute(...) %}` followed immediately by `{{
    /// store_result(name, response=res, agate_table=table) }}` in
    /// dbt-adapters' `statement()` macro feeds `store_result` a tainted
    /// `IntrospectiveValue` for both `response` and `agate_table`. Before
    /// this fix, `agate_table`'s `.expect("agate_table")` downcast
    /// hard-panicked (a tainted value never downcasts to a concrete
    /// `AgateTable`, no matter what its fabricated inner value looks like),
    /// and `response`'s `AdapterResponse::try_from` returned a hard error.
    /// Both must instead degrade to a default, matching how every other
    /// operation on fabricated stub data in this taint system swallows
    /// rather than fails.
    #[test]
    fn store_result_with_tainted_response_and_agate_table_does_not_panic() {
        use crate::introspective_taint::IntrospectiveValue;

        let store = ResultStore::default();
        let store_result = store.store_result();
        let tainted_response = IntrospectiveValue::wrap(Value::from("ok"));
        let tainted_table = IntrospectiveValue::wrap(Value::from(()));
        store_result(&[
            Value::from("main"),
            tainted_response,
            Value::from(Kwargs::from_iter([("agate_table", tainted_table)])),
        ])
        .unwrap();

        let loaded = load_named(&store, "main").unwrap();
        assert!(!loaded.is_none());
    }
}
