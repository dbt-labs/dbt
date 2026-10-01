//! Parse-time listener that warns when a `{% snapshot %}`/`{% docs %}` block
//! name is followed by a non-empty suffix dbt-core silently discards.

use std::path::PathBuf;

use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, fs_err};
use minijinja::listener::{BlockNameKind, TokenizerEventListener};
use minijinja::machinery::{Span, Token};

/// Emits a deprecation warning for malformed `{% snapshot %}`/`{% docs %}`
/// block names via the parser's `on_malformed_block_name` event.
#[derive(Debug)]
pub struct MalformedBlockNameListener {
    path: PathBuf,
}

impl MalformedBlockNameListener {
    /// Creates a new listener for the given source file path and IO args.
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn block_label(kind: BlockNameKind) -> &'static str {
        match kind {
            BlockNameKind::Snapshot => "Snapshot",
            BlockNameKind::Docs => "Docs",
        }
    }

    fn warning(&self, kind: BlockNameKind, name: &str, name_span: &Span) -> FsError {
        let location = CodeLocationWithFile::new(
            name_span.start_line,
            name_span.start_col,
            name_span.start_offset,
            self.path.clone(),
        );
        *fs_err!(
            code => ErrorCode::MalformedBlockName,
            loc => location,
            "{} block name `{}` is followed by extra characters that dbt \
             silently ignores. Use a name that is a single valid identifier. \
             This will become an error in a future release.",
            Self::block_label(kind),
            name,
        )
    }
}

impl TokenizerEventListener for MalformedBlockNameListener {
    fn on_source_token(&self, _token: &Token<'_>, _span: &Span) {}

    fn on_malformed_block_name(&self, kind: BlockNameKind, name: &str, name_span: &Span) {
        emit_warn_log_from_fs_error(self.warning(kind, name, name_span));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn warning_for(kind: BlockNameKind, name: &str) -> FsError {
        let listener = MalformedBlockNameListener::new(PathBuf::from("test.sql"));
        let span = Span {
            start_line: 3,
            start_col: 5,
            start_offset: 17,
            ..Span::default()
        };
        listener.warning(kind, name, &span)
    }

    #[test]
    fn malformed_snapshot_builds_warning() {
        let warning = warning_for(BlockNameKind::Snapshot, "stg_crm");

        assert_eq!(warning.code, ErrorCode::MalformedBlockName);
        assert_eq!(
            warning.location,
            Some(CodeLocationWithFile::new(3, 5, 17, "test.sql"))
        );
        assert!(
            warning
                .context
                .starts_with("Snapshot block name `stg_crm` is followed")
        );
    }

    #[test]
    fn malformed_docs_builds_warning() {
        let warning = warning_for(BlockNameKind::Docs, "my_doc");

        assert_eq!(warning.code, ErrorCode::MalformedBlockName);
        assert!(
            warning
                .context
                .starts_with("Docs block name `my_doc` is followed")
        );
    }
}
