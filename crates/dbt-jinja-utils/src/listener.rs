use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fmt::Write as _,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, RwLock, RwLockReadGuard},
};

use minijinja::{
    CodeLocation, JinjaLayoutEventKind, MacroSpans, OutputTracker, OutputTrackerLocation,
    TypecheckingEventListener,
    listener::{MacroStart, RenderingEventListener},
    machinery::Span,
};

use dbt_common::{
    CompiledSpans, ErrorCode,
    io_args::IoArgs,
    tracing::dbt_emit::{emit_error_log_message, emit_warn_log_message},
};

/// Trait for creating and destroying rendering event listeners
pub trait RenderingEventListenerFactory: Send + Sync {
    /// Creates new rendering event listeners
    fn create_listeners(
        &self,
        filename: &Path,
        offset: &dbt_frontend_common::error::CodeLocation,
    ) -> Vec<Rc<dyn RenderingEventListener>>;

    /// Destroys a rendering event listener
    fn destroy_listener(&self, _filename: &Path, _listener: Rc<dyn RenderingEventListener>);

    /// Creates rendering and tokenizer listeners for the same render.
    fn create_listener_bundle(
        &self,
        filename: &Path,
        offset: &dbt_frontend_common::error::CodeLocation,
        _source_sql: &str,
    ) -> Vec<Rc<dyn RenderingEventListener>> {
        self.create_listeners(filename, offset)
    }

    /// get macro spans
    fn drain_macro_spans(&self, filename: &Path) -> MacroSpans;

    /// Raw source spans of `ref()`/`source()` calls and bare `this`
    /// references recorded for `filename` during this render (see
    /// [`DefaultRenderingEventListener::ref_source_this_spans`]). Default
    /// implementation returns nothing.
    fn drain_ref_source_this_spans(&self, _filename: &Path) -> Vec<(u32, u32)> {
        Vec::new()
    }

    /// Builds the [`CompiledSpans`] for a freshly rendered node from its
    /// converted macro spans. The default carries macro spans only; an
    /// implementation may override this to attach additional spans associated
    /// with `filename`.
    fn compiled_spans(
        &self,
        macro_spans: Vec<dbt_common::MacroSpan>,
        _filename: &Path,
    ) -> Box<dyn CompiledSpans> {
        Box::new(dbt_common::MacroSpansOnly { macro_spans })
    }

    /// Rebuilds the [`CompiledSpans`] for a node restored from the compiled-SQL
    /// cache. The default ignores `reclassify_spans`; an implementation may
    /// override this to reattach them.
    fn create_spans(
        &self,
        macro_spans: Vec<dbt_common::MacroSpan>,
        reclassify_spans: Vec<dbt_frontend_common::span::ReclassifySpan>,
    ) -> Box<dyn CompiledSpans> {
        let _ = reclassify_spans;
        Box::new(dbt_common::MacroSpansOnly { macro_spans })
    }
}

/// Per-filename raw source byte ranges `[start, end)` of `ref()`/`source()`
/// calls and bare `this` references (see
/// [`DefaultRenderingEventListener::ref_source_this_spans`]).
type RefSourceThisSpansByFile = Arc<RwLock<HashMap<PathBuf, Vec<(u32, u32)>>>>;

/// Default implementation of the `ListenerFactory` trait
#[derive(Default, Debug)]
pub struct DefaultRenderingEventListenerFactory {
    /// Suppress malicious return warning
    pub quiet: bool,
    /// macro spans
    pub macro_spans: Arc<RwLock<HashMap<PathBuf, MacroSpans>>>,
    /// ref/source/this raw source spans, keyed by filename.
    pub ref_source_this_spans: RefSourceThisSpansByFile,
    /// Whether to check for mangled refs
    pub check_mangled_refs: bool,
    /// IO args for warning emission
    pub io_args: IoArgs,
}

impl DefaultRenderingEventListenerFactory {
    /// Creates a new rendering event listener factory
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            macro_spans: Arc::new(RwLock::new(HashMap::new())),
            ref_source_this_spans: Arc::new(RwLock::new(HashMap::new())),
            check_mangled_refs: false,
            io_args: IoArgs::default(),
        }
    }

    /// Creates a new rendering event listener factory with mangled ref checking
    pub fn with_mangled_ref_checking(quiet: bool, io_args: IoArgs) -> Self {
        Self {
            quiet,
            macro_spans: Arc::new(RwLock::new(HashMap::new())),
            ref_source_this_spans: Arc::new(RwLock::new(HashMap::new())),
            check_mangled_refs: true,
            io_args,
        }
    }

    /// Creates the default listeners and returns the shared output-tracker
    /// location used by the macro-span listener, so an additional listener
    /// (e.g. a reclassify capture listener supplied by a decorating factory)
    /// can observe the same rendered-output position. `create_listeners`
    /// delegates here and discards the location.
    pub fn create_listeners_with_tracker(
        &self,
        filename: &Path,
    ) -> (
        Vec<Rc<dyn RenderingEventListener>>,
        Rc<OutputTrackerLocation>,
    ) {
        // Share the output tracker location so other listeners can observe the
        // current render position at the moment their hooks fire.
        let shared_tracker = Rc::new(OutputTrackerLocation::default());
        let mut listeners: Vec<Rc<dyn RenderingEventListener>> = vec![Rc::new(
            DefaultRenderingEventListener::with_tracker(self.quiet, shared_tracker.clone()),
        )];

        if self.check_mangled_refs {
            listeners.push(Rc::new(crate::mangled_ref::MangledRefWarningPrinter::new(
                filename.to_path_buf(),
                shared_tracker.clone(),
            )));
        }

        (listeners, shared_tracker)
    }
}

