use dbt_antlr4::{
    Arena, InputStream,
    char_stream::{CharStream, InputData as _},
    int_stream::{self, IntStream},
    token::Token,
    token_stream::UnbufferedTokenStream,
};
use dbt_frontend_common::Dialect;
use dbt_frontend_common::error::CodeLocation;
use dbt_frontend_common::span::Span;
use minijinja::machinery::WhitespaceConfig;
use minijinja::syntax::SyntaxConfig;
use std::borrow::Cow;

/// Returns the positions of all SQL statement-delimiting semicolons in the
/// input string.
pub fn sql_find_statement_spans(input: &str, dialect: Option<Dialect>) -> Vec<Span> {
    let dialect = dialect.unwrap_or(Dialect::Trino);
    let input_stream = InputStream::new(input);

    Arena::with(|arena| do_sql_find_statement_delimiters(arena, input_stream, dialect))
}

fn do_sql_find_statement_delimiters<'input, 'arena>(
    arena: &'arena Arena,
    input_stream: impl CharStream<'input>,
    dialect: Dialect,
) -> Vec<Span> {
    macro_rules! dialect_dispatch {
        ($dialect_crate:tt, $module:tt) => {
            (
                UnbufferedTokenStream::new_unbuffered($dialect_crate::Lexer::<_>::new(
                    arena,
                    input_stream,
                ))
                .token_iter()
                .collect::<Vec<_>>(),
                $dialect_crate::$module::SEMI_COLON,
                $dialect_crate::$module::UNPAIRED_TOKEN,
            )
        };
    }

    let (token_stream, semi_colon, unpaired_token) = match dialect {
        Dialect::Bigquery => dialect_dispatch!(dbt_lexer_bigquery, bigquerylexer),
        Dialect::Redshift => dialect_dispatch!(dbt_lexer_redshift, redshiftlexer),
        Dialect::Snowflake => dialect_dispatch!(dbt_lexer_snowflake, snowflakelexer),
        Dialect::Databricks => dialect_dispatch!(dbt_lexer_databricks, databrickslexer),
        _ => dialect_dispatch!(dbt_lexer_trino, trinolexer),
    };

    let mut result = vec![];
    let mut start_token = None;
    let mut last_token = None;
    let mut unpaired_token_found = false;
    for token in token_stream {
        if start_token.is_none() {
            start_token = Some(token.clone());
        }
        if !unpaired_token_found {
            if token.get_channel() == 0 && token.get_token_type() == semi_colon {
                let start_token_ = start_token.unwrap();
                let span = Span::new(
                    CodeLocation::new(
                        start_token_.get_line(),
                        start_token_.get_char_position_in_line() as u32,
                        start_token_.get_start_index() as u32,
                    ),
                    CodeLocation::new(
                        token.get_line(),
                        token.get_char_position_in_line() as u32,
                        token.get_start_index() as u32,
                    ),
                );
                result.push(span);
                start_token = None;
            } else if token.get_token_type() == unpaired_token {
                // Unpaired token (unclosed quote, comment, etc.) found -- the
                // current statement is malformed (as far as our grammar
                // dictates) and thus we cannot trust any tokens after this
                // point, just lump everything into the current statement and
                // return it
                unpaired_token_found = true;
                continue;
            }
        }
        last_token = Some(token);
    }
    if let Some(last_token) = last_token {
        let start_token = start_token.unwrap();
        if start_token.get_start_index() != last_token.get_start_index() {
            result.push(Span::new(
                CodeLocation::new(
                    start_token.get_line(),
                    start_token.get_char_position_in_line() as u32,
                    start_token.get_start_index() as u32,
                ),
                CodeLocation::new(
                    last_token.get_line(),
                    last_token.get_char_position_in_line() as u32,
                    last_token.get_start_index() as u32,
                ),
            ));
        }
    }
    result
}

/// An input stream that can "mask" off parts of the input so they're
/// effectively invisible to the lexer.
///
/// This makes the lexer effectively "see" only a concatenated string of all visible
/// spans of the input, while producing tokens with offsets relative to the original
/// input.
struct MaskedInputStream<'input> {
    // The underlying input data
    data: &'input str,
    // Sorted, non-overlapping spans of the input that are visible
    visible_spans: Vec<minijinja::machinery::Span>,
    name: String,
    index: isize,
    last_item: Option<i32>,
}

