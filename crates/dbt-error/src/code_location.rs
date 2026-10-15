use std::{path::PathBuf, sync::Arc};

use dbt_yaml::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::terminal_hyperlinks::wrap_file_hyperlink;
use crate::utils;

use super::preprocessor_location;

type CodeLocation = dbt_frontend_common::error::CodeLocation;

/// Represents a concrete location in some source file.
#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Hash, JsonSchema)]
pub struct CodeLocationWithFile {
    pub line: u32,
    pub col: u32,
    pub index: u32,
    pub file: Arc<PathBuf>,
    // An optional pointer to a corresponding location in some intermediate
    // preprocessed code, for example after macro expansion. Mainly intended for
    // debugging purposes.
    expanded: Option<Box<CodeLocationWithFile>>,
}

impl From<CodeLocationWithFile> for CodeLocation {
    fn from(location: CodeLocationWithFile) -> Self {
        CodeLocation {
            line: location.line,
            col: location.col,
            index: location.index,
        }
    }
}

impl PartialOrd for CodeLocationWithFile {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for CodeLocationWithFile {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.file
            .cmp(&other.file)
            .then(self.index.cmp(&other.index))
    }
}

impl CodeLocationWithFile {
    /// Constructs a new [CodeLocation] with the specified line, column and file
    /// path.
    pub fn new(line: u32, column: u32, index: u32, file: impl Into<PathBuf>) -> Self {
        CodeLocationWithFile {
            line,
            col: column,
            index,
            file: Arc::new(file.into()),
            expanded: None,
        }
    }

    /// Constructs a new [CodeLocation] with the specified line, column and file
    /// path.
    pub fn new_with_arc(line: u32, column: u32, index: u32, file: impl Into<Arc<PathBuf>>) -> Self {
        CodeLocationWithFile {
            line,
            col: column,
            index,
            file: file.into(),
            expanded: None,
        }
    }

    /// Using the specified span information, maps this location to a
    /// pre-expanded location in the corresponding source file.
    pub fn with_macro_spans(
        self,
        spans: &[preprocessor_location::MacroSpan],
        expanded_file: impl Into<Option<Arc<PathBuf>>>,
    ) -> Self {
        let expanded = expanded_file.into().map(|path| {
            Box::new(CodeLocationWithFile::new_with_arc(
                self.line, self.col, self.index, path,
            ))
        });
        CodeLocationWithFile {
            expanded,
            ..self.get_source_location_with_file(spans)
        }
    }

    /// Whether this code location has line and column number info.
    pub fn has_position(&self) -> bool {
        // 0:0 means unknown location
        self.line != 0 || self.col != 0
    }

    pub fn line_opt(&self) -> Option<u32> {
        if self.line == 0 {
            None
        } else {
            Some(self.line)
        }
    }

    pub fn col_opt(&self) -> Option<u32> {
        if self.col == 0 { None } else { Some(self.col) }
    }

    pub fn get_source_location(
        &self,
        macro_spans: &[preprocessor_location::MacroSpan],
    ) -> CodeLocation {
        let location = self.to_owned().into();

        let mut prev_macro_end = CodeLocation::new(1, 1, 0);
        let mut prev_expanded_end = CodeLocation::new(1, 1, 0);
        for macro_span in macro_spans {
            if macro_span.expanded_span.contains(&location) {
                return macro_span.macro_span.start.to_owned();
            } else if location < macro_span.expanded_span.start {
                return prev_macro_end + (location - prev_expanded_end);
            }
            prev_macro_end.clone_from(&macro_span.macro_span.stop);
            prev_expanded_end.clone_from(&macro_span.expanded_span.stop);
        }
        prev_macro_end + (location.to_owned() - prev_expanded_end)
    }

