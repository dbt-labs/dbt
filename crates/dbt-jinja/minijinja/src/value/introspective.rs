//! Taint-propagating wrapper for values that stand in for something the
//! renderer cannot actually know (the canonical example being a
//! warehouse-dependent adapter call evaluated without a real connection).
//! Consumers outside this crate (e.g. `dbt-adapter`'s `Adapter::Parse` mode)
//! use this to mark such values; the VM (`vm/mod.rs`) and `Macro::call`
//! (`vm/macro_object.rs`) both consume it, which is why it lives in
//! minijinja itself rather than in a downstream crate.
//!
//! `IntrospectiveValue` wraps an arbitrary inner `Value` and marks itself via
//! `Object::is_introspective_stub`. Attribute access, method calls, and
//! iteration on the wrapper delegate to the inner value and re-wrap their
//! results, so taint survives chained access (`taint.foo.bar()`,
//! `for x in taint`) without any changes to the jinja VM for those cases.
//! Binary operators, jinja tests/filters, and `{% if %}`/`{{ }}` are handled
//! separately at dedicated VM choke points, since they cannot be intercepted
//! purely through `Object` trait delegation.

use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use crate::listener::RenderingEventListener;
use crate::value::{Enumerator, Object, ObjectRepr};
use crate::{Error, State, Value};

/// See the module docs for what this wraps and why.
#[derive(Debug)]
pub struct IntrospectiveValue {
    inner: Value,
    /// Whether this wrapper itself claims to be a one-item iterable when
    /// enumerated (see `enumerate`). Only `true` for the value returned
    /// directly at the taint origin; every value *derived* from a tainted
    /// value (attribute/method access, call results, and the single item
    /// `enumerate` itself hands out) is `false`.
    ///
    /// This distinction is load-bearing, not cosmetic: if every derived
    /// value were also "iterable with one item", hashing a tainted value
    /// (`Hash for DynObject` recursively enumerates every nested value,
    /// e.g. via `HashSet`/`HashMap` deduplication elsewhere in the
    /// pipeline) would recurse forever, because each nested item would
    /// itself claim to contain exactly one further nested item, ad
    /// infinitum, regardless of what real data (if any) it wraps. Confirmed
    /// against a real ~1500-model project: this previously produced a
    /// genuine stack overflow during ordinary project parsing.
    iterable: bool,
}

impl IntrospectiveValue {
    /// Wraps `value` so the jinja VM recognizes it as an introspective stub.
    /// Use this only at a taint origin (e.g. the return value of an
    /// introspective adapter call, or a macro call whose body touched
    /// taint); everywhere else this type wraps a value internally, it uses
    /// `wrap_leaf` instead (see `iterable` above).
    pub fn wrap(value: Value) -> Value {
        Value::from_object(IntrospectiveValue {
            inner: value,
            iterable: true,
        })
    }

    pub(crate) fn wrap_leaf(value: Value) -> Value {
        Value::from_object(IntrospectiveValue {
            inner: value,
            iterable: false,
        })
    }

    /// Unpacks this stub into exactly `count` tainted items for fixed-arity
    /// destructuring (`{% set a, b = ... %}`, and each item of `{% for a, b
    /// in list %}`), which -- unlike `enumerate()`'s "exactly one item"
    /// loop-body model (see `iterable`'s doc comment) -- has a statically
    /// known arity to satisfy.
    ///
    /// When the fabricated inner value already has exactly `count` real
    /// items (e.g. `adapter.execute()`'s Parse-mode stub really is a
    /// `(response, table)` pair of correctly-typed defaults, not bare
    /// placeholders), wraps and returns those instead of discarding them --
    /// this preserves fidelity for further Jinja-level attribute/method
    /// access (which `get_value`/`call_method` already degrade gracefully
    /// on regardless of what's really inside). It does *not* make the
    /// result safe to `downcast_object` through, though: the wrapper
    /// itself is what gets stored, not its contents, so native callers
    /// that need the concrete type back out of a destructured item still
    /// need their own `is_introspective_stub()` check before downcasting
    /// (see e.g. `ResultStore::store_result`'s `agate_table` handling).
    ///
    /// Falls back to `count` tainted `undefined` leaves when the inner
    /// value's real arity doesn't match `count` (including the common case
    /// of an empty/scalar Parse-mode stub).
    pub(crate) fn unpack(&self, count: usize) -> Vec<Value> {
        if let Some(items) = self
            .inner
            .as_object()
            .and_then(|o| o.try_iter())
            .map(|it| it.collect::<Vec<_>>())
            .filter(|items| items.len() == count)
        {
            return items
                .into_iter()
                .map(IntrospectiveValue::wrap_leaf)
                .collect();
        }
        (0..count)
            .map(|_| IntrospectiveValue::wrap_leaf(Value::UNDEFINED))
            .collect()
    }
}