impl RenderingEventListenerFactory for DefaultRenderingEventListenerFactory {
    /// Creates new rendering event listeners
    fn create_listeners(
        &self,
        filename: &Path,
        _offset: &dbt_frontend_common::error::CodeLocation,
    ) -> Vec<Rc<dyn RenderingEventListener>> {
        self.create_listeners_with_tracker(filename).0
    }

    fn destroy_listener(&self, filename: &Path, listener: Rc<dyn RenderingEventListener>) {
        if let Some(default_listener) = listener
            .as_any()
            .downcast_ref::<DefaultRenderingEventListener>()
        {
            let new_macro_spans = default_listener.macro_spans.borrow().clone();
            if let Ok(mut macro_spans) = self.macro_spans.write() {
                macro_spans.insert(filename.to_path_buf(), new_macro_spans);
            } else {
                emit_error_log_message(
                    ErrorCode::Generic,
                    "Failed to acquire write lock on macro_spans",
                );
            }
            let new_ref_source_this_spans = default_listener.ref_source_this_spans.borrow().clone();
            if let Ok(mut ref_source_this_spans) = self.ref_source_this_spans.write() {
                ref_source_this_spans.insert(filename.to_path_buf(), new_ref_source_this_spans);
            } else {
                emit_error_log_message(
                    ErrorCode::Generic,
                    "Failed to acquire write lock on ref_source_this_spans",
                );
            }
        }
    }

    fn drain_macro_spans(&self, filename: &Path) -> MacroSpans {
        if let Ok(mut spans) = self.macro_spans.write() {
            spans.remove(filename).unwrap_or_default()
        } else {
            emit_error_log_message(
                ErrorCode::Generic,
                "Failed to acquire write lock on macro_spans",
            );
            MacroSpans::default()
        }
    }

    fn drain_ref_source_this_spans(&self, filename: &Path) -> Vec<(u32, u32)> {
        if let Ok(mut spans) = self.ref_source_this_spans.write() {
            spans.remove(filename).unwrap_or_default()
        } else {
            emit_error_log_message(
                ErrorCode::Generic,
                "Failed to acquire write lock on ref_source_this_spans",
            );
            Vec::new()
        }
    }
}

/// Trait for creating and destroying Jinja type checking event listeners
pub trait JinjaTypeCheckingEventListenerFactory: Send + Sync {
    /// Creates a new type checking event listener
    fn create_listener(
        &self,
        args: &IoArgs,
        offset: dbt_common::CodeLocationWithFile,
        noqa_comments: Option<HashSet<u32>>,
        unique_id: &str,
    ) -> Rc<dyn TypecheckingEventListener>;

    /// Destroys a type checking event listener
    fn destroy_listener(&self, filename: &Path, listener: Rc<dyn TypecheckingEventListener>);

    /// Update the unique id
    /// This is for DagExtractListener (Macro depends on) only
    /// We need to type check sql before unique id is determined
    fn update_unique_id(&self, _old_unique_id: &str, _new_unique_id: &str) {}

    /// Return the sorted list of macro unique-ids that were observed for the given
    /// node unique-id during type-checking. Returns an empty Vec when the factory
    /// has no data for the given key (e.g. in LSP mode).
    fn get_macro_depends_on(&self, _unique_id: &str) -> Vec<String> {
        vec![]
    }

    /// Snapshot of the full macro depends-on graph accumulated during this
    /// invocation (node unique-id -> set of macro unique-ids). Empty by default.
    fn all_macro_depends_on(&self) -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::new()
    }

    /// Determines whether or not the listener factory is able to capture
    /// information on hooks.
    fn can_listen_on_hooks(&self) -> bool {
        true
    }
}

/// Default implementation of the `ListenerFactory` trait
#[derive(Default, Debug)]
pub struct DefaultJinjaTypeCheckEventListenerFactory {
    /// all macro depends on
    /// NOTE(felipecrv): this should probably be changed to an `im` data-structure
    all_depends_on: Arc<RwLock<BTreeMap<String, BTreeSet<String>>>>,
    /// unique-ids of macros observed *directly* calling a method named like
    /// one of `minijinja::INTROSPECTIVE_METHOD_NAMES` during type-checking
    /// (see `DagExtractListener::on_introspective_call`). Callers combine
    /// this with `all_depends_on` to compute the *transitive* set of macros
    /// that reach an introspective call, for `JinjaRenderMode::Symbolic`.
    direct_introspective: Arc<RwLock<BTreeSet<String>>>,
}

impl DefaultJinjaTypeCheckEventListenerFactory {
    /// Lock the depends_on graph for reading.
    pub fn depends_on(&self) -> RwLockReadGuard<'_, BTreeMap<String, BTreeSet<String>>> {
        self.all_depends_on.read().unwrap()
    }

    /// Lock the set of macros observed directly calling an introspective
    /// adapter method for reading.
    pub fn direct_introspective(&self) -> RwLockReadGuard<'_, BTreeSet<String>> {
        self.direct_introspective.read().unwrap()
    }
}