    pub fn is_in_macro_span(&self, macro_spans: &[preprocessor_location::MacroSpan]) -> bool {
        let location = self.to_owned().into();

        let mut prev_macro_end = CodeLocation::new(1, 1, 0);
        let mut prev_expanded_end = CodeLocation::new(1, 1, 0);
        for macro_span in macro_spans {
            if macro_span.expanded_span.contains(&location) {
                return true;
            } else if location < macro_span.expanded_span.start {
                return false;
            }
            prev_macro_end.clone_from(&macro_span.macro_span.stop);
            prev_expanded_end.clone_from(&macro_span.expanded_span.stop);
        }
        false
    }

    pub fn get_source_location_with_file(
        &self,
        macro_spans: &[preprocessor_location::MacroSpan],
    ) -> CodeLocationWithFile {
        let location = self.to_owned().into();

        let mut prev_macro_end = CodeLocation::new(1, 1, 0);
        let mut prev_expanded_end = CodeLocation::new(1, 1, 0);
        for macro_span in macro_spans {
            if macro_span.expanded_span.contains(&location) {
                let location = macro_span.macro_span.start.to_owned();
                return CodeLocationWithFile::new_with_arc(
                    location.line,
                    location.col,
                    location.index,
                    self.file.clone(),
                );
            } else if location < macro_span.expanded_span.start {
                let location = prev_macro_end + (location - prev_expanded_end);
                return CodeLocationWithFile::new_with_arc(
                    location.line,
                    location.col,
                    location.index,
                    self.file.clone(),
                );
            }
            prev_macro_end.clone_from(&macro_span.macro_span.stop);
            prev_expanded_end.clone_from(&macro_span.expanded_span.stop);
        }
        let location = prev_macro_end + (location.to_owned() - prev_expanded_end);
        CodeLocationWithFile::new_with_arc(
            location.line,
            location.col,
            location.index,
            self.file.clone(),
        )
    }

    pub fn with_file(self, file: Arc<PathBuf>) -> Self {
        CodeLocationWithFile { file, ..self }
    }

    pub fn with_offset(self, offset: CodeLocation) -> Self {
        let line = self.line + offset.line - 1;
        let col = if self.line == 1 {
            self.col + offset.col - 1
        } else {
            self.col
        };
        let index = self.index + offset.index;
        CodeLocationWithFile {
            line,
            col,
            index,
            ..self
        }
    }

    pub fn relative_path(&self) -> PathBuf {
        if self.file.is_relative() {
            self.file.as_ref().to_owned()
        } else if let Ok(cwd) = std::env::current_dir() {
            let cwd = utils::canonicalize(cwd.as_path()).unwrap_or(cwd);
            pathdiff::diff_paths(self.file.as_ref(), &cwd)
                .unwrap_or_else(|| self.file.as_ref().to_owned())
        } else {
            self.file.as_ref().to_owned()
        }
    }

    pub fn expanded(&self) -> Option<&CodeLocationWithFile> {
        self.expanded.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn with_expanded_location(self, expanded: CodeLocationWithFile) -> Self {
        CodeLocationWithFile {
            expanded: Some(Box::new(expanded)),
            ..self
        }
    }

    /// Formats this location as `path`, `path:line`, or `path:line:col`.
    fn displayed_span(&self) -> String {
        let relative_path = self.relative_path();
        if !self.has_position() {
            relative_path.display().to_string()
        } else if self.col == 0 {
            format!("{}:{}", relative_path.display(), self.line)
        } else {
            format!("{}:{}:{}", relative_path.display(), self.line, self.col)
        }
    }
}

impl From<PathBuf> for CodeLocationWithFile {
    fn from(file: PathBuf) -> Self {
        CodeLocationWithFile {
            file: Arc::new(file),
            ..Default::default()
        }
    }
}

impl From<dbt_yaml::Span> for CodeLocationWithFile {
    fn from(span: dbt_yaml::Span) -> Self {
        CodeLocationWithFile::new_with_arc(
            span.start.line as u32,
            span.start.column as u32,
            span.start.index as u32,
            span.filename
                .unwrap_or_else(|| PathBuf::from("<unknown>").into()),
        )
    }
}

pub struct MiniJinjaErrorWrapper(pub minijinja::Error);

impl From<MiniJinjaErrorWrapper> for CodeLocationWithFile {
    fn from(err: MiniJinjaErrorWrapper) -> Self {
        if let Some(span) = err.0.significant_span() {
            CodeLocationWithFile {
                file: Arc::new(err.0.significant_name().unwrap_or_default().into()),
                line: span.start_line,
                col: span.start_col,
                index: span.start_offset,
                expanded: None,
            }
        } else {
            CodeLocationWithFile {
                file: Arc::new(err.0.name().unwrap_or_default().into()),
                ..Default::default()
            }
        }
    }
}

impl std::fmt::Display for CodeLocationWithFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            wrap_file_hyperlink(self.file.as_ref(), &self.displayed_span())
        )?;
        if let Some(expanded) = &self.expanded {
            write!(f, " ({expanded})")?;
        }
        Ok(())
    }
}