impl Object for IntrospectiveValue {
    fn is_introspective_stub(self: &Arc<Self>) -> bool {
        true
    }

    fn is_true(self: &Arc<Self>) -> bool {
        // Delegate to the inner (fake) value's own truthiness instead of the
        // default `Object::is_true` (based on `enumerator_len() != Some(0)`):
        // `enumerate` always reports exactly one item for the origin wrapper
        // (see its doc comment), which would make every tainted value
        // evaluate as truthy regardless of what it actually wraps --
        // breaking plain `{% if adapter.get_relation(...) %}`-style checks
        // in every render mode, not just the ones that opt into taint-aware
        // branch exploration via `RenderingEventListener::override_branch`
        // (which intercepts *before* this is ever reached, so this delegate
        // only affects the fallback path used when no listener wants to).
        self.inner.is_true()
    }

    fn repr(self: &Arc<Self>) -> ObjectRepr {
        self.inner
            .as_object()
            .map(|o| o.repr())
            .unwrap_or(ObjectRepr::Plain)
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        // Always answer (never `None`, which would mean "not handled, keep
        // looking elsewhere") so a lookup on a tainted value can never fall
        // through to an untainted result -- including when the inner (fake)
        // stub is empty/scalar and genuinely has no such value, which is the
        // common case (e.g. a representative item drawn from an empty
        // introspective list, see `enumerate` below).
        let value = self
            .inner
            .as_object()
            .and_then(|o| o.get_value(key))
            .unwrap_or(Value::UNDEFINED);
        Some(IntrospectiveValue::wrap_leaf(value))
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        if !self.iterable {
            // A derived/leaf value never claims further items -- see
            // `iterable`'s doc comment for why this bound is required.
            return Enumerator::Empty;
        }
        // An introspective iterable always yields exactly one representative
        // item, itself tainted, regardless of the (fake) inner value's real
        // shape -- this mirrors Turbo's "render a for-loop body once" model.
        // The item is intentionally NOT drawn from `self.inner`'s real first
        // element: besides Parse-mode stubs typically being empty anyway,
        // pulling in a real (potentially large or self-referential) nested
        // object here is exactly what caused the unbounded hash recursion
        // described above -- a plain, content-free seed is sufficient since
        // `get_value`/`get_property` already degrade gracefully regardless
        // of what the item "really" contains.
        Enumerator::Values(vec![IntrospectiveValue::wrap_leaf(Value::UNDEFINED)])
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        // Deliberately decoupled from `enumerate()`'s single-item taint-loop
        // behavior above: `enumerator_len` backs the `length`/`count` filter
        // (`Value::len`) via a plain, non-recursive count, not iteration, so
        // there's no risk of resurrecting the unbounded-hash-recursion bug
        // `enumerate` is written to avoid. Delegating to the inner value's
        // real length instead of the default `Object::enumerator_len` (which
        // would derive `Some(1)` from `enumerate()` above) matters for the
        // same reason `is_true` delegates: `{{ some_relation_columns|length
        // }}`-style checks are common in dbt macros and must not be forced
        // to `1` in every render mode just because the value happens to be
        // tainted.
        self.inner.len()
    }

    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        // Swallow errors from the inner (fake) value the same way as
        // `get_value`/`get_property`: an operation that can't be meaningfully
        // performed on fabricated stub data degrades to a tainted
        // `UNDEFINED` rather than a hard render error. Before taint existed,
        // this code path was simply never reached (e.g. a `{% for %}` over
        // an empty stub list ran zero iterations), so there is no prior
        // behavior to preserve here.
        let value = self
            .inner
            .call(state, args, listeners)
            .unwrap_or(Value::UNDEFINED);
        Ok(IntrospectiveValue::wrap_leaf(value))
    }

    fn call_method(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        method: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let value = self
            .inner
            .call_method(state, method, args, listeners)
            .unwrap_or(Value::UNDEFINED);
        Ok(IntrospectiveValue::wrap_leaf(value))
    }

    fn get_property(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        name: &str,
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        let value = self
            .inner
            .as_object()
            .and_then(|object| object.get_property(state, name, listeners).ok())
            .unwrap_or(Value::UNDEFINED);
        Ok(IntrospectiveValue::wrap_leaf(value))
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result
    where
        Self: Sized + 'static,
    {
        // The default `Object::render` falls back to `Debug` for
        // `ObjectRepr::Plain` (the common case here, since most stub values
        // are scalars); render as the inner (fake) value instead so a hole
        // marker isn't the only thing distinguishing this from the real
        // rendering path -- callers that care about taint use
        // `is_introspective_stub`, not the rendered text.
        fmt::Display::fmt(&self.inner, f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Environment, ErrorKind};

    #[derive(Debug)]
    struct TestRelation;

    impl Object for TestRelation {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Seq
        }

        fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
            match key.as_str() {
                Some("name") => Some(Value::from("col_a")),
                _ => None,
            }
        }

        fn call_method(
            self: &Arc<Self>,
            _state: &State<'_, '_>,
            method: &str,
            _args: &[Value],
            _listeners: &[Rc<dyn RenderingEventListener>],
        ) -> Result<Value, Error> {
            match method {
                "upper_name" => Ok(Value::from("COL_A")),
                _ => Err(Error::from(ErrorKind::UnknownMethod)),
            }
        }

        fn enumerate(self: &Arc<Self>) -> Enumerator {
            Enumerator::Values(vec![
                Value::from("col_a"),
                Value::from("col_b"),
                Value::from("col_c"),
            ])
        }
    }

    /// Stands in for the common shape of a Parse-mode adapter stub: an empty
    /// collection with no real items to draw a representative from (e.g.
    /// `get_columns_in_relation`'s `empty_vec_value()`).
    #[derive(Debug)]
    struct EmptyRelation;

    impl Object for EmptyRelation {
        fn repr(self: &Arc<Self>) -> ObjectRepr {
            ObjectRepr::Seq
        }

        fn enumerate(self: &Arc<Self>) -> Enumerator {
            Enumerator::Values(vec![])
        }
    }

    fn test_env() -> Environment<'static> {
        let mut env = Environment::new();
        env.add_function("is_tainted", |v: Value| v.is_introspective_stub());
        env
    }

    /// Minimal listener that opts into introspective-hole/branch handling,
    /// standing in for `DefaultRenderingEventListener` (defined in
    /// `dbt-jinja-utils`, which this crate does not depend on).
    #[derive(Debug)]
    struct TaintGateListener;

    impl RenderingEventListener for TaintGateListener {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn name(&self) -> &str {
            "TaintGateListener"
        }

        fn on_macro_start(
            &self,
            _file_path: Option<&std::path::Path>,
            _line: &u32,
            _col: &u32,
            _offset: &u32,
        ) {
        }

        fn on_macro_stop(
            &self,
            _file_path: Option<&std::path::Path>,
            _line: &u32,
            _col: &u32,
            _offset: &u32,
        ) {
        }

        fn on_malicious_return(&self, _location: &crate::CodeLocation) {}

        fn on_function_start(&self) {}

        fn on_function_end(&self) {}

        fn wants_introspective_holes(&self) -> bool {
            true
        }
    }

    fn taint_gate() -> Vec<Rc<dyn RenderingEventListener>> {
        vec![Rc::new(TaintGateListener)]
    }

    #[test]
    fn wrapped_value_is_introspective_stub() {
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        assert!(wrapped.is_introspective_stub());
    }

    #[test]
    fn get_attr_propagates_taint_and_forwards_value() {
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let name = wrapped.get_attr("name").unwrap();
        assert!(name.is_introspective_stub());
        assert_eq!(name.to_string(), "col_a");
    }

    #[test]
    fn attribute_access_on_item_from_empty_collection_stays_tainted() {
        // Regression test: iterating a tainted, genuinely EMPTY stub (the
        // common case -- most Parse-mode introspective methods return an
        // empty list/table) must still yield a tainted representative item,
        // and further attribute access on that item (e.g. `col.name` in
        // `{% for col in adapter.get_columns_in_relation(this) %}`) must
        // stay tainted rather than silently falling back to a plain
        // (untainted) `undefined` -- which would render as an empty string
        // and leave broken SQL behind even in `Symbolic` mode.
        let env = test_env();
        let wrapped = IntrospectiveValue::wrap(Value::from_object(EmptyRelation));
        let out = env
            .render_str(
                "{% for col in rel %}[{{ col.name }}:{{ col }}]{% endfor %}",
                context! { rel => wrapped },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "[{{}}:{{}}]");
    }

    #[test]
    fn enumerate_yields_exactly_one_tainted_item() {
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let items: Vec<Value> = wrapped.try_iter().unwrap().collect();
        assert_eq!(items.len(), 1);
        assert!(items[0].is_introspective_stub());
        // The item is a content-free placeholder, not a real element drawn
        // from `TestRelation` -- see `IntrospectiveValue::iterable`'s doc
        // comment for why pulling in real (potentially deep) nested data
        // here is unsafe.
        assert_eq!(items[0].to_string(), "");
    }

    #[test]
    fn item_from_enumerate_is_not_itself_iterable() {
        // The single item `enumerate` hands out must not claim to be
        // iterable itself, or hashing a tainted value would recurse forever
        // (every nested item would claim to contain exactly one further
        // nested item, regardless of what it wraps).
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let item = wrapped.try_iter().unwrap().next().unwrap();
        assert!(item.try_iter().unwrap().next().is_none());
    }

    #[test]
    fn hashing_a_tainted_value_terminates() {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let mut hasher = DefaultHasher::new();
        // Regression test for a real stack overflow found by running the
        // Symbolic linter over a ~1500-model project: hashing a tainted
        // value (e.g. via a `HashSet`/`HashMap` elsewhere in the rendering
        // pipeline) must terminate.
        wrapped.hash(&mut hasher);
        let _ = hasher.finish();
    }

    #[test]
    fn call_method_propagates_taint_and_forwards_result() {
        let env = test_env();
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let out = env
            .render_str(
                "{{ is_tainted(rel.upper_name()) }}|{{ rel.upper_name() }}",
                context! { rel => wrapped },
                &[],
            )
            .unwrap();
        assert_eq!(out, "True|COL_A");
    }

    #[test]
    fn for_loop_over_tainted_value_iterates_once_with_tainted_item() {
        let env = test_env();
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let out = env
            .render_str(
                "{% for x in rel %}[{{ is_tainted(x) }}:{{ x }}]{% endfor %}",
                context! { rel => wrapped },
                &[],
            )
            .unwrap();
        assert_eq!(out, "[True:]");
    }

    #[test]
    fn tuple_unpack_of_tainted_value_yields_tainted_items_instead_of_erroring() {
        // Regression test: `{% set a, b = ... %}` compiles to the same
        // `UnpackList` instruction used to unpack each `{% for a, b in
        // list %}` item, but unlike a for-loop it requires an exact arity
        // match. Before the fix, unpacking a tainted origin (whose
        // `enumerate()` always reports exactly one item, see `iterable`'s
        // doc comment) against any arity other than 1 failed with "cannot
        // unpack: sequence of wrong length" -- e.g. dbt-adapters'
        // `{% set res, table = adapter.execute(...) %}` in `statement()`.
        let env = test_env();
        let wrapped = IntrospectiveValue::wrap(Value::from_object(TestRelation));
        let out = env
            .render_str(
                "{% set a, b = rel %}{{ is_tainted(a) }},{{ is_tainted(b) }}",
                context! { rel => wrapped },
                &[],
            )
            .unwrap();
        assert_eq!(out, "True,True");
    }

    #[test]
    fn and_short_circuit_respects_a_real_always_false_rhs() {
        // Regression test: `is_incremental() and flags.WHICH != "lint"`. The
        // LHS is a tainted stub whose fake default is falsy (Parse-mode: the
        // relation doesn't exist), and the RHS is a *real*, always-false
        // comparison (flags.WHICH really is "lint" during lint). In every
        // real invocation this `and` is false, so a guarded
        // `{% call statement() %}` must never run during lint.
        //
        // Before the fix, `and`'s short-circuit point (`JumpIfFalseOrPop`)
        // decided whether to short-circuit purely from the tainted LHS's fake
        // (falsy) value, without ever consulting branch-override -- so when a
        // later exploration pass wanted to treat the LHS as truthy (to check
        // the "is_incremental() is true" branch for real parse errors), it
        // short-circuited on the LHS's *original* fake-falsy value regardless,
        // and the real RHS was never evaluated as part of that exploration.
        let env = test_env();
        let out = env
            .render_str(
                "{% if incr and which != 'lint' %}RAN{% endif %}",
                context! {
                    incr => IntrospectiveValue::wrap(Value::from(false)),
                    which => Value::from("lint"),
                },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "");
    }

    #[test]
    fn or_short_circuit_takes_the_explored_branch_over_a_real_false_rhs() {
        // Mirrors `and_short_circuit_respects_a_real_always_false_rhs` for
        // `or`'s short-circuit point (`JumpIfTrueOrPop`). The tainted LHS's
        // fake default is falsy, but exploration (`TaintGateListener`, like
        // the production Symbolic listener) wants to check the "LHS is true"
        // branch. `or` short-circuits once its LHS is true regardless of the
        // RHS, so this must render as if the LHS really were true -- without
        // this fix, the short-circuit point ignored exploration and used the
        // fake-falsy value directly, continuing on to (and being decided by)
        // a real, always-false RHS instead.
        let env = test_env();
        let out = env
            .render_str(
                "{% if incr or which == 'lint' %}RAN{% endif %}",
                context! {
                    incr => IntrospectiveValue::wrap(Value::from(false)),
                    which => Value::from("nope"),
                },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "RAN");
    }

    #[test]
    fn unpack_preserves_real_items_when_arity_matches() {
        // Review follow-up on `tuple_unpack_of_tainted_value_yields_
        // tainted_items_instead_of_erroring`: substituting bare `undefined`
        // for every unpacked item would "kick the can down the road" for
        // callers that need a *concrete* type back out, not just a taint
        // marker -- e.g. `adapter.execute()`'s Parse-mode stub really is a
        // `(response, table)` pair of correctly-typed defaults (not empty
        // placeholders), and `{{ store_result(name, response=res,
        // agate_table=table) }}` immediately downstream in `statement()`
        // downcasts `table` to a concrete `AgateTable`. When the fabricated
        // inner value's real arity already matches, unpack should hand
        // back those real (tainted-leaf-wrapped) items instead of discarding
        // them.
        let wrapped = IntrospectiveValue {
            inner: Value::from(vec![Value::from("a-response"), Value::from("a-table")]),
            iterable: true,
        };
        let items = wrapped.unpack(2);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|v| v.is_introspective_stub()));
        assert_eq!(items[0].to_string(), "a-response");
        assert_eq!(items[1].to_string(), "a-table");
    }

    #[test]
    fn unpack_falls_back_to_undefined_leaves_when_arity_does_not_match() {
        // The common Parse-mode case: an empty/scalar fabricated inner
        // value can't supply `count` real items, so unpack must still
        // produce exactly `count` tainted placeholders rather than
        // erroring or under/over-supplying the stack.
        let wrapped = IntrospectiveValue {
            inner: Value::from(vec![Value::from("only-one")]),
            iterable: true,
        };
        let items = wrapped.unpack(2);
        assert_eq!(items.len(), 2);
        assert!(items.iter().all(|v| v.is_introspective_stub()));
        assert_eq!(items[0].to_string(), "");
        assert_eq!(items[1].to_string(), "");
    }

    #[test]
    fn slicing_a_tainted_value_propagates_taint_instead_of_erroring() {
        // Regression test: `ops::slice` only understands `ObjectRepr::Seq`/
        // `Iterable` (or scalars it knows how to index); a tainted value's
        // fabricated inner value is usually neither (e.g. a `None` stub,
        // `ObjectRepr::Plain`), which previously failed with "value of type
        // plain object cannot be sliced" instead of propagating taint like
        // every other operator.
        let env = test_env();
        let out = env
            .render_str(
                "{{ s[0:2] }}",
                context! { s => IntrospectiveValue::wrap(Value::from(())) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}");
    }

    #[test]
    fn untainted_object_is_not_introspective_stub() {
        assert!(!Value::from_object(TestRelation).is_introspective_stub());
    }

    #[test]
    fn binary_arithmetic_with_tainted_operand_propagates_taint() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ 1 + n }}|{{ n + 1 }}",
                context! { n => IntrospectiveValue::wrap(Value::from(41)) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}|{{}}");
    }

    #[test]
    fn string_concat_with_tainted_operand_propagates_taint() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ 'prefix_' ~ s }}",
                context! { s => IntrospectiveValue::wrap(Value::from("col")) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}");
    }

    #[test]
    fn comparison_with_tainted_operand_propagates_taint_instead_of_bool() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ s == 'col' }}",
                context! { s => IntrospectiveValue::wrap(Value::from("col")) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}");
    }

    #[test]
    fn filter_on_tainted_value_propagates_taint_and_skips_real_filter() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ s | upper }}",
                context! { s => IntrospectiveValue::wrap(Value::from("col")) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}");
    }

    #[test]
    fn macro_call_taints_result_when_an_argument_is_tainted() {
        // Regression test for a real false negative found on a production
        // project: a macro like dbt_utils.pivot() takes a tainted `values`
        // list, iterates it internally (each item individually stays
        // tainted per the for-loop rule), but absorbs those items into a
        // plain, untainted list via `namespace()`/`.append()` before
        // joining and emitting the result *inside its own body*. That inner
        // emit sees an untainted list (only its elements were tainted), so
        // it does not become a hole -- silently rendering wrong/empty SQL.
        // The call-boundary rule (any tainted argument taints the overall
        // result) catches this at the *outer* call site instead, regardless
        // of what happened inside the macro.
        let env_source = "\
{%- macro pivot(values) -%}\
{%- set ns = namespace(parts=[]) -%}\
{%- for v in values -%}\
{%- set _ = ns.parts.append(v ~ '_x') -%}\
{%- endfor -%}\
{{ ns.parts | join(', ') }}\
{%- endmacro -%}\
select {{ pivot(vals) }} from t";
        let env = test_env();
        let out = env
            .render_str(
                env_source,
                context! { vals => IntrospectiveValue::wrap(Value::from(Vec::<Value>::new())) },
                &taint_gate(),
            )
            .unwrap();
        // The outer `{{ pivot(vals) }}` becomes a hole because the call
        // result is tainted (via the call-boundary rule), even though
        // `pivot`'s own internal `{{ ns.parts | join(', ') }}` emit did not
        // see a tainted value directly.
        assert_eq!(out, "select {{}} from t");
    }

    #[test]
    fn test_on_tainted_value_propagates_taint_instead_of_bool() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ s is none }}",
                context! { s => IntrospectiveValue::wrap(Value::from(())) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "{{}}");
    }

    #[test]
    fn untainted_values_are_unaffected() {
        let env = test_env();
        let out = env
            .render_str(
                "{{ 1 + 1 }}|{{ 'a' ~ 'b' }}|{{ 'x' == 'x' }}|{{ 'x' | upper }}|{{ none is none }}",
                context! {},
                &[],
            )
            .unwrap();
        assert_eq!(out, "2|ab|True|X|True");
    }

    #[test]
    fn appending_a_tainted_item_taints_the_list() {
        // Regression test for a real false negative found on a production
        // project: macros like `get_filtered_columns_in_relation`/
        // `dbt_utils.unpivot` build up a plain list via
        // `{% set cols = [] %}` + `.append()` from items drawn from an
        // internal (not argument-passed) introspective call, then return
        // the list directly. Unlike `macro_call_taints_result_when_an_
        // argument_is_tainted`, there is no tainted *argument* here to
        // trigger the call-boundary rule -- the taint must be absorbed by
        // the container itself as tainted items are appended into it.
        let env = test_env();
        let out = env
            .render_str(
                "{%- set cols = [] -%}\
{%- for c in rel -%}\
{%- set _ = cols.append(c) -%}\
{%- endfor -%}\
{{ is_tainted(cols) }}",
                context! { rel => IntrospectiveValue::wrap(Value::from_object(TestRelation)) },
                &[],
            )
            .unwrap();
        assert_eq!(out, "True");
    }

    #[test]
    fn removing_an_absent_value_from_a_tainted_list_degrades_instead_of_erroring() {
        // Regression test for a real production failure: a macro builds a
        // column-order list from a tainted `adapter.get_columns_in_relation()`
        // result (tainting the list itself, per
        // `appending_a_tainted_item_taints_the_list`), then does
        // `col_order.remove('_DBT_SOURCE_RELATION')` assuming that column is
        // present. Because the list's real contents are unreliable once
        // tainted, `remove()` can legitimately find nothing to remove and
        // must degrade like every other introspective-stub operation
        // (`IntrospectiveValue::call`/`call_method`) instead of raising a
        // hard "value not found in list" render error.
        let env = test_env();
        let out = env
            .render_str(
                "{%- set cols = [] -%}\
{%- for c in rel -%}\
{%- set _ = cols.append(c) -%}\
{%- endfor -%}\
{%- set _ = cols.remove('not_actually_there') -%}\
done",
                context! { rel => IntrospectiveValue::wrap(Value::from_object(TestRelation)) },
                &[],
            )
            .unwrap();
        assert_eq!(out, "done");
    }

    #[test]
    fn removing_an_absent_value_from_an_untainted_list_still_errors() {
        // Contrast with the above: a plain (never-tainted) list must keep
        // raising a hard error for a genuine "value not found" -- the new
        // leniency in `remove_impl` only applies once the container has
        // actually absorbed fabricated data.
        let env = test_env();
        let err = env
            .render_str(
                "{%- set cols = ['a', 'b'] -%}\
{%- set _ = cols.remove('c') -%}\
done",
                context! {},
                &[],
            )
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidOperation);
    }

    #[test]
    fn calling_a_native_function_with_a_tainted_argument_degrades_instead_of_erroring() {
        // Regression test for a real production failure: a macro sizes a
        // `{% for i in range(results['col']|length) %}` loop off a value
        // derived from an introspective query result. `|length` on a
        // tainted operand short-circuits to the tainted operand itself
        // (`ApplyFilter`'s override handling), which previously then hit
        // `range()`'s real Rust implementation and failed to convert to
        // `i32` with "cannot convert plain object to i32" -- a hard render
        // error, unlike every other introspective-stub operation. Plain
        // (non-macro) function calls now get the same override
        // short-circuit filters/tests/`in` already have.
        let env = test_env();
        let out = env
            .render_str(
                "{%- set d = {} -%}\
{%- for c in rel -%}\
{%- set _ = d.update({'col': c}) -%}\
{%- endfor -%}\
{%- for i in range(d['col'] | length) -%}X{%- endfor -%}\
done",
                context! { rel => IntrospectiveValue::wrap(Value::from_object(TestRelation)) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "done");
    }

    #[test]
    fn macro_call_with_a_tainted_argument_skips_the_body_instead_of_executing_it() {
        // Regression test for a real user report: `dbt_utils.date_spine`
        // passes a tainted `upper_bound` (force-tainted, per
        // `is_known_introspective_macro`, since it came from a macro that
        // touches `statement()`) into `dbt_utils.generate_series`, whose
        // body calls `get_powers_of_two`, which raises
        // `exceptions.raise_compiler_error("upper bound must be positive")`
        // for a non-positive value (stood in for here by `fail()`, since
        // `exceptions` lives in `dbt-jinja-utils`, which this crate doesn't
        // depend on). A macro call used to always execute its body for real
        // even with a tainted argument (see the removed
        // `macro_call_still_executes_and_absorbs_taint_from_an_argument`),
        // so this fabricated stand-in could actually reach that guard and
        // fail the whole render -- a call the real run (with a live
        // warehouse) would never make. Since a tainted argument means
        // *everything* the macro could compute from it is equally
        // unknowable, skipping the body outright (mirroring how a plain
        // native function call already short-circuits on a tainted
        // argument) avoids ever reaching a guard like this.
        let mut env = test_env();
        env.add_function("fail", |msg: String| -> Result<Value, Error> {
            Err(Error::new(ErrorKind::InvalidOperation, msg))
        });
        let env_source = "\
{%- macro boom(n) -%}\
{%- if n <= 0 -%}\
{{ fail(\"n must be positive\") }}\
{%- endif -%}\
ok\
{%- endmacro -%}\
select {{ boom(n) }} from t";
        let out = env
            .render_str(
                env_source,
                context! { n => IntrospectiveValue::wrap(Value::from(0)) },
                &taint_gate(),
            )
            .unwrap();
        assert_eq!(out, "select {{}} from t");
    }

    #[test]
    fn updating_a_map_with_a_tainted_value_taints_the_map() {
        // Mirrors `appending_a_tainted_item_taints_the_list` for the dict
        // side of the same idiom (`namespace()`/`{% set d = {} %}` +
        // `.update()`/`.setdefault()`).
        let env = test_env();
        let out = env
            .render_str(
                "{%- set d = {} -%}\
{%- for c in rel -%}\
{%- set _ = d.update({'col': c}) -%}\
{%- endfor -%}\
{{ is_tainted(d) }}",
                context! { rel => IntrospectiveValue::wrap(Value::from_object(TestRelation)) },
                &[],
            )
            .unwrap();
        assert_eq!(out, "True");
    }

    #[test]
    fn binary_op_taint_short_circuit_is_gated_off_by_default() {
        // Without a listener opting in via `wants_introspective_holes`,
        // arithmetic on a tainted value falls through to the real operator
        // on the fake inner (`Object`-wrapped) value exactly as it would for
        // any other custom object today -- here that means the same
        // `InvalidOperation` error a non-numeric object would already
        // produce, not a new failure mode. This is what keeps tainting the
        // adapter's introspective methods (a later step) from silently
        // changing behavior for every render mode other than
        // `JinjaRenderMode::Symbolic`.
        let env = test_env();
        let err = env
            .render_str(
                "{{ is_tainted(1 + n) }}",
                context! { n => IntrospectiveValue::wrap(Value::from(41)) },
                &[],
            )
            .unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidOperation);
    }
}
