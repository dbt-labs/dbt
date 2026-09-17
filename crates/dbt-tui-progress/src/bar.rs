//! Internal progress bar types for managing contextual progress indicators.
//!
//! This module contains the internal `ContextualProgressBar` struct that wraps
//! indicatif's `ProgressBar` with additional context tracking and counter support.

use std::fmt;
use std::fmt::Display;
use std::sync::Arc;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use counter::Counter;
use indicatif::ProgressBar;
use indicatif::ProgressState;
use unicode_segmentation::UnicodeSegmentation as _;

use crate::BORDERLINE_CONTEXT_THRESHOLD;
use crate::SLOW_CONTEXT_THRESHOLD;
use crate::styles::DIM;
use crate::styles::GREEN;
use crate::styles::ProgressStyleType;
use crate::styles::RED;
use crate::styles::YELLOW;
use crate::styles::format_duration_short;

const IDLE_CONTEXT_DEBOUNCE: Duration = Duration::from_millis(250);

/// A context item representing an in-progress task with timing information.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct ContextItem {
    name: String,
    start_time: Instant,
    idle_start: Option<Instant>,
    idle_duration: Duration,
}

impl ContextItem {
    pub fn new(name: String) -> Self {
        Self {
            name,
            start_time: Instant::now(),
            idle_start: None,
            idle_duration: Duration::ZERO,
        }
    }

    fn set_idle(&mut self) {
        if self.idle_start.is_none() {
            self.idle_start = Some(Instant::now());
        }
    }

    fn set_active(&mut self) {
        if let Some(idle_start) = self.idle_start.take() {
            self.idle_duration = self.idle_duration.saturating_add(idle_start.elapsed());
        }
    }

    fn active_elapsed(&self) -> Duration {
        let current_idle = self
            .idle_start
            .map(|idle_start| idle_start.elapsed())
            .unwrap_or_default();
        self.start_time
            .elapsed()
            .saturating_sub(self.idle_duration.saturating_add(current_idle))
    }

    fn is_idle(&self) -> bool {
        self.idle_start
            .is_some_and(|idle_start| idle_start.elapsed() >= IDLE_CONTEXT_DEBOUNCE)
    }

    fn display_text(&self) -> String {
        if let Some(idle_start) = self.idle_start {
            let idle_elapsed = idle_start.elapsed();
            if idle_elapsed >= IDLE_CONTEXT_DEBOUNCE {
                return format!(
                    "{} [idle {}]",
                    self.name,
                    format_duration_short(idle_elapsed)
                );
            }
        }

        format!(
            "{} [{}]",
            self.name,
            format_duration_short(self.active_elapsed())
        )
    }

    fn display_width(&self) -> usize {
        self.display_text().graphemes(true).count()
    }

    fn format_with_limit(&self, max_len: usize) -> String {
        if max_len == 0 {
            return String::new();
        }

        let text = self.display_text();
        let graphemes = text.graphemes(true).collect::<Vec<&str>>();
        let text = if graphemes.len() <= max_len {
            text
        } else if max_len <= 3 {
            ".".repeat(max_len)
        } else {
            graphemes
                .into_iter()
                .take(max_len.saturating_sub(3))
                .chain(std::iter::once("..."))
                .collect::<String>()
        };

        if self.is_idle() {
            return DIM.apply_to(text).to_string();
        }

        let elapsed = self.active_elapsed();
        let color = console::Style::new();
        let color = if elapsed > SLOW_CONTEXT_THRESHOLD {
            color.red().bold()
        } else if elapsed > BORDERLINE_CONTEXT_THRESHOLD {
            color.yellow().bold()
        } else {
            color
        };
        color.apply_to(text).to_string()
    }
}

impl Display for ContextItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.format_with_limit(usize::MAX).as_str())
    }
}

impl AsRef<str> for ContextItem {
    fn as_ref(&self) -> &str {
        self.name.as_str()
    }
}

/// A progress bar that can display contextual information on in-progress tasks.
#[derive(Clone)]
pub(crate) struct ContextualProgressBar {
    main_bar: ProgressBar,
    context_bar: Option<ProgressBar>,
    counters: Arc<RwLock<Counter<String>>>,
    items: Arc<RwLock<Vec<ContextItem>>>,
}

