use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use minijinja::value::{from_args, Kwargs, Object, Value};
use minijinja::{Environment, Error};

/// A copy-on-write object that holds an assembled query.
#[derive(Debug, Clone)]
pub struct Query {
    table: Arc<str>,
    filters: Arc<HashMap<String, Value>>,
    limit: Option<usize>,
    offset: Option<usize>,
}

impl Query {
    /// Creates an empty query object for a specific table.
    fn new(table: String) -> Self {
        Query {
            table: Arc::from(table),
            filters: Default::default(),
            limit: None,
            offset: None,
        }
    }

    /// Filters the query down by the given keyword arguments.
    fn filter(&self, kwargs: Kwargs) -> Self {
        let mut rv = self.clone();
        let filters_mut = Arc::make_mut(&mut rv.filters);
        for arg in kwargs.args() {
            filters_mut.insert(arg.to_string(), kwargs.get::<Value>(arg).unwrap());
        }
        rv
    }

    /// Limits the query to `count` rows.
    fn limit(&self, count: usize) -> Self {
        let mut rv = self.clone();
        rv.limit = Some(count);
        rv
    }

    /// Offsets the query by `count` rows.
    fn offset(&self, count: usize) -> Self {
        let mut rv = self.clone();
        rv.offset = Some(count);
        rv
    }
}

impl Object for Query {
    /// Implements a method dispatch for the query so it can be further reduced.
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "filter" => {
                let (kwargs,) = from_args(args)?;
                Ok(Value::from_object(self.filter(kwargs)))
            }
            "limit" => {
                let (limit,) = from_args(args)?;
                Ok(Value::from_object(self.limit(limit)))
            }
            "offset" => {
                let (offset,) = from_args(args)?;
                Ok(Value::from_object(self.offset(offset)))
            }
            _ => Err(minijinja::Error::from(minijinja::ErrorKind::UnknownMethod)),
        }
    }

    fn render(self: &Arc<Self>, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<Query table={:?}>", self.table)
    }
}

/// Like [`Query::new`] but wraps it in a [`Value`].
fn query(table: String) -> Value {
    Value::from_object(Query::new(table))
}

/// Filters a query by some keyword arguments as filter function.
fn filter_filter(obj: &Query, kwargs: Kwargs) -> Result<Value, Error> {
    Ok(Value::from_object(obj.filter(kwargs)))
}

/// Applies a limit to a query as filter function.
fn limit_filter(obj: &Query, limit: usize) -> Result<Value, Error> {
    Ok(Value::from_object(obj.limit(limit)))
}

/// Applies an offset to a query as filter function.
fn offset_filter(obj: &Query, offset: usize) -> Result<Value, Error> {
    Ok(Value::from_object(obj.offset(offset)))
}

fn main() {
    let mut env = Environment::default();
    env.add_function("query", query);

    // alternative approach with filters
    env.add_filter("filter", filter_filter);
    env.add_filter("limit", limit_filter);
    env.add_filter("offset", offset_filter);

    let expr = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("no filter provided, using default one");
        "query('my_table').filter(is_active=true)".into()
    });
    println!("filter: {}", expr);
    let rv = env.compile_expression(&expr).unwrap().eval(()).unwrap();
    println!("result: {:#?}", rv);
}
