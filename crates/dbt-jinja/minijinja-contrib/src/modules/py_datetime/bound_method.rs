use std::fmt;
use std::rc::Rc;
use std::sync::Arc;

use minijinja::listener::RenderingEventListener;
use minijinja::value::Object;
use minijinja::{Error, State, Value};

/// A callable that captures a receiver object and the name of one of its
/// methods, so it can be passed around and invoked later — mirroring Python's
/// bound methods (`obj.method`).
#[derive(Clone)]
pub struct BoundMethod {
    pub receiver: Value,
    pub method: String,
}

impl BoundMethod {
    pub fn new(receiver: Value, method: impl Into<String>) -> Self {
        Self {
            receiver,
            method: method.into(),
        }
    }
}

impl fmt::Debug for BoundMethod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<bound method {}>", self.method)
    }
}

impl Object for BoundMethod {
    fn call(
        self: &Arc<Self>,
        state: &State<'_, '_>,
        args: &[Value],
        listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<Value, Error> {
        self.receiver
            .call_method(state, &self.method, args, listeners)
    }
}