/// A location without an associate file path.
///
/// Can be converted to a concrete [CodeLocation] by calling
/// [AbstractLocation::with_file].
pub trait AbstractLocation {
    fn with_file(&self, file: impl Into<PathBuf>) -> CodeLocationWithFile;
    fn with_arc_file(&self, file: impl Into<Arc<PathBuf>>) -> CodeLocationWithFile;
}

impl AbstractLocation for dbt_frontend_common::error::CodeLocation {
    fn with_file(&self, file: impl Into<PathBuf>) -> CodeLocationWithFile {
        self.with_arc_file(Arc::new(file.into()))
    }
    fn with_arc_file(&self, file: impl Into<Arc<PathBuf>>) -> CodeLocationWithFile {
        CodeLocationWithFile::new_with_arc(self.line, self.col, self.index, file.into())
    }
}

impl AbstractLocation for (u32, u32, u32) {
    fn with_file(&self, file: impl Into<PathBuf>) -> CodeLocationWithFile {
        self.with_arc_file(Arc::new(file.into()))
    }
    fn with_arc_file(&self, file: impl Into<Arc<PathBuf>>) -> CodeLocationWithFile {
        CodeLocationWithFile::new_with_arc(self.0, self.1, self.2, file.into())
    }
}

trait CodeLocationExtension {
    fn with_macro_spans(self, spans: &[preprocessor_location::MacroSpan]) -> CodeLocation;
    fn get_source_location(&self, macro_spans: &[preprocessor_location::MacroSpan])
    -> CodeLocation;
}

impl CodeLocationExtension for CodeLocation {
    /// Using the specified span information.
    fn with_macro_spans(self, spans: &[preprocessor_location::MacroSpan]) -> Self {
        self.get_source_location(spans)
    }

    fn get_source_location(
        &self,
        macro_spans: &[preprocessor_location::MacroSpan],
    ) -> CodeLocation {
        let location = self.to_owned();

        let mut prev_macro_end = dbt_frontend_common::error::CodeLocation::new(1, 1, 0);
        let mut prev_expanded_end = dbt_frontend_common::error::CodeLocation::new(1, 1, 0);
        for macro_span in macro_spans {
            if macro_span.expanded_span.contains(&location) {
                return macro_span.macro_span.start.to_owned();
            } else if location < macro_span.expanded_span.start {
                return prev_macro_end + (location - prev_expanded_end);
            }
            prev_macro_end.clone_from(&macro_span.macro_span.stop);
            prev_expanded_end.clone_from(&macro_span.expanded_span.stop);
        }
        prev_macro_end + (location.to_owned() - prev_expanded_end)
    }
}

#[derive(Clone, Default, Debug, Eq, PartialEq, Serialize, Deserialize, Hash, PartialOrd, Ord)]
pub struct Span {
    pub start: CodeLocationWithFile,
    pub stop: CodeLocation,
}

