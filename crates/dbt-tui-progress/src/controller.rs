//! Progress bar controller for managing multiple concurrent progress indicators.
//!
//! This module provides `ProgressController`, a thread-safe manager for terminal
//! progress bars and spinners. It wraps indicatif's `MultiProgress` and provides
//! a clean API for creating, updating, and removing progress indicators.

use std::fmt;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::Condvar;
use std::sync::Mutex;
use std::time::Duration;

use indicatif::MultiProgress;
use scc::HashMap as SccHashMap;

use crate::bar::ContextualProgressBar;

/// A thread-safe controller for managing multiple progress bars and spinners.
///
/// The controller is generic over the ID type, allowing callers to use any
/// hashable type (enums, strings, etc.) to identify progress bars. This
/// decouples the identifier from the display text.
///
/// # Type Parameters
///
/// * `Id` - The type used to identify progress bars. Must be `Hash + Eq + Clone + Send + Sync`.
///
/// # Example
///
/// ```ignore
/// use dbt_tui_progress::ProgressController;
///
/// #[derive(Debug, Clone, Hash, Eq, PartialEq)]
/// enum MyProgressId {
///     Loading,
///     Processing,
/// }
///
/// let mut ctrl = ProgressController::<MyProgressId>::new();
/// ctrl.start_ticker();
/// ctrl.start_spinner(MyProgressId::Loading, "Loading...");
/// ctrl.remove_spinner(&MyProgressId::Loading);
/// ```
pub struct ProgressController<Id = String>
where
    Id: Hash + Eq + Clone + Send + Sync + 'static,
{
    /// Multiplexer for active (currently on-screen) progress bars and spinners.
    controller: MultiProgress,
    /// Map from ID to active spinner.
    spinners: Arc<SccHashMap<Id, ContextualProgressBar>>,
    /// Map from ID to active progress bar.
    bars: Arc<SccHashMap<Id, ContextualProgressBar>>,
    /// Shutdown signal for the ticker thread.
    shutdown: Arc<(Mutex<bool>, Condvar)>,
    /// Handle to the ticker thread.
    ticker: Option<std::thread::JoinHandle<()>>,
}

