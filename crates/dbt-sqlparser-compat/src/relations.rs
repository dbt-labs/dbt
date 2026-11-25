//! Traversals of the relations a node reads, each with one closure.
//!
//! The names and the behavior follow the `Iterator` methods of the same name.

use std::ops::ControlFlow;

use sqlparser::ast::ObjectName;

use crate::{Flow, Visitor, Walk};

/// Calls `f` on the name of each relation that `node` reads, in the order
/// they occur.
pub fn for_each<N, F>(node: &N, f: F)
where
    N: Walk + ?Sized,
    F: FnMut(&ObjectName),
{
    struct Each<F>(F);
    impl<F: FnMut(&ObjectName)> Visitor for Each<F> {
        fn pre_visit_relation(&mut self, relation: &ObjectName) -> Flow {
            (self.0)(relation);
            ControlFlow::Continue(())
        }
    }
    let _ = node.walk(&mut Each(f));
}

#[cfg(test)]
mod tests {
    use sqlparser::dialect::GenericDialect;
    use sqlparser::parser::Parser;

    use super::*;

    #[test]
    fn for_each_visits_each_relation() {
        let statements = Parser::parse_sql(
            &GenericDialect {},
            "SELECT * FROM a JOIN b ON (SELECT 1 FROM c) = 1",
        )
        .expect("parses");
        let mut seen = vec![];
        for_each(&statements, |relation| seen.push(relation.to_string()));
        assert_eq!(seen, ["a", "b", "c"]);
    }
}
