//! What a kind of node that a feature adds to dbt resolves with.
//!
//! The kind is a [`ResolvableNode`](crate::resolve::resolvable_node::ResolvableNode), as dbt's
//! own kinds are. It owns top-level keys of properties files, and its feature flag turns it on:
//!
//! ```yaml
//! widget:
//!   - name: orders_widget
//!     depends_on: [ref('orders')]
//!     part: bolt
//! part:
//!   - name: bolt
//! ```
//!
//! When it resolves a package, it takes the entries under its keys
//! ([`ResolveCx::experimental_package`]), decides what each one deserializes to, its `config:`
//! included, and gives back the nodes they define ([`ResolveCx::add_experimental_nodes`]). dbt
//! resolves each node's `depends_on` with the project's own `ref()`, `source()` and `metric()`
//! rules. The nodes are of the kind's own type
//! ([`InjectedNodeType`](dbt_schemas::schemas::InjectedNodeType)): dbt reads only their common
//! and base attributes.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dbt_common::path::DbtPath;
use dbt_common::stdfs;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{CodeLocationWithFile, ErrorCode, FsError, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::schemas::common::DbtChecksum;
use dbt_schemas::schemas::project::{ExperimentalNodeConfig, Tags};
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use dbt_schemas::schemas::{CommonAttributes, InjectedNodeType, NodeBaseAttributes};
use dbt_yaml::Spanned;
use indexmap::IndexMap;
use minijinja::value::Value as MinijinjaValue;
use serde::de::DeserializeOwned;

use crate::resolve::properties_index::PropertiesEntry;
use crate::resolve::resolvable_node::ResolveCx;
use crate::resolve::resolve_exposures::resolve_yaml_depends_on;
use crate::utils::get_node_fqn;

/// What one package gives a kind of node, by key: the entries under each of the kind's top-level
/// keys in the package's properties files.
pub struct ExperimentalPackage<'a> {
    /// The package's name.
    pub package_name: &'a str,
    pub(crate) entries: BTreeMap<&'static str, Vec<ExperimentalEntry>>,
    /// What entries are rendered with; `None` reads them as written.
    pub(crate) jinja: Option<(&'a JinjaEnv, &'a BTreeMap<String, MinijinjaValue>)>,
}

impl<'a> ExperimentalPackage<'a> {
    /// A package whose entries are read as written, without rendering Jinja: for the unit tests
    /// of a kind of node.
    pub fn unrendered(
        package_name: &'a str,
        entries: BTreeMap<&'static str, Vec<ExperimentalEntry>>,
    ) -> Self {
        Self {
            package_name,
            entries,
            jinja: None,
        }
    }

