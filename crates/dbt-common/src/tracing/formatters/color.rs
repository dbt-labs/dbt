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

pub fn maybe_apply_color(style: &Style, value: &str, colorize: bool) -> String {
    if colorize {
        style.apply_to(value).to_string()
    } else {
        value.to_string()
    }
}
