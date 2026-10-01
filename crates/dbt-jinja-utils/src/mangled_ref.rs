//! Rendering-based checker for mangled ref/source calls.
//!
//! A mangled ref/source is one where the `{{ ref() }}` or `{{ source() }}` call
//! is directly adjacent to other content without whitespace separation.
//!
//! Examples of mangled refs:
//! - `foo{{ ref('a') }}` - identifier characters before ref
//! - `{{ ref('a') }}bar` - identifier characters after ref

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use dbt_common::{ErrorCode, tracing::dbt_emit::emit_warn_log_message};
use minijinja::{CodeLocation, OutputTrackerLocation, listener::RenderingEventListener};

use minijinja::machinery::Span;

/// A recorded ref/source call with its position in the source code.
#[derive(Debug, Clone)]
struct RefSourceInfo {
    ref_type: String,
    /// Location in source for error reporting
    report_line: u32,
    report_col: u32,
    /// Position in source code (byte offset)
    source_start: u32,
    source_end: u32,
    /// Position in rendered output at the moment `on_ref_or_source` fires.
    /// For a direct `{{ ref('x') }}` expression this equals the macro span's
    /// `expanded_span.start_offset`. For a ref inside a control-flow block
    /// (e.g. `{%- if ... %} ... {{ ref('x') }} ...`) the output cursor has
    /// already advanced past the preceding block text, so this correctly points
    /// to where the ref value will be emitted rather than the block boundary.
    render_start_offset: u32,
    /// True when this ref was evaluated as an argument to another function
    /// (e.g. `{{ macro_call(ref('x'), ...) }}`).  In that case the ref's
    /// string value goes through the outer function's template and is never
    /// placed directly into the SQL stream at the current output position, so
    /// the adjacency check must be skipped to avoid false positives.
    is_argument: bool,
}

/// A printer that collects ref/source positions during rendering and checks for mangled refs.
///
/// This works by:
/// 1. Recording ref/source calls with their source positions via `on_ref_or_source`
/// 2. After rendering completes, using macro_spans to map source positions to rendered positions
/// 3. Checking the rendered output around each mapped position
///
/// Implements `RenderingEventListener` to integrate with the rendering system.
#[derive(Debug)]
pub struct MangledRefWarningPrinter {
    /// File path for error reporting
    path: PathBuf,
    /// Collected ref/source info during rendering
    ref_source_info: Arc<Mutex<Vec<RefSourceInfo>>>,
    /// Function call depth - only record refs at depth 0 (top level).
    /// This also covers re-rendered template strings from var() since we
    /// increment function_depth around render_str calls.
    function_depth: Cell<u32>,
    /// Shared output tracker so we can read the current render offset when
    /// `on_ref_or_source` fires, giving us the true "before" position even
    /// for refs inside whitespace-stripped control-flow blocks.
    output_tracker_location: Rc<OutputTrackerLocation>,
    /// Set to `true` after a function at depth 0 finishes.  If the *next*
    /// depth-0 event is `on_function_start` (rather than another
    /// `on_ref_or_source`) it means the previously completed refs were
    /// arguments to that outer function and should be flagged as such.
    awaiting_outer_fn_start: Cell<bool>,
    /// Index into `ref_source_info` that marks the beginning of the current
    /// "run" of depth-0 refs.  All refs from this index onward belong to the
    /// same potential argument list and will be marked `is_argument` together
    /// if an outer function call is detected.
    ref_run_start: Cell<usize>,
}

impl MangledRefWarningPrinter {
    /// Create a new warning printer.
    pub fn new(path: PathBuf, output_tracker_location: Rc<OutputTrackerLocation>) -> Self {
        Self {
            path,
            ref_source_info: Arc::new(Mutex::new(Vec::new())),
            function_depth: Cell::new(0),
            output_tracker_location,
            awaiting_outer_fn_start: Cell::new(false),
            ref_run_start: Cell::new(0),
        }
    }

    fn emit_warning(&self, info: &RefSourceInfo, message: &str) {
        let location = CodeLocation {
            line: info.report_line,
            col: info.report_col,
            file: self.path.clone(),
        };
        emit_warn_log_message(
            ErrorCode::MangledRef,
            format!(
                "Mangled {}() {}. \
                This may cause unexpected behavior. Add whitespace or use a separator.\n  --> {}\n\
                To suppress this warning, add to dbt_project.yml:\n  \
                custom_checks:\n    analysis.mangled_ref: off",
                info.ref_type, message, location
            ),
        );
    }
}

