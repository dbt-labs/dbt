//! Experimentals that add their own kind of node to a project.
//!
//! An experimental reads its definitions under its keys: its type, and any other keys it reads for
//! definitions that aren't nodes. Each key works in both of two places:
//!
//! - **`experimental:` in properties files.** Entries under the key:
//!
//!   ```yaml
//!   experimental:
//!     widget:
//!       - name: orders_widget
//!         depends_on: [ref('orders')]
//!         part: bolt
//!     part:
//!       - name: bolt
//!   ```
//! - **Experimental paths.** Directories a project names for the key in its `dbt_project.yml`,
//!   whose files hold definitions of that key's kind:
//!
//!   ```yaml
//!   experimental-paths:
//!     widget: ["widgets"]
//!     part: ["parts"]
//!   ```
//!
//! The experimental resolves one package at a time, seeing all of the package's entries and files
//! at once. It decides what each one deserializes to, its `config:` included, and returns the nodes
//! they define. dbt resolves each node's `depends_on` with the project's own `ref()`, `source()`
//! and `metric()` rules. The experimental's own data for a node is opaque to dbt.
//!
//! An experimental is off unless its feature flag is on. A reserved key is never released: entries
//! or `experimental-paths` for an experimental that is off, or no longer exists, are ignored with a
//! warning, never treated as an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::io_args::project_flag_bool;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, fs_err};
use dbt_flags::FlagDef;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::{from_yaml_raw, into_typed_with_jinja};
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::ExperimentalData;
use dbt_yaml::Spanned;
use indexmap::IndexMap;
use minijinja::value::Value as MinijinjaValue;
use serde::de::DeserializeOwned;

/// Adds nodes of its own kind to a project.
pub trait ResolveExperimental: Send + Sync {
    /// The kind of node the experimental adds, e.g. `widget`: its key under `experimental:` and
    /// `experimental-paths:`, the prefix of its nodes' unique ids
    /// (`<experimental type>.<package>.<name>`), and a value `resource_type:` selects. It must be
    /// among the reserved keys the experimental is registered with.
    fn experimental_type(&self) -> &'static str;

    /// Other keys the experimental reads, under `experimental:` and `experimental-paths:`, for
    /// definitions that aren't nodes of their own but that its nodes name (e.g. `part`). Each must
    /// be reserved, like an experimental type.
    fn definition_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// The feature flag that turns the experimental on. It defaults to off.
    fn flag(&self) -> &'static FlagDef;

    /// Turns one package's entries and files into node definitions.
    ///
    /// A definition the experimental rejects should be reported with
    /// [`ExperimentalDiagnostics::error`] and get no node; an error returned here rejects the whole
    /// package's.
    fn resolve(
        &self,
        package: &ExperimentalPackage<'_>,
        diagnostics: &mut ExperimentalDiagnostics,
    ) -> Result<Vec<ExperimentalNodeDef>, ExperimentalError>;
}

/// What one package gives an experimental, by key: its entries under `experimental: <key>:` in
/// properties files, and its files under `experimental-paths: {<key>: [...]}`.
pub struct ExperimentalPackage<'a> {
    /// The package's name.
    pub package_name: &'a str,
    pub(crate) entries: BTreeMap<&'static str, Vec<ExperimentalEntry>>,
    pub(crate) files: BTreeMap<&'static str, Vec<ExperimentalFile>>,
    /// What entries and files are rendered with; `None` reads them as written.
    pub(crate) jinja: Option<(&'a JinjaEnv, &'a BTreeMap<String, MinijinjaValue>)>,
}

impl<'a> ExperimentalPackage<'a> {
    /// A package whose entries and files are read as written, without rendering Jinja: for a
    /// experimental's unit tests.
    pub fn unrendered(
        package_name: &'a str,
        entries: BTreeMap<&'static str, Vec<ExperimentalEntry>>,
        files: BTreeMap<&'static str, Vec<ExperimentalFile>>,
    ) -> Self {
        Self {
            package_name,
            entries,
            files,
            jinja: None,
        }
    }