impl JinjaTypeCheckingEventListenerFactory for DefaultJinjaTypeCheckEventListenerFactory {
    /// Creates a new type checking event listener
    fn create_listener(
        &self,
        _args: &IoArgs,
        _offset: dbt_common::CodeLocationWithFile,
        _noqa_comments: Option<HashSet<u32>>,
        unique_id: &str,
    ) -> Rc<dyn TypecheckingEventListener> {
        // create a WarningPrinter instance
        // TODO: enable warning printer
        // Rc::new(WarningPrinter::new(
        //     args.clone(),
        //     filename.to_path_buf(),
        //     noqa_comments,
        // ))
        Rc::new(DagExtractListener::new(unique_id))
    }

    fn destroy_listener(&self, _filename: &Path, listener: Rc<dyn TypecheckingEventListener>) {
        if let Some(dag_extract_listener) = listener.as_any().downcast_ref::<DagExtractListener>() {
            let depends_on = dag_extract_listener.depends_on.borrow().clone();
            if let Ok(mut all_depends_on) = self.all_depends_on.write() {
                for (reference, definition) in depends_on {
                    all_depends_on
                        .entry(reference)
                        .or_default()
                        .insert(definition);
                }
            }
            if dag_extract_listener.introspective_call.get()
                && let Ok(mut direct_introspective) = self.direct_introspective.write()
            {
                direct_introspective.insert(dag_extract_listener.unique_id.clone());
            }
        }
    }

    fn update_unique_id(&self, old_unique_id: &str, new_unique_id: &str) {
        // delete the old unique id and insert the new unique id
        if let Ok(mut all_depends_on) = self.all_depends_on.write()
            && let Some(depends_on) = all_depends_on.remove(old_unique_id)
        {
            all_depends_on.insert(new_unique_id.to_string(), depends_on);
        }
        if let Ok(mut direct_introspective) = self.direct_introspective.write()
            && direct_introspective.remove(old_unique_id)
        {
            direct_introspective.insert(new_unique_id.to_string());
        }
    }

    fn get_macro_depends_on(&self, unique_id: &str) -> Vec<String> {
        self.all_depends_on
            .read()
            .unwrap()
            .get(unique_id)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn all_macro_depends_on(&self) -> BTreeMap<String, BTreeSet<String>> {
        self.all_depends_on.read().unwrap().clone()
    }
}

struct DagExtractListener {
    unique_id: String,
    depends_on: RefCell<Vec<(String, String)>>, // (ref, def)
    /// Set if this macro was observed directly calling a method named like
    /// one of `minijinja::INTROSPECTIVE_METHOD_NAMES` (see
    /// `on_introspective_call`).
    introspective_call: Cell<bool>,
}

impl DagExtractListener {
    pub fn new(unique_id: &str) -> Self {
        Self {
            unique_id: unique_id.to_string(),
            depends_on: RefCell::new(vec![]),
            introspective_call: Cell::new(false),
        }
    }
}

impl TypecheckingEventListener for DagExtractListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn warn(&self, _message: &str) {}

    fn set_span(&self, _span: &Span) {}

    fn new_block(&self, _block_id: usize) {}

    fn flush(&self) {}

    fn on_lookup(&self, _span: &Span, _simple_name: &str, _full_name: &str, _def_spans: Vec<Span>) {
    }

    fn on_function_call(
        &self,
        _source_span: &Span,
        _def_span: &Span,
        _def_path: &Path,
        def_unique_id: &str,
    ) {
        if def_unique_id == self.unique_id {
            return;
        }
        self.depends_on
            .borrow_mut()
            .push((self.unique_id.clone(), def_unique_id.to_string()));
    }

    fn on_introspective_call(&self, _method_name: &str) {
        self.introspective_call.set(true);
    }
}

#[allow(dead_code)]
struct WarningPrinter {
    args: IoArgs,
    path: PathBuf,
    noqa_comments: Option<HashSet<u32>>,
    current_block: RefCell<usize>,
    pending_warnings: RefCell<HashMap<usize, Vec<(CodeLocation, String)>>>,
    current_span: RefCell<Option<Span>>,
}

impl WarningPrinter {
    #[allow(dead_code)]
    pub fn new(args: IoArgs, path: PathBuf, noqa_comments: Option<HashSet<u32>>) -> Self {
        Self {
            args,
            path,
            noqa_comments,
            current_block: RefCell::new(0),
            pending_warnings: RefCell::new(HashMap::new()),
            current_span: RefCell::new(None),
        }
    }
}

impl TypecheckingEventListener for WarningPrinter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn on_lookup(&self, _span: &Span, _simple_name: &str, _full_name: &str, _def_spans: Vec<Span>) {
        //
    }
    fn warn(&self, message: &str) {
        // todo: consider self.offset
        if self.noqa_comments.is_some()
            && self
                .noqa_comments
                .as_ref()
                .unwrap()
                .contains(&self.current_span.borrow().unwrap().start_line)
        {
            return;
        }
        let binding = self.current_span.borrow(); // TODO: do not use the current_span
        let current_span = binding.as_ref().unwrap();
        let location = CodeLocation {
            line: current_span.start_line,
            col: current_span.start_col,
            file: self.path.clone(),
        };

        self.pending_warnings
            .borrow_mut()
            .entry(*self.current_block.borrow())
            .or_default()
            .push((location, message.to_string()));
    }

    fn new_block(&self, block_id: usize) {
        *self.current_block.borrow_mut() = block_id;
        self.pending_warnings
            .borrow_mut()
            .insert(block_id, Vec::new());
    }

    fn set_span(&self, span: &Span) {
        *self.current_span.borrow_mut() = Some(*span);
    }

    fn flush(&self) {
        let mut warnings: Vec<_> = self
            .pending_warnings
            .borrow()
            .values()
            .flat_map(|warnings| warnings.iter().cloned())
            .collect();
        warnings.sort_by(|(loc1, msg1), (loc2, msg2)| {
            (loc1.line, loc1.col, msg1).cmp(&(loc2.line, loc2.col, msg2))
        });
        warnings.iter().for_each(|(location, message)| {
            emit_warn_log_message(
                ErrorCode::JinjaTypeCheckFailed,
                format!("{}\n  --> {}", message, location),
            );
        });
    }
}

