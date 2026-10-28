use std::collections::{BTreeMap, HashMap};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_common::path::DbtPath;
use dbt_common::stdfs;
use dbt_common::tracing::dbt_emit::{
    emit_error_log_from_fs_error, emit_warn_log_from_fs_error, emit_warn_log_message,
};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::common::DbtChecksum;
use dbt_schemas::schemas::project::{PluginNodeConfig, Tags};
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use dbt_schemas::schemas::{
    CommonAttributes, DbtPluginNode, DbtPluginNodeAttr, NodeBaseAttributes,
};
use dbt_schemas::state::DbtPackage;
use minijinja::value::Value as MinijinjaValue;

use super::resolve_exposures::resolve_yaml_depends_on;
use super::resolve_properties::MinimalPropertiesEntry;
use crate::args::ResolveArgs;
use crate::plugin::{
    ActivePlugins, PluginDiagnostics, PluginEntry, PluginFile, PluginNodeDef, PluginPackage, keys,
};
use crate::utils::get_node_fqn;

type PluginNodes = HashMap<String, Arc<DbtPluginNode>>;

/// What resolving one package's plugin nodes needs.
pub(crate) struct PluginNodeContext<'a> {
    pub(crate) args: &'a ResolveArgs,
    pub(crate) package: &'a DbtPackage,
    pub(crate) root_package: &'a DbtPackage,
    pub(crate) database: &'a str,
    pub(crate) schema: &'a str,
    pub(crate) adapter_type: AdapterType,
    pub(crate) env: &'a JinjaEnv,
    pub(crate) base_ctx: &'a BTreeMap<String, MinijinjaValue>,
}

/// The enabled and disabled plugin nodes resolved so far.
#[derive(Default)]
pub(crate) struct ResolvedPluginNodes {
    pub(crate) enabled: PluginNodes,
    pub(crate) disabled: PluginNodes,
}

impl ResolvedPluginNodes {
    fn contains(&self, unique_id: &str) -> bool {
        self.enabled.contains_key(unique_id) || self.disabled.contains_key(unique_id)
    }

    fn insert(&mut self, node: DbtPluginNode) {
        let unique_id = node.__common_attr__.unique_id.clone();
        if node.__base_attr__.enabled {
            self.enabled.insert(unique_id, Arc::new(node));
        } else {
            self.disabled.insert(unique_id, Arc::new(node));
        }
    }
}

/// Resolves a package's plugin nodes: each plugin that is on gets the package's entries and files
/// under each of its keys, in properties files and `plugin-paths`, all at once.
///
/// Two entries under the same key with the same name are reported, and neither reaches the
/// plugin. A node the plugin defines twice is reported and gets no second node.
pub(crate) fn resolve_plugins(
    ctx: &PluginNodeContext<'_>,
    plugins: &ActivePlugins,
    properties: &mut BTreeMap<String, BTreeMap<String, MinimalPropertiesEntry>>,
    nodes: &mut ResolvedPluginNodes,
) -> FsResult<()> {
    let package_name = ctx.package.dbt_project.name.as_str();
    let mut files = plugin_path_files(ctx, plugins)?;
    for plugin in &plugins.on {
        let plugin_type = plugin.plugin_type();
        let mut entries = BTreeMap::new();
        let mut plugin_files = BTreeMap::new();
        for key in keys(plugin.as_ref()) {
            if let Some(by_name) = properties.remove(key) {
                entries.insert(key, plugin_entries(key, package_name, by_name));
            }
            if let Some(key_files) = files.remove(key) {
                plugin_files.insert(key, key_files);
            }
        }
        let package = PluginPackage {
            package_name,
            entries,
            files: plugin_files,
            jinja: Some((ctx.env, ctx.base_ctx)),
        };
        let mut diagnostics = PluginDiagnostics::default();
        let defs = plugin.resolve(&package, &mut diagnostics);
        report(plugin_type, diagnostics);
        let defs = match defs {
            Ok(defs) => defs,
            Err(error) => {
                emit_error_log_from_fs_error(
                    *error.into_fs_error(plugin_type, ErrorCode::PluginError),
                );
                continue;
            }
        };
        for def in defs {
            let unique_id = format!("{plugin_type}.{package_name}.{}", def.name);
            if nodes.contains(&unique_id) {
                emit_error_log_from_fs_error(*fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => def.path.clone(),
                    "There is more than one {plugin_type} named '{}' in package \
                     '{package_name}'",
                    def.name
                ));
                continue;
            }
            // The checksum covers the file that defines the node.
            let checksum = DbtChecksum::hash(
                &stdfs::read(ctx.package.package_root_path.join(&def.path)).unwrap_or_default(),
            );
            nodes.insert(plugin_node(ctx, plugin_type, def, checksum)?);
        }
    }
    Ok(())
}

