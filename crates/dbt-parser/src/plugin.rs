//! Plugins that add their own kind of node to a project.
//!
//! A plugin's nodes come from one or both of two sources, each keyed by the plugin's type:
//!
//! - **`plugins:` in properties files.** Entries under the plugin's type:
//!
//!   ```yaml
//!   plugins:
//!     widget:
//!       - name: orders_widget
//!         depends_on: [ref('orders')]
//!   ```
//! - **Plugin paths.** Directories a project names for the plugin in its `dbt_project.yml`:
//!
//!   ```yaml
//!   plugin-paths:
//!     widget: ["widgets"]
//!   ```
//!
//! Either way, dbt resolves each node's `depends_on` with the project's own `ref()`, `source()`
//! and `metric()` rules. The plugin's own data for a node is opaque to dbt.
//!
//! A plugin is off unless its feature flag is on. A reserved plugin type is never released:
//! entries or `plugin-paths` for a plugin that is off, or no longer exists, are ignored with a
//! warning, never treated as an error.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::io_args::project_flag_bool;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, fs_err};
use dbt_flags::FlagDef;
use dbt_schemas::schemas::PluginData;
use dbt_yaml::Spanned;
use indexmap::IndexMap;

/// Adds nodes of its own kind to a project.
pub trait ResolvePlugin: Send + Sync {
    /// The kind of node the plugin adds, e.g. `widget`: its key under `plugins:` and
    /// `plugin-paths:`, the prefix of its nodes' unique ids (`<plugin type>.<package>.<name>`), and
    /// a value `resource_type:` selects. It must be among the reserved plugin types the plugin is
    /// registered with.
    fn plugin_type(&self) -> &'static str;

    /// The feature flag that turns the plugin on. It defaults to off.
    fn flag(&self) -> &'static FlagDef;

    /// Checks one entry under `plugins: <plugin type>:` in a properties file.
    ///
    /// dbt reads the entry's `name`, `description`, `config` (`enabled`, `tags`, `meta`) and
    /// `depends_on` itself, rendering them as it does an exposure's. An error rejects the entry,
    /// which gets no node; the rest of the project still resolves.
    fn resolve_entry(
        &self,
        entry: &PluginEntry<'_>,
        diagnostics: &mut PluginDiagnostics,
    ) -> Result<PluginEntryData, PluginError> {
        let _ = (entry, diagnostics);
        Err(PluginError::new("this plugin reads no properties entries"))
    }

    /// Turns the files under one package's `plugin-paths` for this plugin into node definitions.
    ///
    /// A file the plugin rejects should be reported with [`PluginDiagnostics::error`]; an error
    /// returned here rejects all of them.
    fn resolve_files(
        &self,
        files: &[PluginFile],
        diagnostics: &mut PluginDiagnostics,
    ) -> Result<Vec<PluginNodeDef>, PluginError> {
        let _ = (files, diagnostics);
        Err(PluginError::new("this plugin reads no plugin-paths"))
    }
}

/// One entry under `plugins: <plugin type>:` in a properties file.
#[derive(Debug)]
pub struct PluginEntry<'a> {
    /// The entry's `name`, which names its node.
    pub name: &'a str,
    /// The package whose properties file holds the entry.
    pub package_name: &'a str,
    /// The properties file, relative to its package's root.
    pub path: &'a Path,
    /// The entry's own properties: everything but `name`, `description`, `config` and
    /// `depends_on`, as written (unrendered), with their spans.
    pub properties: &'a BTreeMap<String, dbt_yaml::Value>,
}

impl PluginEntry<'_> {
    /// Where `value`, one of the entry's properties, is in its file.
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

/// What a plugin keeps from one properties entry.
#[derive(Debug, Default)]
pub struct PluginEntryData {
    /// The plugin's own data for the node.
    pub data: PluginData,
    /// More `ref()`, `source()` and `metric()` calls the node depends on, beyond its own
    /// `depends_on`: for instance, ones nested in its properties.
    pub depends_on: Vec<Spanned<String>>,
}

/// A file under one of a package's `plugin-paths` for a plugin.
#[derive(Debug, Clone)]
pub struct PluginFile {
    /// The file, relative to its package's root.
    pub path: PathBuf,
    /// The file on disk.
    pub absolute_path: PathBuf,
}

/// A node a plugin defines from its files.
#[derive(Debug, Default)]
pub struct PluginNodeDef {
    /// The node's name. Its unique id is `<plugin type>.<package>.<name>`.
    pub name: String,
    /// The file that defines the node, relative to its package's root.
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
    /// The plugin that is on and adds nodes of `plugin_type`.
    pub(crate) fn of_type(&self, plugin_type: &str) -> Option<&Arc<dyn ResolvePlugin>> {
        self.on
            .iter()
            .find(|plugin| plugin.plugin_type() == plugin_type)
    }

    /// Whether `plugin_type` is reserved.
    pub(crate) fn is_reserved(&self, plugin_type: &str) -> bool {
        self.reserved.contains(&plugin_type)
    }

    /// Why what a project gives a reserved `plugin_type` isn't read.
    pub(crate) fn why_ignored(&self, plugin_type: &str) -> String {
        match self
            .off
            .iter()
            .find(|plugin| plugin.plugin_type() == plugin_type)
        {
            Some(plugin) => format!(
                "the `{plugin_type}` plugin is off; turn it on with the `{}` feature flag",
                plugin.flag().name
            ),
            None => format!("this dbt has no `{plugin_type}` plugin"),
        }
    }
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
        fn flag(&self) -> &'static FlagDef {
            &WIDGETS_FLAG
        }
    }

    fn plugins(project_flags: &str) -> ActivePlugins {
        let project_flags: dbt_yaml::Value = dbt_yaml::from_str(project_flags).unwrap();
        ResolvePlugins {
            plugins: vec![Arc::new(Widgets)],
            reserved: vec!["widget", "gadget"],
        }
        .turned_on(Some(&project_flags))
    }

    /// A plugin is off unless its flag is on, here in the project's `flags:`.
    #[test]
    fn plugins_are_on_only_when_their_flag_is() {
        assert!(plugins("{}").of_type("widget").is_none());
        assert!(plugins("test_widgets: true").of_type("widget").is_some());
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
        assert_eq!(
            plugins.why_ignored("gadget"),
            "this dbt has no `gadget` plugin"
        );
    }
}
