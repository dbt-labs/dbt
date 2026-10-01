//! This module implements `DynJinjaObject`, dynamic Jinja object that can have methods,
//! values and state attached to it at runtime without the need for Rust structs, `Serialize`
//! or other boilerplate code.

use minijinja::{
    listener::RenderingEventListener,
    value::{Enumerator, Object, Value, ValueMap},
};
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::{Send, Sync};
use std::rc::Rc;
use std::sync::Arc;

type JinjaCallback<O> = fn(
    obj: &Arc<O>,
    state: &minijinja::State,
    args: &[Value],
    listeners: &[Rc<dyn RenderingEventListener>],
) -> Result<Value, minijinja::Error>;

/// `DynJinjaObject` can be used to implement simple objects in Jinja without the need for new
/// Rust types that derive `Serialize` or `impl Object`. You can think of it as a `ValueMap` that
/// can have custom methods and private state attached to it at runtime via a builder interface.
///
/// The main use case is for simple "intermediate" objects with very simple behaviors. If you need
/// to implement a complex object with many methods and/or complicated state management, it is
/// still preferred to use its own `struct` for that.
#[derive(Debug)]
pub struct DynJinjaObject<S: Sync + Send + Debug, R: Object> {
    /// The type name of this object, used in error messages
    type_name: &'static str,
    /// The private state of this object. Won't get exposed to the Jinja runtime
    state: S,
    /// The public repr of this object. Will get exposed to the Jinja runtime as-is via its `impl
    /// Object`.
    repr: Arc<R>,
    /// If present, this method will be called when the object is used as a callable, like `obj()`
    callback: Option<JinjaCallback<Self>>,
    /// Methods in this map will be used as methods of this object, like `obj.my_method()`
    methods: HashMap<&'static str, JinjaCallback<Self>>,
}

impl<S: Sync + Send + Debug + Default, R: Object + Default> DynJinjaObject<S, R> {
    /// Create a new empty `DynJinjaObject` with the specified type name
    pub fn empty(type_name: &'static str) -> Self {
        Self {
            type_name,
            state: Default::default(),
            repr: Default::default(),
            methods: Default::default(),
            callback: Default::default(),
        }
    }
}

impl<S: Sync + Send + Debug, R: Object> DynJinjaObject<S, R> {
    /// Create a new `DynJinjaObject` with the specified type name, state and repr
    pub fn new(type_name: &'static str, state: S, repr: R) -> Self {
        Self::new_arc(type_name, state, Arc::new(repr))
    }

    /// Create a new `DynJinjaObject` with the specified type name, state and repr
    pub fn new_arc(type_name: &'static str, state: S, repr: Arc<R>) -> Self {
        Self {
            type_name,
            state,
            repr,
            callback: None,
            methods: Default::default(),
        }
    }

    /// Replace this object's private state
    pub fn with_state(mut self, state: S) -> Self {
        self.state = state;
        self
    }

    /// Replace this object's public repr
    pub fn with_repr(mut self, repr: R) -> Self {
        self.repr = Arc::new(repr);
        self
    }

    /// Make this object callable by providing a callback
    pub fn with_callback(mut self, callback: JinjaCallback<Self>) -> Self {
        self.callback = Some(callback);
        self
    }

    /// Attach a named method to this object
    pub fn with_method(mut self, name: &'static str, method: JinjaCallback<Self>) -> Self {
        self.methods.insert(name, method);
        self
    }

    /// Get the object's type name
    pub fn type_name(&self) -> &'static str {
        self.type_name
    }

    /// Get the object's private state
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Get the object's public repr
    pub fn repr_ref(&self) -> &R {
        self.repr.as_ref()
    }
}