/// Listener that tracks which macros are invoked during rendering.
/// Used to populate `depends_on.macros` for `state:modified` support.
#[derive(Debug, Default)]
pub struct MacroDependencyListener {
    macro_deps: RefCell<Vec<String>>,
}

impl MacroDependencyListener {
    /// Creates a new, empty dependency tracker.
    pub fn new() -> Self {
        Self {
            macro_deps: RefCell::new(Vec::new()),
        }
    }

    /// Drains collected template names, deduplicates, and returns them as dbt
    /// `macro.<package>.<name>` unique IDs.
    pub fn drain_macro_unique_ids(&self) -> Vec<String> {
        let mut deps = self.macro_deps.borrow_mut();
        deps.sort();
        deps.dedup();
        deps.drain(..)
            .map(|template_name| format!("macro.{template_name}"))
            .collect()
    }
}

impl RenderingEventListener for MacroDependencyListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "MacroDependencyListener"
    }

    fn on_macro_start(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_macro_stop(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_malicious_return(&self, _location: &CodeLocation) {}
    fn on_function_start(&self) {}
    fn on_function_end(&self) {}

    fn on_macro_dependency(&self, template_name: &str) {
        self.macro_deps.borrow_mut().push(template_name.to_string());
    }
}

/// A source location (`path:line:col`) captured for a [`JinjaTraceFrame`].
#[derive(Debug, PartialEq, Eq)]
struct JinjaTraceLocation {
    file_path: String,
    line_no: u32,
    char_no: u32,
}

impl JinjaTraceLocation {
    fn new(path: &Path, span: &Span) -> Self {
        Self {
            file_path: path.to_string_lossy().into_owned(),
            line_no: span.start_line,
            char_no: span.start_col,
        }
    }

    /// Renders the location, stripping the noisy internal-package prefix
    /// (`dbt_internal_packages/<package>/macros/`) so paths read cleanly.
    fn display(&self) -> String {
        let path = if self.file_path.is_empty() {
            "<unknown>"
        } else {
            shorten_macro_path(&self.file_path)
        };
        format!("{path}:{}:{}", self.line_no, self.char_no)
    }
}

/// Strips the `dbt_internal_packages/<package>/macros/` prefix from a macro path
/// so internal macros render as e.g. `etc/statement.sql` instead of the full
/// `dbt_internal_packages/dbt-adapters/macros/etc/statement.sql`. Paths that do
/// not match (user project files) are returned unchanged.
fn shorten_macro_path(path: &str) -> &str {
    const INTERNAL: &str = "dbt_internal_packages/";
    const MACROS: &str = "/macros/";
    if let Some(start) = path.find(INTERNAL) {
        if let Some(macros) = path[start..].find(MACROS) {
            return &path[start + macros + MACROS.len()..];
        }
    }
    path
}

/// A single macro execution captured by [`JinjaTraceListener`], annotated with
/// its nesting depth, the call site (where it was invoked from), and the
/// location where the macro is defined.
#[derive(Debug)]
struct JinjaTraceFrame {
    depth: usize,
    name: String,
    call_site: Option<JinjaTraceLocation>,
    definition: JinjaTraceLocation,
}

/// Listener that captures a nested trace of macro executions during rendering.
/// Used to produce a diagnostic dump when a materialization fails.
#[derive(Debug, Default)]
pub struct JinjaTraceListener {
    tree: RefCell<Vec<JinjaTraceFrame>>,
    depth: Cell<usize>,
}

impl JinjaTraceListener {
    /// Creates a new, empty trace listener.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns true if no macro executions have been recorded.
    pub fn is_empty(&self) -> bool {
        self.tree.borrow().is_empty()
    }

    /// Returns the collected macro call tree as human-readable text.
    pub fn format_trace(&self) -> String {
        let tree = self.tree.borrow();
        let mut out = String::from("Jinja macro call stack (most recent call last):\n\n");

        let frames: Vec<_> = tree.iter().collect();
        let mut i = 0;

        while i < frames.len() {
            let frame = &frames[i];

            // Collapse only genuine repeats: immediately adjacent frames that
            // are the same macro (same definition) invoked from the same call
            // site. Same name alone is not enough — the same macro called from
            // two different places is two distinct calls, not a repeat.
            let repeat_count = frames[i..]
                .iter()
                .take_while(|f| {
                    f.depth == frame.depth
                        && f.definition == frame.definition
                        && f.call_site == frame.call_site
                })
                .count();

            let indent = "  ".repeat(frame.depth);
            let definition = frame.definition.display();

            if repeat_count > 1 {
                let _ = writeln!(out, "{indent}{} ×{repeat_count} ({definition})", frame.name);
            } else {
                let _ = writeln!(out, "{indent}{} ({definition})", frame.name);
            }

            if let Some(call_site) = &frame.call_site {
                let _ = writeln!(out, "{indent}  @ {}", call_site.display());
            }

            let _ = writeln!(out);

            i += repeat_count;
        }
        out
    }
}

impl RenderingEventListener for JinjaTraceListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "JinjaTraceListener"
    }

    fn tracks_macro_call_sites(&self) -> bool {
        true
    }

    // The call tree is built entirely from macro execution events below; the
    // remaining rendering signals are not needed here.
    fn on_macro_start(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_macro_stop(&self, _file_path: Option<&Path>, _line: &u32, _col: &u32, _offset: &u32) {}
    fn on_malicious_return(&self, _location: &CodeLocation) {}
    fn on_function_start(&self) {}
    fn on_function_end(&self) {}

    fn on_macro_execute_start(
        &self,
        name: &str,
        call_site: Option<(&Path, &Span)>,
        def_path: &Path,
        def_span: &Span,
    ) {
        self.tree.borrow_mut().push(JinjaTraceFrame {
            depth: self.depth.get(),
            name: name.to_string(),
            call_site: call_site.map(|(path, span)| JinjaTraceLocation::new(path, span)),
            definition: JinjaTraceLocation::new(def_path, def_span),
        });
        self.depth.set(self.depth.get() + 1);
    }

    fn on_macro_execute_end(&self, _name: &str) {
        self.depth.set(self.depth.get().saturating_sub(1));
    }
}