    /// The entries under the top-level key `key`, one per name, ordered by name. `key` is one of
    /// the kind's keys.
    pub fn entries(&self, key: &str) -> &[ExperimentalEntry] {
        self.entries.get(key).map_or(&[], Vec::as_slice)
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

    /// `value`, part of an entry in the file `path`, as a `T`, rendered as [`Self::deserialize`]
    /// renders an entry.
    pub fn deserialize_value<T: DeserializeOwned>(
        &self,
        value: &dbt_yaml::Value,
        path: &Path,
    ) -> Result<T, ExperimentalError> {
        self.render(value.clone(), path)
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

/// One entry under one of a kind's top-level keys in a properties file.
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

/// A node an experimental defines.
#[derive(Debug, Default)]
pub struct ExperimentalNodeDef {
    /// The node's name. Its unique id is `<experimental type>.<package>.<name>`.
    pub name: String,
    /// The properties file that defines the node, relative to its package's root.
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

impl ResolveCx<'_> {
    /// Takes the entries under each of `keys`, the top-level keys of a kind of node, out of the
    /// package's properties.
    ///
    /// Two entries under the same key with the same name are reported, and neither is given.
    pub fn experimental_package(&mut self, keys: &[&'static str]) -> ExperimentalPackage<'_> {
        let package_name = self.package_name;
        let entries = keys
            .iter()
            .map(|key| {
                let by_name = self.properties.take_entries(key);
                (*key, experimental_entries(key, package_name, by_name))
            })
            .collect();
        ExperimentalPackage {
            package_name,
            entries,
            jinja: Some((&self.jinja_env, &self.base_ctx)),
        }
    }

    /// Adds the nodes that `defs` define to the package's nodes, and reports `diagnostics`.
    ///
    /// Each definition comes with what the kind keeps for its node. `node` builds the node, of
    /// the kind's own type, from that and from the attributes that dbt resolves for the
    /// definition.
    ///
    /// `defs` that are an error give no node, and the error is reported. A node that is defined
    /// twice is reported and gets no second node.
    pub fn add_experimental_nodes<T: InjectedNodeType, D>(
        &mut self,
        experimental_type: &str,
        defs: Result<Vec<(ExperimentalNodeDef, D)>, ExperimentalError>,
        diagnostics: ExperimentalDiagnostics,
        node: impl Fn(CommonAttributes, NodeBaseAttributes, D) -> T,
    ) -> FsResult<()> {
        report(experimental_type, diagnostics);
        let defs = match defs {
            Ok(defs) => defs,
            Err(error) => {
                emit_error_log_from_fs_error(
                    *error.into_fs_error(experimental_type, ErrorCode::ExperimentalError),
                );
                return Ok(());
            }
        };
        let package_name = self.package_name;
        for (def, data) in defs {
            let unique_id = format!("{experimental_type}.{package_name}.{}", def.name);
            if self.nodes.experimentals.contains_key(&unique_id)
                || self.disabled_nodes.experimentals.contains_key(&unique_id)
            {
                emit_error_log_from_fs_error(*fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => def.path.clone(),
                    "There is more than one {experimental_type} named '{}' in package \
                     '{package_name}'",
                    def.name
                ));
                continue;
            }
            // The checksum covers the file that defines the node.
            let checksum = DbtChecksum::hash(
                &stdfs::read(self.package.package_root_path.join(&def.path)).unwrap_or_default(),
            );
            let (common, base) = self.node_attributes(experimental_type, def, checksum)?;
            let nodes = if base.enabled {
                &mut self.nodes
            } else {
                &mut self.disabled_nodes
            };
            nodes
                .experimentals
                .insert(unique_id, Arc::new(node(common, base, data)));
        }
        Ok(())
    }

    /// The attributes of a node from its definition, its `depends_on` resolved with the
    /// project's own rules.
    fn node_attributes(
        &self,
        experimental_type: &str,
        def: ExperimentalNodeDef,
        checksum: DbtChecksum,
    ) -> FsResult<(CommonAttributes, NodeBaseAttributes)> {
        let package_name = self.package_name;
        let unique_id = format!("{experimental_type}.{package_name}.{}", def.name);
        let fqn = get_node_fqn(
            package_name,
            def.path.clone(),
            vec![def.name.clone()],
            &self.package.dbt_project.all_source_paths(),
        );
        let config = ExperimentalNodeConfig {
            enabled: Some(def.enabled),
            meta: Some(def.meta.clone()),
            tags: Tags(Some(StringOrArrayOfStrings::ArrayOfStrings(
                def.tags.clone(),
            ))),
        };
        let (refs, sources, metrics) = resolve_yaml_depends_on(
            &def.depends_on,
            &self.jinja_env,
            &self.base_ctx,
            &config,
            self.database,
            self.schema,
            self.adapter_type,
            package_name,
            &self.dbt_state.root_package().dbt_project.name,
            fqn.clone(),
            &def.path.to_string_lossy(),
            self.arg.static_analysis,
        )?;
        Ok((
            CommonAttributes {
                name: def.name,
                package_name: package_name.to_string(),
                path: DbtPath::from(&def.path),
                original_file_path: DbtPath::from(&def.path),
                unique_id,
                fqn,
                description: def.description,
                checksum,
                tags: def.tags,
                meta: def.meta,
                ..Default::default()
            },
            NodeBaseAttributes {
                adapter: self.adapter_type,
                enabled: def.enabled,
                refs,
                sources,
                metrics,
                ..Default::default()
            },
        ))
    }
}

/// The entries under one key, reporting names that appear more than once.
fn experimental_entries(
    key: &str,
    package_name: &str,
    by_name: BTreeMap<String, PropertiesEntry>,
) -> Vec<ExperimentalEntry> {
    let mut entries = Vec::new();
    for (name, mpe) in by_name {
        if mpe.schema_value.is_null() {
            continue;
        }
        if !mpe.duplicate_paths.is_empty() {
            let paths = std::iter::once(&mpe.relative_path)
                .chain(&mpe.duplicate_paths)
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            emit_error_log_from_fs_error(*fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => mpe.relative_path.clone(),
                "There is more than one entry named '{name}' under `{key}:` in package \
                 '{package_name}': {paths}"
            ));
            continue;
        }
        entries.push(ExperimentalEntry {
            name,
            path: mpe.relative_path,
            value: mpe.schema_value,
        });
    }
    entries
}

/// Reports what a kind of node reported, under the experimental codes.
fn report(experimental_type: &str, diagnostics: ExperimentalDiagnostics) {
    for error in diagnostics.errors {
        emit_error_log_from_fs_error(
            *error.into_fs_error(experimental_type, ErrorCode::ExperimentalError),
        );
    }
    for warning in diagnostics.warnings {
        emit_warn_log_from_fs_error(
            *warning.into_fs_error(experimental_type, ErrorCode::ExperimentalWarning),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
