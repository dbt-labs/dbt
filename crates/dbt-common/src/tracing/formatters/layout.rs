use std::fmt;

use super::{
    color::{DIM, MaybeColored},
    constants::{ACTION_WIDTH, DEFAULT_TERMINAL_WIDTH},
};

/// Displays a centered delimiter line with '=' padding characters and optional DIM color.
///
/// This is used to create section headers in terminal output, such as
/// "Execution Summary", "Test Failures", and "Errors and Warnings".
/// `width` falls back to DEFAULT_TERMINAL_WIDTH.
///
/// # Example
/// ```ignore
/// let header = Delimiter::new(" Execution Summary ", Some(80), true);
/// // Displays: DIM-colored "========================= Execution Summary ========================="
/// ```
#[derive(Clone, Copy)]
pub struct Delimiter<'a> {
    pub text: &'a str,
    pub width: Option<usize>,
    pub colorize: bool,
}

impl<'a> Delimiter<'a> {
    pub fn new(text: &'a str, width: Option<usize>, colorize: bool) -> Self {
        Self {
            text,
            width,
            colorize,
        }
    }
}

impl fmt::Display for Delimiter<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            MaybeColored::new(
                &DIM,
                Centered {
                    text: self.text,
                    width: self.width.unwrap_or(DEFAULT_TERMINAL_WIDTH),
                },
                self.colorize,
            )
        )
    }
}

/// `text` centered between '=' padding to `width` characters.
struct Centered<'a> {
    text: &'a str,
    width: usize,
}

impl fmt::Display for Centered<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:=^width$}", self.text, width = self.width)
    }
}

/// Text that knows its own length, so it can be aligned without being rendered first.
pub trait MeasuredText: fmt::Display {
    /// Length of the rendered text in bytes
    fn byte_len(&self) -> usize;
    /// Length of the rendered text in characters
    fn char_count(&self) -> usize;
}

impl MeasuredText for &str {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn char_count(&self) -> usize {
        self.chars().count()
    }
}

/// Displays `text` with its first character uppercased.
#[derive(Clone, Copy)]
pub struct CapitalizedFirst<'a>(pub &'a str);

impl CapitalizedFirst<'_> {
    fn split(&self) -> (Option<char>, &str) {
        let mut chars = self.0.chars();
        (chars.next(), chars.as_str())
    }
}

impl fmt::Display for CapitalizedFirst<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, rest) = self.split();
        for upper in first.into_iter().flat_map(char::to_uppercase) {
            fmt::Write::write_char(f, upper)?;
        }
        f.write_str(rest)
    }
}

impl MeasuredText for CapitalizedFirst<'_> {
    fn byte_len(&self) -> usize {
        let (first, rest) = self.split();
        let first: usize = first
            .into_iter()
            .flat_map(char::to_uppercase)
            .map(char::len_utf8)
            .sum();
        first + rest.len()
    }

    fn char_count(&self) -> usize {
        let (first, rest) = self.split();
        first.into_iter().flat_map(char::to_uppercase).count() + rest.chars().count()
    }
}

/// Displays action text right aligned to a fixed width defined by ACTION_WIDTH.
///
/// Use it for the first column of progress messages. Text of ACTION_WIDTH bytes or more
/// is displayed as is.
#[derive(Clone, Copy)]
pub struct RightAligned<T>(pub T);

impl<T: MeasuredText> fmt::Display for RightAligned<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.byte_len() < ACTION_WIDTH {
            for _ in self.0.char_count()..ACTION_WIDTH {
                fmt::Write::write_char(f, ' ')?;
            }
        }
        write!(f, "{}", self.0)
    }
}
