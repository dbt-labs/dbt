use dbt_common::{CodeLocationWithFile, ErrorCode, FsError};
use minijinja::TypecheckingEventListener;
use minijinja::machinery::Span;
use std::cell::RefCell;
use std::path::PathBuf;

/// A side-effect-free typechecking listener that collects YAML diagnostics.
///
/// Collected diagnostics can be accessed with [`Self::drain_diagnostics`] after typechecking.
pub struct YamlTypecheckingEventListener {
    current_path: PathBuf,
    current_span: Span,
    diagnostics: RefCell<Vec<FsError>>,
}

impl YamlTypecheckingEventListener {
    /// Creates a new YamlTypecheckingEventListener
    ///
    /// # Arguments
    ///
    /// * `current_path` - The path to the current file being typechecked
    /// * `current_span` - The span context for error reporting
    pub fn new(current_path: PathBuf, current_span: Span) -> Self {
        Self {
            current_path,
            current_span,
            diagnostics: RefCell::new(Vec::new()),
        }
    }

    /// Drains and returns the collected diagnostics in reporting order.
    pub fn drain_diagnostics(&mut self) -> Vec<FsError> {
        self.diagnostics.get_mut().drain(..).collect()
    }
}

impl TypecheckingEventListener for YamlTypecheckingEventListener {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    // Avoid normal warning getting to user when we typecheck yaml
    fn warn(&self, _message: &str) {}

    fn warn_filter(&self, message: &str) {
        // Adjust span with current_span offset
        let adjusted_span = self.current_span;

        let location = CodeLocationWithFile::new(
            adjusted_span.start_line,
            adjusted_span.start_col,
            adjusted_span.start_offset,
            self.current_path.clone(),
        );

        let diagnostic =
            FsError::new(ErrorCode::JinjaError, message.to_string()).with_location(location);
        self.diagnostics.borrow_mut().push(diagnostic);
    }

    fn set_span(&self, _span: &Span) {
        // Span is already set in the listener
    }

    fn new_block(&self, _block_id: usize) {
        // Not needed for YAML typechecking
    }

    fn on_model_reference(
        &self,
        _name: &str,
        _identifier_span: &Span,
        _start_line: &u32,
        _start_col: &u32,
        _start_offset: &u32,
        _end_line: &u32,
        _end_col: &u32,
        _end_offset: &u32,
    ) {
        // Not needed for YAML typechecking
    }

    fn flush(&self) {
        // Not needed for YAML typechecking
    }

    fn on_lookup(&self, _span: &Span, _name: &str, _kind: &str, _dependencies: Vec<Span>) {
        // Not needed for YAML typechecking
    }
}