/// default implementation of RenderingEventListener
#[derive(Debug)]
pub struct DefaultRenderingEventListener {
    /// Suppress malicious return warning
    pub quiet: bool,

    /// io args
    pub args: IoArgs,

    /// macro spans
    pub macro_spans: RefCell<MacroSpans>,

    /// inner Vec<MacroStart> means during one function start/stop
    macro_start_stack: RefCell<Vec<Vec<MacroStart>>>,

    /// Output tracker location for tracking expanded positions
    output_tracker_location: Rc<OutputTrackerLocation>,

    /// Raw (pre-render) source byte ranges `[start, end)` of `ref()`/`source()`
    /// calls and bare `this` references encountered during this render. Lets a
    /// consumer determine, alongside `macro_spans`, whether a rendered span
    /// came from real `ref`/`source`/`this` provenance rather than literal text
    /// or other templating -- without any text matching on the rendered output.
    pub ref_source_this_spans: RefCell<Vec<(u32, u32)>>,
}

impl Default for DefaultRenderingEventListener {
    fn default() -> Self {
        Self {
            quiet: false,
            args: IoArgs::default(),
            macro_spans: RefCell::new(MacroSpans::default()),
            macro_start_stack: RefCell::new(vec![vec![]]),
            output_tracker_location: Rc::new(OutputTrackerLocation::default()),
            ref_source_this_spans: RefCell::new(Vec::new()),
        }
    }
}

impl DefaultRenderingEventListener {
    /// Creates a new rendering event listener
    pub fn new(quiet: bool) -> Self {
        Self {
            quiet,
            args: IoArgs::default(),
            macro_spans: RefCell::new(MacroSpans::default()),
            macro_start_stack: RefCell::new(vec![vec![]]),
            output_tracker_location: Rc::new(OutputTrackerLocation::default()),
            ref_source_this_spans: RefCell::new(Vec::new()),
        }
    }

    /// Creates a new rendering event listener with a shared output tracker location.
    /// Use this when the output position needs to be observable by another listener
    /// (e.g. `MangledRefWarningPrinter`) at the same time.
    pub fn with_tracker(quiet: bool, output_tracker_location: Rc<OutputTrackerLocation>) -> Self {
        Self {
            quiet,
            args: IoArgs::default(),
            macro_spans: RefCell::new(MacroSpans::default()),
            macro_start_stack: RefCell::new(vec![vec![]]),
            output_tracker_location,
            ref_source_this_spans: RefCell::new(Vec::new()),
        }
    }
}

#[derive(Clone, Copy)]
struct SpanPosition {
    line: u32,
    col: u32,
    offset: u32,
}

fn push_raw_source_spans(
    raw: &str,
    source_span: Span,
    expanded_start: SpanPosition,
    spans: &mut Vec<(Span, Span)>,
) {
    let mut source = SpanPosition {
        line: source_span.start_line,
        col: source_span.start_col,
        offset: source_span.start_offset,
    };
    let mut expanded = expanded_start;

    for part in raw.split_inclusive('\n') {
        if part.is_empty() {
            continue;
        }
        let source_start = source;
        let expanded_start = expanded;
        advance_span_position(&mut source, part);
        advance_span_position(&mut expanded, part);
        spans.push((
            span_from_positions(source_start, source),
            span_from_positions(expanded_start, expanded),
        ));
    }
}

fn advance_span_position(position: &mut SpanPosition, text: &str) {
    position.offset += text.len() as u32;
    if let Some(last_newline) = text.rfind('\n') {
        position.line += text.bytes().filter(|byte| *byte == b'\n').count() as u32;
        position.col = (text.len() - last_newline) as u32;
    } else {
        position.col += text.len() as u32;
    }
}

fn span_from_positions(start: SpanPosition, end: SpanPosition) -> Span {
    Span {
        start_line: start.line,
        start_col: start.col,
        start_offset: start.offset,
        end_line: end.line,
        end_col: end.col,
        end_offset: end.offset,
    }
}

impl RenderingEventListener for DefaultRenderingEventListener {
    fn on_function_start(&self) {
        self.macro_start_stack.borrow_mut().push(vec![]);
    }

    fn on_function_end(&self) {
        // assert the the top level of the stack is empty
        let mut macro_start_stack = self.macro_start_stack.borrow_mut();
        if !macro_start_stack.last().unwrap().is_empty() {
            unreachable!("MacroStart stack is not empty");
        }
        macro_start_stack.pop();
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "DefaultRenderingEventListener"
    }