impl<Id> ProgressController<Id>
where
    Id: Hash + Eq + Clone + Send + Sync + 'static,
{
    /// Creates a new progress controller.
    ///
    /// The controller starts without a ticker thread. Call `start_ticker()` to
    /// enable animations.
    pub fn new() -> Self {
        ProgressController {
            controller: MultiProgress::new(),
            spinners: Arc::new(SccHashMap::new()),
            bars: Arc::new(SccHashMap::new()),
            shutdown: Arc::new((Mutex::new(false), Condvar::new())),
            ticker: None,
        }
    }

    /// Starts the background ticker thread for progress bar animations.
    ///
    /// The ticker thread periodically updates all active progress bars and
    /// spinners to animate them. If already started, this is a no-op.
    pub fn start_ticker(&mut self) {
        if self.ticker.is_some() {
            // Already started
            return;
        }

        let shutdown = Arc::clone(&self.shutdown);
        let spinners = Arc::clone(&self.spinners);
        let bars = Arc::clone(&self.bars);

        self.ticker = Some(std::thread::spawn(move || {
            loop {
                // Wait for the next tick
                let (lock, cvar) = &*shutdown;
                let Ok(lock) = lock.lock() else {
                    // Lock poisoned so we stop bothering
                    return;
                };

                let shut_down_flag = cvar.wait_timeout(lock, Duration::from_millis(80));
                if let Ok((flag, _)) = shut_down_flag
                    && *flag
                {
                    // Shutdown requested
                    break;
                }

                spinners.iter_sync(|_, spinner| {
                    spinner.tick();
                    true
                });
                bars.iter_sync(|_, bar| {
                    bar.tick();
                    true
                });
            }
        }));
    }

    /// Executes a closure while progress bars are suspended.
    ///
    /// This temporarily hides progress bars from the terminal, allowing clean
    /// output (e.g., log messages) without visual artifacts. Progress bars are
    /// restored after the closure completes.
    pub fn with_suspended<F, R>(&self, f: F) -> R
    where
        F: FnOnce() -> R,
    {
        self.controller.suspend(f)
    }

    // -------------------------------------------------------------------------
    // Spinner operations
    // -------------------------------------------------------------------------

    /// Starts a new spinner with the given ID and display prefix.
    ///
    /// If a spinner with this ID already exists, this is a no-op.
    pub fn start_spinner(&self, id: Id, prefix: impl Into<String>) {
        // Do nothing if already exists
        let _ = self.spinners.entry_sync(id).or_insert_with(|| {
            let spinner = ContextualProgressBar::new_spinner(prefix.into());
            spinner.progress_bars().for_each(|pb| {
                self.controller.add(pb.clone());
            });
            spinner
        });
    }

    /// Adds a context item (in-progress task) to a spinner.
    pub fn add_spinner_context(&self, id: &Id, item: &str) {
        let _ = self.spinners.read_sync(id, |_, spinner| {
            spinner.push(item);
        });
    }

    /// Finishes a context item on a spinner, optionally updating a counter.
    ///
    /// If `status` is provided (e.g., "succeeded", "failed"), the corresponding
    /// counter is incremented.
    pub fn finish_spinner_context(&self, id: &Id, item: &str, status: Option<&str>) {
        let _ = self.spinners.read_sync(id, |_, spinner| {
            spinner.delete(item);
            if let Some(status) = status {
                spinner.inc_counter(status, 1);
            }
        });
    }

    /// Marks a spinner context item idle without removing it.
    pub fn set_spinner_context_idle(&self, id: &Id, item: &str) {
        let _ = self.spinners.read_sync(id, |_, spinner| {
            spinner.set_idle(item);
        });
    }

    /// Marks a spinner context item active again.
    pub fn set_spinner_context_active(&self, id: &Id, item: &str) {
        let _ = self.spinners.read_sync(id, |_, spinner| {
            spinner.set_active(item);
        });
    }

    /// Removes a spinner by ID.
    pub fn remove_spinner(&self, id: &Id) {
        if let Some((_, spinner)) = self.spinners.remove_sync(id) {
            spinner.finish_and_clear();
            spinner.progress_bars().for_each(|pb| {
                self.controller.remove(pb);
            });
        }
    }

    // -------------------------------------------------------------------------
    // Bar operations
    // -------------------------------------------------------------------------

    /// Starts a new progress bar with counters and context items support.
    ///
    /// `total` is the total number of items to process, and `id` is a unique
    /// identifier for the bar, it is the caller's responsibility to ensure
    /// that the `id` is globally unique. `prefix` is the text that will be
    /// displayed before the bar.
    /// NOTE: if a bar with this ID already exists, this is a no-op.
    ///
    /// Bars can track in-progress tasks with context items, which can be added
    /// and finished as the task progresses. In-progress tasks will be rendered
    /// alongside the bar.
    pub fn start_bar(&self, id: Id, total: u64, prefix: impl Into<String>) {
        // Do nothing if already exists
        self.bars.entry_sync(id).or_insert_with(|| {
            let bar = ContextualProgressBar::new_bar(total, prefix.into());

            bar.progress_bars().for_each(|pb| {
                self.controller.add(pb.clone());
            });

            bar
        });
    }

    /// Starts a new plain progress bar (no context items support).
    ///
    /// `total` is the total number of items to process, and `id` is a unique
    /// identifier for the bar, it is the caller's responsibility to ensure
    /// that the `id` is globally unique. `prefix` is the text that will be
    /// displayed before the bar.
    /// NOTE: if a bar with this ID already exists, this is a no-op.
    ///
    /// Plain bars do not render in-progress tasks. If context items are added
    /// to a plain bar, they will be ignored.
    pub fn start_plain_bar(&self, id: Id, total: u64, prefix: impl Into<String>) {
        // Do nothing if already exists
        self.bars.entry_sync(id).or_insert_with(|| {
            let bar = ContextualProgressBar::new_plain_bar(total, prefix.into());

            bar.progress_bars().for_each(|pb| {
                self.controller.add(pb.clone());
            });

            bar
        });
    }

    /// Adds a context item (in-progress task) to a progress bar.
    pub fn add_bar_context(&self, id: &Id, item: &str) {
        let _ = self.bars.read_sync(id, |_, bar| {
            bar.push(item);
        });
    }

    /// Finishes a context item on a progress bar, auto-increments, and optionally updates a counter.
    ///
    /// If `status` is provided (e.g., "succeeded", "failed"), the corresponding
    /// counter is incremented. The bar position is always incremented by 1.
    pub fn finish_bar_context(&self, id: &Id, item: &str, status: Option<&str>) {
        self.bars.read_sync(id, |_, bar| {
            bar.delete(item);
            bar.inc(1);
            if let Some(status) = status {
                bar.inc_counter(status, 1);
            }
        });
    }

    /// Removes a context item from a progress bar without advancing it.
    ///
    /// For context items that are not units of the bar's total, e.g. a named stage of a
    /// phase shown alongside the items it is working through.
    pub fn remove_bar_context(&self, id: &Id, item: &str) {
        self.bars.read_sync(id, |_, bar| {
            bar.delete(item);
        });
    }

    /// Marks a progress bar context item idle without removing it.
    pub fn set_bar_context_idle(&self, id: &Id, item: &str) {
        self.bars.read_sync(id, |_, bar| {
            bar.set_idle(item);
        });
    }

    /// Marks a progress bar context item active again.
    pub fn set_bar_context_active(&self, id: &Id, item: &str) {
        self.bars.read_sync(id, |_, bar| {
            bar.set_active(item);
        });
    }

    /// Increments a progress bar by the specified amount.
    pub fn inc_bar(&self, id: &Id, inc: u64) {
        self.bars.read_sync(id, |_, bar| {
            bar.inc(inc);
        });
    }

    /// Updates a counter on a progress bar.
    pub fn update_counter(&self, id: &Id, counter_name: &str, step: i64) {
        self.bars.read_sync(id, |_, bar| {
            bar.inc_counter(counter_name, step);
        });
    }

    /// Removes a progress bar by ID.
    pub fn remove_bar(&self, id: &Id) {
        if let Some((_, bar)) = self.bars.remove_sync(id) {
            bar.finish_and_clear();
            bar.progress_bars().for_each(|pb| {
                self.controller.remove(pb);
            });
        }
    }
}

impl<Id> Default for ProgressController<Id>
where
    Id: Hash + Eq + Clone + Send + Sync + 'static,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Id> fmt::Debug for ProgressController<Id>
where
    Id: Hash + Eq + Clone + Send + Sync + 'static,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgressController").finish_non_exhaustive()
    }
}

impl<Id> Drop for ProgressController<Id>
where
    Id: Hash + Eq + Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        // Best-effort attempt to shut down cleanly. If anything goes wrong we
        // just give up quietly.
        let (lock, cvar) = &*self.shutdown;
        {
            let Ok(mut shutdown) = lock.lock() else {
                // Lock poisoned, so we can't proceed
                return;
            };

            *shutdown = true;
            cvar.notify_all();
            // Guard dropped here: the ticker thread's `wait_timeout` needs to
            // reacquire this same mutex to observe the flag and return, which
            // would otherwise deadlock against the `join()` below.
        }

        // Wait for the ticker thread to finish
        if let Some(ticker) = self.ticker.take() {
            let _ = ticker.join();
        }
    }
}
