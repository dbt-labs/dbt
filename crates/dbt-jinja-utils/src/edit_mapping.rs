//! Text edits, and how they move the positions of a [`MacroSpans`].
//!
//! A [`MacroSpans`] maps text a template rendered to the template it came from.
//! When the rendered text is edited afterwards, an [`EditMapping`] carries the
//! second step: where each position of the rendered text lands in the edited
//! text. Applying it to the spans composes the two, so a position in the edited
//! text still leads back to the template.

use minijinja::MacroSpans;

/// Replaces `text[start..stop]` with `text`. An insertion has `start == stop`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlEdit {
    /// Byte offset where the replaced text starts (inclusive).
    pub start: usize,
    /// Byte offset where the replaced text ends (exclusive).
    pub stop: usize,
    /// What takes its place.
    pub text: String,
}

/// Which end of a span a position is, which decides how an edit at that very
/// position treats it: a span that starts where text is inserted begins after
/// the new text, and one that ends there ends before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Start,
    End,
}

/// Edits to a text that do not overlap, in text order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditMapping {
    edits: Vec<SqlEdit>,
}

impl EditMapping {
    /// The mapping of `edits`, which must not overlap.
    pub fn new(mut edits: Vec<SqlEdit>) -> Self {
        edits.sort_by_key(|edit| (edit.start, edit.stop));
        debug_assert!(
            edits.windows(2).all(|pair| pair[0].stop <= pair[1].start),
            "edits overlap: {edits:?}"
        );
        Self { edits }
    }

    /// `text` with the edits applied.
    pub fn apply(&self, text: &str) -> String {
        let mut result = String::with_capacity(text.len());
        let mut cursor = 0;
        for edit in &self.edits {
            result.push_str(&text[cursor..edit.start]);
            result.push_str(&edit.text);
            cursor = edit.stop;
        }
        result.push_str(&text[cursor..]);
        result
    }

    /// Where the edited text has the position that `offset` is in the original.
    ///
    /// A position inside replaced text maps to the start of its replacement as a
    /// span start, and to the end of it as a span end.
    fn map_offset(&self, offset: usize, side: Side) -> usize {
        let mut shift = 0isize;
        for edit in &self.edits {
            let removed = edit.stop - edit.start;
            let past = match side {
                Side::Start => offset >= edit.stop,
                Side::End => offset > edit.stop,
            };
            if past {
                shift += edit.text.len() as isize - removed as isize;
            } else if side == Side::Start && offset >= edit.start {
                return (edit.start as isize + shift) as usize;
            } else if side == Side::End && offset > edit.start {
                return (edit.start as isize + shift) as usize + edit.text.len();
            } else {
                break;
            }
        }
        (offset as isize + shift) as usize
    }

    /// Moves the rendered-text side of every span in `macro_spans`.
    ///
    /// `edited` is the text the edits produced; it gives the line and column of
    /// the new offsets.
    pub fn remap_macro_spans(&self, macro_spans: &mut MacroSpans, edited: &str) {
        let line_starts: Vec<usize> = std::iter::once(0)
            .chain(edited.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_col = |offset: usize| {
            let line = line_starts.partition_point(|start| *start <= offset) - 1;
            let col = edited[line_starts[line]..offset].chars().count();
            (line as u32 + 1, col as u32 + 1)
        };
        for (_, expanded) in &mut macro_spans.items {
            let start = self.map_offset(expanded.start_offset as usize, Side::Start);
            let end = self.map_offset(expanded.end_offset as usize, Side::End);
            (expanded.start_line, expanded.start_col) = line_col(start);
            (expanded.end_line, expanded.end_col) = line_col(end);
            expanded.start_offset = start as u32;
            expanded.end_offset = end as u32;
        }
    }
}

#[cfg(test)]
mod tests {
    use minijinja::machinery::Span;

    use super::*;

    fn edit(start: usize, stop: usize, text: &str) -> SqlEdit {
        SqlEdit {
            start,
            stop,
            text: text.to_string(),
        }
    }

