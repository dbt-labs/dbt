//! What a warning can carry: the fixes it suggests, and which warnings the
//! source suppresses.

use std::collections::HashMap;

use crate::error::ErrorCode;
use crate::span::{AbsoluteSpan, Span};

/// A suggested edit: replace the text at `span` with `replacement`.
#[derive(Debug, Clone)]
pub struct Fix {
    pub span: AbsoluteSpan,
    pub replacement: String,
}

impl Fix {
    pub fn new(span: AbsoluteSpan, replacement: String) -> Self {
        Self { span, replacement }
    }

    pub fn with_offset(&self, offset: &Span) -> Self {
        Self {
            span: self.span.with_offset(offset),
            replacement: self.replacement.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Noqas {
    /// `-- noqa-file` — suppress every violation in the file.
    All,
    /// Per-line and file-level suppression.
    Lines {
        /// 0-indexed line number → `None` (suppress all codes) or `Some(vec)` (suppress only
        /// those `ErrorCode`s). Populated by `-- noqa` and `-- noqa: CODE,CODE` comments.
        by_line: HashMap<usize, Option<Vec<ErrorCode>>>,
        /// `ErrorCode`s suppressed for the entire file (`-- sqlfluff:disable CODE`).
        disabled: Vec<ErrorCode>,
    },
}

impl Noqas {
    /// Return `true` if `code` at `line` (0-indexed) is suppressed.
    pub fn is_suppressed(&self, code: ErrorCode, line: usize) -> bool {
        match self {
            Noqas::All => true,
            Noqas::Lines { by_line, disabled } => {
                if disabled.contains(&code) {
                    return true;
                }
                match by_line.get(&line) {
                    Some(None) => true,
                    Some(Some(codes)) => codes.contains(&code),
                    None => false,
                }
            }
        }
    }
}

impl Default for Noqas {
    fn default() -> Self {
        Noqas::Lines {
            by_line: HashMap::new(),
            disabled: Vec::new(),
        }
    }
}
