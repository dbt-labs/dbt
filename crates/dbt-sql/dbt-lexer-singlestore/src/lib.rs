#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod singlestore {
        pub mod singlestorelexer {
            pub use dbt_lexer_databricks::databrickslexer::*;
            pub use dbt_lexer_databricks::Lexer as SingleStoreLexer;
        }

        pub use singlestorelexer::SingleStoreLexer as Lexer;
    }
}

pub use generated::singlestore::*;
