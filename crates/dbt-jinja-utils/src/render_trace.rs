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
struct TemplateExecution {
    path: PathBuf,
    event_start: usize,
    output_start: usize,
    macro_depth: usize,
    emit_depth: usize,
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
    templates: RefCell<Vec<TemplateExecution>>,
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
            templates: RefCell::new(Vec::new()),
            location: RefCell::new(Rc::new(OutputTrackerLocation::default())),
            events: RefCell::new(Vec::new()),
            emits: RefCell::new(Vec::new()),
            macros: RefCell::new(Vec::new()),
            function_frames: RefCell::new(Vec::new()),
            buffers: RefCell::new(Vec::new()),
        }
    }

    fn is_source_template(&self) -> bool {
        self.templates
            .borrow()
            .last()
            .is_some_and(|frame| frame.path == self.source_path)
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
        if let Some(frame) = self.templates.borrow_mut().last_mut() {
            frame.output_start = location.index() as usize;
        }
        self.location.replace(location);
    }

    fn on_template_execution_start(&self, name: &str) {
        self.templates.borrow_mut().push(TemplateExecution {
            path: PathBuf::from(name),
            event_start: self.events.borrow().len(),
            output_start: self.position(),
            macro_depth: self.macros.borrow().len(),
            emit_depth: self.emits.borrow().len(),
        });
    }

    fn on_template_execution_parent(&self, name: &str) {
        self.templates
            .borrow_mut()
            .last_mut()
            .expect("missing template")
            .path = PathBuf::from(name);
    }

    fn on_template_execution_end(&self) {
        self.templates.borrow_mut().pop();
    }

    fn on_output_replaced(&self, value: &minijinja::Value) {
        let frames = self.templates.borrow();
        let frame = frames.last().expect("missing template");
        self.events.borrow_mut().truncate(frame.event_start);
        self.macros.borrow_mut().truncate(frame.macro_depth);
        self.emits.borrow_mut().truncate(frame.emit_depth);
        // Template and named-block renders expose strings; non-string returns
        // produce empty output. Neither is a literal copy of this source.
        let end = frame.output_start + value.as_str().unwrap_or_default().len();
        self.record("templated", &Span::default(), frame.output_start..end);
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
        if self.is_source_template() && path == Some(self.source_path.as_path()) {
            self.macros
                .borrow_mut()
                .push((*offset as usize, self.position()));
        }
    }

    // Keep the shared trait signature; line/column are unused by trace events.
    // @codescene(disable:"Excess Number of Function Arguments")
    fn on_macro_stop(&self, path: Option<&Path>, _line: &u32, _col: &u32, offset: &u32) {
        if !self.is_source_template() || path != Some(self.source_path.as_path()) {
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
        if self.is_source_template() {
            self.record("literal", span, start..start + raw.len());
        } else {
            // Foreign template bytes have no editable range in this source.
            self.record("templated", &Span::default(), start..start + raw.len());
        }
    }

    fn on_emit_start(&self, span: &Span) {
        let span = if self.is_source_template() {
            *span
        } else {
            Span::default()
        };
        self.emits.borrow_mut().push((span, self.position()));
    }

    fn on_emit_end(&self, _span: &Span) {
        if let Some((span, start)) = self.emits.borrow_mut().pop() {
            self.record("templated", &span, start..self.position());
        }
    }

    fn on_jinja_layout_event(&self, kind: JinjaLayoutEventKind, span: &Span) {
        if !self.is_source_template() {
            return;
        }
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
        if !self.is_source_template() {
            return;
        }
        let position = self.position();
        self.record("loop_iteration", span, position..position);
    }

    fn on_jinja_loop_skipped_end(&self, span: &Span) {
        if !self.is_source_template() {
            return;
        }
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
        assert_valid_trace(&trace);
        trace
    }

    fn assert_valid_trace(trace: &RenderTrace) {
        for event in &trace.events {
            assert!(
                event.source_start <= event.source_end
                    && trace.source.is_char_boundary(event.source_start)
                    && trace.source.is_char_boundary(event.source_end),
                "invalid source range: {event:?}"
            );
            assert!(
                event.rendered_start <= event.rendered_end
                    && trace.rendered.is_char_boundary(event.rendered_start)
                    && trace.rendered.is_char_boundary(event.rendered_end),
                "invalid rendered range: {event:?}"
            );
            if event.kind == "literal" {
                assert_eq!(
                    &trace.source[event.source_start..event.source_end],
                    &trace.rendered[event.rendered_start..event.rendered_end]
                );
            }
        }
        assert_balanced_layout(trace);
        assert_output_coverage(trace);
    }

    fn assert_output_coverage(trace: &RenderTrace) {
        let mut covered = vec![false; trace.rendered.len()];
        for event in &trace.events {
            if matches!(event.kind.as_str(), "literal" | "templated" | "macro") {
                covered[event.rendered_start..event.rendered_end].fill(true);
            }
        }
        assert!(
            covered.iter().all(|byte| *byte),
            "unmapped output: {trace:?}"
        );
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
    fn buffered_blocks_publish_only_final_literals_with_default_factory() {
        for (source, expected, literal) in [
            ("A{% block b %}B{% endblock %}C", "ABC", "ABC"),
            (
                "A{% block b %}B{% block c %}C{% endblock %}D{% endblock %}E",
                "ABCDE",
                "ABCDE",
            ),
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
            assert!(observer.templates.borrow().is_empty());
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

    #[test]
    fn inheritance_keeps_foreign_output_generated_and_child_literals_exact() {
        for default_tracker in [false, true] {
            check_inheritance(default_tracker);
        }
    }

    fn check_inheritance(default_tracker: bool) {
        let source = "{% extends 'parent.sql' %}{% block b %}{% if true %}C{{ 2 }}{% endif %}D{% endblock %}";
        let mut environment = Environment::new();
        environment
            .add_template("grandparent.sql", "G{% block outer %}unused{% endblock %}Z")
            .unwrap();
        environment.add_template("parent.sql", "{% extends 'grandparent.sql' %}{% block outer %}P{{ 1 }}{% for n in [1, 2] %}Q{% endfor %}{% block b %}B{% endblock %}R{% endblock %}").unwrap();
        let normal = environment
            .render_named_str("model.sql", source, (), &[])
            .unwrap();
        let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
        let mut listeners: Vec<Rc<dyn RenderingEventListener>> = Vec::new();
        if default_tracker {
            listeners.push(Rc::new(
                crate::listener::DefaultRenderingEventListener::default(),
            ));
        }
        listeners.push(observer.clone());
        let rendered = environment
            .render_named_str("model.sql", source, (), &listeners)
            .unwrap();
        let trace = observer.finish(source, rendered);
        assert_eq!(trace.rendered, normal);
        assert_eq!(trace.rendered, "GP1QQC2DRZ");
        let mut literal = String::new();
        for event in &trace.events {
            let src = &source[event.source_start..event.source_end];
            let dst = &trace.rendered[event.rendered_start..event.rendered_end];
            if event.kind == "literal" {
                assert_eq!(src, dst);
                literal.push_str(src);
            } else if event.kind.starts_with("block_") {
                assert!(src == "{% if true %}" || src == "{% endif %}", "{src}");
            }
        }
        assert_eq!(literal, "CD");
        assert!(!trace.events.iter().any(|e| e.kind.starts_with("loop_")));
        assert!(observer.templates.borrow().is_empty());
    }

    #[test]
    fn loop_control_closes_exited_layout_scopes_only() {
        for control in ["continue", "break"] {
            for body in [
                format!("select {{{{ n }}}};{{% {control} %}}"),
                format!("{{% if n > 0 %}}select {{{{ n }}}};{{% {control} %}}{{% endif %}}"),
                format!(
                    "{{% if false %}}unused{{% elif true %}}{{% if true %}}select {{{{ n }}}};{{% {control} %}}{{% endif %}}{{% endif %}}"
                ),
            ] {
                let source = format!(
                    "{{% if true %}}{{% for n in [1, 2] %}}{body}{{% else %}}unused{{% endfor %}}select 9{{% endif %}}"
                );
                let trace = render(&source, true);
                assert_eq!(
                    trace.rendered,
                    if control == "continue" {
                        "select 1;select 2;select 9"
                    } else {
                        "select 1;select 9"
                    }
                );
                assert_balanced_layout(&trace);
            }
            let source = format!(
                "{{% for outer in [1, 2] %}}{{% for n in [1, 2] %}}{{% if true %}}{{% {control} %}}{{% endif %}}{{% endfor %}}select {{{{ outer }}}};{{% endfor %}}"
            );
            let trace = render(&source, true);
            assert_eq!(trace.rendered, "select 1;select 2;");
            assert_balanced_layout(&trace);
        }
    }

    fn assert_balanced_layout(trace: &RenderTrace) {
        let mut depth = 0;
        for event in &trace.events {
            match event.kind.as_str() {
                "block_start" | "loop_iteration" => depth += 1,
                "block_end" | "loop_skipped" => {
                    assert!(depth > 0, "unexpected closing event: {event:?}");
                    depth -= 1;
                }
                _ => {}
            }
        }
        assert_eq!(depth, 0, "unclosed layout: {:?}", trace.events);
    }

    #[test]
    fn explicit_returns_replace_provenance_only_for_their_output_buffer() {
        for (source, expected, literal) in [
            ("select 0 {{ return('select 1') }}", "select 1", ""),
            ("{% if true %}discard{{ return('λ') }}{% endif %}", "λ", ""),
            ("discard{{ return('') }}", "", ""),
            ("discard{{ return(42) }}", "", ""),
            (
                "A{% block b %}B{{ return('X') }}{% endblock %}C",
                "AXC",
                "AC",
            ),
            (
                "A{% block b %}B{{ return('longer λ') }}{% endblock %}C",
                "Alonger λC",
                "AC",
            ),
            ("A{% block b %}B{{ return('') }}{% endblock %}C", "AC", "AC"),
            (
                "A{% block b %}B{% block c %}C{{ return('X') }}{% endblock %}D{% endblock %}E",
                "ABXDE",
                "ABDE",
            ),
            (
                "{% macro m() %}discard{{ return('X') }}{% endmacro %}A{{ m() }}C",
                "AXC",
                "AC",
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
            assert_eq!(literals, literal, "{source}");
            assert_balanced_layout(&trace);
            for (index, _) in trace.rendered.char_indices() {
                assert!(
                    trace
                        .events
                        .iter()
                        .any(|e| e.rendered_start <= index && e.rendered_end > index),
                    "unmapped output: {trace:?}"
                );
            }
        }
    }

    #[test]
    fn simple_inheritance_maps_only_the_child_block() {
        let source = "{% extends 'parent.sql' %}{% block b %}C{% endblock %}";
        let mut environment = Environment::new();
        environment
            .add_template("parent.sql", "P{% block b %}B{% endblock %}Q")
            .unwrap();
        let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
        let listeners: Vec<Rc<dyn RenderingEventListener>> = vec![observer.clone()];
        let rendered = environment
            .render_named_str("model.sql", source, (), &listeners)
            .unwrap();
        let trace = observer.finish(source, rendered);
        assert_eq!(trace.rendered, "PCQ");
        assert_valid_trace(&trace);
        let literal = trace.events.iter().find(|e| e.kind == "literal").unwrap();
        assert_eq!(literal.source_start, source.find('C').unwrap());
        assert_eq!((literal.rendered_start, literal.rendered_end), (1, 2));
        let foreign: Vec<_> = trace
            .events
            .iter()
            .filter(|e| e.kind == "templated")
            .map(|e| {
                (
                    e.source_start,
                    e.source_end,
                    e.rendered_start,
                    e.rendered_end,
                )
            })
            .collect();
        assert_eq!(foreign, [(0, 0, 0, 1), (0, 0, 2, 3)]);
    }

    #[test]
    fn observer_only_returns_discard_stale_buffer_events() {
        for (source, expected) in [
            ("select 0 {{ return('select 1') }}", "select 1"),
            ("A{% block b %}B{{ return('X') }}{% endblock %}C", "AXC"),
            ("A{% block b %}B{{ return('') }}{% endblock %}C", "AC"),
        ] {
            let observer = Rc::new(RenderTraceListener::new(Path::new("model.sql")));
            let listeners: Vec<Rc<dyn RenderingEventListener>> = vec![observer.clone()];
            let rendered = Environment::new()
                .render_named_str("model.sql", source, (), &listeners)
                .unwrap();
            let trace = observer.finish(source, rendered);
            assert_eq!(trace.rendered, expected);
            assert_valid_trace(&trace);
            assert!(observer.templates.borrow().is_empty());
            assert!(observer.buffers.borrow().is_empty());
        }
    }
    #[test]
    fn recursive_loops_open_each_direct_and_captured_invocation() {
        for (items, expected) in [
            (
                "[{'n': 1, 'children': [{'n': 2, 'children': []}]}]",
                "select 1;select 2;",
            ),
            ("[{'n': 1, 'children': []}]", "select 1;"),
            ("[]", ""),
        ] {
            let source = format!(
                "{{% for item in {items} recursive %}}select {{{{ item.n }}}};{{{{ loop(item.children) }}}}{{% endfor %}}"
            );
            let trace = render(&source, true);
            assert_eq!(trace.rendered, expected);
        }
        let source = "{% for item in [{'n': 1, 'children': [{'n': 2, 'children': []}]}] recursive %}select {{ item.n }};{{ loop(item.children) | upper }}{% endfor %}";
        let trace = render(source, true);
        assert_eq!(trace.rendered, "select 1;SELECT 2;");
    }

}
