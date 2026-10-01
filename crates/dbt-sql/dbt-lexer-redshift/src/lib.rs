#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod redshift {
        pub mod redshiftlexer;

        pub use redshiftlexer::RedshiftLexer as Lexer;
    }
}

pub use generated::redshift::*;
