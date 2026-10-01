use minijinja::Value;
use minijinja::value::Object;
use std::collections::HashMap;
use std::ops::Deref;
use std::sync::{Mutex, MutexGuard};

type HandlerFn = Box<dyn Fn(&[Value]) -> Result<Value, minijinja::Error> + Send + Sync>;

/// A recorded `call_method` invocation on a Jinja object.
#[derive(Debug, Clone)]
/// A recorded `call_method` invocation on a Jinja object.
/// Used to track method calls and arguments for test assertions.
pub struct JinjaCall {
    /// The method name invoked on the mock object.
    pub method: String,
    /// The arguments passed to the method.
    pub args: Vec<Value>,
}

/// A generic MagicMock-style `minijinja::value::Object`.
///
/// Inject this into a Jinja environment as any global (e.g. `adapter`,
/// `config`, or any custom object). Every `call_method` invocation is
/// recorded and can be asserted on after rendering.
///
/// - **Recording**: Every method call is recorded with name + raw args.
/// - **Handlers**: Register a response handler per method name via [`on`](Self::on).
///   If a handler exists, its return value is used. Handlers receive the
///   raw `&[Value]` args and handle as needed.
/// - **Attributes**: Register static attribute values via [`set_attr`](Self::set_attr).
///   These are returned by `get_value` for property-style access (e.g. `obj.field`).
/// - **Unhandled access**: Panics on any unregistered method call or attribute
///   access, so tests fail loudly when macros hit unmocked methods.
///
/// # Example
///
/// ```ignore
/// use std::sync::Arc;
/// use dbt_jinja_utils::mock_object::MockJinjaObject;
///
/// let mock = Arc::new(MockJinjaObject::new());
///
/// // Static response
/// mock.on("get_name", |_args| Ok(Value::from("Alice")));
///
/// // Attribute access (obj.status in Jinja)
/// mock.set_attr("status", Value::from("active"));
///
/// // Branch on arguments
/// mock.on("lookup", |args| {
///     let key = args.first().and_then(|v| v.as_str());
///     match key {
///         Some("users") => Ok(Value::from(42)),
///         _ => Ok(Value::UNDEFINED),
///     }
/// });
///
/// // Inject into a Jinja environment and render
/// env.add_global("my_obj", Value::from_dyn_object(mock.clone()));
/// env.render_str("{{ my_obj.get_name() }}", ctx, &[]).unwrap();
///
/// // Assert on recorded calls (single lock for all assertions)
/// let calls = mock.observed_calls();
/// calls.assert_called("get_name");
/// calls.assert_not_called("missing_method");
/// assert_eq!(calls.to("get_name").count(), 1);
/// ```
pub struct MockJinjaObject {
    calls: Mutex<Vec<JinjaCall>>,
    handlers: Mutex<HashMap<&'static str, HandlerFn>>,
    attributes: Mutex<HashMap<&'static str, Value>>,
}

impl MockJinjaObject {
    /// Create a new [MockJinjaObject].
    ///
    /// Panics on any `call_method` without a registered handler and on any
    /// `get_value` for an unregistered attribute, so tests fail loudly when
    /// macros hit unmocked methods.
    pub fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            handlers: Mutex::new(HashMap::new()),
            attributes: Mutex::new(HashMap::new()),
        }
    }

    /// Register a response handler for a specific method name.
    pub fn on<F>(&self, method: &'static str, handler: F) -> &Self
    where
        F: Fn(&[Value]) -> Result<Value, minijinja::Error> + Send + Sync + 'static,
    {
        self.handlers
            .lock()
            .unwrap()
            .insert(method, Box::new(handler));
        self
    }

    /// Register a static attribute value for property-style access.
    ///
    /// When Jinja evaluates `obj.field`, it calls `get_value("field")` on the
    /// underlying Object. Attributes registered here are returned by that path.
    pub fn set_attr(&self, name: &'static str, value: Value) -> &Self {
        self.attributes.lock().unwrap().insert(name, value);
        self
    }

    /// Acquire the lock on recorded calls and return a guard wrapper.
    ///
    /// The returned [`ObservedCalls`] holds the mutex and dereferences to
    /// `[JinjaCall]`, so all inspection happens without cloning. Drop it
    /// to release the lock.
    pub fn observed_calls(&self) -> ObservedCalls<'_> {
        ObservedCalls {
            guard: self.calls.lock().unwrap(),
        }
    }
}

/// A held lock over the mock's recorded calls.
///
/// Dereferences to `[JinjaCall]`, giving zero-copy access to the full
/// call log. Drop this value to release the lock.
pub struct ObservedCalls<'a> {
    guard: MutexGuard<'a, Vec<JinjaCall>>,
}

impl Deref for ObservedCalls<'_> {
    type Target = [JinjaCall];

    fn deref(&self) -> &[JinjaCall] {
        &self.guard
    }
}

impl<'a> ObservedCalls<'a> {
    /// Iterate over calls to a specific method.
    pub fn to(&self, method: &str) -> impl Iterator<Item = &JinjaCall> {
        self.iter().filter(move |c| c.method == method)
    }

    /// Assert at least one call was made to the given method.
    pub fn assert_called(&self, method: &str) {
        assert!(
            self.to(method).next().is_some(),
            "Expected at least one call to '{method}', but none were made",
        );
    }

    /// Assert no calls to a specific method were made.
    pub fn assert_not_called(&self, method: &str) {
        if self.to(method).next().is_some() {
            let unexpected: Vec<_> = self.to(method).collect();
            panic!(
                "Expected no calls to '{method}', but got {}: {unexpected:?}",
                unexpected.len(),
            );
        }
    }
}

impl std::fmt::Debug for ObservedCalls<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl Default for MockJinjaObject {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MockJinjaObject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let handlers: Vec<_> = self.handlers.lock().unwrap().keys().copied().collect();
        let attributes: Vec<_> = self.attributes.lock().unwrap().keys().copied().collect();
        let call_count = self.calls.lock().unwrap().len();
        f.debug_struct("MockJinjaObject")
            .field("handlers", &handlers)
            .field("attributes", &attributes)
            .field("calls", &call_count)
            .finish()
    }
}

impl Object for MockJinjaObject {
    fn get_value(self: &std::sync::Arc<Self>, key: &Value) -> Option<Value> {
        let key_str = key.as_str()?;
        let result = self.attributes.lock().unwrap().get(key_str).cloned();
        if result.is_none() {
            panic!(
                "MockJinjaObject: no attribute registered for '{key_str}'. \
                 Register it with mock.set_attr(\"{key_str}\", value)."
            );
        }
        result
    }

    fn call_method(
        self: &std::sync::Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        {
            let method = name.to_string();
            let args = args.to_vec();
            let call = JinjaCall { method, args };
            let mut calls = self.calls.lock().unwrap();
            calls.push(call);
        }

        if let Some(handler) = self.handlers.lock().unwrap().get(name) {
            return handler(args);
        }

        panic!(
            "MockJinjaObject: no handler registered for method '{name}'. \
             Register one with mock.on(\"{name}\", |args| ...)."
        );
    }
}