    fn create_output_tracker<'a>(
        &self,
        w: &'a mut (dyn std::fmt::Write + 'a),
    ) -> Option<OutputTracker<'a>> {
        Some(OutputTracker::with_location(
            w,
            self.output_tracker_location.clone(),
        ))
    }

    fn on_macro_start(&self, _file_path: Option<&Path>, line: &u32, col: &u32, offset: &u32) {
        // Capture current expanded location from our own tracker
        let expanded_line = self.output_tracker_location.line();
        let expanded_col = self.output_tracker_location.col();
        let expanded_offset = self.output_tracker_location.index();

        self.macro_start_stack
            .borrow_mut()
            .last_mut()
            .unwrap()
            .push(MacroStart {
                line: *line,
                col: *col,
                offset: *offset,
                expanded_line,
                expanded_col,
                expanded_offset,
            });
    }

    fn on_macro_stop(&self, _file_path: Option<&Path>, line: &u32, col: &u32, offset: &u32) {
        // Get current expanded location from our own tracker
        let expanded_line = self.output_tracker_location.line();
        let expanded_col = self.output_tracker_location.col();
        let expanded_offset = self.output_tracker_location.index();

        let mut macro_start_stack = self.macro_start_stack.borrow_mut();
        let macro_start_stack_length = macro_start_stack.len();
        let macro_start_stack_last = macro_start_stack.last_mut().unwrap();
        let macro_start_stack_last_length = macro_start_stack_last.len();
        if macro_start_stack_length == 1 && macro_start_stack_last_length == 1 {
            let macro_start = macro_start_stack_last.pop().unwrap();
            self.macro_spans.borrow_mut().push(
                Span {
                    start_line: macro_start.line,
                    start_col: macro_start.col,
                    start_offset: macro_start.offset,
                    end_line: *line,
                    end_col: *col,
                    end_offset: *offset,
                },
                Span {
                    start_line: macro_start.expanded_line,
                    start_col: macro_start.expanded_col,
                    start_offset: macro_start.expanded_offset,
                    end_line: expanded_line,
                    end_col: expanded_col,
                    end_offset: expanded_offset,
                },
            );
        } else {
            macro_start_stack_last.pop();
        }
    }

    fn on_jinja_layout_event(&self, _kind: JinjaLayoutEventKind, _source_span: &Span) {}

    fn on_raw_emit(&self, raw: &str, source_span: &Span) {
        push_raw_source_spans(
            raw,
            *source_span,
            SpanPosition {
                line: self.output_tracker_location.line(),
                col: self.output_tracker_location.col(),
                offset: self.output_tracker_location.index(),
            },
            &mut self.macro_spans.borrow_mut().raw_source_spans,
        );
    }

    fn on_malicious_return(&self, location: &CodeLocation) {
        // Whenever we encounter a malicious return, it means a false MacroStart is issued
        // We should remove the false MacroStart from the stack
        let mut macro_start_stack = self.macro_start_stack.borrow_mut();
        let macro_start_stack_last = macro_start_stack.last_mut().unwrap();
        macro_start_stack_last.clear();
        if !self.quiet {
            // We should also warn it
            emit_warn_log_message(
                ErrorCode::JinjaTopLevelReturn,
                format!(
                    "return is not at the top level of the block.\nIts value is final and cannot be modified by surrounding expressions.\nExample: return(0) + 1. The + 1 is ignored and the macro returns 0.\n  --> {}",
                    location
                ),
            );
        }
    }

    fn on_ref_or_source(
        &self,
        _name: &str,
        _start_line: u32,
        _start_col: u32,
        start_offset: u32,
        _end_line: u32,
        _end_col: u32,
        end_offset: u32,
    ) {
        self.ref_source_this_spans
            .borrow_mut()
            .push((start_offset, end_offset));
    }

    fn on_this_reference(
        &self,
        _start_line: u32,
        _start_col: u32,
        start_offset: u32,
        _end_line: u32,
        _end_col: u32,
        end_offset: u32,
    ) {
        self.ref_source_this_spans
            .borrow_mut()
            .push((start_offset, end_offset));
    }
}

/// Function registry (to resolve a macro's qualified name to a dbt
/// unique-id) plus the statically computed set of introspective unique-ids
/// (see `JinjaEnv::introspective_macros`), as supplied to
/// [`SymbolicRenderingEventListener::with_macro_registry`].
type IntrospectiveMacroRegistry = (
    Arc<minijinja::compiler::typecheck::FunctionRegistry>,
    Arc<RwLock<HashSet<String>>>,
);

/// Listener used exclusively by `JinjaRenderMode::Symbolic`'s render path
/// (`sdf_linter::rendered_linter::render_symbolic_lint_targets`). Wraps a
/// [`DefaultRenderingEventListener`] for the macro-span-tracking behavior
/// every render mode needs, adding only the introspective-taint-specific
/// state (opting into `RenderingEventListener::wants_introspective_holes`
/// unconditionally). Keeping this state and behavior off
/// `DefaultRenderingEventListener` itself means every other render path
/// (dbt run/compile, Turbo/Rendered lint, LSP, ...) can't accidentally
/// depend on or be affected by it.
#[derive(Debug)]
pub struct SymbolicRenderingEventListener {
    inner: DefaultRenderingEventListener,

