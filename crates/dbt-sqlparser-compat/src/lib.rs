//! A `dyn`-compatible front end for the sqlparser AST walk.
//!
//! sqlparser's `Visit::visit` and `VisitMut::visit` are generic over the
//! visitor, so each visitor type compiles its own copy of the whole walk
//! (hundreds of KB in a release binary). [`visit`] and [`visit_mut`] take a
//! `&mut dyn` [`Visitor`] or [`VisitorMut`] instead and forward to it from one
//! private sqlparser visitor. The walk of each node type is compiled once, in
//! this crate.
//!
//! Because the visitor is a trait object, the value that stops a walk has one
//! type for all visitors: [`Break`].
//!
//! For a walk that needs only one closure, [`exprs`] and [`relations`] have
//! traversals named after the `Iterator` methods they act like, such as
//! [`exprs::for_each`] and [`exprs::find_map`].

use std::any::Any;
use std::fmt;
use std::ops::ControlFlow;

pub mod exprs;
pub mod relations;

use sqlparser::ast::{
    self as sql, Expr, Function, ObjectName, Query, Select, SetExpr, Statement, TableFactor, Value,
};

/// What a [`Visitor`] or [`VisitorMut`] hook returns: continue, or stop the
/// walk with a [`Break`].
pub type Flow = ControlFlow<Break>;

/// Why a walk stopped: nothing ([`Break::stop`]), or a value of any type
/// ([`Break::value`]).
///
/// `Break` is one `Box<dyn Any + Send>` (two pointers), and the null pointer
/// that a box can't hold encodes `ControlFlow::Continue`. So [`Flow`] is two
/// pointers wide too, and every hook returns it in registers. Keep it that
/// way: the `flow_fits_in_registers` test fails if it grows.
pub struct Break(Box<dyn Any + Send>);

impl Break {
    /// Stop the walk with no value. The visitor keeps any result in its own
    /// state. Doesn't allocate: the box holds `()`.
    pub fn stop() -> Self {
        Self(Box::new(()))
    }

    /// Stop the walk with `value`, for example a message that says why the
    /// input is rejected. Get it back with [`Break::downcast`].
    pub fn value<T: Any + Send>(value: T) -> Self {
        Self(Box::new(value))
    }

    /// Whether the walk stopped with no value.
    pub fn is_stop(&self) -> bool {
        self.0.is::<()>()
    }

    /// The value, if it is a `T`. Otherwise returns `self` unchanged.
    pub fn downcast<T: Any>(self) -> Result<T, Self> {
        self.0.downcast::<T>().map(|value| *value).map_err(Self)
    }

    /// A reference to the value, if it is a `T`.
    pub fn downcast_ref<T: Any>(&self) -> Option<&T> {
        self.0.downcast_ref::<T>()
    }

    /// The value as a message, if it is a `String` or a `&'static str`.
    fn message(&self) -> Option<&str> {
        self.downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| self.downcast_ref::<&'static str>().copied())
    }
}

/// A `String` or `&'static str` value displays as itself.
impl fmt::Display for Break {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message() {
            Some(message) => f.write_str(message),
            None if self.is_stop() => f.write_str("the walk stopped"),
            None => f.write_str("the walk stopped with a value"),
        }
    }
}

impl fmt::Debug for Break {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.message() {
            Some(message) => f.debug_tuple("Break").field(&message).finish(),
            None if self.is_stop() => f.write_str("Break::stop()"),
            None => f.write_str("Break(..)"),
        }
    }
}

/// The hooks of a read-only walk. Each hook that returns
/// `ControlFlow::Break` stops the walk.
///
/// The hooks are the same as the hooks of `sqlparser::ast::Visitor`. A `pre_`
/// hook runs before the children of a node, and a `post_` hook runs after them.
pub trait Visitor {
    fn pre_visit_query(&mut self, _query: &Query) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &Query) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, _relation: &ObjectName) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_relation(&mut self, _relation: &ObjectName) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, _table_factor: &TableFactor) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_table_factor(&mut self, _table_factor: &TableFactor) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, _expr: &Expr) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, _expr: &Expr) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, _statement: &Statement) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_statement(&mut self, _statement: &Statement) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_value(&mut self, _value: &Value) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_value(&mut self, _value: &Value) -> Flow {
        ControlFlow::Continue(())
    }
}

