use std::fmt;

pub use crate::pretty_string::{BLUE, CYAN, DIM, GREEN, MAGENTA, PLAIN, RED, WHITE, YELLOW};
use console::Style;
use dbt_tracing::SeverityNumber;

pub fn severity_to_color_style(severity_number: SeverityNumber) -> &'static Style {
    match severity_number {
        SeverityNumber::Error => &RED,
        SeverityNumber::Warn => &YELLOW,
        SeverityNumber::Unspecified
        | SeverityNumber::Trace
        | SeverityNumber::Debug
        | SeverityNumber::Info => &PLAIN,
    }
}

/// Displays `value`, styled with `style` when `colorize` is set.
#[derive(Clone, Copy)]
pub struct MaybeColored<'a, D> {
    pub style: &'a Style,
    pub value: D,
    pub colorize: bool,
}

impl<'a, D: fmt::Display> MaybeColored<'a, D> {
    pub fn new(style: &'a Style, value: D, colorize: bool) -> Self {
        Self {
            style,
            value,
            colorize,
        }
    }
}

impl<D: fmt::Display> fmt::Display for MaybeColored<'_, D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.colorize {
            write!(f, "{}", self.style.apply_to(&self.value))
        } else {
            write!(f, "{}", self.value)
        }
    }
}