    /// Per-ordinal forced branch decisions for tainted `{% if %}`
    /// conditions, consulted by `resolve_introspective_branch`. Set once at
    /// construction time (via
    /// [`with_branch_overrides`](Self::with_branch_overrides)) so each
    /// render pass explores one specific alternate branch.
    introspective_branch_overrides: HashMap<usize, bool>,

    /// Counts calls to `resolve_introspective_branch` during this render
    /// pass, which doubles as the stable ordinal assigned to each tainted
    /// `{% if %}` decision encountered (in evaluation order).
    introspective_branch_ordinal: Cell<usize>,

    /// Macro qualified-name (`"package.macro_name"`, matching
    /// `Macro::call`'s own `qualified_name`) lookup used by
    /// `is_known_introspective_macro`, backed by the function registry (to
    /// resolve a qualified name to a dbt unique-id) and the statically
    /// computed set of introspective unique-ids (see
    /// `JinjaEnv::introspective_macros`). `None` means "don't know" (the
    /// check always answers `false`).
    introspective_macro_registry: Option<IntrospectiveMacroRegistry>,
}

impl SymbolicRenderingEventListener {
    /// Creates a new listener.
    pub fn new(quiet: bool) -> Self {
        Self {
            inner: DefaultRenderingEventListener::new(quiet),
            introspective_branch_overrides: HashMap::new(),
            introspective_branch_ordinal: Cell::new(0),
            introspective_macro_registry: None,
        }
    }

    /// Creates a new listener with a shared output tracker location. Use this
    /// when the output position needs to be observable by another listener at
    /// the same time (e.g. a `SqlfluffTemplatedFileListener` built with
    /// `owns_output_tracker: false`, so the two can be combined in one render
    /// pass without fighting over which one attaches the tracker).
    pub fn with_tracker(quiet: bool, output_tracker_location: Rc<OutputTrackerLocation>) -> Self {
        Self {
            inner: DefaultRenderingEventListener::with_tracker(quiet, output_tracker_location),
            introspective_branch_overrides: HashMap::new(),
            introspective_branch_ordinal: Cell::new(0),
            introspective_macro_registry: None,
        }
    }

    /// The macro spans accumulated by the wrapped
    /// [`DefaultRenderingEventListener`] during this render.
    pub fn macro_spans(&self) -> std::cell::Ref<'_, MacroSpans> {
        self.inner.macro_spans.borrow()
    }

    /// The ref/source/this raw source spans accumulated by the wrapped
    /// [`DefaultRenderingEventListener`] during this render.
    pub fn ref_source_this_spans(&self) -> std::cell::Ref<'_, Vec<(u32, u32)>> {
        self.inner.ref_source_this_spans.borrow()
    }

    /// Forces the tainted `{% if %}` decision at each given ordinal (in
    /// evaluation order, 0-based) to the given branch for this render pass;
    /// any tainted decision not present takes the default ("then") branch.
    /// Used to explore one alternate branch per render pass, mirroring
    /// Turbo's static `{% if %}/{% else %}` variant generation.
    pub fn with_branch_overrides(mut self, overrides: HashMap<usize, bool>) -> Self {
        self.introspective_branch_overrides = overrides;
        self
    }

    /// Number of tainted `{% if %}` decisions encountered during this render
    /// pass, i.e. the number of distinct ordinals a caller could override in
    /// a subsequent pass.
    pub fn branch_count(&self) -> usize {
        self.introspective_branch_ordinal.get()
    }

    /// Supplies the data `is_known_introspective_macro` needs to answer:
    /// `jinja_env.jinja_function_registry` (to resolve a macro's qualified
    /// name to its dbt unique-id) and `jinja_env.introspective_macros` (the
    /// statically-computed set of unique-ids known to reach an introspective
    /// adapter call), so a whole macro call can be treated as an opaque
    /// taint boundary.
    pub fn with_macro_registry(
        mut self,
        function_registry: Arc<minijinja::compiler::typecheck::FunctionRegistry>,
        introspective_macros: Arc<RwLock<HashSet<String>>>,
    ) -> Self {
        self.introspective_macro_registry = Some((function_registry, introspective_macros));
        self
    }
}

impl RenderingEventListener for SymbolicRenderingEventListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn name(&self) -> &str {
        "SymbolicRenderingEventListener"
    }

    fn create_output_tracker<'a>(
        &self,
        w: &'a mut (dyn std::fmt::Write + 'a),
    ) -> Option<OutputTracker<'a>> {
        self.inner.create_output_tracker(w)
    }

    fn on_macro_start(&self, file_path: Option<&Path>, line: &u32, col: &u32, offset: &u32) {
        self.inner.on_macro_start(file_path, line, col, offset)
    }

    fn on_macro_stop(&self, file_path: Option<&Path>, line: &u32, col: &u32, offset: &u32) {
        self.inner.on_macro_stop(file_path, line, col, offset)
    }

    fn on_raw_emit(&self, raw: &str, source_span: &Span) {
        self.inner.on_raw_emit(raw, source_span)
    }

    fn on_malicious_return(&self, location: &CodeLocation) {
        self.inner.on_malicious_return(location)
    }

    fn on_function_start(&self) {
        self.inner.on_function_start()
    }

    fn on_function_end(&self) {
        self.inner.on_function_end()
    }

    fn on_ref_or_source(
        &self,
        name: &str,
        start_line: u32,
        start_col: u32,
        start_offset: u32,
        end_line: u32,
        end_col: u32,
        end_offset: u32,
    ) {
        self.inner.on_ref_or_source(
            name,
            start_line,
            start_col,
            start_offset,
            end_line,
            end_col,
            end_offset,
        )
    }

    fn on_this_reference(
        &self,
        start_line: u32,
        start_col: u32,
        start_offset: u32,
        end_line: u32,
        end_col: u32,
        end_offset: u32,
    ) {
        self.inner.on_this_reference(
            start_line,
            start_col,
            start_offset,
            end_line,
            end_col,
            end_offset,
        )
    }

    fn wants_introspective_holes(&self) -> bool {
        true
    }

    fn on_value_override(&self, source_span: &Span, expanded_span: &Span) {
        self.inner
            .macro_spans
            .borrow_mut()
            .push(*source_span, *expanded_span);
    }

    fn resolve_introspective_branch(&self) -> bool {
        let ordinal = self.introspective_branch_ordinal.get();
        self.introspective_branch_ordinal.set(ordinal + 1);
        self.introspective_branch_overrides
            .get(&ordinal)
            .copied()
            .unwrap_or(true)
    }

    fn is_known_introspective_macro(&self, qualified_name: &str) -> bool {
        let Some((function_registry, introspective_macros)) = &self.introspective_macro_registry
        else {
            return false;
        };
        let Some(unique_id) = function_registry
            .get(qualified_name)
            .and_then(|funcsign| funcsign.get_unique_id())
        else {
            return false;
        };
        introspective_macros.read().unwrap().contains(&unique_id)
    }
}