/// The hooks of a walk that may change the nodes it visits. Each hook that
/// returns `ControlFlow::Break` stops the walk.
///
/// The hooks are the same as the hooks of `sqlparser::ast::VisitorMut`. A
/// `pre_` hook runs before the children of a node, and a `post_` hook runs
/// after them. A node that a `pre_` hook replaces is walked as replaced.
pub trait VisitorMut {
    fn pre_visit_query(&mut self, _query: &mut Query) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_query(&mut self, _query: &mut Query) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_relation(&mut self, _relation: &mut ObjectName) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_relation(&mut self, _relation: &mut ObjectName) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_table_factor(&mut self, _table_factor: &mut TableFactor) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_table_factor(&mut self, _table_factor: &mut TableFactor) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_expr(&mut self, _expr: &mut Expr) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_expr(&mut self, _expr: &mut Expr) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_statement(&mut self, _statement: &mut Statement) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_statement(&mut self, _statement: &mut Statement) -> Flow {
        ControlFlow::Continue(())
    }

    fn pre_visit_value(&mut self, _value: &mut Value) -> Flow {
        ControlFlow::Continue(())
    }

    fn post_visit_value(&mut self, _value: &mut Value) -> Flow {
        ControlFlow::Continue(())
    }
}

/// Walks `node` with `visitor`.
pub fn visit<N: Walk + ?Sized>(node: &N, visitor: &mut dyn Visitor) -> Flow {
    node.walk(visitor)
}

/// Walks `node` with `visitor`, which may change it.
pub fn visit_mut<N: Walk + ?Sized>(node: &mut N, visitor: &mut dyn VisitorMut) -> Flow {
    node.walk_mut(visitor)
}

/// An AST node that [`visit`] and [`visit_mut`] can walk.
///
/// The trait is sealed: this crate implements it for each node type that it
/// compiles a walk for. To walk a new node type, add it to the list below.
pub trait Walk: private::Sealed {
    #[doc(hidden)]
    fn walk(&self, visitor: &mut dyn Visitor) -> Flow;

    #[doc(hidden)]
    fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow;
}

mod private {
    pub trait Sealed {}
}

macro_rules! impl_walk {
    ($($node:ty),+ $(,)?) => {$(
        impl private::Sealed for $node {}

        impl Walk for $node {
            // Not inlined, so callers share this crate's one copy of the walk.
            // These are the only calls of sqlparser's walk that `clippy.toml`
            // allows.
            #[inline(never)]
            #[allow(clippy::disallowed_methods)]
            fn walk(&self, visitor: &mut dyn Visitor) -> Flow {
                sql::Visit::visit(self, &mut Forward(visitor))
            }

            #[inline(never)]
            #[allow(clippy::disallowed_methods)]
            fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow {
                sql::VisitMut::visit(self, &mut ForwardMut(visitor))
            }
        }
    )+};
}

impl_walk!(
    Statement,
    Query,
    SetExpr,
    Select,
    TableFactor,
    Expr,
    Function,
    ObjectName,
);

impl<T: Walk> private::Sealed for [T] {}

impl<T: Walk> Walk for [T] {
    fn walk(&self, visitor: &mut dyn Visitor) -> Flow {
        for node in self {
            node.walk(visitor)?;
        }
        ControlFlow::Continue(())
    }

    fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow {
        for node in self {
            node.walk_mut(visitor)?;
        }
        ControlFlow::Continue(())
    }
}

impl<T: Walk> private::Sealed for Vec<T> {}

impl<T: Walk> Walk for Vec<T> {
    fn walk(&self, visitor: &mut dyn Visitor) -> Flow {
        self.as_slice().walk(visitor)
    }

    fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow {
        self.as_mut_slice().walk_mut(visitor)
    }
}

impl<T: Walk + ?Sized> private::Sealed for Box<T> {}

impl<T: Walk + ?Sized> Walk for Box<T> {
    fn walk(&self, visitor: &mut dyn Visitor) -> Flow {
        (**self).walk(visitor)
    }

    fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow {
        (**self).walk_mut(visitor)
    }
}

impl<T: Walk> private::Sealed for Option<T> {}

impl<T: Walk> Walk for Option<T> {
    fn walk(&self, visitor: &mut dyn Visitor) -> Flow {
        match self {
            Some(node) => node.walk(visitor),
            None => ControlFlow::Continue(()),
        }
    }