/// The entries under one key, reporting names that appear more than once.
fn plugin_entries(
    key: &str,
    package_name: &str,
    by_name: BTreeMap<String, MinimalPropertiesEntry>,
) -> Vec<PluginEntry> {
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
                "There is more than one {key} named '{name}' in package '{package_name}': {paths}"
            ));
            continue;
        }
        entries.push(PluginEntry {
            name,
            path: mpe.relative_path,
            value: mpe.schema_value,
        });
    }
    entries
}

/// The files under a package's `plugin-paths`, by key.
///
/// A key that isn't reserved, or whose plugin is off, is ignored with a warning. A path that
/// overlaps one dbt reads, or another key's, is an error.
fn plugin_path_files(
    ctx: &PluginNodeContext<'_>,
    plugins: &ActivePlugins,
) -> FsResult<BTreeMap<&'static str, Vec<PluginFile>>> {
    let mut files_by_key = BTreeMap::new();
    let project = &ctx.package.dbt_project;
    let Some(plugin_paths) = &project.plugin_paths else {
        return Ok(files_by_key);
    };
    let package_name = project.name.as_str();
    let dbt_paths = project.all_dbt_paths();
    let claims = Claims::new(&dbt_paths, plugin_paths);
    for (key, paths) in plugin_paths {
        if !plugins.is_reserved(key) {
            emit_warn_log_message(
                ErrorCode::UnknownPlugin,
                format!(
                    "Ignoring `plugin-paths: {key}` in package '{package_name}': there is no \
                     `{key}` plugin"
                ),
            );
            continue;
        }
        let Some(key) = plugins
            .reading(key)
            .and_then(|plugin| keys(plugin.as_ref()).find(|own| own == key))
        else {
            emit_warn_log_message(
                ErrorCode::PluginNotEnabled,
                format!(
                    "Ignoring `plugin-paths: {key}` in package '{package_name}': {}",
                    plugins.why_ignored(key)
                ),
            );
            continue;
        };

        let files = files_by_key.entry(key).or_insert_with(Vec::new);
        for path in paths {
            if let Some(other) = claims.overlap(key, path) {
                emit_error_log_from_fs_error(*fs_err!(
                    ErrorCode::PluginPathOverlap,
                    "`plugin-paths: {key}` in package '{package_name}' has '{path}', which \
                     overlaps '{other}'. A key's paths can't overlap a path dbt reads or \
                     another key's."
                ));
                continue;
            }
            if let Some(dir) = contained_dir(ctx.package, key, path) {
                collect_files(&ctx.package.package_root_path, &dir, files)?;
            }
        }
    }
    Ok(files_by_key)
}

/// The paths a package's dbt paths and `plugin-paths` claim, normalized, by who claims them:
/// `None` for dbt, else a key.
///
/// A path's overlaps are its ancestors and descendants among them, found by lookup rather than
/// by comparing it with every other path.
struct Claims<'a>(BTreeMap<PathBuf, Vec<(Option<&'a str>, &'a str)>>);

impl<'a> Claims<'a> {
    fn new(dbt_paths: &'a [String], plugin_paths: &'a BTreeMap<String, Vec<String>>) -> Self {
        let mut claims: BTreeMap<_, Vec<_>> = BTreeMap::new();
        let dbt = dbt_paths.iter().map(|path| (None, path));
        let plugins = plugin_paths
            .iter()
            .flat_map(|(key, paths)| paths.iter().map(|path| (Some(key.as_str()), path)));
        for (owner, path) in dbt.chain(plugins) {
            claims
                .entry(normalize(path))
                .or_default()
                .push((owner, path.as_str()));
        }
        Self(claims)
    }

    /// A path that overlaps `path`, one inside the other, claimed by anyone but `key`.
    fn overlap(&self, key: &str, path: &str) -> Option<&'a str> {
        let path = normalize(path);
        let ancestors = path.ancestors().filter_map(|ancestor| self.0.get(ancestor));
        // Paths order by component, so the paths inside `path` directly follow it.
        let descendants = self
            .0
            .range(path.clone()..)
            .take_while(|(other, _)| other.starts_with(&path))
            .map(|(_, claims)| claims);
        ancestors
            .chain(descendants)
            .flatten()
            .find(|(owner, _)| *owner != Some(key))
            .map(|(_, other)| *other)
    }
}

/// `path` without `.` components, so `./evals/` and `evals` are the same path.
fn normalize(path: &str) -> PathBuf {
    Path::new(path)
        .components()
        .filter(|component| !matches!(component, Component::CurDir))
        .collect()
}

