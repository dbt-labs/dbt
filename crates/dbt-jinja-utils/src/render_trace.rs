//! Ordered source/output provenance from one real Jinja render.

use std::cell::RefCell;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use minijinja::listener::RenderingEventListener;
use minijinja::machinery::Span;
use minijinja::{CodeLocation, JinjaLayoutEventKind, OutputTrackerLocation};
use serde::Serialize;

/// An event observed during execution, in execution order.
#[derive(Debug, Clone, Serialize)]
pub struct RenderTraceEvent {
    /// Event category: literal, templated, macro, layout, or loop boundary.
    pub kind: String,
    /// Inclusive UTF-8 byte offset in the original render input.
    pub source_start: usize,
    /// Exclusive UTF-8 byte offset in the original render input.
    pub source_end: usize,
    /// Inclusive UTF-8 byte offset in the rendered SQL.
    pub rendered_start: usize,
    /// Exclusive UTF-8 byte offset in the rendered SQL.
    pub rendered_end: usize,
}

/// Exact input, output and ordered observations from a successful render.
#[derive(Debug, Serialize)]
pub struct RenderTrace {
    /// Input to Jinja, without newline normalization or source rewriting here.
    pub source: String,
    /// Jinja output, before ephemeral CTE injection.
    pub rendered: String,
    /// Execution events; source ranges may repeat or move backwards in loops.
    pub events: Vec<RenderTraceEvent>,
}

#[derive(Debug)]
struct OutputBuffer {
    location: Rc<OutputTrackerLocation>,
    parent_start: usize,
    local_start: usize,
    event_start: usize,
    macro_depth: usize,
    emit_depth: usize,
}

#[derive(Debug)]
pub(crate) struct RenderTraceListener {
    source_path: PathBuf,
    location: RefCell<Rc<OutputTrackerLocation>>,
    events: RefCell<Vec<RenderTraceEvent>>,
    emits: RefCell<Vec<(Span, usize)>>,
    macros: RefCell<Vec<(usize, usize)>>,
    function_frames: RefCell<Vec<usize>>,
    buffers: RefCell<Vec<OutputBuffer>>,
}

impl RenderTraceListener {
    pub(crate) fn new(source_path: &Path) -> Self {
        Self {
            source_path: source_path.to_owned(),
            location: RefCell::new(Rc::new(OutputTrackerLocation::default())),
            events: RefCell::new(Vec::new()),
            emits: RefCell::new(Vec::new()),
            macros: RefCell::new(Vec::new()),
            function_frames: RefCell::new(Vec::new()),
            buffers: RefCell::new(Vec::new()),
        }
    }

    fn position(&self) -> usize {
        self.location.borrow().index() as usize
    }

    fn record(&self, kind: &str, span: &Span, rendered: Range<usize>) {
        self.events.borrow_mut().push(RenderTraceEvent {
            kind: kind.to_owned(),
            source_start: span.start_offset as usize,
            source_end: span.end_offset as usize,
            rendered_start: rendered.start,
            rendered_end: rendered.end,
        });
    }

    pub(crate) fn finish(&self, source: &str, rendered: String) -> RenderTrace {
        RenderTrace {
            source: source.to_owned(),
            rendered,
            events: self.events.take(),
        }
    }
}

