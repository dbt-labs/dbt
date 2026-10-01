use serde::{Deserialize, Serialize};

use crate::compiler::tokens::Span;

/// Source-side Jinja layout facts used by downstream lint/format passes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct JinjaLayoutEvents {
    /// Events in source order.
    pub items: Vec<JinjaLayoutEvent>,
}

impl JinjaLayoutEvents {
    /// Push a new event.
    pub fn push(&mut self, event: JinjaLayoutEvent) {
        self.items.push(event);
    }

    /// Extend the list with another list.
    pub fn extend(&mut self, other: JinjaLayoutEvents) {
        self.items.extend(other.items);
    }

    /// Clear the list.
    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// A source-side Jinja layout event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JinjaLayoutEvent {
    /// The layout role of this template tag.
    pub kind: JinjaLayoutEventKind,
    /// The source span of the complete tag, including delimiters.
    pub source_span: Span,
    /// The rendered span or zero-width rendered position associated with this tag.
    pub rendered_span: Option<Span>,
    /// Whether the tag is the only non-whitespace content on its source line(s).
    pub source_line_standalone: bool,
    /// The first identifier in a block tag, for example `if`, `else`, or `endif`.
    pub tag_name: Option<String>,
    /// A stable block id shared by matching block start/mid/end tags.
    pub block_id: Option<u32>,
}

/// The layout role of a Jinja source tag.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
pub enum JinjaLayoutEventKind {
    /// A block opener such as `{% if %}` or `{% for %}`.
    BlockStart,
    /// A block middle tag such as `{% else %}` or `{% elif %}`.
    BlockMid,
    /// A block closer such as `{% endif %}` or `{% endfor %}`.
    BlockEnd,
    /// A non-nesting block tag such as `{% do %}` or `{% set x = y %}`.
    BlockStandalone,
    /// An expression tag such as `{{ value }}`.
    Variable,
    /// A Jinja comment tag.
    Comment,
}