    /// The entries under `experimental: <key>:`, one per name, ordered by name. `key` is the
    /// experimental's type or one of its definition keys.
    pub fn entries(&self, key: &str) -> &[ExperimentalEntry] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }

    /// The files under `experimental-paths: {<key>: [...]}`, in path order, hidden ones skipped.
    /// `key` is the experimental's type or one of its definition keys.
    pub fn files(&self, key: &str) -> &[ExperimentalFile] {
        self.files.get(key).map_or(&[], Vec::as_slice)
    }

    /// `entry` as a `T`, rendered as dbt renders properties: fields typed `Verbatim<_>` stay as
    /// written. Keys `T` doesn't read are reported as in any properties file, and the rest still
    /// deserializes.
    ///
    /// A field inside an enum that serde tags (`#[serde(tag = ...)]` or `untagged`) is always
    /// rendered, `Verbatim` or not: serde buffers the enum before its fields deserialize. To keep
    /// such a field as written, take the value as `Verbatim<dbt_yaml::Value>` and deserialize it
    /// with [`Self::deserialize_value`] once its variant is known.
    pub fn deserialize<T: DeserializeOwned>(
        &self,
        entry: &ExperimentalEntry,
    ) -> Result<T, ExperimentalError> {
        self.render(entry.value.clone(), &entry.path)
    }

    /// `value`, part of an entry or file at `path`, as a `T`, rendered as [`Self::deserialize`]
    /// renders an entry.
    pub fn deserialize_value<T: DeserializeOwned>(
        &self,
        value: &dbt_yaml::Value,
        path: &Path,
    ) -> Result<T, ExperimentalError> {
        self.render(value.clone(), path)
    }

    /// The YAML in `file` as a `T`, rendered as [`Self::deserialize`] renders an entry.
    pub fn read_file<T: DeserializeOwned>(
        &self,
        file: &ExperimentalFile,
    ) -> Result<T, ExperimentalError> {
        let text = std::fs::read_to_string(&file.absolute_path).map_err(|e| {
            ExperimentalError::new(format!("Couldn't read {}: {e}", file.path.display()))
        })?;
        let value: dbt_yaml::Value = from_yaml_raw(
            &text,
            Some(&file.path),
            true,
            self.jinja
                .and_then(|(env, base_ctx)| dependency_package_name_from_ctx(env, base_ctx)),
        )
        .map_err(|e| ExperimentalError::from_fs_error(*e, &file.path))?;
        self.render(value, &file.path)
    }

    fn render<T: DeserializeOwned>(
        &self,
        value: dbt_yaml::Value,
        path: &Path,
    ) -> Result<T, ExperimentalError> {
        let Some((env, base_ctx)) = self.jinja else {
            return dbt_yaml::from_value(value).map_err(|e| {
                let error = ExperimentalError::new(e.display_no_mark().to_string());
                match e.span() {
                    Some(span) => error.at(CodeLocationWithFile::new_with_arc(
                        span.start.line as u32,
                        span.start.column as u32,
                        span.start.index as u32,
                        path.to_path_buf(),
                    )),
                    None => error,
                }
            });
        };
        into_typed_with_jinja(
            value,
            false,
            env,
            base_ctx,
            &[],
            dependency_package_name_from_ctx(env, base_ctx),
            true,
        )
        .map_err(|e| ExperimentalError::from_fs_error(*e, path))
    }
}

/// One entry under one of an experimental's keys in a properties file.
#[derive(Debug, Clone)]
pub struct ExperimentalEntry {
    /// The entry's `name`.
    pub name: String,
    /// The properties file, relative to its package's root.
    pub path: PathBuf,
    /// The entry as written: unrendered, with its spans.
    pub value: dbt_yaml::Value,
}

impl ExperimentalEntry {
    /// Where `value`, the entry or a value in it, is in its file.
    pub fn location_of(&self, value: &dbt_yaml::Value) -> CodeLocationWithFile {
        let start = &value.span().start;
        CodeLocationWithFile::new_with_arc(
            start.line as u32,
            start.column as u32,
            start.index as u32,
            self.path.to_path_buf(),
        )
    }
}

/// A file under one of a package's `experimental-paths` for one of an experimental's keys.
#[derive(Debug, Clone)]
pub struct ExperimentalFile {
    /// The file, relative to its package's root.
    pub path: PathBuf,
    /// The file on disk.
    pub absolute_path: PathBuf,
}

/// A node an experimental defines.
#[derive(Debug, Default)]
pub struct ExperimentalNodeDef {
    /// The node's name. Its unique id is `<experimental type>.<package>.<name>`.
    pub name: String,
    /// The file that defines the node, relative to its package's root: a properties file or a
    /// file under `experimental-paths`.
    pub path: PathBuf,
    /// The node's description.
    pub description: Option<String>,
    /// Whether the node is enabled.
    pub enabled: bool,
    /// The node's tags, for `tag:` selection.
    pub tags: Vec<String>,
    /// The node's `meta`.
    pub meta: IndexMap<String, dbt_yaml::Value>,
    /// `ref()`, `source()` and `metric()` calls the node depends on.
    pub depends_on: Vec<Spanned<String>>,
    /// The experimental's own data for the node.
    pub data: ExperimentalData,
}

