#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod snowflake {
        pub mod snowflakelexer;

        pub use snowflakelexer::SnowflakeLexer as Lexer;
    }
}

pub use generated::snowflake::*;
