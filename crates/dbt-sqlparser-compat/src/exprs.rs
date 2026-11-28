//! Traversals of the expressions in a node, each with one closure.
//!
//! The names and the behavior follow the `Iterator` methods of the same name.

use std::ops::ControlFlow;

use sqlparser::ast::Expr;

use crate::{Break, Flow, Visitor, VisitorMut, Walk};

/// Calls `f` on each expression in `node`, outermost first.
pub fn for_each<N, F>(node: &N, f: F)
where
    N: Walk + ?Sized,
    F: FnMut(&Expr),
{
    struct Each<F>(F);
    impl<F: FnMut(&Expr)> Visitor for Each<F> {
        fn pre_visit_expr(&mut self, expr: &Expr) -> Flow {
            (self.0)(expr);
            ControlFlow::Continue(())
        }
    }
    let _ = node.walk(&mut Each(f));
}

/// The first `Some` that `f` returns for an expression in `node`, outermost
/// first. The walk stops there.
pub fn find_map<N, T, F>(node: &N, f: F) -> Option<T>
where
    N: Walk + ?Sized,
    F: FnMut(&Expr) -> Option<T>,
{
    struct Find<F, T> {
        f: F,
        found: Option<T>,
    }
    impl<F: FnMut(&Expr) -> Option<T>, T> Visitor for Find<F, T> {
        fn pre_visit_expr(&mut self, expr: &Expr) -> Flow {
            match (self.f)(expr) {
                Some(found) => {
                    // The value stays here, so the break doesn't allocate.
                    self.found = Some(found);
                    ControlFlow::Break(Break::stop())
                }
                None => ControlFlow::Continue(()),
            }
        }
    }
    let mut find = Find { f, found: None };
    let _ = node.walk(&mut find);
    find.found
}

/// Whether `f` returns `true` for an expression in `node`. The walk stops at
/// the first one.
pub fn any<N, F>(node: &N, mut f: F) -> bool
where
    N: Walk + ?Sized,
    F: FnMut(&Expr) -> bool,
{
    find_map(node, |expr| f(expr).then_some(())).is_some()
}

/// Calls `f` on each expression in `node`, innermost first. `f` may replace
/// the expression: its children are already visited, and the replacement is
/// not visited.
pub fn for_each_mut<N, F>(node: &mut N, f: F)
where
    N: Walk + ?Sized,
    F: FnMut(&mut Expr),
{
    struct Each<F>(F);
    impl<F: FnMut(&mut Expr)> VisitorMut for Each<F> {
        fn post_visit_expr(&mut self, expr: &mut Expr) -> Flow {
            (self.0)(expr);
            ControlFlow::Continue(())
        }
    }
    let _ = node.walk_mut(&mut Each(f));
}

#[cfg(test)]
mod tests {
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser;

    use super::*;

    fn parse_expr(sql: &str) -> Expr {
        Parser::new(&GenericDialect {})
            .try_with_sql(sql)
            .and_then(|mut parser| parser.parse_expr())
            .expect("parses")
    }

    #[test]
    fn for_each_visits_outermost_first() {
        let mut seen = vec![];
        for_each(&parse_expr("a + f(b)"), |expr| seen.push(expr.to_string()));
        assert_eq!(seen, ["a + f(b)", "a", "f(b)", "b"]);
    }

    #[test]
    fn find_map_stops_at_the_first_match() {
        let mut seen = 0;
        let found = find_map(&parse_expr("a + b + c"), |expr| {
            seen += 1;
            match expr {
                Expr::Identifier(ident) => Some(ident.value.clone()),
                _ => None,
            }
        });
        assert_eq!(found.as_deref(), Some("a"));
        // `a + b + c`, `a + b`, then `a`.
        assert_eq!(seen, 3);
        assert_eq!(find_map(&parse_expr("1"), |_| None::<()>), None);
    }

    #[test]
    fn any_finds_a_match() {
        let expr = parse_expr("a + f(b)");
        assert!(any(&expr, |expr| matches!(expr, Expr::Function(_))));
        assert!(!any(&expr, |expr| matches!(expr, Expr::Lambda(_))));
    }

    #[test]
    fn for_each_mut_visits_innermost_first_and_skips_replacements() {
        let mut expr = parse_expr("a + b");
        let mut seen = vec![];
        for_each_mut(&mut expr, |expr| {
            seen.push(expr.to_string());
            if let Expr::Identifier(ident) = expr {
                // Not visited: a replacement is never walked.
                *expr = parse_expr(&format!("g({ident})"));
            }
        });
        assert_eq!(seen, ["a", "b", "g(a) + g(b)"]);
        assert_eq!(expr.to_string(), "g(a) + g(b)");
    }
}
