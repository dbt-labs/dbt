#![allow(clippy::cognitive_complexity)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::let_and_return)]
#![allow(clippy::needless_bool)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::should_implement_trait)]
use std::fmt;

pub mod ident;
pub mod literal;
pub mod statements;
pub mod tokenizer;
pub mod types;

mod keywords;

pub use keywords::is_keyword_ignore_ascii_case;

/// Joins [Display](fmt::Display) values with a dot separator.
///
/// Each format clones the iterator, not the values.
pub fn join_dotted<I>(iter: I) -> impl fmt::Display
where
    I: Iterator + Clone,
    I::Item: fmt::Display,
{
    Join { iter, sep: "." }
}

/// Joins [Display](fmt::Display) values with a `", "` separator.
///
/// Each format clones the iterator, not the values.
pub fn join_comma_separated<I>(iter: I) -> impl fmt::Display
where
    I: Iterator + Clone,
    I::Item: fmt::Display,
{
    Join { iter, sep: ", " }
}

pub fn join_separated<I>(iter: I, sep: &'static str) -> impl fmt::Display
where
    I: Iterator + Clone,
    I::Item: fmt::Display,
{
    Join { iter, sep }
}

struct Join<I> {
    iter: I,
    sep: &'static str,
}

impl<I> fmt::Display for Join<I>
where
    I: Iterator + Clone,
    I::Item: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, value) in self.iter.clone().enumerate() {
            if i != 0 {
                f.write_str(self.sep)?;
            }
            write!(f, "{value}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod join_tests {
    use super::*;

    #[test]
    fn join_comma_separated_joins_display_values() {
        assert_eq!(
            join_comma_separated(std::iter::empty::<&str>()).to_string(),
            ""
        );
        assert_eq!(join_comma_separated(["a"].iter()).to_string(), "a");
        assert_eq!(
            join_comma_separated(["a", "b", "c"].iter()).to_string(),
            "a, b, c"
        );
        assert_eq!(join_dotted([1, 2, 3].iter()).to_string(), "1.2.3");
    }

    #[test]
    fn join_can_be_formatted_more_than_once() {
        let parts = ["a", "b"];
        let columns = join_comma_separated(parts.iter());
        assert_eq!(format!("({columns}) ({columns})"), "(a, b) (a, b)");
    }

    /// A [Display](fmt::Display) value that panics if it is ever cloned.
    struct NoClone(&'static str);

    impl Clone for NoClone {
        fn clone(&self) -> Self {
            panic!("joined value {:?} was cloned", self.0)
        }
    }

    impl fmt::Display for NoClone {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    #[test]
    fn join_clones_the_iterator_not_the_values() {
        let parts = [NoClone("a"), NoClone("b"), NoClone("c")];

        let dotted = join_dotted(parts.iter());
        assert_eq!(format!("{dotted} {dotted}"), "a.b.c a.b.c");

        let mapped = join_comma_separated(parts.iter().map(|part| format!("[{part}]")));
        assert_eq!(format!("{mapped} {mapped}"), "[a], [b], [c] [a], [b], [c]");
    }
}
