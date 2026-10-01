//! Cache key for use in suspend vm errors
use crate::Value;

/// Cache key for execution cache
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    node_id: String,
    function_name: String,
    args: Vec<Value>,
}

impl CacheKey {
    /// Creates a new cache key from a string
    pub fn new<S1, S2>(node_id: S1, function_name: S2, args: Vec<Value>) -> Self
    where
        S1: Into<String>,
        S2: Into<String>,
    {
        Self {
            node_id: node_id.into(),
            function_name: function_name.into(),
            args,
        }
    }

    /// Returns the node id
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Returns the function name
    pub fn function_name(&self) -> &str {
        &self.function_name
    }

    /// Returns the input arguments
    pub fn args(&self) -> &[Value] {
        &self.args
    }
}