#[cfg(test)]
mod introspective_hole_tests {
    use super::*;
    use dbt_adapter::introspective_taint::IntrospectiveValue;
    use minijinja::{Environment, Value, context};

    #[test]
    fn tainted_emit_is_substituted_with_hole_when_enabled() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> =
            Rc::new(SymbolicRenderingEventListener::new(true));
        let out = env
            .render_str(
                "select {{ col }} from t",
                context! { col => IntrospectiveValue::wrap(Value::from("real_col")) },
                std::slice::from_ref(&listener),
            )
            .unwrap();
        assert_eq!(out, "select {{}} from t");

        let symbolic_listener = listener
            .as_any()
            .downcast_ref::<SymbolicRenderingEventListener>()
            .unwrap();
        assert!(!symbolic_listener.macro_spans().items.is_empty());
    }

    #[test]
    fn tainted_emit_is_rendered_normally_on_the_default_listener() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> =
            Rc::new(DefaultRenderingEventListener::new(true));
        let out = env
            .render_str(
                "select {{ col }} from t",
                context! { col => IntrospectiveValue::wrap(Value::from("real_col")) },
                &[listener],
            )
            .unwrap();
        assert_eq!(out, "select real_col from t");
    }

    #[test]
    fn untainted_emit_is_unaffected_when_holes_enabled() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> =
            Rc::new(SymbolicRenderingEventListener::new(true));
        let out = env
            .render_str(
                "select {{ col }} from t",
                context! { col => Value::from("real_col") },
                &[listener],
            )
            .unwrap();
        assert_eq!(out, "select real_col from t");
    }

    #[test]
    fn tainted_if_condition_defaults_to_then_branch_and_counts_one_decision() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> =
            Rc::new(SymbolicRenderingEventListener::new(true));
        let out = env
            .render_str(
                "{% if rel %}A{% else %}B{% endif %}",
                context! { rel => IntrospectiveValue::wrap(Value::from(())) },
                std::slice::from_ref(&listener),
            )
            .unwrap();
        assert_eq!(out, "A");

        let symbolic_listener = listener
            .as_any()
            .downcast_ref::<SymbolicRenderingEventListener>()
            .unwrap();
        assert_eq!(symbolic_listener.branch_count(), 1);
    }

    #[test]
    fn overriding_the_ordinal_forces_the_else_branch() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> = Rc::new(
            SymbolicRenderingEventListener::new(true)
                .with_branch_overrides(HashMap::from([(0, false)])),
        );
        let out = env
            .render_str(
                "{% if rel %}A{% else %}B{% endif %}",
                context! { rel => IntrospectiveValue::wrap(Value::from(())) },
                &[listener],
            )
            .unwrap();
        assert_eq!(out, "B");
    }

    #[test]
    fn untainted_if_condition_is_unaffected() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> =
            Rc::new(SymbolicRenderingEventListener::new(true));
        let out = env
            .render_str(
                "{% if flag %}A{% else %}B{% endif %}",
                context! { flag => Value::from(false) },
                std::slice::from_ref(&listener),
            )
            .unwrap();
        assert_eq!(out, "B");
        let symbolic_listener = listener
            .as_any()
            .downcast_ref::<SymbolicRenderingEventListener>()
            .unwrap();
        assert_eq!(symbolic_listener.branch_count(), 0);
    }

    #[test]
    fn two_tainted_decisions_get_sequential_ordinals_overridable_independently() {
        let env = Environment::new();
        let listener: Rc<dyn RenderingEventListener> = Rc::new(
            SymbolicRenderingEventListener::new(true)
                .with_branch_overrides(HashMap::from([(1, false)])),
        );
        let out = env
            .render_str(
                "{% if a %}1{% else %}0{% endif %}-{% if b %}1{% else %}0{% endif %}",
                context! {
                    a => IntrospectiveValue::wrap(Value::from(())),
                    b => IntrospectiveValue::wrap(Value::from(())),
                },
                &[listener],
            )
            .unwrap();
        // First tainted decision (ordinal 0) has no override -> default "then"
        // branch. Second (ordinal 1) is forced to the "else" branch.
        assert_eq!(out, "1-0");
    }
}