impl<'input> MaskedInputStream<'input> {
    fn new(
        name: String,
        data: &'input str,
        visible_spans: Vec<minijinja::machinery::Span>,
    ) -> Self {
        // We truncate everything past the last visible span, so we don't have
        // to deal with EOF problems:
        let last_visible_offset = visible_spans
            .last()
            .map_or(0, |span| span.end_offset as usize);

        let mut res = Self {
            data: &data[..last_visible_offset],
            visible_spans,
            name,
            index: 0,
            last_item: None,
        };
        res.index = res.clamp_index_right(0);
        res
    }

    /// If the index is within a visible span, returns the index; otherwise,
    /// returns the smallest visible index greater than the given index. If the
    /// index is past the last visible span, returns one past the last index.
    fn clamp_index_right(&self, index: isize) -> isize {
        if index >= self.data.len() as isize {
            return self.data.len() as isize;
        }

        for span in &self.visible_spans {
            if span.start_offset as isize <= index && index < span.end_offset as isize {
                return index;
            } else if span.start_offset as isize > index && span.end_offset > span.start_offset {
                return span.start_offset as isize;
            }
        }
        self.data.len() as isize
    }
}

impl<'input> CharStream<'input> for MaskedInputStream<'input> {
    fn get_text(&self, a: isize, b: isize) -> Cow<'input, str> {
        let mut res = String::new();

        for span in &self.visible_spans {
            if span.end_offset as isize <= a {
                continue;
            }
            if span.start_offset as isize >= b {
                break;
            }

            let start = if span.start_offset as isize > a {
                span.start_offset as isize
            } else {
                a
            };
            let end = if span.end_offset as isize > b {
                b
            } else {
                span.end_offset as isize
            };

            res.push_str(&self.data[start as usize..end as usize]);
        }
        Cow::Owned(res)
    }
}

impl IntStream for MaskedInputStream<'_> {
    #[inline]
    fn consume(&mut self) {
        if let Some(index) = self.data.offset(self.index, 1) {
            self.last_item = self.data.item(self.index);
            self.index = self.clamp_index_right(index);
        } else {
            panic!("cannot consume EOF");
        }
    }

    #[inline]
    fn la(&mut self, offset: isize) -> i32 {
        if offset == 1 {
            return self.data.item(self.index).unwrap_or(int_stream::EOF);
        }
        if offset != -1 {
            panic!("should not be called with offset {offset}");
        }

        self.last_item.unwrap_or(int_stream::EOF)
    }

    #[inline]
    fn mark(&mut self) -> isize {
        -1
    }

    #[inline]
    fn release(&mut self, _marker: isize) {}

    #[inline]
    fn index(&self) -> isize {
        self.index
    }

    #[inline]
    fn seek(&mut self, index: isize) {
        self.index = self.clamp_index_right(index);
    }

    #[inline]
    fn size(&self) -> isize {
        self.data.len() as isize
    }

    fn get_source_name(&self) -> String {
        self.name.clone()
    }
}

/// Returns the positions of all SQL statement-delimiting semicolons in the dbt
/// Jinja-SQL input string.
pub fn jinja_sql_find_statement_spans(input: &str, dialect: Option<Dialect>) -> Vec<Span> {
    let mut jinja_tokenizer = minijinja::machinery::Tokenizer::new(
        input,
        "<string>",
        false,
        SyntaxConfig::default(),
        WhitespaceConfig::default(),
    );
    // Extract all TemplateData spans from the Jinja token stream:
    let sql_spans = std::iter::from_fn(move || jinja_tokenizer.next_token().transpose())
        .filter_map(|token| match token {
            Ok((minijinja::machinery::Token::TemplateData(_), span)) => Some(span),
            _ => None,
        })
        .collect::<Vec<_>>();

    let input_stream = MaskedInputStream::new("<string>".to_owned(), input, sql_spans);

    Arena::with(|arena| {
        do_sql_find_statement_delimiters(arena, input_stream, dialect.unwrap_or(Dialect::Trino))
    })
}