impl Span {
    pub fn with_macro_spans(
        self,
        spans: &[preprocessor_location::MacroSpan],
        expanded_file: Option<Arc<PathBuf>>,
    ) -> Self {
        Span {
            start: self.start.with_macro_spans(spans, expanded_file),
            stop: self.stop.with_macro_spans(spans),
        }
    }

    pub fn with_offset(self, offset: CodeLocation) -> Self {
        Span {
            start: self.start.with_offset(offset),
            stop: self.stop.with_offset(&offset),
        }
    }

    pub fn from_serde_span(span: dbt_yaml::Span, file: impl Into<PathBuf>) -> Self {
        Span {
            start: CodeLocationWithFile::new(
                span.start.line as u32,
                span.start.column as u32,
                span.start.index as u32,
                file,
            ),
            stop: CodeLocation::new(
                span.end.line as u32,
                span.end.column as u32,
                span.end.index as u32,
            ),
        }
    }

    pub fn is_valid(&self) -> bool {
        self.start.has_position() && self.stop.has_position()
    }
}

pub trait AbstractSpan {
    fn with_file(&self, file: impl Into<PathBuf>) -> Span;
    fn with_arc_file(&self, file: impl Into<Arc<PathBuf>>) -> Span;
}

impl AbstractSpan for dbt_frontend_common::span::Span {
    fn with_file(&self, file: impl Into<PathBuf>) -> Span {
        self.with_arc_file(Arc::new(file.into()))
    }
    fn with_arc_file(&self, file: impl Into<Arc<PathBuf>>) -> Span {
        Span {
            start: self.start.with_arc_file(file),
            stop: self.stop,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_hyperlinks::{strip_osc8_hyperlinks, with_terminal_hyperlinks};

    #[test]
    fn display_is_plain_without_hyperlinks() {
        let loc = CodeLocationWithFile::new(43, 9, 100, "models/foo.sql");
        assert_eq!(loc.to_string(), loc.displayed_span());
        with_terminal_hyperlinks(false, || {
            assert_eq!(loc.to_string(), loc.displayed_span());
        });
    }

    #[test]
    fn display_wraps_path_in_osc8_when_enabled() {
        let loc = CodeLocationWithFile::new(43, 9, 100, "models/foo.sql");
        let plain = loc.displayed_span();
        with_terminal_hyperlinks(true, || {
            let rendered = loc.to_string();
            assert!(
                rendered.contains("\x1b]8;;file://"),
                "expected OSC 8 file URI, got {rendered:?}"
            );
            assert!(rendered.contains(&plain));
            assert_eq!(strip_osc8_hyperlinks(&rendered).as_ref(), plain);
        });
    }

    #[test]
    fn display_hyperlinks_expanded_location_separately() {
        let loc = CodeLocationWithFile::new(43, 9, 100, "models/foo.sql").with_expanded_location(
            CodeLocationWithFile::new(44, 19, 0, "target/compiled/models/foo.sql"),
        );
        let expected_plain = format!(
            "{} ({})",
            CodeLocationWithFile::new(43, 9, 100, "models/foo.sql").displayed_span(),
            CodeLocationWithFile::new(44, 19, 0, "target/compiled/models/foo.sql").displayed_span()
        );
        with_terminal_hyperlinks(true, || {
            let rendered = loc.to_string();
            let file_uri_count = rendered.matches("\x1b]8;;file://").count();
            assert_eq!(
                file_uri_count, 2,
                "source and expanded locations should each be hyperlinked, got {rendered:?}"
            );
            assert_eq!(strip_osc8_hyperlinks(&rendered).as_ref(), expected_plain);
        });
    }

    #[test]
    fn display_omits_column_when_zero() {
        let loc = CodeLocationWithFile::new(12, 0, 0, "models/foo.sql");
        with_terminal_hyperlinks(false, || {
            assert_eq!(loc.to_string(), loc.displayed_span());
            assert!(
                !loc.displayed_span().contains(":12:"),
                "column 0 should be omitted, got {}",
                loc.displayed_span()
            );
        });
    }
}