/// A problem an experimental reports. dbt reports it under the experimental codes
/// ([`ErrorCode::ExperimentalError`], [`ErrorCode::ExperimentalWarning`]), never another.
#[derive(Debug, Clone)]
pub struct ExperimentalError {
    message: String,
    location: Option<CodeLocationWithFile>,
}

impl ExperimentalError {
    /// A problem with no location.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            location: None,
        }
    }

    /// What went wrong.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The same problem, at `location`.
    pub fn at(mut self, location: CodeLocationWithFile) -> Self {
        self.location = Some(location);
        self
    }

    /// `error`, which dbt raised reading `path`, as an experimental's problem.
    pub(crate) fn from_fs_error(error: FsError, path: &Path) -> Self {
        match error.location {
            Some(location) => Self::new(error.context).at(location),
            None => Self::new(format!("{}: {}", path.display(), error.context)),
        }
    }

    /// As a dbt error under `code`, naming the experimental that reported it.
    pub fn into_fs_error(self, experimental_type: &str, code: ErrorCode) -> Box<FsError> {
        match self.location {
            Some(location) => fs_err!(
                code => code,
                loc => location,
                "{experimental_type}: {}",
                self.message
            ),
            None => fs_err!(code, "{experimental_type}: {}", self.message),
        }
    }
}

/// Errors and warnings an experimental reports while it carries on.
#[derive(Debug, Default)]
pub struct ExperimentalDiagnostics {
    pub(crate) errors: Vec<ExperimentalError>,
    pub(crate) warnings: Vec<ExperimentalError>,
}

impl ExperimentalDiagnostics {
    /// Reports an error. The invocation fails once it finishes.
    pub fn error(&mut self, error: ExperimentalError) {
        self.errors.push(error);
    }

    /// Reports a warning.
    pub fn warn(&mut self, warning: ExperimentalError) {
        self.warnings.push(warning);
    }

    /// The errors reported so far.
    pub fn errors(&self) -> &[ExperimentalError] {
        &self.errors
    }

    /// The warnings reported so far.
    pub fn warnings(&self) -> &[ExperimentalError] {
        &self.warnings
    }
}

/// The experimentals a project is resolved with, and every experimental type ever reserved.
#[derive(Clone, Default)]
pub struct ResolveExperimentals {
    /// The registered experimentals, on or off.
    pub experimentals: Vec<Arc<dyn ResolveExperimental>>,
    /// Every reserved experimental type, including those of experimentals that are not registered.
    pub reserved: Vec<&'static str>,
}

impl ResolveExperimentals {
    /// Sorts the experimentals by whether their flag is on, reading the root project's `flags:`.
    pub(crate) fn turned_on(self, project_flags: Option<&dbt_yaml::Value>) -> ActiveExperimentals {
        let (on, off) = self.experimentals.into_iter().partition(|experimental| {
            experimental
                .flag()
                .resolve_with_project(&project_flag_bool(project_flags))
                .value
        });
        ActiveExperimentals {
            on,
            off,
            reserved: self.reserved,
        }
    }
}

/// One invocation's experimentals: the ones whose flag is on, and every reserved experimental type.
pub struct ActiveExperimentals {
    pub(crate) on: Vec<Arc<dyn ResolveExperimental>>,
    off: Vec<Arc<dyn ResolveExperimental>>,
    reserved: Vec<&'static str>,
}

impl ActiveExperimentals {
    /// The experimental that is on and reads `key`, under `experimental:` and
    /// `experimental-paths:`: its type or a definition key.
    pub(crate) fn reading(&self, key: &str) -> Option<&Arc<dyn ResolveExperimental>> {
        self.on
            .iter()
            .find(|experimental| reads(experimental.as_ref(), key))
    }

    /// Whether `key` is reserved.
    pub(crate) fn is_reserved(&self, key: &str) -> bool {
        self.reserved.contains(&key)
    }

    /// Why what a project gives a reserved `key` isn't read.
    pub(crate) fn why_ignored(&self, key: &str) -> String {
        match self
            .off
            .iter()
            .find(|experimental| reads(experimental.as_ref(), key))
        {
            Some(experimental) => format!(
                "the `{}` experimental is off; turn it on with the `{}` feature flag",
                experimental.experimental_type(),
                experimental.flag().name
            ),
            None => format!("this dbt has no `{key}` experimental"),
        }
    }
}