impl<S: Sync + Send + Debug, R: Object> Object for DynJinjaObject<S, R> {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        self.repr.get_value(key)
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        self.repr.enumerate()
    }

    fn enumerator_len(self: &Arc<Self>) -> Option<usize> {
        self.repr.enumerator_len()
    }

    fn is_true(self: &Arc<Self>) -> bool {
        self.repr.is_true()
    }

    fn call(
        self: &Arc<Self>,
        state: &minijinja::State,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match self.callback {
            Some(callback) => callback(self, state, args, listeners),
            None => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                format!("'{}' object is not callable", self.type_name),
            )),
        }
    }

    fn call_method(
        self: &Arc<Self>,
        state: &minijinja::State<'_, '_>,
        name: &str,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match self.methods.get(name) {
            Some(method) => method(self, state, args, listeners),
            None => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("'{}' object has no method '{}'", self.type_name, name),
            )),
        }
    }
}

impl<K: Debug + Send + Sync + Eq + std::hash::Hash, V: Debug + Send + Sync, R: Object>
    DynJinjaObject<HashMap<K, V>, R>
{
    /// Add or replace a key/value pair in the private state
    pub fn with_state_entry(mut self, key: K, value: V) -> Self {
        self.state.insert(key, value);
        self
    }
}

impl<S: Debug + Send + Sync> DynJinjaObject<S, ValueMap> {
    /// Add or replace a key/value pair in the public repr
    pub fn with_repr_entry(mut self, key: impl Into<Value>, value: impl Into<Value>) -> Self {
        let Some(mut_ref) = Arc::get_mut(&mut self.repr) else {
            panic!("There should be no clones of the repr Arc")
        };
        mut_ref.insert(key.into(), value.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::jinja_assert;

    #[test]
    fn simple() {
        let obj = DynJinjaObject::<u8, ValueMap>::empty("MyObject")
            .with_state(111)
            .with_repr(ValueMap::from([
                ("num".into(), 222.into()),
                ("string".into(), "test".into()),
            ]))
            .with_callback(|obj, _state, _args, _listeners| {
                Ok(Value::from(format!("callback(state={})", obj.state)))
            })
            .with_method("my_method", |_obj, _state, args, _listeners| {
                let mut result = "my_method(".to_string();

                for arg in args {
                    result.push_str(arg.to_string().as_str());
                    result.push(',');
                }

                result.pop();
                result.push(')');

                Ok(Value::from(result))
            });

        let template = "
        obj            : {{ obj }}
        num            : {{ obj.num }}
        string         : {{ obj.string }}
        my_state       : '{{ obj.my_state }}'
        obj()          : {{ obj() }}
        obj.my_method(): {{ obj.my_method(obj.num, 333) }}
        <keyvals>
        {% for key in obj %}
            ({{ key }}, {{ obj[key] }})
        {% endfor %}
        </keyvals>
        ";

        let expect = "
        obj            : {'num': 222, 'string': 'test'}
        num            : 222
        string         : test
        my_state       : ''
        obj()          : callback(state=111)
        obj.my_method(): my_method(222,333)
        <keyvals>
        (num, 222)
        (string, test)
        </keyvals>
        ";

        jinja_assert(obj, template, expect);
    }

    #[test]
    fn nested() {
        let obj = DynJinjaObject::<(), ValueMap>::empty("Parent")
            .with_repr(ValueMap::from([(
                "child".into(),
                Value::from_object(
                    DynJinjaObject::<(), ValueMap>::empty("Child")
                        .with_repr(ValueMap::from([("num".into(), 111.into())]))
                        .with_callback(|_obj, _state, _args, _listeners| {
                            Ok(Value::from("child_callback"))
                        }),
                ),
            )]))
            .with_callback(|_obj, _state, _args, _listeners| Ok(Value::from("parent_callback")));

        let template = "
        obj          : {{ obj }}
        obj()        : {{ obj() }}
        obj.child    : {{ obj.child }}
        obj.child.num: {{ obj.child.num }}
        {% set child = obj.child %}
        child()      : {{ child() }}
        ";

        let expect = "
        obj          : {'child': {'num': 111}}
        obj()        : parent_callback
        obj.child    : {'num': 111}
        obj.child.num: 111
        child()      : child_callback
        ";

        jinja_assert(obj, template, expect);
    }
}
