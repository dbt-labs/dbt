//! Plugins that add their own kind of node to a project.
//!
//! A plugin reads its definitions under its keys: its type, and any other keys it reads for
//! definitions that aren't nodes. Each key works in both of two places:
//!
//! - **`plugins:` in properties files.** Entries under the key:
//!
//!   ```yaml
//!   plugins:
//!     widget:
//!       - name: orders_widget
//!         depends_on: [ref('orders')]
//!         part: bolt
//!     part:
//!       - name: bolt
//!   ```
//! - **Plugin paths.** Directories a project names for the key in its `dbt_project.yml`, whose
//!   files hold definitions of that key's kind:
//!
//!   ```yaml
//!   plugin-paths:
//!     widget: ["widgets"]
//!     part: ["parts"]
//!   ```
//!
//! The plugin resolves one package at a time, seeing all of the package's entries and files at
//! once. It decides what each one deserializes to, its `config:` included, and returns the nodes
//! they define. dbt resolves each node's `depends_on` with the project's own `ref()`, `source()`
//! and `metric()` rules. The plugin's own data for a node is opaque to dbt.
//!
//! A plugin is off unless its feature flag is on. A reserved key is never released: entries or
//! `plugin-paths` for a plugin that is off, or no longer exists, are ignored with a warning,
//! never treated as an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::io_args::project_flag_bool;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, fs_err};
use dbt_flags::FlagDef;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::{from_yaml_raw, into_typed_with_jinja};
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::PluginData;
use dbt_yaml::Spanned;
use indexmap::IndexMap;
use minijinja::value::Value as MinijinjaValue;
use serde::de::DeserializeOwned;

/// Adds nodes of its own kind to a project.
pub trait ResolvePlugin: Send + Sync {
    /// The kind of node the plugin adds, e.g. `widget`: its key under `plugins:` and
    /// `plugin-paths:`, the prefix of its nodes' unique ids (`<plugin type>.<package>.<name>`), and
    /// a value `resource_type:` selects. It must be among the reserved keys the plugin is
    /// registered with.
    fn plugin_type(&self) -> &'static str;

    /// Other keys the plugin reads, under `plugins:` and `plugin-paths:`, for definitions that
    /// aren't nodes of their own but that its nodes name (e.g. `part`). Each must be reserved,
    /// like a plugin type.
    fn definition_keys(&self) -> &'static [&'static str] {
        &[]
    }

    /// The feature flag that turns the plugin on. It defaults to off.
    fn flag(&self) -> &'static FlagDef;

    /// Turns one package's entries and files into node definitions.
    ///
    /// A definition the plugin rejects should be reported with [`PluginDiagnostics::error`] and
    /// get no node; an error returned here rejects the whole package's.
    fn resolve(
        &self,
        package: &PluginPackage<'_>,
        diagnostics: &mut PluginDiagnostics,
    ) -> Result<Vec<PluginNodeDef>, PluginError>;
}

/// What one package gives a plugin, by key: its entries under `plugins: <key>:` in properties
/// files, and its files under `plugin-paths: {<key>: [...]}`.
pub struct PluginPackage<'a> {
    /// The package's name.
    pub package_name: &'a str,
    pub(crate) entries: BTreeMap<&'static str, Vec<PluginEntry>>,
    pub(crate) files: BTreeMap<&'static str, Vec<PluginFile>>,
    /// What entries and files are rendered with; `None` reads them as written.
    pub(crate) jinja: Option<(&'a JinjaEnv, &'a BTreeMap<String, MinijinjaValue>)>,
}

impl<'a> PluginPackage<'a> {
    /// A package whose entries and files are read as written, without rendering Jinja: for a
    /// plugin's unit tests.
    pub fn unrendered(
        package_name: &'a str,
        entries: BTreeMap<&'static str, Vec<PluginEntry>>,
        files: BTreeMap<&'static str, Vec<PluginFile>>,
    ) -> Self {
        Self {
            package_name,
            entries,
            files,
            jinja: None,
        }
    }

