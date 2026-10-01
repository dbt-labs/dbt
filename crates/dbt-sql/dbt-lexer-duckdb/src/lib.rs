#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod duckdb {
        pub mod duckdblexer;

        pub use duckdblexer::DuckdbLexer as Lexer;
    }
}

pub use generated::duckdb::*;
