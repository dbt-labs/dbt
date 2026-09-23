//! This module contains the listener trait and its implementations.
//!  

use std::fmt::Write;
use std::path::Path;

use crate::compiler::tokens::Token;
use crate::layout::JinjaLayoutEventKind;
use crate::output_tracker::OutputTracker;
use crate::value::Value;
use crate::{machinery::Span, CodeLocation};

/// A listener for rendering events. This is used for LSP
pub trait RenderingEventListener: std::fmt::Debug {
    /// Returns the listener as an `Any` trait object.
    fn as_any(&self) -> &dyn std::any::Any;

    /// Returns the name of the listener.
    fn name(&self) -> &str;

    /// Creates an OutputTracker for the given writer.
    /// If this listener tracks macro spans, it will use its internal location tracker.
    /// Otherwise, a plain OutputTracker is created.
    fn create_output_tracker<'a>(&self, _w: &'a mut (dyn Write + 'a)) -> Option<OutputTracker<'a>> {
        None
    }

    /// Called when a macro start is encountered.
    /// The expanded location can be obtained from the output_tracker_location if needed.
    fn on_macro_start(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32);

    /// Called when a macro stop is encountered.
    /// The expanded location can be obtained from the output_tracker_location if needed.
    fn on_macro_stop(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32);

    /// Called when a Jinja layout boundary is encountered during rendering.
    fn on_jinja_layout_event(&self, _kind: JinjaLayoutEventKind, _source_span: &Span) {}

    /// Called when a Jinja loop enters a repeated iteration.
    fn on_jinja_loop_iteration_start(&self, _source_span: &Span) {}

    /// Called when a Jinja loop ended without iterating.
    fn on_jinja_loop_skipped_end(&self, _source_span: &Span) {}

    /// Called when raw template text is emitted into rendered output.
    fn on_raw_emit(&self, _raw: &str, _source_span: &Span) {}

    /// Whether this listener wants introspective-stub values (see
    /// `Value::is_introspective_stub`) to be substituted with a hole marker
    /// instead of rendered as-is. Checking taint on every emitted value has a
    /// small cost, so the vm only does so when at least one listener opts in.
    /// Defaults to `false`.
    fn wants_introspective_holes(&self) -> bool {
        false
    }

    /// Generic value-substitution hook: the VM calls this at every point
    /// where it's about to use a computed value (binary op operands, filter
    /// and test arguments, an emitted expression, a call's result/arguments,
    /// ...) and, if it returns `Some(v)`, uses `v` in place of the original
    /// value instead of proceeding normally. Returning `None` means "nothing
    /// to override here", so the VM proceeds exactly as if no listener had
    /// been consulted at all.
    ///
    /// This is the *only* introspective-taint-shaped hook the VM itself
    /// (`vm::eval_impl`) knows about -- it has no concept of "introspective
    /// stubs" or "holes" itself, only "a listener may want to override a
    /// value". The default implementation supplies the introspective-taint
    /// behavior this crate ships with (see
    /// [`wants_introspective_holes`](Self::wants_introspective_holes) and
    /// `Value::is_introspective_stub`); a listener that doesn't want that
    /// behavior at all just leaves this at its default (always `None`).
    fn override_value(&self, value: &Value) -> Option<Value> {
        if self.wants_introspective_holes() && value.is_introspective_stub() {
            Some(value.clone())
        } else {
            None
        }
    }

    /// Generic branch-override hook, the `{% if %}`-condition counterpart to
    /// [`override_value`](Self::override_value): the VM calls this with a
    /// `{% if %}` condition before coercing it to a boolean itself. Returning
    /// `Some(bool)` tells the VM to take that branch instead of evaluating
    /// the condition normally; `None` means "nothing to override here".
    ///
    /// The default implementation supplies this crate's introspective-taint
    /// behavior: when the condition is an introspective-stub value (see
    /// `Value::is_introspective_stub`) and this listener wants introspective
    /// holes, delegates to
    /// [`resolve_introspective_branch`](Self::resolve_introspective_branch).
    fn override_branch(&self, condition: &Value) -> Option<bool> {
        if self.wants_introspective_holes() && condition.is_introspective_stub() {
            Some(self.resolve_introspective_branch())
        } else {
            None
        }
    }

    /// Called when [`override_value`](Self::override_value) substituted a
    /// value that was about to be emitted with a hole marker instead of
    /// rendering it, so the caller can map the resulting output position
    /// back to `source_span` (e.g. to suppress or reposition diagnostics
    /// that land inside the hole).
    fn on_value_override(&self, _source_span: &Span, _expanded_span: &Span) {}

    /// Called by the default implementation of
    /// [`override_branch`](Self::override_branch) when a `{% if %}`
    /// condition is an introspective-stub value (see
    /// `Value::is_introspective_stub`), instead of coercing it to a real
    /// boolean. Returns whether to take the "then" branch.
    ///
    /// The listener is responsible for assigning a stable, per-render
    /// ordinal to each call (e.g. a call counter) and consulting any
    /// override recorded for that ordinal, so a caller can drive repeated
    /// render passes over the same template that each force a different
    /// tainted decision to the opposite branch -- this is how both branches
    /// of a tainted `{% if %}` end up represented across a bounded number of
    /// render variants, mirroring how Turbo mode explores `{% if %}/{% else
    /// %}` branches statically. Defaults to always taking the "then" branch.
    fn resolve_introspective_branch(&self) -> bool {
        true
    }

    /// Whether `qualified_name` (`"package.macro_name"`, matching how
    /// `Macro::call` identifies itself when reporting to
    /// `on_macro_execute_start`) is statically known to reach an
    /// introspective (warehouse-dependent) adapter call -- directly, or
    /// transitively through another macro it calls. When `true`, `Macro::call`
    /// unconditionally taints the macro's return value (see
    /// `Value::is_introspective_stub`), regardless of which internal branch
    /// this particular render actually took.
    ///
    /// This exists because fine-grained, per-value taint propagation through
    /// arbitrary macro control flow can lose track of taint: a macro can
    /// filter/branch on a tainted value (e.g. "does this column match the
    /// exclude list?") in a way that discards the taint before it ever
    /// reaches an emitted value, while still producing output whose shape
    /// depends on unknowable (warehouse-dependent) data. Treating the whole
    /// call as an opaque taint boundary for such macros trades away
    /// fine-grained hole placement for correctness, mirroring how
    /// `JinjaRenderMode::Turbo` always treats an entire macro call as one
    /// opaque hole. Defaults to `false` (unknown/no static analysis
    /// available), which preserves fine-grained propagation.
    fn is_known_introspective_macro(&self, _qualified_name: &str) -> bool {
        false
    }

    /// Called immediately before a Jinja expression is emitted into rendered output.
    fn on_emit_start(&self, _source_span: &Span) {}

    /// Called immediately after a Jinja expression is emitted into rendered output.
    fn on_emit_end(&self, _source_span: &Span) {}

    /// Called when a malicious return is encountered.
    /// It means return is not on the top level of block
    /// e.g. {{ return(1) + 1 }}
    fn on_malicious_return(&self, _location: &CodeLocation);

    /// Called when a function is being entered.
    fn on_function_start(&self);

    /// Called when a function is being exited.
    fn on_function_end(&self);

    /// Called immediately before a named function call is evaluated, with the
    /// resolved function `name` and its `args`. Paired with
    /// [`on_function_call_end`](Self::on_function_call_end) around the call so a
    /// listener can observe the rendered-output position before and after the
    /// call. Domain-agnostic: the engine attaches no meaning to `name`.
    fn on_function_call_start(&self, _name: &str, _args: &[Value]) {}

    /// Called immediately after a named function call is evaluated.
    fn on_function_call_end(&self, _name: &str) {}

    /// Called when a macro is invoked during rendering, to track macro dependencies.
    /// The `template_name` has the form `{package_name}.{macro_name}`.
    fn on_macro_dependency(&self, _template_name: &str) {}

    /// Whether this listener wants macro call-site locations to be tracked.
    /// Capturing the call site clones a path on every call, so the vm only does
    /// so when at least one listener opts in. Defaults to `false`.
    fn tracks_macro_call_sites(&self) -> bool {
        false
    }

    /// Called just before a macro body begins executing.
    ///
    /// - `name` is the qualified `{package}.{macro_name}`.
    /// - `call_site` is where the macro was invoked from (the calling
    ///   template's path and span), when available.
    /// - `def_path`/`def_span` is where the macro is defined.
    ///
    /// Paired with [`on_macro_execute_end`](Self::on_macro_execute_end) around the
    /// body, including when the body returns an error, so callers can maintain a
    /// balanced call stack.
    fn on_macro_execute_start(
        &self,
        _name: &str,
        _call_site: Option<(&Path, &Span)>,
        _def_path: &Path,
        _def_span: &Span,
    ) {
    }

    /// Called after a macro body finishes executing (on both success and error).
    fn on_macro_execute_end(&self, _name: &str) {}

    /// Called when a ref() or source() call is rendered.
    /// This is used to detect mangled refs by checking if there are
    /// non-whitespace characters adjacent to the ref/source span.
    #[allow(clippy::too_many_arguments)]
    fn on_ref_or_source(
        &self,
        _name: &str,
        _start_line: u32,
        _start_col: u32,
        _start_offset: u32,
        _end_line: u32,
        _end_col: u32,
        _end_offset: u32,
    ) {
    }

    /// Called when a ref() or source() call is resolved to its unique_id
    fn on_ref_or_source_resolved(&self, _unique_id: &str) {}

    /// Called when the literal `this` context variable is looked up (a bare
    /// `{{ this }}` reference). `this` is bound as a plain context variable
    /// holding a `Relation` object rather than dispatched as a function call,
    /// so it can't be observed through [`on_ref_or_source`](Self::on_ref_or_source).
    /// Mirrors that hook's shape so consumers can uniformly determine
    /// ref/source/this provenance of a rendered span.
    #[allow(clippy::too_many_arguments)]
    fn on_this_reference(
        &self,
        _start_line: u32,
        _start_col: u32,
        _start_offset: u32,
        _end_line: u32,
        _end_col: u32,
        _end_offset: u32,
    ) {
    }

    /// Called after rendering to check and emit mangled ref warnings.
    /// Only MangledRefWarningPrinter implements this; default is no-op.
    fn check_and_emit_mangled_ref_warnings(
        &self,
        _rendered_sql: &str,
        _macro_spans: &[(Span, Span)],
    ) {
    }
}

/// Which dbt block a malformed-name event refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockNameKind {
    /// A `{% snapshot %}` block.
    Snapshot,
    /// A `{% docs %}` block.
    Docs,
}

