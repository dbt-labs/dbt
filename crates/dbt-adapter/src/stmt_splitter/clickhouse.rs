//! Statement splitting for ClickHouse SQL.
//!
//! None of the engine's lexers covers ClickHouse's lexis: string literals use
//! backslash escapes (`\'`, `\\`, `\xNN`) *and* the server accepts
//! PostgreSQL-style heredocs (`$tag$ … $tag$`, which `persist_docs` relies
//! on). This scanner knows exactly the constructs that can hide a `;`:
//! quoted literals and identifiers, heredocs, and `--`, `//`, `# `, `#!` and
//! nested `/* */` comments (rules per ClickHouse's `Lexer.cpp`, checked
//! against a 26.3 server).

/// Split `sql` on top-level `;` (the `StmtSplitter::split` contract): the
/// input's surrounding whitespace and the delimiters are removed, everything
/// else is kept verbatim (statements carry their own leading whitespace and
/// comments); a trailing whitespace-only segment is dropped.
pub(crate) fn split_statements(sql: &str) -> Vec<&str> {
    let sql = trim_keeping_hash_comment_space(sql);
    let mut statements = Vec::new();
    let mut start = 0;
    let mut scanner = Scanner::new(sql);
    while let Some((pos, byte)) = scanner.next_significant() {
        if byte == b';' {
            statements.push(&sql[start..pos]);
            start = pos + 1;
        }
    }
    let tail = &sql[start..];
    if !tail.trim().is_empty() {
        statements.push(tail);
    }
    statements
}

/// `str::trim`, except that a trailing `# ` comment keeps its space: ClickHouse
/// only treats `#` as a comment when a space (or `!`) follows it.
fn trim_keeping_hash_comment_space(sql: &str) -> &str {
    let trimmed = sql.trim();
    let start = sql.len() - sql.trim_start().len();
    let end = start + trimmed.len();
    if trimmed.ends_with('#') && sql[end..].starts_with(' ') {
        &sql[start..=end]
    } else {
        trimmed
    }
}

/// Whether `sql` holds no statement at all: only whitespace and comments. A
/// lone literal or heredoc is not empty; it goes to the server, which rejects it.
pub(crate) fn is_empty_or_comment_only(sql: &str) -> bool {
    Scanner::new(sql).next_significant().is_none()
}

