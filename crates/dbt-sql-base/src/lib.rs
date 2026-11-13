//! Dependency-free SQL basics: the identifier type and the dialect enum with its
//! lexical rules.
//!
//! This crate depends on no other dbt crate, so any crate that handles SQL
//! identifiers or dialects can use it.

mod dialect;
mod error;
mod ident;

pub use dialect::{
    DRAFT_SUFFIX_LEN, Dialect, LOWERCASE_DRAFT_SUFFIX, QUOTED_DRAFT_SUFFIX_LEN,
    QUOTED_LOWERCASE_DRAFT_SUFFIX, QUOTED_UPPERCASE_DRAFT_SUFFIX, UPPERCASE_DRAFT_SUFFIX,
    parse_dot,
};
pub use error::ParseError;
pub use ident::{Ident, Identifier};