    /// The span of `needle` in `text`.
    fn span_of(text: &str, needle: &str) -> Span {
        let offset = text.find(needle).unwrap();
        let before = &text[..offset];
        let line = 1 + before.matches('\n').count() as u32;
        let col = 1 + before.rsplit('\n').next().unwrap().chars().count() as u32;
        Span {
            start_line: line,
            start_col: col,
            start_offset: offset as u32,
            end_line: line,
            end_col: col + needle.chars().count() as u32,
            end_offset: (offset + needle.len()) as u32,
        }
    }

    fn spans(items: Vec<Span>) -> MacroSpans {
        MacroSpans {
            items: items.into_iter().map(|s| (Span::default(), s)).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn apply_replaces_and_inserts() {
        let mapping = EditMapping::new(vec![edit(7, 9, "XYZ"), edit(3, 3, "-"), edit(12, 12, "!")]);
        assert_eq!(mapping.apply("abcdefghijkl"), "abc-defgXYZjkl!");
    }

    #[test]
    fn a_span_before_the_edits_does_not_move() {
        let text = "select a, b from t";
        let mapping = EditMapping::new(vec![edit(14, 14, "(")]);
        let mut macro_spans = spans(vec![span_of(text, "a, b")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        assert_eq!(macro_spans.items[0].1, span_of(&edited, "a, b"));
    }

    #[test]
    fn a_span_after_the_edits_moves_with_its_text() {
        let text = "select 'é',\n  t.a from main.t where x = 1";
        let at = text.find("main.t").unwrap();
        let mapping = EditMapping::new(vec![
            edit(at, at, "(select * from "),
            edit(at + "main.t".len(), at + "main.t".len(), ") as t"),
        ]);
        let mut macro_spans = spans(vec![span_of(text, "t.a"), span_of(text, "where x = 1")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        assert_eq!(macro_spans.items[0].1, span_of(&edited, "t.a"));
        assert_eq!(macro_spans.items[1].1, span_of(&edited, "where x = 1"));
    }

    #[test]
    fn a_span_around_an_edit_grows_by_it() {
        let text = "from main.t where";
        let at = text.find("main.t").unwrap();
        let mapping = EditMapping::new(vec![edit(at, at, "(select * from ")]);
        let mut macro_spans = spans(vec![span_of(text, "from main.t")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        assert_eq!(
            macro_spans.items[0].1,
            span_of(&edited, "from (select * from main.t")
        );
    }

    #[test]
    fn text_inserted_at_a_span_boundary_stays_outside_the_span() {
        // `main.t` starts where the opening goes in and ends where the closing does.
        let text = "from main.t where";
        let at = text.find("main.t").unwrap();
        let mapping = EditMapping::new(vec![
            edit(at, at, "(select * from "),
            edit(at + 6, at + 6, ")"),
        ]);
        let mut macro_spans = spans(vec![span_of(text, "main.t")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        assert_eq!(macro_spans.items[0].1, span_of(&edited, "main.t"));
    }

    #[test]
    fn a_replaced_marker_leaves_a_span_that_started_on_it_at_the_replacement() {
        let text = "from /*m*/main.t where";
        let marker = text.find("/*m*/").unwrap();
        let table = text.find("main.t").unwrap();
        let mapping = EditMapping::new(vec![
            edit(marker, table, "(select * from "),
            edit(table + 6, table + 6, ")"),
        ]);
        // The expansion of `{{ ref(..) }}`: marker and table.
        let mut macro_spans = spans(vec![span_of(text, "/*m*/main.t")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        let span = macro_spans.items[0].1;
        assert_eq!(span.start_offset as usize, edited.find("(select").unwrap());
        // It covers the replacement and the table, not the closing parenthesis.
        assert_eq!(
            &edited[span.start_offset as usize..span.end_offset as usize],
            "(select * from main.t"
        );
        assert_eq!(span.start_col, 6);
    }

    #[test]
    fn lines_after_a_multi_line_text_are_found_from_the_edited_text() {
        let text = "select 1\nfrom /*m*/a\nwhere true";
        let marker = text.find("/*m*/").unwrap();
        let mapping = EditMapping::new(vec![
            edit(marker, marker + 5, "(select * from "),
            edit(marker + 6, marker + 6, ")"),
        ]);
        let mut macro_spans = spans(vec![span_of(text, "where true")]);
        let edited = mapping.apply(text);
        mapping.remap_macro_spans(&mut macro_spans, &edited);
        assert_eq!(macro_spans.items[0].1, span_of(&edited, "where true"));
    }
}
