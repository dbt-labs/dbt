use core::fmt;

#[derive(Debug)]
pub enum Token<'source> {
    LParen,
    RParen,
    /// Left bracket: [.
    LBracket,
    /// Right bracket: ].
    RBracket,
    /// Left angled bracket: <.
    LAngle,
    /// Right angled bracket: >.
    RAngle,
    Comma,
    Colon,
    /// Word includes keywords, identifiers, quoted identifiers, string literals,
    /// numeric literals and similar continuous pieces of source text.
    ///
    /// The parsers using this tokenizer are expected to interpret the actual
    /// meaning of the [Word](Token::Word) tokens.
    Word(&'source str),
}

impl Eq for Token<'_> {}

impl PartialEq for Token<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Token::LParen, Token::LParen) => true,
            (Token::RParen, Token::RParen) => true,
            (Token::LBracket, Token::LBracket) => true,
            (Token::RBracket, Token::RBracket) => true,
            (Token::LAngle, Token::LAngle) => true,
            (Token::RAngle, Token::RAngle) => true,
            (Token::Comma, Token::Comma) => true,
            (Token::Colon, Token::Colon) => true,
            (Token::Word(a), Token::Word(b)) => a.eq_ignore_ascii_case(b),
            _ => false,
        }
    }
}

impl fmt::Display for Token<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::LAngle => write!(f, "<"),
            Token::RAngle => write!(f, ">"),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Word(w) => write!(f, "{w}"),
        }
    }
}

/// The type of quote used for quoted identifiers or string literals.
#[derive(Debug, Copy, Clone)]
pub enum QuotingStyle {
    /// Single quote: '.
    Single,
    /// Double quote: ".
    Double,
    /// Backtick: `.
    Backtick,
    /// U&"..." style quoted identifier like in PostgreSQL.
    UAndDouble,
    // /// [...] style like in SQL Server (if `QUOTED_IDENTIFIER = OFF`).
    // Bracketed,
}

impl QuotingStyle {
    pub const fn opening(&self) -> &str {
        match self {
            QuotingStyle::Single => "'",
            QuotingStyle::Double => "\"",
            QuotingStyle::Backtick => "`",
            QuotingStyle::UAndDouble => "U&\"",
            // QuotingStyle::Bracketed => "[",
        }
    }

    pub const fn closing(&self) -> &str {
        match self {
            QuotingStyle::Single => "'",
            QuotingStyle::Double => "\"",
            QuotingStyle::Backtick => "`",
            QuotingStyle::UAndDouble => "\"",
            // QuotingStyle::Bracketed => "]",
        }
    }
}

/// ASCII character to [QuotingStyle] mapping.
impl TryFrom<u8> for QuotingStyle {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            b'\'' => Ok(QuotingStyle::Single),
            b'"' => Ok(QuotingStyle::Double),
            b'`' => Ok(QuotingStyle::Backtick),
            _ => Err(()),
        }
    }
}

fn is_whitespace(c: u8) -> bool {
    c == b' ' || c == b'\t' || c == b'\n' || c == b'\r'
}

pub struct Tokenizer<'source> {
    input: &'source str,
    position: usize,
}

impl<'source> Tokenizer<'source> {
    pub fn new(input: &'source str) -> Self {
        Tokenizer { input, position: 0 }
    }

    /// Looks at the current byte without consuming it.
    fn _peek_byte(&self) -> Option<u8> {
        let input = self.input.as_bytes();
        input.get(self.position).copied()
    }

    /// Consumes the current byte and returns it.
    fn _next_byte(&mut self) -> Option<u8> {
        self._peek_byte().inspect(|_| {
            self.position += 1;
        })
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self._peek_byte() {
            if is_whitespace(b) {
                self.position += 1;
            } else {
                break;
            }
        }
    }