impl RenderingEventListener for MangledRefWarningPrinter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "MangledRefWarningPrinter"
    }

    fn on_macro_start(
        &self,
        _file_path: Option<&std::path::Path>,
        _line: &u32,
        _col: &u32,
        _offset: &u32,
    ) {
        // Not used - we use macro_spans instead
    }

    fn on_macro_stop(
        &self,
        _file_path: Option<&std::path::Path>,
        _line: &u32,
        _col: &u32,
        _offset: &u32,
    ) {
        // Not used - we use macro_spans instead
    }

    fn on_malicious_return(&self, _location: &CodeLocation) {
        self.function_depth
            .set(self.function_depth.get().saturating_sub(1));
    }

    fn on_function_start(&self) {
        let depth = self.function_depth.get();

        // If we were waiting to see whether the previously completed refs are
        // arguments and a new outer function is starting, mark them all as
        // arguments so the adjacency check is suppressed for them.
        if depth == 0 && self.awaiting_outer_fn_start.get() {
            let run_start = self.ref_run_start.get();
            if let Ok(mut refs) = self.ref_source_info.lock() {
                for ref_info in refs[run_start..].iter_mut() {
                    ref_info.is_argument = true;
                }
            }
            self.awaiting_outer_fn_start.set(false);
        }

        self.function_depth.set(depth + 1);
    }

    fn on_function_end(&self) {
        let new_depth = self.function_depth.get().saturating_sub(1);
        self.function_depth.set(new_depth);

        // After any function completes at depth 0, watch for an outer function
        // start that would reveal the just-completed refs were arguments.
        if new_depth == 0 {
            self.awaiting_outer_fn_start.set(true);
        }
    }

    fn on_ref_or_source(
        &self,
        name: &str,
        start_line: u32,
        start_col: u32,
        start_offset: u32,
        _end_line: u32,
        _end_col: u32,
        end_offset: u32,
    ) {
        // Only record refs at top level (function_depth == 0)
        // This filters out refs inside macros and re-rendered template strings from var()
        if self.function_depth.get() > 0 {
            return;
        }

        // If we see another ref while awaiting an outer function start, it means
        // the previous refs were NOT arguments (they were peers in a concatenation
        // or similar).  Keep the same run_start so they stay grouped, but reset
        // the awaiting flag — refs like `ref('x') ~ ref('y')` should all share the
        // same run and none should be marked as arguments unless an outer function
        // start follows.
        if !self.awaiting_outer_fn_start.get() {
            // Starting a new run of depth-0 refs.
            let current_len = self.ref_source_info.lock().map(|r| r.len()).unwrap_or(0);
            self.ref_run_start.set(current_len);
        }
        self.awaiting_outer_fn_start.set(false);

        // Capture where the ref's value will be emitted in the rendered output.
        // Using the shared output tracker gives the correct position even when the
        // ref is inside a whitespace-stripped control-flow block, where the
        // containing macro_span's start would point to a different location.
        let render_start_offset = self.output_tracker_location.index();

        self.ref_source_info.lock().unwrap().push(RefSourceInfo {
            ref_type: name.to_string(),
            report_line: start_line,
            report_col: start_col,
            source_start: start_offset,
            source_end: end_offset,
            render_start_offset,
            is_argument: false,
        });
    }

    fn check_and_emit_mangled_ref_warnings(
        &self,
        rendered_sql: &str,
        macro_spans: &[(Span, Span)],
    ) {
        let infos = self.ref_source_info.lock().unwrap();
        let rendered_bytes = rendered_sql.as_bytes();

        for info in infos.iter() {
            // Skip refs that were passed as arguments to another function.
            // Their string value is embedded inside the outer function's rendered
            // output (not at the top-level SQL stream position recorded below),
            // so any adjacency check here would be against the wrong character.
            if info.is_argument {
                continue;
            }

            // Find the macro_span that contains this ref/source call.
            // The ref/source call is at [info.source_start, info.source_end) in the source.
            // Each tuple is (source_span, expanded_span).
            let containing_span = macro_spans.iter().find(|(source_span, _)| {
                info.source_start >= source_span.start_offset
                    && info.source_end <= source_span.end_offset
            });

            let Some((_, expanded_span)) = containing_span else {
                // No containing span found - ref might be in a {% %} block that doesn't produce output
                continue;
            };

            // Check if the span actually produced output
            if expanded_span.start_offset == expanded_span.end_offset {
                // This span didn't produce any output (e.g., used as function argument)
                continue;
            }

            // Use the render position captured at on_ref_or_source time for the
            // "before" check.  This is accurate even when the ref lives inside a
            // whitespace-stripped control-flow block (`{%- if ... %}`), where the
            // containing macro_span's start would point to the block boundary
            // (right after the stripped whitespace) rather than to the ref itself.
            let render_before_pos = info.render_start_offset as usize;

            // Check character before the ref output in rendered result
            if render_before_pos > 0 {
                if let Some(&c) = rendered_bytes.get(render_before_pos - 1) {
                    if is_identifier_char(c as char) {
                        self.emit_warning(info, "has non-whitespace content directly before it");
                    }
                }
            }

            // For the "after" check use the containing macro_span's end, which is
            // correct for direct `{{ ref('x') }}` expressions.
            let rendered_end = expanded_span.end_offset as usize;
            if let Some(&c) = rendered_bytes.get(rendered_end) {
                if is_identifier_char(c as char) {
                    self.emit_warning(info, "has non-whitespace content directly after it");
                }
            }
        }
    }
}

/// Check if a character could be part of a table/identifier name.
/// These are the characters that would cause "mangling" if adjacent to a ref/source output.
fn is_identifier_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.'
}