/// `path` in `package`, if it is a directory inside the package.
///
/// Containment is checked on canonical paths, so it holds for symlinks as well as `..`, as for
/// `skill-paths`. A path that doesn't exist is not an error.
fn contained_dir(package: &DbtPackage, key: &str, path: &str) -> Option<PathBuf> {
    let root = &package.package_root_path;
    let dir = root.join(path);
    if !dir.is_dir() {
        return None;
    }
    let package_name = &package.dbt_project.name;
    match (stdfs::canonicalize(root), stdfs::canonicalize(&dir)) {
        (Ok(canonical_root), Ok(canonical_dir)) if canonical_dir.starts_with(&canonical_root) => {
            Some(dir)
        }
        (Ok(_), Ok(canonical_dir)) => {
            emit_warn_log_message(
                ErrorCode::PluginPathEscapesProject,
                format!(
                    "Skipping '{path}' in `plugin-paths: {key}` of '{package_name}': it \
                     resolves to {}, which is outside that project",
                    canonical_dir.display()
                ),
            );
            None
        }
        _ => {
            emit_warn_log_message(
                ErrorCode::PluginPathEscapesProject,
                format!(
                    "Skipping '{path}' in `plugin-paths: {key}` of '{package_name}': dbt \
                     could not resolve it to confirm it stays inside the project"
                ),
            );
            None
        }
    }
}

/// Every file under `dir`, recursively and in order, skipping hidden ones.
fn collect_files(root: &Path, dir: &Path, files: &mut Vec<PluginFile>) -> FsResult<()> {
    let mut entries: Vec<PathBuf> = stdfs::read_dir(dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('.'))
        })
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else {
            files.push(PluginFile {
                path: path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                absolute_path: path,
            });
        }
    }
    Ok(())
}

/// Reports what a plugin reported, under the plugin codes.
pub(crate) fn report(plugin_type: &str, diagnostics: PluginDiagnostics) {
    for error in diagnostics.errors {
        emit_error_log_from_fs_error(*error.into_fs_error(plugin_type, ErrorCode::PluginError));
    }
    for warning in diagnostics.warnings {
        emit_warn_log_from_fs_error(*warning.into_fs_error(plugin_type, ErrorCode::PluginWarning));
    }
}

/// A plugin node from its definition, its `depends_on` resolved with the project's own rules.
pub(crate) fn plugin_node(
    ctx: &PluginNodeContext<'_>,
    plugin_type: &str,
    def: PluginNodeDef,
    checksum: DbtChecksum,
) -> FsResult<DbtPluginNode> {
    let package_name = ctx.package.dbt_project.name.as_str();
    let unique_id = format!("{plugin_type}.{package_name}.{}", def.name);
    let fqn = get_node_fqn(
        package_name,
        def.path.clone(),
        vec![def.name.clone()],
        &ctx.package.dbt_project.all_source_paths(),
    );
    let config = PluginNodeConfig {
        enabled: Some(def.enabled),
        meta: Some(def.meta.clone()),
        tags: Tags(Some(StringOrArrayOfStrings::ArrayOfStrings(
            def.tags.clone(),
        ))),
    };
    let (refs, sources, metrics) = resolve_yaml_depends_on(
        &def.depends_on,
        ctx.env,
        ctx.base_ctx,
        &config,
        ctx.database,
        ctx.schema,
        ctx.adapter_type,
        package_name,
        &ctx.root_package.dbt_project.name,
        fqn.clone(),
        &def.path.to_string_lossy(),
        ctx.args.static_analysis,
    )?;
    Ok(DbtPluginNode {
        __common_attr__: CommonAttributes {
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
        __base_attr__: NodeBaseAttributes {
            adapter: ctx.adapter_type,
            enabled: def.enabled,
            refs,
            sources,
            metrics,
            ..Default::default()
        },
        __plugin_attr__: DbtPluginNodeAttr {
            plugin_type: plugin_type.to_string(),
            data: def.data,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(paths: &[&str]) -> Vec<String> {
        paths.iter().map(|path| path.to_string()).collect()
    }

    #[test]
    fn plugin_paths_overlap_when_one_is_inside_the_other() {
        let dbt_paths = paths(&["models", "seeds/raw"]);
        let plugin_paths = BTreeMap::from([
            ("widget".to_string(), paths(&["widgets", "widgets/more"])),
            ("gadget".to_string(), paths(&["gadgets/one"])),
        ]);
        let claims = Claims::new(&dbt_paths, &plugin_paths);
        assert_eq!(claims.overlap("widget", "models"), Some("models"));
        assert_eq!(
            claims.overlap("widget", "./models/widgets/"),
            Some("models")
        );
        assert_eq!(claims.overlap("widget", "seeds"), Some("seeds/raw"));
        assert_eq!(claims.overlap("widget", "gadgets"), Some("gadgets/one"));
        assert_eq!(claims.overlap("gadget", "./widgets"), Some("widgets"));
        assert!(claims.overlap("widget", ".").is_some());
        // A plugin's own paths may overlap each other.
        assert_eq!(claims.overlap("widget", "widgets/more"), None);
        assert_eq!(claims.overlap("widget", "models_old"), None);
        assert_eq!(claims.overlap("widget", "seeds/raw_old"), None);
        assert_eq!(claims.overlap("widget", "gadgets_old"), None);
    }
}