/// Splits the input string into SQL statements using semicolons as delimiters.
pub fn sql_split_statements(input: &str, dialect: Option<Dialect>) -> Vec<&str> {
    let sql_buf = input.trim();
    do_sql_split_statements(sql_buf, dialect)
}

/// Return the statement whose result is exposed by a Snowflake flow chain.
///
/// Snowflake's lexer keeps operators inside comments and quoted values in
/// single tokens, so they cannot be mistaken for statement separators.
pub fn snowflake_terminal_flow_statement(input: &str) -> &str {
    if !input.contains("->>") {
        return input;
    }

    #[derive(Clone, Copy)]
    enum Match {
        None,
        Minus(isize),
        Arrow(isize),
    }

    let input_stream = InputStream::new(input);
    let result_start = Arena::with(|arena| {
        let mut token_stream = UnbufferedTokenStream::new_unbuffered(
            dbt_lexer_snowflake::Lexer::<_>::new(arena, input_stream),
        );
        let tokens = token_stream.token_iter();

        let mut current_match = Match::None;
        let mut result_start = 0;
        for token in tokens.filter(|token| token.get_channel() == 0) {
            if token.get_text() == "->>" {
                result_start = token.get_stop_index() + 1;
                current_match = Match::None;
                continue;
            }

            current_match = match (current_match, token.get_text()) {
                (Match::Minus(stop), ">") if token.get_start_index() == stop + 1 => {
                    Match::Arrow(token.get_stop_index())
                }
                (Match::Arrow(stop), ">") if token.get_start_index() == stop + 1 => {
                    result_start = token.get_stop_index() + 1;
                    Match::None
                }
                (_, "-") => Match::Minus(token.get_stop_index()),
                (_, "->") => Match::Arrow(token.get_stop_index()),
                _ => Match::None,
            };
        }
        result_start
    });

    &input[result_start as usize..]
}

fn do_sql_split_statements(input: &str, dialect: Option<Dialect>) -> Vec<&str> {
    let mut result = vec![];
    for span in sql_find_statement_spans(input, dialect) {
        let statement = span.slice(input);
        result.push(statement);
    }
    result
}

/// Check if a statement is empty or contains only comments and whitespace
/// using the appropriate dialect-specific SQL lexer.
///
/// Falls back to the Trino lexer if the dialect is not fully supported yet.
pub fn is_empty_or_comment_only(statement: &str, dialect: Dialect) -> bool {
    use super::CaseInsensitiveInputStream;
    use Dialect::*;
    use dbt_antlr4::{TokenSource, int_stream::EOF};

    let trimmed = statement.trim();
    if trimmed.is_empty() {
        return true;
    }
    let input_stream = CaseInsensitiveInputStream::new(trimmed);

    // Use the same macro pattern as do_sql_find_statement_delimiters
    macro_rules! dialect_dispatch {
        ($dialect_crate:tt, $module:tt) => {
            Arena::with(|arena| {
                let mut lexer = $dialect_crate::Lexer::<_>::new(arena, input_stream);
                loop {
                    let token = lexer.next_token();
                    if token.get_token_type() == EOF {
                        break;
                    }
                    // If we find any token on the default channel (channel 0), it's not comment-only
                    if token.get_channel() == 0 {
                        return false;
                    }
                }
                true
        })};
    }

    match dialect {
        Bigquery => dialect_dispatch!(dbt_lexer_bigquery, bigquerylexer),
        Redshift => dialect_dispatch!(dbt_lexer_redshift, redshiftlexer),
        Snowflake => dialect_dispatch!(dbt_lexer_snowflake, snowflakelexer),
        Databricks => dialect_dispatch!(dbt_lexer_databricks, databrickslexer),
        Trino => dialect_dispatch!(dbt_lexer_trino, trinolexer),
        _ => {
            // Fallback to Trino (in release builds) lexer for not fully supported dialects.
            // If you hit this assert and wants to fallback to Trino without panicking,
            // add an explicit match arm, but preferably implement a proper lexer for the dialect.
            debug_assert!(
                false,
                "is_empty_or_comment_only() should only be called with fully supported dialects, but got {dialect:?}"
            );
            dialect_dispatch!(dbt_lexer_trino, trinolexer)
        }
    }
}

#[cfg(test)]
mod tests;
