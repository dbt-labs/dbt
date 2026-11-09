use dbt_antlr4::char_stream::CharStream;
use dbt_antlr4::int_stream::EOF;
use dbt_frontend_common::error::CodeLocation;
use dbt_frontend_common::span::Span;

#[allow(clippy::cognitive_complexity)]
pub fn find_singlestore_statement_spans<'input>(mut stream: impl CharStream<'input>) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut start_loc: Option<CodeLocation> = None;
    let mut last_loc: Option<CodeLocation> = None;
    let mut line: u32 = 1;
    let mut col: u32 = 0;

    macro_rules! curr_loc {
        () => {
            CodeLocation::new(line, col, stream.index() as u32)
        };
    }

    macro_rules! consume_char {
        () => {{
            let ch = stream.la(1);
            if ch != EOF {
                stream.consume();
                if ch == '\n' as i32 {
                    line += 1;
                    col = 0;
                } else {
                    col += 1;
                }
            }
            ch
        }};
    }

    loop {
        let ch = stream.la(1);
        if ch == EOF {
            break;
        }

        if start_loc.is_none() {
            start_loc = Some(curr_loc!());
        }

        match ch as u8 {
            b';' => {
                let semi_loc = curr_loc!();
                consume_char!();
                let start = start_loc.take().unwrap();
                spans.push(Span::new(start, semi_loc));
                last_loc = None;
                continue;
            }
            b'\'' => {
                consume_char!();
                loop {
                    let c = stream.la(1);
                    if c == EOF {
                        break;
                    }
                    if c == '\\' as i32 {
                        consume_char!();
                        if stream.la(1) != EOF {
                            consume_char!();
                        }
                    } else if c == '\'' as i32 {
                        consume_char!();
                        if stream.la(1) == '\'' as i32 {
                            consume_char!();
                        } else {
                            break;
                        }
                    } else {
                        consume_char!();
                    }
                }
            }
            b'"' => {
                consume_char!();
                loop {
                    let c = stream.la(1);
                    if c == EOF {
                        break;
                    }
                    if c == '\\' as i32 {
                        consume_char!();
                        if stream.la(1) != EOF {
                            consume_char!();
                        }
                    } else if c == '"' as i32 {
                        consume_char!();
                        if stream.la(1) == '"' as i32 {
                            consume_char!();
                        } else {
                            break;
                        }
                    } else {
                        consume_char!();
                    }
                }
            }
            b'`' => {
                consume_char!();
                loop {
                    let c = stream.la(1);
                    if c == EOF {
                        break;
                    }
                    if c == '`' as i32 {
                        consume_char!();
                        if stream.la(1) == '`' as i32 {
                            consume_char!();
                        } else {
                            break;
                        }
                    } else {
                        consume_char!();
                    }
                }
            }
            b'#' => {
                consume_char!();
                loop {
                    let c = stream.la(1);
                    if c == EOF {
                        break;
                    }
                    let consumed = consume_char!();
                    if consumed == '\n' as i32 {
                        break;
                    }
                }
            }
            b'-' => {
                consume_char!();
                if stream.la(1) == '-' as i32 {
                    consume_char!();
                    let next = stream.la(1);
                    if next == EOF
                        || next == ' ' as i32
                        || next == '\t' as i32
                        || next == '\r' as i32
                        || next == '\n' as i32
                        || (0..32).contains(&next)
                    {
                        loop {
                            let c = stream.la(1);
                            if c == EOF {
                                break;
                            }
                            let consumed = consume_char!();
                            if consumed == '\n' as i32 {
                                break;
                            }
                        }
                    }
                }
            }
            b'/' => {
                consume_char!();
                if stream.la(1) == '*' as i32 {
                    consume_char!();
                    loop {
                        let c = stream.la(1);
                        if c == EOF {
                            break;
                        }
                        let ch = consume_char!();
                        if ch == '*' as i32 && stream.la(1) == '/' as i32 {
                            consume_char!();
                            break;
                        }
                    }
                }
            }
            b'*' => {
                consume_char!();
                if stream.la(1) == '/' as i32 {
                    consume_char!();
                }
            }
            _ => {
                consume_char!();
            }
        }
        last_loc = Some(curr_loc!());
    }

    if let (Some(start), Some(last)) = (start_loc, last_loc) {
        if start.index != last.index {
            spans.push(Span::new(start, last));
        }
    }

    spans
}

#[allow(clippy::cognitive_complexity)]
pub fn is_singlestore_empty_or_comment_only(statement: &str) -> bool {
    let bytes = statement.as_bytes();
    let mut i = 0;
    let len = bytes.len();

    while i < len {
        let b = bytes[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        if b == b'#' {
            i += 1;
            while i < len && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }

        if b == b'-' && i + 1 < len && bytes[i + 1] == b'-' {
            let next = if i + 2 < len {
                Some(bytes[i + 2])
            } else {
                None
            };
            if match next {
                None => true,
                Some(nb) => nb.is_ascii_whitespace() || nb < 32,
            } {
                i += 2;
                while i < len && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            return false;
        }

        if b == b'/' && i + 1 < len && bytes[i + 1] == b'*' {
            if i + 2 < len && bytes[i + 2] == b'!' {
                return false;
            }
            i += 2;
            let mut closed = false;
            while i + 1 < len {
                if bytes[i] == b'*' && bytes[i + 1] == b'/' {
                    i += 2;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                i = len;
            }
            continue;
        }

        return false;
    }

    true
}