impl RenderingEventListener for RenderTraceListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "RenderTraceListener"
    }

    fn on_output_tracker(&self, location: Rc<OutputTrackerLocation>) {
        if let Some(buffer) = self.buffers.borrow_mut().last_mut() {
            buffer.local_start = location.index() as usize;
        }
        self.location.replace(location);
    }

    fn on_output_buffer_start(&self) {
        self.buffers.borrow_mut().push(OutputBuffer {
            location: self.location.borrow().clone(),
            parent_start: self.position(),
            local_start: 0,
            event_start: self.events.borrow().len(),
            macro_depth: self.macros.borrow().len(),
            emit_depth: self.emits.borrow().len(),
        });
    }

    fn on_output_buffer_end(&self, emitted: bool) {
        let buffer = self
            .buffers
            .borrow_mut()
            .pop()
            .expect("unbalanced output buffer");
        let mut events = self.events.borrow_mut();
        if emitted {
            for event in &mut events[buffer.event_start..] {
                event.rendered_start =
                    event.rendered_start - buffer.local_start + buffer.parent_start;
                event.rendered_end = event.rendered_end - buffer.local_start + buffer.parent_start;
            }
        } else {
            events.truncate(buffer.event_start);
        }
        self.macros.borrow_mut().truncate(buffer.macro_depth);
        self.emits.borrow_mut().truncate(buffer.emit_depth);
        self.location.replace(buffer.location);
    }

    // The existing RenderingEventListener contract requires line/column arguments,
    // even though this byte-oriented observer only needs the path and offset.
    // @codescene(disable:"Excess Number of Function Arguments")
    fn on_macro_start(&self, path: Option<&Path>, _line: &u32, _col: &u32, offset: &u32) {
        if path == Some(self.source_path.as_path()) {
            self.macros
                .borrow_mut()
                .push((*offset as usize, self.position()));
        }
    }

    // Keep the shared trait signature; line/column are unused by trace events.
    // @codescene(disable:"Excess Number of Function Arguments")
    fn on_macro_stop(&self, path: Option<&Path>, _line: &u32, _col: &u32, offset: &u32) {
        if path != Some(self.source_path.as_path()) {
            return;
        }
        if let Some((source_start, rendered_start)) = self.macros.borrow_mut().pop() {
            self.events.borrow_mut().push(RenderTraceEvent {
                kind: "macro".to_owned(),
                source_start,
                source_end: *offset as usize,
                rendered_start,
                rendered_end: self.position(),
            });
        }
    }

    fn on_raw_emit(&self, raw: &str, span: &Span) {
        let start = self.position();
        self.record("literal", span, start..start + raw.len());
    }

    fn on_emit_start(&self, span: &Span) {
        self.emits.borrow_mut().push((*span, self.position()));
    }

    fn on_emit_end(&self, _span: &Span) {
        if let Some((span, start)) = self.emits.borrow_mut().pop() {
            self.record("templated", &span, start..self.position());
        }
    }

    fn on_jinja_layout_event(&self, kind: JinjaLayoutEventKind, span: &Span) {
        let name = match kind {
            JinjaLayoutEventKind::BlockStart => "block_start",
            JinjaLayoutEventKind::BlockMid => "block_mid",
            JinjaLayoutEventKind::BlockEnd => "block_end",
            JinjaLayoutEventKind::BlockStandalone => "block_standalone",
            JinjaLayoutEventKind::Variable => "variable",
            JinjaLayoutEventKind::Comment => "comment",
        };
        let position = self.position();
        self.record(name, span, position..position);
    }

    fn on_jinja_loop_iteration_start(&self, span: &Span) {
        let position = self.position();
        self.record("loop_iteration", span, position..position);
    }

    fn on_jinja_loop_skipped_end(&self, span: &Span) {
        let position = self.position();
        self.record("loop_skipped", span, position..position);
    }

    fn on_function_start(&self) {
        self.function_frames
            .borrow_mut()
            .push(self.macros.borrow().len());
    }

    fn on_function_end(&self) {
        if let Some(depth) = self.function_frames.borrow_mut().pop() {
            self.macros.borrow_mut().truncate(depth);
        }
    }

    fn on_malicious_return(&self, _location: &CodeLocation) {
        if let Some(depth) = self.function_frames.borrow().last() {
            self.macros.borrow_mut().truncate(*depth);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jinja_environment::JinjaEnv;
    use crate::listener::DefaultRenderingEventListenerFactory;
    use crate::utils::{macro_spans_to_macro_span_vec, render_sql, render_sql_with_trace};
    use minijinja::{Environment, Value};
    use std::collections::BTreeMap;

    fn render(source: &str, flag: bool) -> RenderTrace {
        let mut environment = Environment::new();
        environment.set_keep_trailing_newline(true);
        let env = JinjaEnv::new(environment);
        let context = BTreeMap::from([("flag".to_owned(), Value::from(flag))]);
        let path = Path::new("model.sql");
        let normal_factory = DefaultRenderingEventListenerFactory::default();
        let traced_factory = DefaultRenderingEventListenerFactory::default();
        let normal = render_sql(source, &env, &context, &normal_factory, path).unwrap();
        let trace = render_sql_with_trace(source, &env, &context, &traced_factory, path).unwrap();
        assert_eq!(trace.rendered, normal);
        // The additional observer must not change dbt's existing macro spans.
        use crate::listener::RenderingEventListenerFactory;
        assert_eq!(
            serde_json::to_value(macro_spans_to_macro_span_vec(
                &normal_factory.drain_macro_spans(path)
            ))
            .unwrap(),
            serde_json::to_value(macro_spans_to_macro_span_vec(
                &traced_factory.drain_macro_spans(path)
            ))
            .unwrap()
        );
        for event in &trace.events {
            assert!(event.source_start <= event.source_end);
            assert!(event.rendered_start <= event.rendered_end);
            assert!(trace.source.is_char_boundary(event.source_start));
            assert!(trace.source.is_char_boundary(event.source_end));
            assert!(trace.rendered.is_char_boundary(event.rendered_start));
            assert!(trace.rendered.is_char_boundary(event.rendered_end));
            if event.kind == "literal" {
                assert_eq!(
                    &trace.source[event.source_start..event.source_end],
                    &trace.rendered[event.rendered_start..event.rendered_end]
                );
            }
        }
        trace
    }

    #[test]
    fn identical_branches_have_distinct_provenance() {
        let source = "{% if flag %}select 1{% else %}select 1{% endif %}";
        let first = render(source, true);
        let second = render(source, false);
        assert_eq!(first.rendered, second.rendered);
        let literal = |trace: &RenderTrace| {
            trace
                .events
                .iter()
                .find(|event| event.kind == "literal")
                .unwrap()
                .source_start
        };
        assert_ne!(literal(&first), literal(&second));
    }

    #[test]
    fn loops_preserve_repeated_source_and_skipped_execution() {
        let trace = render("{% for x in [1, 2] %}select {{ x }};{% endfor %}", true);
        let literals: Vec<_> = trace
            .events
            .iter()
            .filter(|event| event.kind == "literal")
            .collect();
        assert_eq!(literals[0].source_start, literals[2].source_start);
        assert_ne!(literals[0].rendered_start, literals[2].rendered_start);
        assert!(
            trace
                .events
                .iter()
                .any(|event| event.kind == "loop_iteration")
        );
        let skipped = render("{% for x in [] %}never{% endfor %}select 1", true);
        assert!(
            skipped
                .events
                .iter()
                .any(|event| event.kind == "loop_skipped")
        );
        assert_eq!(skipped.rendered, "select 1");
    }

    #[test]
    fn captures_macros_unicode_and_whitespace_keep_exact_offsets() {
        let trace = render(
            "{# é #}\r\n{% macro m(x) %}{{ x }}{% endmacro %}{% set value %}λ{% endset %}\r\nselect '{{ value }}', {{ m(42) }}\r\n",
            true,
        );
        assert!(trace.rendered.contains("'λ', 42"));
        let trace = render("select {%- if flag %} {{ 7 }} {% endif -%};", true);
        assert!(trace.events.iter().any(|event| event.kind == "templated"));
    }

    #[test]
    fn observer_uses_the_vm_tracker_without_a_default_factory() {
        let source = "select {{ 42 }}";
        let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
        let listeners: Vec<Rc<dyn RenderingEventListener>> = vec![observer.clone()];
        let rendered = Environment::new()
            .render_named_str("model.sql", source, (), &listeners)
            .unwrap();
        let trace = observer.finish(source, rendered);
        let event = trace
            .events
            .iter()
            .find(|event| event.kind == "templated")
            .unwrap();
        assert_eq!(
            &trace.rendered[event.rendered_start..event.rendered_end],
            "42"
        );
        assert_eq!(event.rendered_start, "select ".len());
    }

    #[test]
    fn elif_chain_emits_one_closing_tag() {
        for choice in 0..4 {
            let source = format!(
                "{{% set choice = {choice} %}}{{% if choice == 0 %}}select 1\
                 {{% elif choice == 1 %}}select 2{{% elif choice == 2 %}}select 3\
                 {{% else %}}select 4{{% endif %}}"
            );
            let trace = render(&source, true);
            assert_eq!(trace.rendered, format!("select {}", choice + 1));
            let ends: Vec<_> = trace
                .events
                .iter()
                .filter(|e| e.kind == "block_end")
                .collect();
            assert_eq!(ends.len(), 1, "choice {choice}");
            assert_eq!(
                &source[ends[0].source_start..ends[0].source_end],
                "{% endif %}"
            );
        }
    }

    #[test]
    fn elif_nested_if_keeps_its_own_closing_tag() {
        let source = "{% if false %}unused{% elif true %}\
                      {% if flag %}select 1{% else %}select 2{% endif %}{% endif %}";
        for flag in [true, false] {
            let trace = render(source, flag);
            assert_eq!(trace.rendered, if flag { "select 1" } else { "select 2" });
            let ends: Vec<_> = trace
                .events
                .iter()
                .filter(|e| e.kind == "block_end")
                .collect();
            assert_eq!(ends.len(), 2);
            assert_ne!(ends[0].source_start, ends[1].source_start);
        }
    }

    #[test]
    fn elif_empty_loop_iterations_keep_repeated_closing_tags() {
        let source = "{% for unused in [1, 2] %}\
                      {% if false %}{% elif true %}{% endif %}{% endfor %}";
        let trace = render(source, true);
        assert!(trace.rendered.is_empty());
        let ends: Vec<_> = trace
            .events
            .iter()
            .filter(|e| {
                e.kind == "block_end" && &source[e.source_start..e.source_end] == "{% endif %}"
            })
            .collect();
        assert_eq!(ends.len(), 2);
        // Same byte ranges, but two genuine loop executions: never globally deduplicate.
        assert_eq!(ends[0].source_start, ends[1].source_start);
        assert_eq!(ends[0].rendered_start, ends[1].rendered_start);
    }

    #[test]
    fn raw_whitespace_control_uses_the_emitted_payload_span() {
        for source in [
            "{% raw -%} select 1 {%- endraw %}",
            "é\r\n{% raw -%}\r\n λ 😀 \r\n{%- endraw %}Z",
            "{% raw %} {{ value }} {% endraw %}",
            "{% raw -%}   {%- endraw %}select 1",
        ] {
            render(source, true);
        }
    }

    #[test]
    fn for_else_closes_only_the_executed_body() {
        for (items, expected, endings) in [
            ("[]", "select 0", 1),
            ("[1]", "select 1;", 1),
            ("[1, 2]", "select 1;select 2;", 2),
        ] {
            let source = format!(
                "{{% for x in {items} %}}select {{{{ x }}}};{{% else %}}select 0{{% endfor %}}"
            );
            let trace = render(&source, true);
            assert_eq!(trace.rendered, expected);
            assert_eq!(
                trace
                    .events
                    .iter()
                    .filter(|e| e.kind == "block_end")
                    .count(),
                endings,
                "{items}"
            );
        }
    }

    #[test]
    fn filtered_loops_report_only_executed_body_iterations() {
        for (condition, expected, repeats) in
            [("n == 3", "3", 0), ("n > 3", "", 0), ("n > 1", "23", 1)]
        {
            let source =
                format!("{{% for n in [1, 2, 3] if {condition} %}}{{{{ n }}}}{{% endfor %}}");
            let trace = render(&source, true);
            assert_eq!(trace.rendered, expected);
            assert_eq!(
                trace
                    .events
                    .iter()
                    .filter(|e| e.kind == "loop_iteration")
                    .count(),
                repeats,
                "{condition}"
            );
        }
    }

    #[test]
    fn buffered_blocks_use_final_output_positions_with_default_factory() {
        for (source, expected) in [
            ("A{% block b %}B{% endblock %}C", "ABC"),
            (
                "A{% block b %}B{% block c %}C{% endblock %}D{% endblock %}E",
                "ABCDE",
            ),
        ] {
            let trace = render(source, true);
            assert_eq!(trace.rendered, expected);
            let literals: String = trace
                .events
                .iter()
                .filter(|e| e.kind == "literal")
                .map(|e| &trace.rendered[e.rendered_start..e.rendered_end])
                .collect();
            assert_eq!(literals, expected);
        }
    }

    #[test]
    fn buffered_blocks_restore_the_observer_only_tracker() {
        let source = "A{% block b %}B{% block c %}C{% endblock %}D{% endblock %}E";
        let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
        let listeners: Vec<Rc<dyn RenderingEventListener>> = vec![observer.clone()];
        let rendered = Environment::new()
            .render_named_str("model.sql", source, (), &listeners)
            .unwrap();
        let trace = observer.finish(source, rendered);
        assert_eq!(trace.rendered, "ABCDE");
        let positions: Vec<_> = trace
            .events
            .iter()
            .filter(|e| e.kind == "literal")
            .map(|e| (e.source_start, e.rendered_start, e.rendered_end))
            .collect();
        assert_eq!(
            positions,
            [
                (0, 0, 1),
                (source.find("B{").unwrap(), 1, 2),
                (source.find("C{").unwrap(), 2, 3),
                (source.find("D{").unwrap(), 3, 4),
                (source.len() - 1, 4, 5),
            ]
        );
    }

    #[test]
    fn buffered_block_values_and_captures_are_not_final_literals() {
        for (source, expected, literal) in [
            (
                "A{% block b %}b{% endblock %}C{{ self.b() | upper }}D",
                "AbCBD",
                "AbCD",
            ),
            (
                "A{% set saved %}{% block b %}B{% endblock %}{% endset %}C{{ saved }}D",
                "ACBD",
                "ACD",
            ),
        ] {
            let trace = render(source, true);
            assert_eq!(trace.rendered, expected);
            let literals: String = trace
                .events
                .iter()
                .filter(|e| e.kind == "literal")
                .map(|e| &trace.rendered[e.rendered_start..e.rendered_end])
                .collect();
            assert_eq!(literals, literal);
        }
    }

    #[test]
    fn failed_buffered_blocks_restore_both_tracker_configurations() {
        for default_tracker in [false, true] {
            let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
            let mut listeners: Vec<Rc<dyn RenderingEventListener>> = Vec::new();
            if default_tracker {
                listeners.push(Rc::new(
                    crate::listener::DefaultRenderingEventListener::default(),
                ));
            }
            listeners.push(observer.clone());
            let result = Environment::new().render_named_str(
                "model.sql",
                "A{% block b %}B{% include 'missing.sql' %}{% endblock %}C",
                (),
                &listeners,
            );
            assert!(result.is_err());
            assert_eq!(observer.position(), 1);
            assert!(observer.buffers.borrow().is_empty());
            assert_eq!(
                observer
                    .events
                    .borrow()
                    .iter()
                    .filter(|e| e.kind == "literal")
                    .count(),
                1
            );
        }
    }
}