    fn walk_mut(&mut self, visitor: &mut dyn VisitorMut) -> Flow {
        match self {
            Some(node) => node.walk_mut(visitor),
            None => ControlFlow::Continue(()),
        }
    }
}

/// The one sqlparser visitor that the read-only walk is compiled for.
struct Forward<'a>(&'a mut dyn Visitor);

impl sql::Visitor for Forward<'_> {
    type Break = Break;

    fn pre_visit_query(&mut self, query: &Query) -> Flow {
        self.0.pre_visit_query(query)
    }

    fn post_visit_query(&mut self, query: &Query) -> Flow {
        self.0.post_visit_query(query)
    }

    fn pre_visit_relation(&mut self, relation: &ObjectName) -> Flow {
        self.0.pre_visit_relation(relation)
    }

    fn post_visit_relation(&mut self, relation: &ObjectName) -> Flow {
        self.0.post_visit_relation(relation)
    }

    fn pre_visit_table_factor(&mut self, table_factor: &TableFactor) -> Flow {
        self.0.pre_visit_table_factor(table_factor)
    }

    fn post_visit_table_factor(&mut self, table_factor: &TableFactor) -> Flow {
        self.0.post_visit_table_factor(table_factor)
    }

    fn pre_visit_expr(&mut self, expr: &Expr) -> Flow {
        self.0.pre_visit_expr(expr)
    }

    fn post_visit_expr(&mut self, expr: &Expr) -> Flow {
        self.0.post_visit_expr(expr)
    }

    fn pre_visit_statement(&mut self, statement: &Statement) -> Flow {
        self.0.pre_visit_statement(statement)
    }

    fn post_visit_statement(&mut self, statement: &Statement) -> Flow {
        self.0.post_visit_statement(statement)
    }

    fn pre_visit_value(&mut self, value: &Value) -> Flow {
        self.0.pre_visit_value(value)
    }

    fn post_visit_value(&mut self, value: &Value) -> Flow {
        self.0.post_visit_value(value)
    }
}

/// The one sqlparser visitor that the mutable walk is compiled for.
struct ForwardMut<'a>(&'a mut dyn VisitorMut);

impl sql::VisitorMut for ForwardMut<'_> {
    type Break = Break;

    fn pre_visit_query(&mut self, query: &mut Query) -> Flow {
        self.0.pre_visit_query(query)
    }

    fn post_visit_query(&mut self, query: &mut Query) -> Flow {
        self.0.post_visit_query(query)
    }

    fn pre_visit_relation(&mut self, relation: &mut ObjectName) -> Flow {
        self.0.pre_visit_relation(relation)
    }

    fn post_visit_relation(&mut self, relation: &mut ObjectName) -> Flow {
        self.0.post_visit_relation(relation)
    }

    fn pre_visit_table_factor(&mut self, table_factor: &mut TableFactor) -> Flow {
        self.0.pre_visit_table_factor(table_factor)
    }

    fn post_visit_table_factor(&mut self, table_factor: &mut TableFactor) -> Flow {
        self.0.post_visit_table_factor(table_factor)
    }

    fn pre_visit_expr(&mut self, expr: &mut Expr) -> Flow {
        self.0.pre_visit_expr(expr)
    }

    fn post_visit_expr(&mut self, expr: &mut Expr) -> Flow {
        self.0.post_visit_expr(expr)
    }

    fn pre_visit_statement(&mut self, statement: &mut Statement) -> Flow {
        self.0.pre_visit_statement(statement)
    }

    fn post_visit_statement(&mut self, statement: &mut Statement) -> Flow {
        self.0.post_visit_statement(statement)
    }

    fn pre_visit_value(&mut self, value: &mut Value) -> Flow {
        self.0.pre_visit_value(value)
    }

    fn post_visit_value(&mut self, value: &mut Value) -> Flow {
        self.0.post_visit_value(value)
    }
}

#[cfg(test)]
mod tests {
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser;

    use super::*;

    fn parse(sql: &str) -> Vec<Statement> {
        Parser::parse_sql(&GenericDialect {}, sql).expect("parses")
    }

    /// Records the order of the hooks, as sqlparser's own tests do.
    #[derive(Default)]
    struct Trace(Vec<String>);

    impl Visitor for Trace {
        fn pre_visit_statement(&mut self, statement: &Statement) -> Flow {
            self.0.push(format!("PRE: STATEMENT: {statement}"));
            ControlFlow::Continue(())
        }