/// Walks `sql` and yields the bytes that sit outside comments and whitespace.
/// A quoted literal, identifier or heredoc is yielded as its opening byte with
/// the body skipped: it counts as content but cannot hide a `;`.
struct Scanner<'a> {
    sql: &'a str,
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Scanner<'a> {
    fn new(sql: &'a str) -> Self {
        Self {
            sql,
            bytes: sql.as_bytes(),
            pos: 0,
        }
    }

    fn next_significant(&mut self) -> Option<(usize, u8)> {
        while let Some(&byte) = self.bytes.get(self.pos) {
            let pos = self.pos;
            let next = self.bytes.get(pos + 1).copied();
            match byte {
                b'\'' | b'"' | b'`' => {
                    self.skip_quoted(byte);
                    return Some((pos, byte));
                }
                b'$' => {
                    self.pos = self.heredoc_end(pos).unwrap_or(pos + 1);
                    return Some((pos, byte));
                }
                b'-' if next == Some(b'-') => self.skip_line_comment(),
                b'/' if next == Some(b'/') => self.skip_line_comment(),
                // MySQL-style `# ` and shebang `#!` comments; a bare `#word` is not a comment.
                b'#' if matches!(next, Some(b' ' | b'!')) => self.skip_line_comment(),
                b'/' if next == Some(b'*') => self.skip_block_comment(),
                _ if byte.is_ascii_whitespace() => self.pos += 1,
                _ => {
                    self.pos += 1;
                    return Some((pos, byte));
                }
            }
        }
        None
    }

    /// Quoted literal/identifier with backslash escapes and doubled quotes
    /// (`'it''s'`). An unterminated literal swallows the rest of the input,
    /// which keeps a broken statement whole instead of splitting it.
    fn skip_quoted(&mut self, quote: u8) {
        self.pos += 1;
        while let Some(&byte) = self.bytes.get(self.pos) {
            self.pos += 1;
            match byte {
                b'\\' => self.pos += 1,
                b if b == quote && self.bytes.get(self.pos) == Some(&quote) => self.pos += 1,
                b if b == quote => return,
                _ => {}
            }
        }
        self.pos = self.bytes.len();
    }

    fn skip_line_comment(&mut self) {
        while self.pos < self.bytes.len() && self.bytes[self.pos] != b'\n' {
            self.pos += 1;
        }
    }

    /// Block comments nest (`/* a /* b */ c */`), as in ClickHouse.
    fn skip_block_comment(&mut self) {
        self.pos += 2;
        let mut depth = 1;
        while self.pos < self.bytes.len() {
            match (self.bytes[self.pos], self.bytes.get(self.pos + 1)) {
                (b'/', Some(b'*')) => {
                    depth += 1;
                    self.pos += 2;
                }
                (b'*', Some(b'/')) => {
                    depth -= 1;
                    self.pos += 2;
                    if depth == 0 {
                        return;
                    }
                }
                _ => self.pos += 1,
            }
        }
    }

    /// End (exclusive) of the heredoc opening at `start`, i.e. just past the
    /// closing `$tag$`; the tag is any run of word characters, empty included.
    /// None when `start` does not open a heredoc or it never closes (then the
    /// `$` is an ordinary character).
    fn heredoc_end(&self, start: usize) -> Option<usize> {
        let rest = &self.sql[start + 1..];
        let tag_len = rest
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'_')
            .count();
        if rest.as_bytes().get(tag_len) != Some(&b'$') {
            return None;
        }
        let opener = &self.sql[start..start + tag_len + 2];
        let body_start = start + opener.len();
        self.sql[body_start..]
            .find(opener)
            .map(|offset| body_start + offset + opener.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_top_level_semicolons_only() {
        assert_eq!(
            split_statements("SELECT 1; SELECT 2; SELECT 3;"),
            vec!["SELECT 1", " SELECT 2", " SELECT 3"]
        );
        assert_eq!(split_statements(";;;"), vec!["", "", ""]);
        assert_eq!(split_statements(""), Vec::<&str>::new());
        assert_eq!(
            split_statements("select 'a;b'; select \"c;d\"; select `e;f`"),
            vec!["select 'a;b'", " select \"c;d\"", " select `e;f`"]
        );
    }

    #[test]
    fn trims_the_input_but_not_the_statements() {
        assert_eq!(split_statements("  select 1  "), vec!["select 1"]);
        assert_eq!(
            split_statements("\n  select 1;\n  select 2;\n"),
            vec!["select 1", "\n  select 2"]
        );
        assert_eq!(split_statements("select 1 ;  "), vec!["select 1 "]);
    }

    #[test]
    fn keeps_comments_as_statements() {
        assert_eq!(
            split_statements("select 1; /* end comment */"),
            vec!["select 1", " /* end comment */"]
        );
        assert_eq!(
            split_statements("select 1 -- trailing ; comment\n; select 2"),
            vec!["select 1 -- trailing ; comment\n", " select 2"]
        );
        assert_eq!(
            split_statements("select 1; /* comment with ; */; select 2"),
            vec!["select 1", " /* comment with ; */", " select 2"]
        );
        assert_eq!(
            split_statements(
                "#!/usr/bin/clickhouse-local ; x\nselect 1 # hash ; comment\n; select 2"
            ),
            vec![
                "#!/usr/bin/clickhouse-local ; x\nselect 1 # hash ; comment\n",
                " select 2"
            ]
        );
        assert_eq!(
            split_statements("select '#'; select 1 #notacomment; select 2"),
            vec!["select '#'", " select 1 #notacomment", " select 2"]
        );
        assert_eq!(
            split_statements("select 1 // slash ; comment\n; select 2"),
            vec!["select 1 // slash ; comment\n", " select 2"]
        );
        assert_eq!(
            split_statements("select 1 /* outer /* inner */ ; select 99; */; select 2"),
            vec!["select 1 /* outer /* inner */ ; select 99; */", " select 2"]
        );
        // a trailing `# ` comment keeps the space the trim would take
        assert_eq!(split_statements("select 1; # \n"), vec!["select 1", " # "]);
        assert_eq!(split_statements("select 2 # \n"), vec!["select 2 # "]);
        assert_eq!(split_statements("select 3 #\n"), vec!["select 3 #"]);
    }

    #[test]
    fn clickhouse_backslash_escapes_and_doubled_quotes() {
        assert_eq!(
            split_statements(r#"select 'don\'t; drop'; select '\x3F', 'a\\'; select 'it''s; ok'"#),
            vec![
                r#"select 'don\'t; drop'"#,
                r#" select '\x3F', 'a\\'"#,
                " select 'it''s; ok'"
            ]
        );
    }

    #[test]
    fn heredocs_hide_delimiters() {
        let sql = "alter table t modify comment $dbt_comment_literal_block$desc \"q\";\n'''x''' -- not a comment\n/* nor this */ $lbl$ nested $lbl$ and $$ bare $$ end$dbt_comment_literal_block$; select 1";
        let statements = split_statements(sql);
        assert_eq!(statements.len(), 2, "{statements:?}");
        assert!(statements[0].ends_with("end$dbt_comment_literal_block$"));
        assert_eq!(statements[1], " select 1");
        assert_eq!(
            split_statements("select $1x$a;b$1x$; select 2"),
            vec!["select $1x$a;b$1x$", " select 2"]
        );
        assert_eq!(
            split_statements("select $a b$x;y$a b$"),
            vec!["select $a b$x", "y$a b$"]
        );
        assert_eq!(
            split_statements("select $1; select 2"),
            vec!["select $1", " select 2"]
        );
        assert_eq!(
            split_statements("select $unterminated$ 1; select 2"),
            vec!["select $unterminated$ 1", " select 2"]
        );
    }

    #[test]
    fn unterminated_literal_keeps_the_rest_whole() {
        assert_eq!(
            split_statements("select 1; select 'unterminated; select 2"),
            vec!["select 1", " select 'unterminated; select 2"]
        );
    }

    #[test]
    fn empty_or_comment_only() {
        for sql in [
            "",
            "   ",
            "/* comment */",
            "-- line comment",
            "# hash comment",
            "#!/usr/bin/clickhouse-local",
            " # ",
            "// slash comment",
            "  /* comment */  ",
            "/* comment */ -- line comment",
            "/* multi\nline\ncomment */",
            "/* outer /* inner */ ; */",
        ] {
            assert!(is_empty_or_comment_only(sql), "{sql:?}");
        }
        for sql in [
            "select 1",
            "select /* comment */ 1",
            "-- comment\nselect 1",
            "/* comment */\nselect 1\n-- trailing",
            r#"select 'don\'t'"#,
            "#notacomment",
            "#\nselect 1",
            "#",
            ";",
            "'oops'",
            "'unterminated",
            "\"id\"",
            "`id`",
            "$$heredoc$$",
            "$tag$a;b$tag$",
        ] {
            assert!(!is_empty_or_comment_only(sql), "{sql:?}");
        }
    }
}
