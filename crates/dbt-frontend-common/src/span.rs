use serde::{Deserialize, Serialize};

use crate::error::CodeLocation;

/// A recorded `{{ reclassify(expr, 'from', 'to') }}` call: the byte offsets of
/// the (warehouse-safe) inner expression in the rendered SQL, plus the label
/// mutation. Produced during rendering and matched against bound expression
/// spans by the binder so the classifier propagation walker can apply the
/// mutation without a second render pass.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReclassifySpan {
    /// Start byte offset of the inner expression in the rendered SQL (inclusive).
    pub start: u32,
    /// End byte offset of the inner expression in the rendered SQL (exclusive).
    pub stop: u32,
    /// Classifier label to remove (or wildcard `classifier.*`).
    pub from_label: String,
    /// Replacement label (empty string to declassify).
    pub to_label: String,
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Serialize, Deserialize, Hash, Ord,
)]
pub struct Span {
    /// start location of the span(inclusive)
    pub start: CodeLocation,
    /// stop location of the span(exclusive)
    pub stop: CodeLocation,
}
impl Span {
    pub fn new(start: CodeLocation, stop: CodeLocation) -> Self {
        Span { start, stop }
    }

    pub fn contains(&self, location: &CodeLocation) -> bool {
        &self.start <= location && location < &self.stop
    }

    pub fn slice<'i>(&self, input: &'i str) -> &'i str {
        // TODO it was
        // input[self.start.index..self.stop.index + 1].to_string()
        // because macro span is exclusive, we removed +1
        // be careful lint fix is inclusive, lint fix will break
        &input[self.start.index as usize..self.stop.index as usize]
    }

    pub fn with_offset(&self, start_location: &CodeLocation) -> Self {
        Span {
            start: self.start.with_offset(start_location),
            stop: self.stop.with_offset(start_location),
        }
    }

    pub fn diff(&self, start_location: &CodeLocation) -> Self {
        Span {
            start: self.start.diff(start_location),
            stop: self.stop.diff(start_location),
        }
    }
}
