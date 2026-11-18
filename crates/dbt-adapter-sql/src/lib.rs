#![allow(clippy::cognitive_complexity)]
#![allow(clippy::collapsible_if)]
#![allow(clippy::if_same_then_else)]
#![allow(clippy::let_and_return)]
#![allow(clippy::needless_bool)]
#![allow(clippy::only_used_in_recursion)]
#![allow(clippy::should_implement_trait)]
use std::fmt;
use std::fmt::Write;

pub mod ident;
pub mod literal;
pub mod statements;
pub mod tokenizer;
pub mod types;

mod keywords;

pub use keywords::is_keyword_ignore_ascii_case;

/// Joins an iterator of [Display](fmt::Display) values with a dot separator, returning
/// the result as a [String].
///
/// This is much better than calling `to_string()` on each [Display](fmt::Display) value
/// and then joining them with a dot, as it avoids unnecessary allocations and intermediate
/// [String] objects.
pub fn join_dotted<T: fmt::Display>(values: impl IntoIterator<Item = T>) -> String {
    let mut out = String::new();
    for (i, value) in values.into_iter().enumerate() {
        if i != 0 {
            out.push('.');
        }
        write!(&mut out, "{value}").unwrap();
    }
    out
}