    fn rest_of_quoted_word(&mut self, start: usize, quote: u8) -> Token<'source> {
        // We have already consumed the opening quote.
        while let Some(b) = self._next_byte() {
            if b == quote {
                // SQL is weird: two consecutive quotes inside a quoted
                // identifier escape the quote character, so we need to peek.
                if let Some(next) = self._peek_byte()
                    && next == quote
                {
                    // Consume the escaped quote and continue.
                    self.position += 1;
                    continue;
                }
                // TODO: handle \{quote} escape sequences
                // This is the closing quote, because it's not escaped.
                //
                // SAFETY: this is a valid UTF8 slice because breaks only
                // occur on whitespece or delimiter ASCII characters.
                let word = &self.input[start..self.position];
                return Token::Word(word);
            }
        }
        debug_assert!(
            self.position > start,
            "quoted word contains at least the opening quote"
        );
        // If we reach here, there was no closing quote. Return the rest of the
        // input as a word. The parsers using this tokenizer validate quoted
        // identifiers for matching quotes.
        let word = &self.input[start..self.position];
        Token::Word(word)
    }

    fn rest_of_word(&mut self, start: usize) -> Option<Token<'source>> {
        // We have already consumed the first non-whitespace, non-delimiter byte of the word.
        while let Some(b) = self._peek_byte() {
            match b {
                b'(' | b')' | b'[' | b']' | b'<' | b'>' => break,
                b',' | b':' => break,
                b'\'' | b'"' | b'`' => break,
                _ if is_whitespace(b) => break,
                _ => {
                    self.position += 1;
                    continue;
                }
            }
        }
        // SAFETY: this is a valid UTF8 slice because breaks only
        // occur on whitespece or delimiter ASCII characters.
        let word = &self.input[start..self.position];
        if start == self.position {
            None
        } else {
            Some(Token::Word(word))
        }
    }

    /// Consumes the next token from the input.
    pub fn next(&mut self) -> Option<Token<'source>> {
        self.skip_whitespace();
        let start = self.position;
        let token = if let Some(b) = self._next_byte() {
            match b {
                b'(' => Token::LParen,
                b')' => Token::RParen,
                b'[' => Token::LBracket,
                b']' => Token::RBracket,
                b'<' => Token::LAngle,
                b'>' => Token::RAngle,
                b',' => Token::Comma,
                b':' => Token::Colon,
                b'\'' | b'"' | b'`' => self.rest_of_quoted_word(start, b),
                _ => return self.rest_of_word(start),
            }
        } else {
            return None;
        };
        Some(token)
    }

    /// Consumes the next token if and only if it matches the provided token.
    pub fn match_(&mut self, pred: impl FnOnce(Token<'source>) -> bool) -> bool {
        let old_pos = self.position;
        if let Some(tok) = self.next()
            && pred(tok)
        {
            return true;
        }
        self.position = old_pos;
        false
    }

    /// Peeks at the next token and applies the provided function to it. If the function
    /// returns `None`, the tokenizer's position is reset to its previous state.
    pub fn peek_and_then<T>(&mut self, f: impl FnOnce(Token<'source>) -> Option<T>) -> Option<T> {
        let old_pos = self.position;
        if let Some(tok) = self.next() {
            let res = f(tok);
            if res.is_some() {
                return res;
            }
        }
        self.position = old_pos;
        None
    }

    /// Speculatively run a closure that may consume multiple tokens. If it returns
    /// `None`, the tokenizer's position is reset to its state before the call.
    pub fn try_<T>(&mut self, f: impl FnOnce(&mut Self) -> Option<T>) -> Option<T> {
        let old_pos = self.position;
        let res = f(self);
        if res.is_none() {
            self.position = old_pos;
        }
        res
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenizer() {
        let mut tokenizer = Tokenizer::new("");
        assert_eq!(tokenizer.next(), None);

        let mut tokenizer = Tokenizer::new("  ");
        assert_eq!(tokenizer.next(), None);

        let mut tokenizer = Tokenizer::new("BOOLEAN");
        assert_eq!(tokenizer.next(), Some(Token::Word("BOOLEAN")));

        let mut tokenizer = Tokenizer::new("  BOOLEAN");
        assert_eq!(tokenizer.next(), Some(Token::Word("BOOLEAN")));

        let mut tokenizer = Tokenizer::new("BOOLEAN  ");
        assert_eq!(tokenizer.next(), Some(Token::Word("BOOLEAN")));

        let mut tokenizer = Tokenizer::new("  BOOLEAN  ");
        assert_eq!(tokenizer.next(), Some(Token::Word("BOOLEAN")));

        let mut tokenizer = Tokenizer::new("TIMESTAMP (3) WITH TIME ZONE");
        assert_eq!(tokenizer.next(), Some(Token::Word("TIMESTAMP")));
        assert_eq!(tokenizer.next(), Some(Token::LParen));
        assert_eq!(tokenizer.next(), Some(Token::Word("3")));
        assert_eq!(tokenizer.next(), Some(Token::RParen));
        assert_eq!(tokenizer.next(), Some(Token::Word("WITH")));
        assert_eq!(tokenizer.next(), Some(Token::Word("TIME")));
        assert_eq!(tokenizer.next(), Some(Token::Word("ZONE")));

        let mut tokenizer = Tokenizer::new("STRUCT<a FLOAT64>");
        assert_eq!(tokenizer.next(), Some(Token::Word("STRUCT")));
        assert_eq!(tokenizer.next(), Some(Token::LAngle));
        assert_eq!(tokenizer.next(), Some(Token::Word("a")));
        assert_eq!(tokenizer.next(), Some(Token::Word("FLOAT64")));
        assert_eq!(tokenizer.next(), Some(Token::RAngle));

        let mut tokenizer = Tokenizer::new("☃");
        assert_eq!(tokenizer.next(), Some(Token::Word("☃")));

        let mut tokenizer = Tokenizer::new("☃☃");
        assert_eq!(tokenizer.next(), Some(Token::Word("☃☃")));

        let mut tokenizer = Tokenizer::new("☃SNOWMAN☃(1)");
        assert_eq!(tokenizer.next(), Some(Token::Word("☃SNOWMAN☃")));
        assert_eq!(tokenizer.next(), Some(Token::LParen));
        assert_eq!(tokenizer.next(), Some(Token::Word("1")));
        assert_eq!(tokenizer.next(), Some(Token::RParen));

        let mut tokenizer = Tokenizer::new("S☃NOWMA☃N");
        assert_eq!(tokenizer.next(), Some(Token::Word("S☃NOWMA☃N")));
    }

    fn all_tokens<'source>(input: &'source str) -> Vec<Token<'source>> {
        let mut tokenizer = Tokenizer::new(input);
        let mut tokens = Vec::new();
        while let Some(tok) = tokenizer.next() {
            tokens.push(tok);
        }
        tokens
    }

    #[test]
    fn test_tokenizing_of_quoted_words() {
        let test_cases = [
            // Double quotes
            (line!(), r#""a"#, vec![Token::Word(r#""a"#)]),
            (line!(), r#""a'b"#, vec![Token::Word(r#""a'b"#)]),
            (line!(), r#""a'b""#, vec![Token::Word(r#""a'b""#)]),
            (
                line!(),
                r#""a'b"c"#,
                vec![Token::Word(r#""a'b""#), Token::Word("c")],
            ),
            (
                line!(),
                r#""a'b"c""#,
                vec![
                    Token::Word(r#""a'b""#),
                    Token::Word("c"),
                    Token::Word(r#"""#),
                ],
            ),
            (
                line!(),
                r#""a'b"c"""#,
                vec![
                    Token::Word(r#""a'b""#),
                    Token::Word("c"),
                    Token::Word(r#""""#),
                ],
            ),
            // Single quotes
            (line!(), r#"'a"#, vec![Token::Word(r#"'a"#)]),
            (line!(), r#"'a\"b"#, vec![Token::Word(r#"'a\"b"#)]),
            (line!(), r#"'a\"b'"#, vec![Token::Word(r#"'a\"b'"#)]),
            (
                line!(),
                r#"'a\"b'c"#,
                vec![Token::Word(r#"'a\"b'"#), Token::Word("c")],
            ),
            (
                line!(),
                r#"'a\"b'c'"#,
                vec![
                    Token::Word(r#"'a\"b'"#),
                    Token::Word("c"),
                    Token::Word(r#"'"#),
                ],
            ),
            (
                line!(),
                r#"'a\"b'c''"#,
                vec![
                    Token::Word(r#"'a\"b'"#),
                    Token::Word("c"),
                    Token::Word(r#"''"#),
                ],
            ),
            // Backticks work the same way
            (
                line!(),
                r#"`a\"b`c``"#,
                vec![
                    Token::Word(r#"`a\"b`"#),
                    Token::Word("c"),
                    Token::Word(r#"``"#),
                ],
            ),
            // Quoted words and other delimiters
            (
                line!(),
                r#""Abra", 'ca' `dabra`: (c)Abra-ca-dabra <d>: Abracadabra[e]: :"oo""na"na:  "#,
                vec![
                    Token::Word(r#""Abra""#),
                    Token::Comma,
                    Token::Word(r#"'ca'"#),
                    Token::Word(r#"`dabra`"#),
                    Token::Colon,
                    Token::LParen,
                    Token::Word("c"),
                    Token::RParen,
                    Token::Word("Abra-ca-dabra"),
                    Token::LAngle,
                    Token::Word("d"),
                    Token::RAngle,
                    Token::Colon,
                    Token::Word("Abracadabra"),
                    Token::LBracket,
                    Token::Word("e"),
                    Token::RBracket,
                    Token::Colon,
                    Token::Colon,
                    Token::Word(r#""oo""na""#),
                    Token::Word(r#"na"#),
                    Token::Colon,
                ],
            ),
            (
                line!(),
                "(a REAL)",
                vec![
                    Token::LParen,
                    Token::Word("a"),
                    Token::Word("REAL"),
                    Token::RParen,
                ],
            ),
        ];
        for (line, input, expected) in test_cases {
            let tokens = all_tokens(input);
            assert_eq!(
                tokens,
                expected,
                "input: r#\"{input}\"# from {}:{line}",
                file!()
            );
        }
    }
}
