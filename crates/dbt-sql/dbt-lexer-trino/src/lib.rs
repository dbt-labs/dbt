#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod trino {
        pub mod trinolexer;

        pub use trinolexer::TrinoLexer as Lexer;
    }
}

pub use generated::trino::*;