impl ContextualProgressBar {
    /// Creates a new plain progress bar (no context items support).
    pub fn new_plain_bar(total: u64, prefix: String) -> Self {
        let progress = ProgressBar::hidden().with_prefix(prefix);
        progress.set_style(ProgressStyleType::FancyWideBar.get_style());
        progress.set_length(total);

        Self {
            main_bar: progress,
            context_bar: None,
            counters: Arc::new(RwLock::new(Counter::new())),
            items: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Creates a new progress bar with context items and counters support.
    pub fn new_bar(total: u64, prefix: String) -> Self {
        let progress = ProgressBar::hidden().with_prefix(prefix);
        progress.set_length(total);

        Self::init(progress, ProgressStyleType::FancyThinBarWithCounters)
    }

    /// Creates a new spinner with context items and counters support.
    pub fn new_spinner(prefix: String) -> Self {
        let progress = ProgressBar::hidden().with_prefix(prefix);

        Self::init(progress, ProgressStyleType::Spinner)
    }

    fn init(main_bar: ProgressBar, style_type: ProgressStyleType) -> Self {
        let new_self = Self {
            main_bar,
            items: Arc::new(RwLock::new(Vec::new())),
            counters: Arc::new(RwLock::new(Counter::with_capacity(5))),
            context_bar: if style_type.needs_context_line() {
                Some(ProgressBar::hidden())
            } else {
                None
            },
        };

        let term_width = console::Term::stdout().size().1;
        let self_clone = new_self.clone();
        let style = style_type.get_style();
        let style = if style_type.needs_context_line() {
            style
        } else {
            style.with_key(
                "context",
                move |_state: &'_ ProgressState, writer: &'_ mut dyn fmt::Write| {
                    self_clone.format_context_msg(writer, term_width.saturating_sub(6) as usize)
                },
            )
        };
        let self_clone = new_self.clone();
        let style = style.with_key(
            "counters",
            move |_state: &'_ ProgressState, writer: &'_ mut dyn fmt::Write| {
                self_clone.format_counters(writer)
            },
        );

        new_self.main_bar.set_style(style);

        let self_clone = new_self.clone();
        if let Some(bar) = new_self.context_bar.as_ref() {
            bar.set_style(style_type.get_context_line_style().with_key(
                "context",
                move |_state: &'_ ProgressState, writer: &'_ mut dyn fmt::Write| {
                    self_clone.format_context_msg(writer, term_width.saturating_sub(6) as usize)
                },
            ));
        }

        new_self
    }

    fn format_counters(&self, writer: &'_ mut dyn fmt::Write) {
        let Ok(counters) = self.counters.read() else {
            let _ = writer.write_str("<N/A>");
            return;
        };

        let mut formatted_parts = Vec::new();

        // Process known statuses in preferred order
        if let Some(&count) = counters.get("succeeded") {
            let part = GREEN.apply_to(format!("{count} succeeded")).to_string();
            formatted_parts.push(part);
        }

        if let Some(&count) = counters.get("failed") {
            let part = RED.apply_to(format!("{count} failed")).to_string();
            formatted_parts.push(part);
        }

        if let Some(&count) = counters.get("skipped") {
            let part = YELLOW.apply_to(format!("{count} skipped")).to_string();
            formatted_parts.push(part);
        }

        // Add any other counters that weren't in our known list
        let known_statuses = ["succeeded", "failed", "skipped"];
        for (status, count) in counters.iter() {
            if !known_statuses.contains(&status.as_str()) {
                formatted_parts.push(format!("{count} {status}"));
            }
        }

        // Add in-progress and idle context counts last
        if let Ok(items) = self.items.read()
            && !items.is_empty()
        {
            let idle_count = items.iter().filter(|item| item.is_idle()).count();
            let active_count = items.len().saturating_sub(idle_count);

            if active_count > 0 {
                let part = DIM
                    .apply_to(format!("{active_count} in-progress"))
                    .to_string();
                formatted_parts.push(part);
            }

            if idle_count > 0 {
                let part = DIM.apply_to(format!("{idle_count} idle")).to_string();
                formatted_parts.push(part);
            }
        }

        // Join all parts with " | "
        if !formatted_parts.is_empty() {
            let _ = writer.write_str(&formatted_parts.join(" | "));
        }
    }

    fn format_context_msg(&self, writer: &'_ mut dyn fmt::Write, max_len: usize) {
        match self.items.read() {
            Ok(items) => {
                let mut remaining = max_len;
                let mut formatted_parts = Vec::new();

                for item in items.iter() {
                    let separator_len = if formatted_parts.is_empty() { 0 } else { 2 };
                    if remaining <= separator_len {
                        break;
                    }

                    let item_limit = remaining - separator_len;
                    let item_width = item.display_width();
                    formatted_parts.push(item.format_with_limit(item_limit));
                    remaining =
                        remaining.saturating_sub(separator_len + item_width.min(item_limit));

                    if item_width > item_limit {
                        break;
                    }
                }

                let _ = writer.write_str(formatted_parts.join(", ").as_str());
            }
            Err(_) => {
                let _ = writer.write_str("<N/A>");
            }
        }
    }
}

impl ContextualProgressBar {
    /// Returns an iterator over the progress bars (main bar and optional context bar).
    pub fn progress_bars(&self) -> impl Iterator<Item = &ProgressBar> {
        std::iter::once(&self.main_bar).chain(self.context_bar.iter())
    }

    /// Increments the progress bar by the specified number of steps.
    ///
    /// Clamps at `length`, so a caller whose upfront total was an estimate (e.g. parse,
    /// where one source file can expand into several parsed assets) parks the bar at
    /// 100% rather than overshooting or moving the denominator mid-run.
    pub fn inc(&self, inc: u64) {
        let length = self.main_bar.length().unwrap_or(0);
        let position = self.main_bar.position();
        self.main_bar.inc(inc.min(length.saturating_sub(position)));
    }

    /// Increments the counter for the given item by the specified step.
    pub fn inc_counter(&self, item: &str, step: i64) {
        let _ = self.counters.write().map(|mut counters| {
            if let Some(count) = counters.get_mut(item) {
                let new_count = (*count as i64 + step).max(0) as usize;
                *count = new_count;
            } else {
                counters.insert(item.to_string(), step.max(0) as usize);
            }
        });
    }

    /// Finishes the progress bar and clears its context items.
    pub fn finish_and_clear(&self) {
        self.main_bar.finish_and_clear();
        if let Some(bar) = &self.context_bar {
            bar.finish_and_clear();
        }
    }

    /// Ticks the progress bar to update animations.
    pub fn tick(&self) {
        self.main_bar.tick();
        if let Some(bar) = &self.context_bar {
            bar.tick();
        }
    }

    /// Pushes a new context item (in-progress task) to the bar.
    pub fn push(&self, item: &str) {
        if let Ok(mut slots) = self.items.write() {
            slots.push(ContextItem::new(item.to_string()));
        }
    }

    /// Removes a context item from the bar.
    pub fn delete(&self, item: &str) {
        if let Ok(mut slots) = self.items.write() {
            if let Some(pos) = slots.iter().position(|x| x.as_ref() == item) {
                slots.remove(pos);
            }
        }
    }

    /// Marks a context item as idle without removing it from the bar.
    pub fn set_idle(&self, item: &str) {
        if let Ok(mut slots) = self.items.write()
            && let Some(slot) = slots.iter_mut().find(|x| x.as_ref() == item)
        {
            slot.set_idle();
        }
    }

    /// Marks a context item active again and accumulates its idle time.
    pub fn set_active(&self, item: &str) {
        if let Ok(mut slots) = self.items.write()
            && let Some(slot) = slots.iter_mut().find(|x| x.as_ref() == item)
        {
            slot.set_active();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A caller's upfront total can be an estimate (e.g. parse, where one source
    /// file can expand into several parsed assets), so `inc` must never leave the
    /// bar showing a position past its length, nor move the length to accommodate one.
    #[test]
    fn inc_past_total_clamps_at_length() {
        let bar = ContextualProgressBar::new_bar(3, "Parsing".to_string());

        for _ in 0..5 {
            bar.inc(1);
        }

        assert_eq!(bar.main_bar.position(), 3);
        assert_eq!(bar.main_bar.length(), Some(3));
    }

    /// A single oversized increment must clamp the same way a run of small ones does.
    #[test]
    fn oversized_single_inc_clamps_at_length() {
        let bar = ContextualProgressBar::new_bar(3, "Parsing".to_string());

        bar.inc(10);

        assert_eq!(bar.main_bar.position(), 3);
        assert_eq!(bar.main_bar.length(), Some(3));
    }
}
