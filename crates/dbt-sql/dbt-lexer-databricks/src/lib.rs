#[rustfmt::skip]
pub mod generated {
    #![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::restriction)]
    #![allow(unused_parens)]
    pub mod databricks {
        pub mod databrickslexer;

        pub use databrickslexer::DatabricksLexer as Lexer;
    }
}

pub use generated::databricks::*;

pub(crate) mod lexer_support {
    use super::*;
    use databrickslexer::BaseLexerType;
    use dbt_antlr4::char_stream::CharStream;
    use dbt_antlr4::token_factory::TokenFactory;

    // Follows https://github.com/apache/spark/blob/26dbf651bf8c2389aeea950816288e1db666c611/sql/api/src/main/antlr4/org/apache/spark/sql/catalyst/parser/SqlBaseLexer.g4#L25-L45
    pub fn is_valid_decimal_boundary<'input, 'arena, Input, TF>(
        recog: &mut BaseLexerType<'input, 'arena, Input, TF>,
    ) -> bool
    where
        'input: 'arena,
        TF: TokenFactory<'input, 'arena> + 'arena,
        Input: CharStream<'input>,
    {
        let input = recog.input.as_mut().expect("Input not set");
        let next = input.la(1);

        if next >= 'A' as i32 && next <= 'Z' as i32 {
            return false;
        }
        if next >= '0' as i32 && next <= '9' as i32 {
            return false;
        }
        if next == '_' as i32 {
            return false;
        }
        true
    }
}
