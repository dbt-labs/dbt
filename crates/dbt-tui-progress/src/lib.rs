//! Terminal progress bar controller for TUI layers.
//!
//! This crate provides a thread-safe progress bar controller that wraps indicatif
//! for managing multiple concurrent progress bars and spinners. It is designed
//! to be used by terminal UI layers that need to display progress indicators
//! alongside other output.
//!
//! # Features
//!
//! - Generic ID type for progress bar identification (decoupled from display text)
//! - Thread-safe operations using `scc::HashMap`
//! - Background ticker thread for smooth animations
//! - Progress bar suspension for clean interleaved output
//! - Contextual progress bars with in-progress task tracking
//! - Counter support (succeeded, failed, skipped, etc.)
//!
//! # Example
//!
//! ```ignore
//! use dbt_tui_progress::ProgressController;
//!
//! #[derive(Debug, Clone, Hash, Eq, PartialEq)]
//! enum TaskId {
//!     Build,
//!     Test,
//! }
//!
//! let mut ctrl = ProgressController::<TaskId>::new();
//! ctrl.start_ticker();
//!
//! // Start a progress bar
//! ctrl.start_bar(TaskId::Build, 10, "Building");
//!
//! // Add context items as tasks start
//! ctrl.add_bar_context(&TaskId::Build, "model_a");
//!
//! // Finish context items as tasks complete
//! ctrl.finish_bar_context(&TaskId::Build, "model_a", Some("succeeded"));
//!
//! // Suspend for clean log output
//! ctrl.with_suspended(|| {
//!     println!("Log message without progress bar artifacts");
//! });
//!
//! // Clean up
//! ctrl.remove_bar(&TaskId::Build);
//! ```

#![allow(clippy::let_and_return)]

use std::time::Duration;

mod bar;
mod controller;
pub mod styles;

pub use controller::ProgressController;
pub use styles::ProgressStyleType;

/// Duration threshold after which context items are highlighted as slow (5 minutes).
pub const SLOW_CONTEXT_THRESHOLD: Duration = Duration::from_secs(300);

/// Duration threshold for borderline slow items (1 minute).
pub const BORDERLINE_CONTEXT_THRESHOLD: Duration = Duration::from_secs(60);