/// Whether `experimental` reads `key`.
fn reads(experimental: &dyn ResolveExperimental, key: &str) -> bool {
    keys(experimental).any(|own| own == key)
}

/// The keys `experimental` reads: its type, then its definition keys.
pub(crate) fn keys(experimental: &dyn ResolveExperimental) -> impl Iterator<Item = &'static str> {
    std::iter::once(experimental.experimental_type())
        .chain(experimental.definition_keys().iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_flags::{Remote, Stage};

    static WIDGETS_FLAG: FlagDef = FlagDef {
        name: "test_widgets",
        description: "Widgets, for testing experimentals.",
        stage: Stage::Preview,
        default: false,
        remote: Remote::Never,
        aliases: &[],
        legacy_env: &[],
        tracking: None,
        remove_by: None,
        docs_url: None,
    };

    struct Widgets;

    impl ResolveExperimental for Widgets {
        fn experimental_type(&self) -> &'static str {
            "widget"
        }
        fn definition_keys(&self) -> &'static [&'static str] {
            &["part"]
        }
        fn flag(&self) -> &'static FlagDef {
            &WIDGETS_FLAG
        }
        fn resolve(
            &self,
            _package: &ExperimentalPackage<'_>,
            _diagnostics: &mut ExperimentalDiagnostics,
        ) -> Result<Vec<ExperimentalNodeDef>, ExperimentalError> {
            Ok(vec![])
        }
    }

    fn experimentals(project_flags: &str) -> ActiveExperimentals {
        let project_flags: dbt_yaml::Value = dbt_yaml::from_str(project_flags).unwrap();
        ResolveExperimentals {
            experimentals: vec![Arc::new(Widgets)],
            reserved: vec!["widget", "part", "gadget"],
        }
        .turned_on(Some(&project_flags))
    }

    /// An experimental is off unless its flag is on, here in the project's `flags:`.
    #[test]
    fn experimentals_are_on_only_when_their_flag_is() {
        assert!(experimentals("{}").reading("widget").is_none());
        assert!(
            experimentals("test_widgets: true")
                .reading("widget")
                .is_some()
        );
    }

    /// An experimental reads its type and its definition keys, and nothing else.
    #[test]
    fn experimentals_read_their_definition_keys() {
        let experimentals = experimentals("test_widgets: true");
        assert!(experimentals.reading("widget").is_some());
        assert!(experimentals.reading("part").is_some());
        assert!(experimentals.reading("gadget").is_none());
    }

    /// An unrendered package reads entries, and parts of them, as written.
    #[test]
    fn unrendered_packages_deserialize_as_written() {
        #[derive(serde::Deserialize)]
        struct Part {
            name: String,
            size: dbt_yaml::Value,
        }
        let value: dbt_yaml::Value = dbt_yaml::from_str("name: bolt\nsize: \"{{ 3 }}\"\n").unwrap();
        let entry = ExperimentalEntry {
            name: "bolt".to_string(),
            path: PathBuf::from("models/parts.yml"),
            value,
        };
        let package = ExperimentalPackage::unrendered(
            "widgets",
            BTreeMap::from([("part", vec![entry.clone()])]),
            BTreeMap::new(),
        );
        let part: Part = package.deserialize(&package.entries("part")[0]).unwrap();
        assert_eq!(part.name, "bolt");
        let size: String = package.deserialize_value(&part.size, &entry.path).unwrap();
        assert_eq!(size, "{{ 3 }}");
        let error = package
            .deserialize_value::<u64>(&part.size, &entry.path)
            .unwrap_err();
        assert!(
            error.message().contains("invalid type"),
            "{}",
            error.message()
        );
    }

    /// The reason a reserved experimental type is ignored names what to do.
    #[test]
    fn ignored_experimental_types_say_why() {
        let experimentals = experimentals("{}");
        assert!(experimentals.is_reserved("gadget"));
        assert!(!experimentals.is_reserved("gizmo"));
        assert_eq!(
            experimentals.why_ignored("widget"),
            "the `widget` experimental is off; turn it on with the `test_widgets` feature flag"
        );
        // A definition key names the experimental that reads it.
        assert_eq!(
            experimentals.why_ignored("part"),
            "the `widget` experimental is off; turn it on with the `test_widgets` feature flag"
        );
        assert_eq!(
            experimentals.why_ignored("gadget"),
            "this dbt has no `gadget` experimental"
        );
    }
}