/// Finds the listener (if any) that opted into the value/branch override
/// hooks ([`RenderingEventListener::override_value`]/
/// [`RenderingEventListener::override_branch`]) among `listeners`. A single
/// opted-in listener drives both -- multiple listeners opting in isn't a
/// meaningful configuration, so the first one wins.
///
/// This is the one place that knows *how* to determine whether a listener
/// wants to override values/branches for a render; the VM (`vm::eval_impl`)
/// just calls it once and consults `override_value`/`override_branch` on the
/// result at each choke point, rather than knowing anything about *why* a
/// listener might want to (this crate's introspective-taint feature is one
/// such reason, but the VM itself has no concept of it).
pub(crate) fn find_override_listener(
    listeners: &[std::rc::Rc<dyn RenderingEventListener>],
) -> Option<&std::rc::Rc<dyn RenderingEventListener>> {
    listeners.iter().find(|l| l.wants_introspective_holes())
}

/// A listener for tokenizer events emitted during template compilation.
pub trait TokenizerEventListener: std::fmt::Debug {
    /// Called when the tokenizer emits a source token.
    fn on_source_token(&self, token: &Token<'_>, span: &Span);

    /// Called when a `{% snapshot %}`/`{% docs %}` block name is followed by a
    /// non-empty suffix that dbt-core's regex extractor silently discards.
    fn on_malformed_block_name(&self, _kind: BlockNameKind, _name: &str, _name_span: &Span) {}

    /// Called with the exact byte range of source text silently dropped by
    /// whitespace control (`{%-`/`-%}`/`{{-`/`-}}`/`{#-`/`-#}`) or
    /// `lstrip_blocks`, right as it's skipped. This text never becomes a
    /// `Token`/AST node of its own — without this hook, a listener has no
    /// way to learn it existed at all, short of re-tokenizing and inferring
    /// gaps between consecutive token spans.
    fn on_trimmed_whitespace(&self, _span: &Span) {}
}

/// A macro start event.
#[derive(Debug, Clone)]
pub struct MacroStart {
    /// The line number of the macro start.
    pub line: u32,
    /// The column number of the macro start.
    pub col: u32,
    /// The offset of the macro start.
    pub offset: u32,
    /// The line number of the expanded macro start.
    pub expanded_line: u32,
    /// The column number of the expanded macro start.
    pub expanded_col: u32,
    /// The offset of the expanded macro start.
    pub expanded_offset: u32,
}
