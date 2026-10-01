use std::iter::Peekable;
use std::str::CharIndices;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Token<'a> {
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `,`
    Comma,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `<`
    Lt,
    /// `<=`
    Lte,
    /// `>`
    Gt,
    /// `>=`
    Gte,
    /// `==`
    Eq,
    /// `!=`
    Neq,
    /// `[0-9]{0,18}`
    Int(&'a str),
    /// `[a-z][a-z0-9_]*`
    Ident(&'a str),
    Eof,
}

pub(crate) fn tokenize(source: &str) -> impl Iterator<Item = Result<Token<'_>, String>> {
    Tokenizer {
        source,
        input: source.char_indices().peekable(),
    }
    .chain(std::iter::once(Ok(Token::Eof)))
}

struct Tokenizer<'src> {
    source: &'src str,
    input: Peekable<CharIndices<'src>>,
}

impl<'src> Iterator for Tokenizer<'src> {
    type Item = Result<Token<'src>, String>;

    fn next(&mut self) -> Option<Self::Item> {
        while self.move_if(|c| c.is_whitespace()) {}
        let (offset, next) = self.input.next()?;
        let token = match next {
            '(' => Token::LParen,
            ')' => Token::RParen,
            ',' => Token::Comma,
            '+' => Token::Plus,
            '-' => Token::Minus,
            '<' => {
                if self.move_if(|c| c == '=') {
                    Token::Lte
                } else {
                    Token::Lt
                }
            }
            '>' => {
                if self.move_if(|c| c == '=') {
                    Token::Gte
                } else {
                    Token::Gt
                }
            }
            '=' => {
                if self.move_if(|c| c == '=') {
                    Token::Eq
                } else {
                    return self.unexpected(offset);
                }
            }
            '!' => {
                if self.move_if(|c| c == '=') {
                    Token::Neq
                } else {
                    return self.unexpected(offset);
                }
            }
            '0'..='9' => {
                let (start, mut end) = (offset, offset + 1);
                while self.move_if(|c| c.is_ascii_digit()) {
                    end += 1;
                }
                if self.next_is(is_ident_continue) {
                    return self.unexpected(end);
                }
                Token::Int(&self.source[start..end])
            }
            'a'..='z' | 'A'..='Z' => {
                let (start, mut end) = (offset, offset + 1);
                while self.move_if(is_ident_continue) {
                    end += 1;
                }
                Token::Ident(&self.source[start..end])
            }
            _ => return self.unexpected(offset),
        };
        Some(Ok(token))
    }
}
impl Tokenizer<'_> {
    fn move_if(&mut self, test: impl FnOnce(char) -> bool) -> bool {
        if self.next_is(test) {
            self.input.next();
            true
        } else {
            false
        }
    }

    fn next_is(&mut self, test: impl FnOnce(char) -> bool + Sized) -> bool {
        self.input.peek().is_some_and(|(_, c)| test(*c))
    }

    fn unexpected<V>(&self, offset: usize) -> Option<Result<V, String>> {
        Some(Err(format!(
            "Unexpected character '{}' at offset {} in: '{}'",
            char_at(self.source, offset),
            offset,
            self.source
        )))
    }
}

/// Panics if offset is out of bounds or not character boundary
fn char_at(s: &str, offset: usize) -> char {
    assert!(
        offset < s.len(),
        "Invalid string offset {} for string of length {}",
        offset,
        s.len()
    );
    assert!(
        s.is_char_boundary(offset),
        "Invalid character offset, not a string boundary: {offset} in '{s}'"
    );
    s[offset..].chars().next().unwrap()
}

fn is_ident_continue(c: char) -> bool {
    matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9' | '_')
}

#[cfg(test)]
mod tests {
    use super::Token::*;
    use super::*;

    #[test]
    fn test_empty() {
        assert_eq!(tokens(""), vec![Eof]);
    }

    #[test]
    fn test_paren() {
        assert_eq!(tokens("("), vec![LParen, Eof]);
        assert_eq!(tokens(")"), vec![RParen, Eof]);
    }

    #[test]
    fn test_operator() {
        assert_eq!(tokens("<"), vec![Lt, Eof]);
        assert_eq!(tokens("<="), vec![Lte, Eof]);
        assert_eq!(tokens(">"), vec![Gt, Eof]);
        assert_eq!(tokens(">="), vec![Gte, Eof]);
        assert_eq!(tokens("=="), vec![Eq, Eof]);
        assert_eq!(tokens("!="), vec![Neq, Eof]);
        assert_eq!(
            try_tokens("===").unwrap_err(),
            "Unexpected character '=' at offset 2 in: '==='"
        );
        assert_eq!(tokens("===="), vec![Eq, Eq, Eof]);
        assert_eq!(tokens("<<===>>"), vec![Lt, Lte, Eq, Gt, Gt, Eof]);
        assert_eq!(
            try_tokens("<<====>>").unwrap_err(),
            "Unexpected character '=' at offset 5 in: '<<====>>'"
        );
    }

    #[test]
    fn test_ident() {
        assert_eq!(tokens("a"), vec![Ident("a"), Eof]);
        assert_eq!(tokens("abcdef"), vec![Ident("abcdef"), Eof]);
        assert_eq!(
            tokens("AlaMa_Kota_i_Psa"),
            vec![Ident("AlaMa_Kota_i_Psa"), Eof]
        );
        assert_eq!(tokens("a123456"), vec![Ident("a123456"), Eof]);
    }

    #[test]
    fn test_integer() {
        assert_eq!(tokens("0"), vec![Int("0"), Eof]);
        assert_eq!(
            tokens("9223372036854775807"),
            vec![Int("9223372036854775807"), Eof]
        );
        assert_eq!(
            tokens("1234567890123456789012345678901234567890"),
            vec![Int("1234567890123456789012345678901234567890"), Eof]
        );

        assert_eq!(
            try_tokens("123a").unwrap_err(),
            "Unexpected character 'a' at offset 3 in: '123a'"
        );
    }

    #[test]
    fn test_smoke() {
        assert_eq!(
            tokens("a+b(), def0 123"),
            vec![
                Ident("a"),
                Plus,
                Ident("b"),
                LParen,
                RParen,
                Comma,
                Ident("def0"),
                Int("123"),
                Eof
            ]
        );

        assert_eq!(
            tokens("min((,,,,))+max---777777 <<===>> party time"),
            vec![
                Ident("min"),
                LParen,
                LParen,
                Comma,
                Comma,
                Comma,
                Comma,
                RParen,
                RParen,
                Plus,
                Ident("max"),
                Minus,
                Minus,
                Minus,
                Int("777777"),
                Lt,
                Lte,
                Eq,
                Gt,
                Gt,
                Ident("party"),
                Ident("time"),
                Eof
            ]
        );
    }

    fn tokens(s: &str) -> Vec<Token<'_>> {
        try_tokens(s).unwrap()
    }
    fn try_tokens(s: &str) -> Result<Vec<Token<'_>>, String> {
        tokenize(s).collect::<Result<_, _>>()
    }
}
