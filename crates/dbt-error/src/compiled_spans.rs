use dbt_frontend_common::span::ReclassifySpan;

use crate::preprocessor_location::MacroSpan;

/// Spans captured while rendering a node, carried as a single dyn value through
/// `SqlInstruction` and the compiled-SQL cache.
pub trait CompiledSpans: std::fmt::Debug + Send + Sync {
    /// Macro expansion spans for the rendered output.
    fn macro_spans(&self) -> &[MacroSpan];

    /// Reclassify offset records, when present; `None` otherwise.
    fn reclassify_spans(&self) -> Option<&[ReclassifySpan]> {
        None
    }

    /// Object-safe clone into a fresh boxed trait object.
    fn clone_box(&self) -> Box<dyn CompiledSpans>;
}

impl Clone for Box<dyn CompiledSpans> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

impl Default for Box<dyn CompiledSpans> {
    fn default() -> Self {
        Box::new(MacroSpansOnly::default())
    }
}

/// A `CompiledSpans` implementation that carries macro spans only.
#[derive(Debug, Clone, Default)]
pub struct MacroSpansOnly {
    pub macro_spans: Vec<MacroSpan>,
}

impl CompiledSpans for MacroSpansOnly {
    fn macro_spans(&self) -> &[MacroSpan] {
        &self.macro_spans
    }

    fn clone_box(&self) -> Box<dyn CompiledSpans> {
        Box::new(self.clone())
    }
}
