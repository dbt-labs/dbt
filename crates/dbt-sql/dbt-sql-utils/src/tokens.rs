use dbt_antlr4::{Arena, int_stream::EOF, token::Token, token_stream::UnbufferedTokenStream};
use dbt_frontend_common::Dialect;

use crate::input_streams::CaseInsensitiveInputStream;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlToken {
    pub text: String,
    pub start: usize,
    pub end: usize,
}

/// Lex SQL into default-channel tokens with byte spans in the original input.
/// Returns `None` when the dialect lexer encounters an unpaired token.
pub fn sql_lex_tokens(input: &str, dialect: Dialect) -> Option<Vec<SqlToken>> {
    let input_stream = CaseInsensitiveInputStream::new(input);

    Arena::with(|arena| {
        macro_rules! dialect_dispatch {
            ($dialect_crate:tt, $module:tt) => {
                (
                    UnbufferedTokenStream::new_unbuffered($dialect_crate::Lexer::<_>::new(
                        arena,
                        input_stream,
                    ))
                    .token_iter()
                    .collect::<Vec<_>>(),
                    $dialect_crate::$module::UNPAIRED_TOKEN,
                )
            };
        }

        let (tokens, unpaired_token) = match dialect {
            Dialect::Bigquery => dialect_dispatch!(dbt_lexer_bigquery, bigquerylexer),
            Dialect::Redshift => dialect_dispatch!(dbt_lexer_redshift, redshiftlexer),
            Dialect::Snowflake => dialect_dispatch!(dbt_lexer_snowflake, snowflakelexer),
            Dialect::Databricks => dialect_dispatch!(dbt_lexer_databricks, databrickslexer),
            _ => dialect_dispatch!(dbt_lexer_trino, trinolexer),
        };

        let mut result = Vec::new();
        for token in tokens {
            if token.get_token_type() == EOF {
                break;
            }
            if token.get_token_type() == unpaired_token {
                return None;
            }
            if token.get_channel() == 0 {
                result.push(SqlToken {
                    text: token.get_text().to_string(),
                    start: usize::try_from(token.get_start_index()).ok()?,
                    end: usize::try_from(token.get_stop_index() + 1).ok()?,
                });
            }
        }
        Some(result)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_snowflake_tokens_with_source_spans() {
        let sql = "select /* clock */ '2024-01-01T01:01:01+00:00' as dbt_run_started_at";
        let tokens = sql_lex_tokens(sql, Dialect::Snowflake).unwrap();

        assert_eq!(
            tokens
                .iter()
                .map(|token| token.text.as_str())
                .collect::<Vec<_>>(),
            [
                "select",
                "'2024-01-01T01:01:01+00:00'",
                "as",
                "dbt_run_started_at"
            ]
        );
        for token in tokens {
            assert_eq!(&sql[token.start..token.end], token.text);
        }
    }
}