        fn post_visit_statement(&mut self, statement: &Statement) -> Flow {
            self.0.push(format!("POST: STATEMENT: {statement}"));
            ControlFlow::Continue(())
        }

        fn pre_visit_relation(&mut self, relation: &ObjectName) -> Flow {
            self.0.push(format!("PRE: RELATION: {relation}"));
            ControlFlow::Continue(())
        }

        fn pre_visit_expr(&mut self, expr: &Expr) -> Flow {
            self.0.push(format!("PRE: EXPR: {expr}"));
            ControlFlow::Continue(())
        }

        fn post_visit_expr(&mut self, expr: &Expr) -> Flow {
            self.0.push(format!("POST: EXPR: {expr}"));
            ControlFlow::Continue(())
        }
    }

    #[test]
    fn hooks_run_in_walk_order() {
        let statements = parse("SELECT a + 1 FROM t");
        let mut trace = Trace::default();
        assert!(visit(&statements, &mut trace).is_continue());
        assert_eq!(
            trace.0,
            [
                "PRE: STATEMENT: SELECT a + 1 FROM t",
                "PRE: EXPR: a + 1",
                "PRE: EXPR: a",
                "POST: EXPR: a",
                "PRE: EXPR: 1",
                "POST: EXPR: 1",
                "POST: EXPR: a + 1",
                "PRE: RELATION: t",
                "POST: STATEMENT: SELECT a + 1 FROM t",
            ]
        );
    }

    #[test]
    fn a_break_stops_the_walk_and_carries_its_value() {
        #[derive(Debug, PartialEq)]
        struct Found(String);

        struct FirstRelation(Vec<String>);
        impl Visitor for FirstRelation {
            fn pre_visit_relation(&mut self, relation: &ObjectName) -> Flow {
                self.0.push(relation.to_string());
                ControlFlow::Break(Break::value(Found(relation.to_string())))
            }
        }

        let statements = parse("SELECT a FROM t1 JOIN t2 ON true");
        let mut visitor = FirstRelation(vec![]);
        let ControlFlow::Break(stopped) = visit(&statements, &mut visitor) else {
            panic!("the walk didn't stop");
        };
        assert_eq!(stopped.downcast::<Found>().unwrap(), Found("t1".into()));
        assert_eq!(visitor.0, ["t1"]);
    }

    /// Every hook returns a `Flow`, so its size is a cost of each hook call.
    ///
    /// The Rust ABI returns a value of up to two pointers in registers (`rax:rdx`
    /// on x86-64). A larger value goes through memory: the caller passes a
    /// hidden pointer and the hook writes `Flow` through it. A `Break` enum
    /// with a `String` variant made `Flow` 24 bytes, for example.
    ///
    /// If this fails, don't change the assertion: make `Break` smaller.
    #[test]
    fn flow_fits_in_registers() {
        assert_eq!(size_of::<Break>(), 2 * size_of::<usize>());
        assert_eq!(size_of::<Flow>(), 2 * size_of::<usize>());
    }

    #[test]
    fn downcast_to_another_type_returns_the_break() {
        let stopped = Break::value(1_u8).downcast::<String>().unwrap_err();
        assert_eq!(stopped.downcast::<u8>().unwrap(), 1);
        assert!(Break::stop().is_stop());
        assert!(!Break::value(1_u8).is_stop());
    }

    #[test]
    fn a_message_displays_as_itself() {
        assert_eq!(Break::value("no".to_owned()).to_string(), "no");
        assert_eq!(Break::value("no").to_string(), "no");
        assert_eq!(Break::stop().to_string(), "the walk stopped");
        assert_eq!(
            Break::value(1_u8).to_string(),
            "the walk stopped with a value"
        );
    }

    #[test]
    fn walks_statements_mutably() {
        let mut statements = parse("SELECT a + a FROM t");
        exprs::for_each_mut(&mut statements, |expr| {
            if let Expr::Identifier(ident) = expr {
                ident.value = "b".to_owned();
            }
        });
        assert_eq!(statements[0].to_string(), "SELECT b + b FROM t");
    }

    #[test]
    fn walks_an_expression() {
        let expr = Parser::new(&GenericDialect {})
            .try_with_sql("f(x, (SELECT y FROM u))")
            .and_then(|mut parser| parser.parse_expr())
            .expect("parses");
        let mut seen = vec![];
        relations::for_each(&expr, |relation| seen.push(relation.to_string()));
        assert_eq!(seen, ["u"]);
    }
}