    /// The entries under `plugins: <key>:`, one per name, ordered by name. `key` is the plugin's
    /// type or one of its definition keys.
    pub fn entries(&self, key: &str) -> &[PluginEntry] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
    }

    /// The files under `plugin-paths: {<key>: [...]}`, in path order, hidden ones skipped. `key`
    /// is the plugin's type or one of its definition keys.
    pub fn files(&self, key: &str) -> &[PluginFile] {
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
    pub fn deserialize<T: DeserializeOwned>(&self, entry: &PluginEntry) -> Result<T, PluginError> {
        self.render(entry.value.clone(), &entry.path)
    }

    /// `value`, part of an entry or file at `path`, as a `T`, rendered as [`Self::deserialize`]
    /// renders an entry.
    pub fn deserialize_value<T: DeserializeOwned>(
        &self,
        value: &dbt_yaml::Value,
        path: &Path,
    ) -> Result<T, PluginError> {
        self.render(value.clone(), path)
    }

    /// The YAML in `file` as a `T`, rendered as [`Self::deserialize`] renders an entry.
    pub fn read_file<T: DeserializeOwned>(&self, file: &PluginFile) -> Result<T, PluginError> {
        let text = std::fs::read_to_string(&file.absolute_path)
            .map_err(|e| PluginError::new(format!("Couldn't read {}: {e}", file.path.display())))?;
        let value: dbt_yaml::Value = from_yaml_raw(
            &text,
            Some(&file.path),
            true,
            self.jinja
                .and_then(|(env, base_ctx)| dependency_package_name_from_ctx(env, base_ctx)),
        )
        .map_err(|e| PluginError::from_fs_error(*e, &file.path))?;
        self.render(value, &file.path)
    }

    fn render<T: DeserializeOwned>(
        &self,
        value: dbt_yaml::Value,
        path: &Path,
    ) -> Result<T, PluginError> {
        let Some((env, base_ctx)) = self.jinja else {
            return dbt_yaml::from_value(value).map_err(|e| {
                let error = PluginError::new(e.display_no_mark().to_string());
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
        .map_err(|e| PluginError::from_fs_error(*e, path))
    }
}

/// One entry under one of a plugin's keys in a properties file.
#[derive(Debug, Clone)]
pub struct PluginEntry {
    /// The entry's `name`.
    pub name: String,
    /// The properties file, relative to its package's root.
    pub path: PathBuf,
    /// The entry as written: unrendered, with its spans.
    pub value: dbt_yaml::Value,
}

impl PluginEntry {
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

/// A file under one of a package's `plugin-paths` for one of a plugin's keys.
#[derive(Debug, Clone)]
pub struct PluginFile {
    /// The file, relative to its package's root.
    pub path: PathBuf,
    /// The file on disk.
    pub absolute_path: PathBuf,
}

/// A node a plugin defines.
#[derive(Debug, Default)]
pub struct PluginNodeDef {
    /// The node's name. Its unique id is `<plugin type>.<package>.<name>`.
    pub name: String,
    /// The file that defines the node, relative to its package's root: a properties file or a
    /// file under `plugin-paths`.
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
    /// The plugin's own data for the node.
    pub data: PluginData,
}

/// A problem a plugin reports. dbt reports it under the plugin codes
/// ([`ErrorCode::PluginError`], [`ErrorCode::PluginWarning`]), never another.
#[derive(Debug, Clone)]
pub struct PluginError {
    message: String,
    location: Option<CodeLocationWithFile>,
}

impl PluginError {
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

    /// `error`, which dbt raised reading `path`, as a plugin's problem.
    pub(crate) fn from_fs_error(error: FsError, path: &Path) -> Self {
        match error.location {
            Some(location) => Self::new(error.context).at(location),
            None => Self::new(format!("{}: {}", path.display(), error.context)),
        }
    }

    /// As a dbt error under `code`, naming the plugin that reported it.
    pub fn into_fs_error(self, plugin_type: &str, code: ErrorCode) -> Box<FsError> {
        match self.location {
            Some(location) => fs_err!(
                code => code,
                loc => location,
                "{plugin_type}: {}",
                self.message
            ),
            None => fs_err!(code, "{plugin_type}: {}", self.message),
        }
    }
}

/// Errors and warnings a plugin reports while it carries on.
#[derive(Debug, Default)]
pub struct PluginDiagnostics {
    pub(crate) errors: Vec<PluginError>,
    pub(crate) warnings: Vec<PluginError>,
}

impl PluginDiagnostics {
    /// Reports an error. The invocation fails once it finishes.
    pub fn error(&mut self, error: PluginError) {
        self.errors.push(error);
    }

    /// Reports a warning.
    pub fn warn(&mut self, warning: PluginError) {
        self.warnings.push(warning);
    }

    /// The errors reported so far.
    pub fn errors(&self) -> &[PluginError] {
        &self.errors
    }

    /// The warnings reported so far.
    pub fn warnings(&self) -> &[PluginError] {
        &self.warnings
    }
}

/// The plugins a project is resolved with, and every plugin type ever reserved.
#[derive(Clone, Default)]
pub struct ResolvePlugins {
    /// The registered plugins, on or off.
    pub plugins: Vec<Arc<dyn ResolvePlugin>>,
    /// Every reserved plugin type, including those of plugins that are not registered.
    pub reserved: Vec<&'static str>,
}

impl ResolvePlugins {
    /// Sorts the plugins by whether their flag is on, reading the root project's `flags:`.
    pub(crate) fn turned_on(self, project_flags: Option<&dbt_yaml::Value>) -> ActivePlugins {
        let (on, off) = self.plugins.into_iter().partition(|plugin| {
            plugin
                .flag()
                .resolve_with_project(&project_flag_bool(project_flags))
                .value
        });
        ActivePlugins {
            on,
            off,
            reserved: self.reserved,
        }
    }
}

/// One invocation's plugins: the ones whose flag is on, and every reserved plugin type.
pub struct ActivePlugins {
    pub(crate) on: Vec<Arc<dyn ResolvePlugin>>,
    off: Vec<Arc<dyn ResolvePlugin>>,
    reserved: Vec<&'static str>,
}

impl ActivePlugins {
    /// The plugin that is on and reads `key`, under `plugins:` and `plugin-paths:`: its type or
    /// a definition key.
    pub(crate) fn reading(&self, key: &str) -> Option<&Arc<dyn ResolvePlugin>> {
        self.on.iter().find(|plugin| reads(plugin.as_ref(), key))
    }

    /// Whether `key` is reserved.
    pub(crate) fn is_reserved(&self, key: &str) -> bool {
        self.reserved.contains(&key)
    }

    /// Why what a project gives a reserved `key` isn't read.
    pub(crate) fn why_ignored(&self, key: &str) -> String {
        match self.off.iter().find(|plugin| reads(plugin.as_ref(), key)) {
            Some(plugin) => format!(
                "the `{}` plugin is off; turn it on with the `{}` feature flag",
                plugin.plugin_type(),
                plugin.flag().name
            ),
            None => format!("this dbt has no `{key}` plugin"),
        }
    }
}

/// Whether `plugin` reads `key`.
fn reads(plugin: &dyn ResolvePlugin, key: &str) -> bool {
    keys(plugin).any(|own| own == key)
}

/// The keys `plugin` reads: its type, then its definition keys.
pub(crate) fn keys(plugin: &dyn ResolvePlugin) -> impl Iterator<Item = &'static str> {
    std::iter::once(plugin.plugin_type()).chain(plugin.definition_keys().iter().copied())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_flags::{Remote, Stage};

    static WIDGETS_FLAG: FlagDef = FlagDef {
        name: "test_widgets",
        description: "Widgets, for testing plugins.",
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

    impl ResolvePlugin for Widgets {
        fn plugin_type(&self) -> &'static str {
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
            _package: &PluginPackage<'_>,
            _diagnostics: &mut PluginDiagnostics,
        ) -> Result<Vec<PluginNodeDef>, PluginError> {
            Ok(vec![])
        }
    }

    fn plugins(project_flags: &str) -> ActivePlugins {
        let project_flags: dbt_yaml::Value = dbt_yaml::from_str(project_flags).unwrap();
        ResolvePlugins {
            plugins: vec![Arc::new(Widgets)],
            reserved: vec!["widget", "part", "gadget"],
        }
        .turned_on(Some(&project_flags))
    }

    /// A plugin is off unless its flag is on, here in the project's `flags:`.
    #[test]
    fn plugins_are_on_only_when_their_flag_is() {
        assert!(plugins("{}").reading("widget").is_none());
        assert!(plugins("test_widgets: true").reading("widget").is_some());
    }

    /// A plugin reads its type and its definition keys, and nothing else.
    #[test]
    fn plugins_read_their_definition_keys() {
        let plugins = plugins("test_widgets: true");
        assert!(plugins.reading("widget").is_some());
        assert!(plugins.reading("part").is_some());
        assert!(plugins.reading("gadget").is_none());
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
        let entry = PluginEntry {
            name: "bolt".to_string(),
            path: PathBuf::from("models/parts.yml"),
            value,
        };
        let package = PluginPackage::unrendered(
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

    /// The reason a reserved plugin type is ignored names what to do.
    #[test]
    fn ignored_plugin_types_say_why() {
        let plugins = plugins("{}");
        assert!(plugins.is_reserved("gadget"));
        assert!(!plugins.is_reserved("gizmo"));
        assert_eq!(
            plugins.why_ignored("widget"),
            "the `widget` plugin is off; turn it on with the `test_widgets` feature flag"
        );
        // A definition key names the plugin that reads it.
        assert_eq!(
            plugins.why_ignored("part"),
            "the `widget` plugin is off; turn it on with the `test_widgets` feature flag"
        );
        assert_eq!(
            plugins.why_ignored("gadget"),
            "this dbt has no `gadget` plugin"
        );
    }
}
