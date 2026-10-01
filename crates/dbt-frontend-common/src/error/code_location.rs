use std::{cmp, ops};

use dbt_antlr4::token::Token;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, Hash)]
pub struct CodeLocation {
    /// 1-based line number
    pub line: u32,
    /// 1-based column number(unicode characters)
    pub col: u32,
    /// 0-based offset(byte characters)
    pub index: u32,
}

impl Ord for CodeLocation {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        if self.index == 0 || other.index == 0 {
            // compare line and column instead, because antlr index is always 0
            if self.line == other.line {
                self.col.cmp(&other.col)
            } else {
                self.line.cmp(&other.line)
            }
        } else {
            self.index.cmp(&other.index)
        }
    }
}

impl CodeLocation {
    /// Create a new CodeLocation with the given line and column
    pub fn new(line: u32, column: u32, index: u32) -> Self {
        CodeLocation {
            line,
            col: column,
            index,
        }
    }

    /// Create a new CodeLocation pointing to the start of file (line 1, column 1)
    ///
    /// Note: this is *not* the same as `CodeLocation::default()`.
    pub fn start_of_file() -> Self {
        CodeLocation {
            line: 1,
            col: 1,
            index: 0,
        }
    }

    /// Whether this code location has valid line and column number info.
    pub fn has_position(&self) -> bool {
        // 0:0 means unknown location
        self.line != 0 || self.col != 0
    }

    pub fn with_offset(&self, start_location: &CodeLocation) -> Self {
        if !self.has_position() {
            return *self;
        }
        if self.line == 1 {
            CodeLocation {
                line: self.line + start_location.line - 1,
                col: self.col + start_location.col - 1,
                index: self.index + start_location.index,
            }
        } else {
            CodeLocation {
                line: self.line + start_location.line - 1,
                col: self.col,
                index: self.index + start_location.index,
            }
        }
    }

    pub fn diff(&self, start_location: &CodeLocation) -> CodeLocation {
        let line = self.line - start_location.line + 1;
        CodeLocation {
            line,
            col: if line == 1 {
                self.col - start_location.col + 1
            } else {
                self.col
            },
            index: self.index - start_location.index,
        }
    }

    /// Returns a new `CodeLocation` advanced as if all characters in `s` were
    /// consumed after this one.
    pub fn advance_by_text(&self, s: &str) -> Self {
        let mut line = self.line;
        let mut col = self.col;
        for c in s.chars() {
            if c == '\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
        CodeLocation::new(line, col, self.index + (s.len() as u32))
    }
}

impl From<&dyn Token> for CodeLocation {
    fn from(token: &dyn Token) -> Self {
        CodeLocation {
            line: token.get_line(),
            col: token.get_char_position_in_line() as u32 + 1,
            index: token.get_start_index() as u32,
        }
    }
}

impl std::fmt::Display for CodeLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

impl std::fmt::Debug for CodeLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}({})", self.line, self.col, self.index)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct LocationDiff {
    line: isize,
    col: isize,
    pos: isize,
}

impl LocationDiff {
    pub fn new(line: isize, col: isize, pos: isize) -> Self {
        Self { line, col, pos }
    }
}

impl ops::Sub<CodeLocation> for CodeLocation {
    type Output = LocationDiff;

    fn sub(self, other: CodeLocation) -> LocationDiff {
        let line_diff = self.line as isize - other.line as isize;
        let col_diff = if line_diff == 0 {
            self.col as isize - other.col as isize
        } else {
            self.col as isize
        };
        LocationDiff {
            line: line_diff,
            col: col_diff,
            pos: self.index as isize - other.index as isize,
        }
    }
}

impl ops::Add<LocationDiff> for CodeLocation {
    type Output = CodeLocation;

    fn add(self, other: LocationDiff) -> CodeLocation {
        let line = (self.line as isize + other.line) as usize;
        let col = if other.line == 0 {
            (self.col as isize + other.col) as usize
        } else {
            other.col as usize
        };
        let pos = (self.index as isize + other.pos) as usize;
        CodeLocation {
            line: line as u32,
            col: col as u32,
            index: pos as u32,
        }
    }
}

impl PartialOrd for CodeLocation {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}
