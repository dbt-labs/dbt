# dbt-tui-progress

Terminal progress bar controller for TUI layers, wrapping indicatif with a clean, type-safe API.

## Features

- **Generic ID type**: Progress bars are identified by any hashable type, decoupling identity from display text
- **Thread-safe**: Uses `scc::HashMap` for concurrent access from multiple threads
- **Background animations**: Ticker thread keeps progress bars animated
- **Suspension support**: Cleanly interleave log output with progress bar display
- **Contextual progress**: Track in-progress tasks with timing and counters

## Usage

```rust
use dbt_tui_progress::ProgressController;

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
enum Phase {
    Render,
    Run,
}

let mut ctrl = ProgressController::<Phase>::new();
ctrl.start_ticker();

// Start a progress bar
ctrl.start_bar(Phase::Render, 100, "Rendering");

// Track in-progress items
ctrl.add_bar_context(&Phase::Render, "model_a");
ctrl.finish_bar_context(&Phase::Render, "model_a", Some("succeeded"));

// Suspend for log output
ctrl.with_suspended(|| {
    println!("Log message");
});

// Clean up
ctrl.remove_bar(&Phase::Render);
```

## API Overview

### `ProgressController<Id>`

Main controller managing multiple progress bars/spinners.

**Lifecycle:**
- `new()` - Create controller
- `start_ticker()` - Start animation thread
- `with_suspended(f)` - Execute closure with progress bars hidden. This is the only way to properly output to stderr/stdout while progress bars are active.

**Spinners:**
- `start_spinner(id, prefix)` - Create spinner
- `add_spinner_context(id, item)` - Add in-progress task
- `finish_spinner_context(id, item, status)` - Complete task with optional status
- `remove_spinner(id)` - Remove spinner

**Progress Bars:**
- `start_bar(id, total, prefix)` - Create bar with counters
- `start_plain_bar(id, total, prefix)` - Create simple bar
- `add_bar_context(id, item)` - Add in-progress task
- `finish_bar_context(id, item, status)` - Complete task (auto-increments)
- `inc_bar(id, inc)` - Manually increment
- `update_counter(id, name, step)` - Update named counter
- `remove_bar(id)` - Remove bar
