use std::fmt;

use dbt_frontend_common::span::Span;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MacroSpan {
    pub macro_span: Span,
    pub expanded_span: Span,
}

// format macrospan as (macro_span.start-macro_span.end) => (expanded_span.start-expanded_span.end)
impl fmt::Display for MacroSpan {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "({}-{}) => ({}-{})",
            self.macro_span.start,
            self.macro_span.stop,
            self.expanded_span.start,
            self.expanded_span.stop
        )
    }
}
