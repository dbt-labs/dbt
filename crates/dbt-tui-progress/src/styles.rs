//! Progress bar styles and color constants for terminal rendering.
//!
//! This module provides pre-defined progress bar styles using indicatif,
//! and color constants for consistent terminal output styling.

use std::{sync::LazyLock, time::Duration};

use console::Style;
use indicatif::ProgressStyle;

/// Green color style for success indicators.
pub static GREEN: LazyLock<Style> = LazyLock::new(|| Style::new().green());

/// Red color style for error indicators.
pub static RED: LazyLock<Style> = LazyLock::new(|| Style::new().red());

/// Yellow color style for warning indicators.
pub static YELLOW: LazyLock<Style> = LazyLock::new(|| Style::new().yellow());

/// Dim color style for de-emphasized text.
pub static DIM: LazyLock<Style> = LazyLock::new(|| Style::new().dim());

/// Available progress bar style variants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressStyleType {
    /// A simple spinner with elapsed time, counters, and context items.
    Spinner,
    /// A wide progress bar with position/total and elapsed time.
    FancyWideBar,
    /// A thin progress bar with counters, displaying context on a separate line.
    FancyThinBarWithCounters,
}

impl ProgressStyleType {
    /// Returns the indicatif `ProgressStyle` for this style type.
    pub fn get_style(&self) -> ProgressStyle {
        match self {
            ProgressStyleType::Spinner => ProgressStyle::with_template(
                "{prefix:.cyan.bold} {spinner:.green.bold} [{elapsed}] {counters} {context}",
            )
            .expect("Progress style template is valid"),
            ProgressStyleType::FancyWideBar => ProgressStyle::default_bar()
                .template("{prefix:.cyan.bold} {spinner:.green} ▐{bar:20.bright_cyan/dim}▌ {pos}/{human_len} [{elapsed}]")
                .expect("Progress style template is valid")
                .progress_chars("█▉▊▋▌▍▎▏ "),
            ProgressStyleType::FancyThinBarWithCounters => ProgressStyle::default_bar()
                .template("{prefix:.cyan.bold} [{bar:20.cyan}] {pos}/{len} {counters}")
                .expect("Progress style template is valid")
                .progress_chars("━━╾─ "),
        }
    }

    /// Returns the style for the context line (used with `FancyThinBarWithCounters`).
    pub fn get_context_line_style(&self) -> ProgressStyle {
        ProgressStyle::with_template("   {context}").expect("Progress style template is valid")
    }

    /// Returns whether this style type needs a separate context line.
    pub fn needs_context_line(&self) -> bool {
        matches!(self, ProgressStyleType::FancyThinBarWithCounters)
    }
}

/// Formats a duration in a short human-readable format.
pub(crate) fn format_duration_short(duration: Duration) -> String {
    let duration = duration.as_secs_f64();
    if duration > 60.0 {
        format!("{}m {:.0}s", duration as u32 / 60, duration % 60.0)
    } else {
        format!("{duration:.1}s")
    }
}
