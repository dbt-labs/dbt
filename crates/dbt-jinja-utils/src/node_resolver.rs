use std::{
    any::Any,
    collections::{BTreeMap, HashMap, HashSet},
    iter::Iterator,
    sync::Arc,
};

use chrono::Utc;
use dbt_adapter::relation::{
    RelationObject, canonical_relation_parts, create_relation, create_relation_from_node,
    create_relation_from_source,
};
use dbt_adapter_core::AdapterType;
use dbt_common::{
    CodeLocationWithFile, ErrorCode, FsError, FsResult, err, fs_err,
    io_args::resolve_require_ref_searches_node_package_before_root,
    tracing::dbt_emit::{
        emit_debug_log_message, emit_error_log_from_fs_error, emit_warn_log_from_fs_error,
        emit_warn_log_message,
    },
};
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::{
    filter::RunFilter,
    schemas::{
        DbtFunction, DbtSource, InternalDbtNodeAttributes, IntrospectionKind, Nodes,
        common::{DbtMaterialization, DbtQuoting, ResolvedQuoting, parse_deprecation_date},
        ref_and_source::{DbtRef, DbtSourceWrapper},
        telemetry::NodeType,
    },
    state::{ModelStatus, NodeResolverTracker},
};
use minijinja::{Value as MinijinjaValue, value::function_object::FunctionObject};

type RefRecord = (String, MinijinjaValue, ModelStatus, Option<MinijinjaValue>);

fn create_ref_relation(
    node: &dyn InternalDbtNodeAttributes,
    adapter_type: AdapterType,
    run_filter: Option<RunFilter>,
) -> FsResult<Box<dyn dbt_schemas::schemas::relations::base::BaseRelation>> {
    if node.base().effective_propagation_target == Some(AdapterType::Databricks) {
        let (database, schema, identifier) = canonical_relation_parts(
            Some(AdapterType::Databricks),
            &node.database(),
            &node.schema(),
            &node.alias(),
        );
        create_relation(
            node.node_adapter(),
            database,
            schema,
            Some(identifier),
            Some(RelationType::from(node.materialized())),
            ResolvedQuoting::falses(),
        )
    } else {
        create_relation_from_node(adapter_type, node, run_filter)
    }
}

fn downgraded_node_dependency_warning(
    error: &FsError,
    location: CodeLocationWithFile,
) -> Option<(FsError, bool)> {
    let has_disabled_or_missing_dependency = match error.code {
        ErrorCode::DisabledDependency | ErrorCode::DependencyNotFound => true,
        _ => return None,
    };

    Some((
        FsError::new(ErrorCode::NodeNotFoundOrDisabled, error.to_string()).with_location(location),
        has_disabled_or_missing_dependency,
    ))
}

/// Search order for unqualified package resources.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum PackageSearchOrder {
    /// Search the package containing the calling node first. Fusion's established
    /// behavior, and the default when the flag is not set.
    #[default]
    NodePackageFirst,
    /// Search the root project first, as dbt-core does by default.
    RootFirst,
}

impl PackageSearchOrder {
    /// Returns the search order configured by the `require_ref_searches_node_package_before_root`
    /// behavior flag, honoring the `DBT_REQUIRE_REF_SEARCHES_NODE_PACKAGE_BEFORE_ROOT` environment
    /// variable ahead of the root project's `flags:` block.
    pub fn resolve(project_flags: Option<&dbt_yaml::Value>) -> Self {
        Self::from_node_package_first(resolve_require_ref_searches_node_package_before_root(
            project_flags,
        ))
    }

    fn from_node_package_first(node_package_first: bool) -> Self {
        if node_package_first {
            Self::NodePackageFirst
        } else {
            Self::RootFirst
        }
    }
}

/// A wrapper around refs and sources with methods to get and insert refs and sources.
///
/// Carries optional `sample_plan` which, when present, remaps source relations to the
/// sampled schema in remote runs (e.g., `<schema>_SAMPLE` or an explicit `remote.schema`).
#[derive(Debug, Default, Clone)]
pub struct NodeResolver {
    /// Map of ref_name (either {project}.{ref_name}, {ref_name}) to (unique_id, relation, status, deferred_relation)
    #[allow(clippy::type_complexity)]
    pub refs: BTreeMap<String, Vec<RefRecord>>,
    /// Map of (package_name.source_name.name ) to (unique_id, relation, status)
    pub sources: BTreeMap<String, Vec<(String, MinijinjaValue, ModelStatus)>>,
    /// Map of function_name (either {project}.{function_name}, {function_name}) to
    /// (unique_id, function_object, status). The function object may be replaced
    /// with its deferred version during defer hydration.
    pub functions: BTreeMap<String, Vec<(String, MinijinjaValue, ModelStatus)>>,
    /// Root project name (needed for resolving refs)
    pub root_package_name: String,
    /// Optional Quoting Config produced by mantle/core manifest needed for back compatibility for defer in fusion
    pub mantle_quoting: Option<DbtQuoting>,
    /// Filters that will be applied to `run` or `build` (supports --empty or --sample)
    pub run_filter: RunFilter,
    /// Optional remap plan for sources when sampling is enabled
    pub renaming: BTreeMap<String, (String, String, String)>,
    /// Whether this is a compile or test command
    pub compile_or_test: bool,
    /// Search order for unqualified `ref()`, `source()`, and `function()` calls.
    pub package_search_order: PackageSearchOrder,
    /// Per-node introspection kind for O(1) lookup by unique_id.
    /// Populated during `set_defer_context`.
    pub node_introspections: HashMap<String, IntrospectionKind>,
    /// Nodes that will produce analyzed schemas (strict SA, not frontier, not source).
    /// For compile safe-introspection: defer upstream if NOT in this set.
    pub has_analyzed_schema: HashSet<String>,
    /// Nodes that will be materialized (selected, not frontier, not source).
    /// For run path: defer upstream if NOT in this set.
    pub nodes_materialized: HashSet<String>,
}

impl NodeResolver {
    /// Create a new NodeResolver from a DbtManifest
    pub fn from_dbt_nodes(
        nodes: &Nodes,
        adapter_type: AdapterType,
        root_package_name: String,
        mantle_quoting: Option<DbtQuoting>,
        run_filter: RunFilter,
        renaming: BTreeMap<String, (String, String, String)>,
        compile_or_test: bool,
        package_search_order: PackageSearchOrder,
    ) -> FsResult<Self> {
        let mut node_resolver = NodeResolver {
            root_package_name,
            mantle_quoting,
            run_filter,
            renaming,
            compile_or_test,
            package_search_order,
            ..Default::default()
        };
        for (_, node) in nodes.iter() {
            // A node's relation is rendered with *its own* adapter, not the
            // target's default. `ref('x')` returns x's relation, and x lives in
            // x's adapter's warehouse, so it must render with x's rules however it
            // is referenced. Both inputs are per-node: the node carries its own
            // quoting (resolved against its `+adapter` during resolve) and the
            // adapter type decides `include_policy` -- DuckDB suppresses the
            // database component for `main`/`memory` while others do not.
            //
            // Each relation stays cached one-per-node; only the adapter type used
            // to build it changes.
            let node_adapter_type = node.node_adapter();

            if let Some(source) = node.as_any().downcast_ref::<DbtSource>() {
                // Sources have no `+adapter`: they are external data, so they take
                // the target's default.
                node_resolver.insert_source(
                    &node.common().package_name,
                    source,
                    adapter_type,
                    ModelStatus::Enabled,
                )?;
            } else if let Some(function) = node.as_any().downcast_ref::<DbtFunction>() {
                node_resolver.insert_function(function, node_adapter_type, ModelStatus::Enabled)?;
            } else {
                match node.resource_type() {
                    NodeType::Model | NodeType::Snapshot | NodeType::Seed => node_resolver
                        .insert_ref(node, node_adapter_type, ModelStatus::Enabled, false)?,
                    _ => (),
                }
            }
        }
        Ok(node_resolver)
    }

    fn search_packages(
        &self,
        maybe_target_package: Option<&str>,
        maybe_node_package: Option<&str>,
    ) -> Vec<Option<String>> {
        match (maybe_target_package, maybe_node_package) {
            (Some(target_package), _) => vec![Some(target_package.to_string())],
            (None, None) => vec![None],
            (None, Some(node_package)) if node_package == self.root_package_name => {
                vec![Some(self.root_package_name.clone()), None]
            }
            (None, Some(node_package)) => match self.package_search_order {
                PackageSearchOrder::NodePackageFirst => vec![
                    Some(node_package.to_string()),
                    Some(self.root_package_name.clone()),
                    None,
                ],
                PackageSearchOrder::RootFirst => vec![
                    Some(self.root_package_name.clone()),
                    Some(node_package.to_string()),
                    None,
                ],
            },
        }
    }

    /// Merge another NodeResolver into this one, avoiding duplicates
    /// This uses functional programming style for cleaner code
    pub fn merge(&mut self, source: NodeResolver) {
        for (key, source_entries) in source.refs {
            let target_entries = self.refs.entry(key).or_default();
            let existing_ids: HashSet<String> = target_entries
                .iter()
                .map(|(id, _, _, _)| id.clone())
                .collect();

            // Add only entries that don't exist in target
            target_entries.extend(
                source_entries
                    .into_iter()
                    .filter(|(unique_id, _, _, _)| !existing_ids.contains(unique_id)),
            );
        }

        for (key, source_entries) in source.sources {
            let target_entries = self.sources.entry(key).or_default();
            let existing_ids: HashSet<String> =
                target_entries.iter().map(|(id, _, _)| id.clone()).collect();

            // Add only entries that don't exist in target
            target_entries.extend(
                source_entries
                    .into_iter()
                    .filter(|(unique_id, _, _)| !existing_ids.contains(unique_id)),
            );
        }

        for (key, source_entries) in source.functions {
            let target_entries = self.functions.entry(key).or_default();
            let existing_ids: HashSet<String> =
                target_entries.iter().map(|(id, _, _)| id.clone()).collect();

            // Add only entries that don't exist in target
            target_entries.extend(
                source_entries
                    .into_iter()
                    .filter(|(unique_id, _, _)| !existing_ids.contains(unique_id)),
            );
        }
    }

    fn push_or_replace_entry(
        entries: &mut Vec<RefRecord>,
        unique_id: &str,
        relation: &MinijinjaValue,
        status: ModelStatus,
        override_existing: bool,
    ) {
        if override_existing
            && let Some(existing) = entries.iter_mut().find(|(id, _, _, _)| id == unique_id)
        {
            *existing = (unique_id.to_string(), relation.clone(), status, None);
            return;
        }

        // Prevent duplicate unique_ids: cross-project publication nodes can be registered
        // via from_dbt_nodes() and again during wave resolution, causing the same unique_id
        // to appear twice and triggering "ambiguous ref" panics at compile time.
        // Only skip when the same unique_id with the same status already exists — a disabled
        // and an enabled entry for the same name are both needed for correct ref resolution.
        if entries
            .iter()
            .any(|(id, _, s, _)| id == unique_id && *s == status)
        {
            return;
        }

        entries.push((unique_id.to_string(), relation.clone(), status, None));
    }

    fn push_or_replace_function_entry(
        entries: &mut Vec<(String, MinijinjaValue, ModelStatus)>,
        unique_id: &str,
        function_object: &MinijinjaValue,
        status: ModelStatus,
        override_existing: bool,
    ) {
        if override_existing
            && let Some(existing) = entries.iter_mut().find(|(id, _, _)| id == unique_id)
        {
            *existing = (unique_id.to_string(), function_object.clone(), status);
            return;
        }

        entries.push((unique_id.to_string(), function_object.clone(), status));
    }

    fn set_deferred_relation(
        entries: &mut [RefRecord],
        unique_id: &str,
        deferred_relation: &MinijinjaValue,
        is_frontier: bool,
        node: &dyn InternalDbtNodeAttributes,
    ) {
        // For each entry that matches the unique_id, set the deferred relation
        entries
            .iter_mut()
            .filter(|(id, _, _, _)| id == unique_id)
            .for_each(|(_, relation, _, deferred)| {
                *deferred = Some(deferred_relation.clone());
                // Update relation to remote for:
                // - Frontier nodes (dependencies): always update
                // - Snapshots: always update (need remote for schema merge)
                // - Incrementals: always update (need remote for schema merge)
                // - Other selected models: keep local (will be analyzed with local schema)
                let is_incremental_or_snapshot = matches!(
                    node.materialized(),
                    DbtMaterialization::Incremental | DbtMaterialization::Snapshot
                );
                if is_frontier || is_incremental_or_snapshot {
                    *relation = deferred_relation.clone();
                }
            });
    }

    fn set_deferred_function(
        entries: &mut [(String, MinijinjaValue, ModelStatus)],
        unique_id: &str,
        deferred_function: &MinijinjaValue,
    ) {
        entries
            .iter_mut()
            .filter(|(id, _, _)| id == unique_id)
            .for_each(|(_, function, _)| *function = deferred_function.clone());
    }
}

impl NodeResolverTracker for NodeResolver {
    fn deep_clone(&self) -> Box<dyn NodeResolverTracker> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    /// Insert or overwrite a ref from a node into the refs map
    fn insert_ref(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        status: ModelStatus,
        override_existing: bool,
    ) -> FsResult<()> {
        // If the latest version and current version are the same, the unversioned ref must point to the latest
        let package_name = &node.package_name();
        let model_name = node.name();
        let unique_id = node.unique_id();
        let (maybe_version, maybe_latest_version) = if node.resource_type() == NodeType::Model {
            (node.version(), node.latest_version())
        } else {
            (None, None)
        };

        let relation = RelationObject::new_with_filter(
            create_ref_relation(node, adapter_type, Some(self.run_filter.clone()))?.into(),
            self.run_filter.clone(),
            node.event_time(),
        )
        .into_value();

        if maybe_version == maybe_latest_version {
            // Lookup by ref name
            let ref_entry = self.refs.entry(model_name.clone()).or_default();
            Self::push_or_replace_entry(
                ref_entry,
                &unique_id,
                &relation,
                status,
                override_existing,
            );

            // Lookup by package and ref name
            let package_ref_entry = self
                .refs
                .entry(format!("{package_name}.{model_name}"))
                .or_default();
            Self::push_or_replace_entry(
                package_ref_entry,
                &unique_id,
                &relation,
                status,
                override_existing,
            );
        }

        // All other entries are versioned, if one exists
        if let Some(version) = maybe_version {
            let model_name_with_version = format!("{model_name}.v{version}");

            // Lookup by ref name (optional version)
            let versioned_ref_entry = self
                .refs
                .entry(model_name_with_version.to_owned())
                .or_default();
            Self::push_or_replace_entry(
                versioned_ref_entry,
                &unique_id,
                &relation,
                status,
                override_existing,
            );

            let package_versioned_ref_entry = self
                .refs
                .entry(format!("{package_name}.{model_name_with_version}"))
                .or_default();
            if override_existing {
                Self::push_or_replace_entry(
                    package_versioned_ref_entry,
                    &unique_id,
                    &relation,
                    status,
                    true,
                );
            } else if !package_versioned_ref_entry
                .iter()
                .any(|(id, _, _, _)| id == &unique_id)
            {
                package_versioned_ref_entry.push((
                    unique_id.to_string(),
                    relation.clone(),
                    status,
                    None,
                ));
            }
        }
        Ok(())
    }

    /// Insert or overwrite a function() from a node into the functions map
    fn insert_function(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        status: ModelStatus,
    ) -> FsResult<()> {
        let package_name = &node.package_name();
        let function_name = node.name();
        let unique_id = node.unique_id();

        // For functions, create a FunctionObject that renders function calls
        let function_object = create_function_object_from_node(adapter_type, node)?.into_value();

        // Lookup by function name
        let function_entry = self.functions.entry(function_name.clone()).or_default();
        Self::push_or_replace_function_entry(
            function_entry,
            &unique_id,
            &function_object,
            status,
            true,
        );

        // Lookup by package and function name
        let package_function_entry = self
            .functions
            .entry(format!("{package_name}.{function_name}"))
            .or_default();
        Self::push_or_replace_function_entry(
            package_function_entry,
            &unique_id,
            &function_object,
            status,
            true,
        );
        Ok(())
    }

    /// Insert a source into the refs and sources map
    fn insert_source(
        &mut self,
        package_name: &str,
        source: &DbtSource,
        adapter_type: AdapterType,
        status: ModelStatus,
    ) -> FsResult<()> {
        // Build base relation and apply sample remapping if configured
        let mut database = source.base().database.clone();
        let mut schema = source.base().schema.clone();
        let mut identifier = source.base().alias.clone();
        let mapper = &self.renaming;
        if mapper.contains_key(&source.unique_id()) {
            // When a plan is present, remap all sources.
            (database, schema, identifier) = mapper[&source.unique_id()].clone();
        }

        let base_rel = create_relation_from_source(
            adapter_type,
            database,
            schema,
            identifier,
            source.quoting(),
            source,
        )?;
        let relation = RelationObject::new_with_filter(
            base_rel.into(),
            self.run_filter.clone(),
            source.deprecated_config.event_time.clone(),
        )
        .into_value();

        self.sources
            .entry(format!(
                "{}.{}.{}",
                package_name,
                source.__source_attr__.source_name,
                source.common().name
            ))
            .or_default()
            .push((source.common().unique_id.clone(), relation.clone(), status));
        self.sources
            .entry(format!(
                "{}.{}",
                source.__source_attr__.source_name,
                source.common().name
            ))
            .or_default()
            .push((source.common().unique_id.clone(), relation, status));
        Ok(())
    }

    fn lookup_self_relation(
        &self,
        unique_id: &str,
        package_name: &str,
        name: &str,
        version: &Option<String>,
    ) -> FsResult<(MinijinjaValue, Option<MinijinjaValue>)> {
        let ref_name = match version {
            Some(version) => format!("{name}.v{version}"),
            None => name.to_string(),
        };
        let search_keys = [format!("{package_name}.{ref_name}"), ref_name.clone()];
        let mut disabled_match = false;

        // Read the live record out of `refs` rather than a parallel index:
        // `set_deferred_relation` rewrites entries in place during defer
        // hydration, so a copy taken at insert time would go stale.
        for key in &search_keys {
            let Some(records) = self.refs.get(key) else {
                continue;
            };

            for (_, relation, status, deferred) in
                records.iter().filter(|(id, _, _, _)| id == unique_id)
            {
                if *status == ModelStatus::Disabled {
                    disabled_match = true;
                } else {
                    return Ok((relation.clone(), deferred.clone()));
                }
            }
        }

        if disabled_match {
            err!(
                ErrorCode::DisabledDependency,
                "Attempted to use disabled ref '{}'",
                ref_name
            )
        } else {
            err!(
                ErrorCode::DependencyNotFound,
                "Ref '{}' not found in project. Searched for '{}'",
                ref_name,
                search_keys[0]
            )
        }
    }

    /// Lookup a ref by package name, model name, and optional version
    fn lookup_ref(
        &self,
        maybe_package_name: &Option<String>,
        name: &str,
        version: &Option<String>,
        maybe_node_package_name: &Option<String>,
    ) -> FsResult<RefRecord> {
        // Create a list of packages to search, where None means to
        // search non-package limited names
        let search_packages = self.search_packages(
            maybe_package_name.as_deref(),
            maybe_node_package_name.as_deref(),
        );

        // Construct possibly versioned ref_name
        let ref_name = format!(
            "{}{}",
            name,
            version
                .as_ref()
                .map(|v| format!(".v{v}"))
                .unwrap_or_default()
        );
        let mut enabled_ref: Option<RefRecord> = None;
        let mut disabled_ref: Option<RefRecord> = None;
        let mut search_ref_names: Vec<String> = Vec::new();
        for maybe_package in search_packages.iter() {
            // If this is a package, use the package name + ref_name to search
            let search_ref_name = if let Some(package_name) = maybe_package {
                format!("{}.{}", package_name.clone(), ref_name)
            } else {
                // If this is not a package, just use the ref_name to search
                ref_name.clone()
            };
            search_ref_names.push(search_ref_name.clone());
            if let Some(res) = self.refs.get(&search_ref_name) {
                let (enabled_refs, disabled_refs): (Vec<_>, Vec<_>) = res
                    .iter()
                    .partition(|(_, _, status, _)| *status != ModelStatus::Disabled);
                // We got a ref or we wouldn't be here
                if !disabled_refs.is_empty() {
                    disabled_ref = Some(disabled_refs[0].clone());
                }
                match enabled_refs.len() {
                    // If there is one enabled ref, use it
                    1 => {
                        enabled_ref = Some(enabled_refs[0].clone());
                        break;
                    }
                    n if n > 1 => {
                        // More than one enabled ref with the same name, issue error
                        return err!(
                            ErrorCode::InvalidConfig,
                            "Found ambiguous ref('{}') pointing to multiple nodes: [{}]",
                            ref_name,
                            res.iter()
                                .map(|(r, _, _, _)| format!("'{r}'"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    // If there are no enabled refs, continue to next package
                    _ => {}
                };
            }
        }
        // If ref not found issue error
        match enabled_ref {
            Some(ref_result) => Ok(ref_result),
            None => {
                if disabled_ref.is_some() {
                    err!(
                        ErrorCode::DisabledDependency,
                        "Attempted to use disabled ref '{}'",
                        ref_name
                    )
                } else {
                    err!(
                        ErrorCode::DependencyNotFound,
                        "Ref '{}' not found in project. Searched for '{}'",
                        ref_name,
                        search_ref_names.join(", ")
                    )
                }
            }
        }
    }

    /// Lookup a source by the calling node's package name, source name, and table name
    fn lookup_source(
        &self,
        node_package_name: &str,
        source_name: &str,
        table_name: &str,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)> {
        let source_table_name = format!("{source_name}.{table_name}");
        let mut disabled_source_name = None;
        let mut search_source_names = Vec::new();
        for maybe_package in self.search_packages(None, Some(node_package_name)) {
            let search_source_name = if let Some(package_name) = maybe_package {
                format!("{package_name}.{source_table_name}")
            } else {
                source_table_name.clone()
            };
            search_source_names.push(search_source_name.clone());

            if let Some(res) = self.sources.get(&search_source_name) {
                let (enabled_sources, disabled_sources): (Vec<_>, Vec<_>) = res
                    .iter()
                    .partition(|(_, _, status)| *status != ModelStatus::Disabled);

                if !disabled_sources.is_empty() && disabled_source_name.is_none() {
                    disabled_source_name = Some(search_source_name.clone());
                }

                match enabled_sources.len() {
                    1 => return Ok(enabled_sources[0].clone()),
                    n if n > 1 => {
                        return err!(
                            ErrorCode::InvalidConfig,
                            "Found ambiguous source('{}') pointing to multiple nodes: [{}]",
                            source_table_name,
                            res.iter()
                                .map(|(r, _, _)| format!("'{r}'"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    _ => {}
                };
            }
        }

        match disabled_source_name {
            Some(disabled_source_name) => err!(
                ErrorCode::DisabledDependency,
                "Attempted to use disabled source '{}'",
                disabled_source_name
            ),
            None => err!(
                ErrorCode::DependencyNotFound,
                "Source '{}' not found in project. Searched for '{}'",
                source_table_name,
                search_source_names.join(", ")
            ),
        }
    }

    /// Lookup a function by package name and function name
    fn lookup_function(
        &self,
        target_package: &Option<String>,
        function_name: &str,
        node_package: &Option<String>,
    ) -> FsResult<(String, MinijinjaValue, ModelStatus)> {
        // Create a list of packages to search, where None means to
        // search non-package limited names
        let search_packages =
            self.search_packages(target_package.as_deref(), node_package.as_deref());

        let mut enabled_function: Option<(String, MinijinjaValue, ModelStatus)> = None;
        let mut disabled_function: Option<(String, MinijinjaValue, ModelStatus)> = None;
        let mut search_function_names: Vec<String> = Vec::new();

        for maybe_package in search_packages.iter() {
            // If this is a package, use the package name + function_name to search
            let search_function_name = if let Some(package_name) = maybe_package {
                format!("{}.{}", package_name.clone(), function_name)
            } else {
                // If this is not a package, just use the function_name to search
                function_name.to_string()
            };
            search_function_names.push(search_function_name.clone());

            if let Some(res) = self.functions.get(&search_function_name) {
                let (enabled_functions, disabled_functions): (Vec<_>, Vec<_>) = res
                    .iter()
                    .partition(|(_, _, status)| *status != ModelStatus::Disabled);

                // We got a function or we wouldn't be here
                if !disabled_functions.is_empty() {
                    disabled_function = Some(disabled_functions[0].clone());
                }

                match enabled_functions.len() {
                    // If there is one enabled function, use it
                    1 => {
                        enabled_function = Some(enabled_functions[0].clone());
                        break;
                    }
                    n if n > 1 => {
                        // More than one enabled function with the same name, issue error
                        return err!(
                            ErrorCode::InvalidConfig,
                            "Found ambiguous function('{}') pointing to multiple nodes: [{}]",
                            function_name,
                            res.iter()
                                .map(|(r, _, _)| format!("'{r}'"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                    // If there are no enabled functions, continue to next package
                    _ => {}
                };
            }
        }

        // If function not found issue error
        match enabled_function {
            Some(function_result) => Ok(function_result),
            None => {
                if disabled_function.is_some() {
                    err!(
                        ErrorCode::DisabledDependency,
                        "Attempted to use disabled function '{}'",
                        function_name
                    )
                } else {
                    err!(
                        ErrorCode::InvalidConfig,
                        "Function '{}' not found in project. Searched for '{}'",
                        function_name,
                        search_function_names.join(", ")
                    )
                }
            }
        }
    }

    fn update_ref_with_deferral(
        &mut self,
        node: &dyn InternalDbtNodeAttributes,
        adapter_type: AdapterType,
        is_frontier: bool,
    ) -> FsResult<()> {
        if node.resource_type() == NodeType::Function {
            let package_name = node.package_name();
            let function_name = node.name();
            let unique_id = node.unique_id();
            let deferred_function =
                create_function_object_from_node(adapter_type, node)?.into_value();

            let function_entry = self.functions.entry(function_name.clone()).or_default();
            Self::set_deferred_function(function_entry, &unique_id, &deferred_function);

            let package_function_entry = self
                .functions
                .entry(format!("{package_name}.{function_name}"))
                .or_default();
            Self::set_deferred_function(package_function_entry, &unique_id, &deferred_function);

            return Ok(());
        }

        let package_name = &node.package_name();
        let model_name = node.name();
        let unique_id = node.unique_id();
        let (maybe_version, maybe_latest_version) = if node.resource_type() == NodeType::Model {
            (node.version(), node.latest_version())
        } else {
            (None, None)
        };

        let deferred_relation = RelationObject::new_with_filter(
            create_ref_relation(node, adapter_type, Some(self.run_filter.clone()))?.into(),
            self.run_filter.clone(),
            node.event_time(),
        )
        .into_value();

        if maybe_version == maybe_latest_version {
            let ref_entry = self.refs.entry(model_name.clone()).or_default();
            Self::set_deferred_relation(
                ref_entry,
                &unique_id,
                &deferred_relation,
                is_frontier,
                node,
            );

            let package_ref_entry = self
                .refs
                .entry(format!("{package_name}.{model_name}"))
                .or_default();
            Self::set_deferred_relation(
                package_ref_entry,
                &unique_id,
                &deferred_relation,
                is_frontier,
                node,
            );
        }

        if let Some(version) = maybe_version {
            let model_name_with_version = format!("{model_name}.v{version}");
            let versioned_ref_entry = self
                .refs
                .entry(model_name_with_version.to_owned())
                .or_default();
            Self::set_deferred_relation(
                versioned_ref_entry,
                &unique_id,
                &deferred_relation,
                is_frontier,
                node,
            );

            let package_versioned_ref_entry = self
                .refs
                .entry(format!("{package_name}.{model_name_with_version}"))
                .or_default();
            Self::set_deferred_relation(
                package_versioned_ref_entry,
                &unique_id,
                &deferred_relation,
                is_frontier,
                node,
            );
        }

        Ok(())
    }

    fn compile_or_test(&self) -> bool {
        self.compile_or_test
    }

    fn set_defer_context(
        &mut self,
        node_introspections: HashMap<String, IntrospectionKind>,
        has_analyzed_schema: HashSet<String>,
        nodes_materialized: HashSet<String>,
    ) {
        self.node_introspections = node_introspections;
        self.has_analyzed_schema = has_analyzed_schema;
        self.nodes_materialized = nodes_materialized;
    }

    fn prefers_deferred(&self, current_node_id: &str, upstream_id: &str) -> bool {
        if self.compile_or_test {
            let ik = self
                .node_introspections
                .get(current_node_id)
                .copied()
                .unwrap_or_default();
            if ik == IntrospectionKind::None {
                return false;
            }
            if ik.is_unsafe() {
                return true;
            }
            // Safe introspection: defer if upstream won't have analyzed schema
            !self.has_analyzed_schema.contains(upstream_id)
        } else {
            // Run path: defer if upstream won't be materialized
            !self.nodes_materialized.contains(upstream_id)
        }
    }
}

/// Resolve the dependencies for a model
/// Returns a set of node unique_ids that had resolution errors
#[allow(clippy::cognitive_complexity)]
pub fn resolve_dependencies(
    nodes: &mut Nodes,
    disabled_nodes: &mut Nodes,
    operations: &mut dbt_schemas::state::Operations,
    node_resolver: &NodeResolver,
) -> HashSet<String> {
    let mut tests_to_disable = Vec::new();
    let mut exposures_to_disable = Vec::new();
    let mut nodes_with_errors = HashSet::new();

    // First pass: identify tests and exposures with disabled dependencies
    for node in nodes.iter_values_mut() {
        // Clone needed values first to avoid borrowing issues
        let node_path = node.common().path.clone();
        let node_package_name = node.package_name();
        let node_unique_id = node.unique_id();
        let is_test = node.is_test();
        let is_exposure = node.resource_type() == NodeType::Exposure;

        let node_base = node.base_mut();

        // Skip nodes already marked disabled (e.g. tests on disabled models).
        // Their deps are irrelevant — they can never run — and resolving them
        // would produce spurious NodeNotFoundOrDisabled warnings.
        if !node_base.enabled {
            continue;
        }

        let mut has_disabled_or_missing_dependency = false;

        // Check refs
        let node_package_name_value = &Some(node_package_name.clone());
        for DbtRef {
            name,
            package,
            version,
            location,
        } in node_base.refs.iter()
        {
            let location = if let Some(location) = location {
                location
                    .clone()
                    .with_file(Arc::new(node_path.to_path_buf()))
            } else {
                CodeLocationWithFile::default()
            };
            match node_resolver.lookup_ref(
                package,
                name,
                &version.as_ref().map(|v| v.to_string()),
                node_package_name_value,
            ) {
                Ok((dependency_id, _, _, _)) => {
                    // Check for self-reference
                    if dependency_id == node_unique_id {
                        let err_with_loc = fs_err!(
                            ErrorCode::CyclicDependency,
                            "Model '{}' cannot reference itself",
                            name
                        )
                        .with_location(location);
                        emit_error_log_from_fs_error(err_with_loc);
                    } else {
                        if !node_base.depends_on.nodes.contains(&dependency_id) {
                            node_base.depends_on.nodes.push(dependency_id.clone());
                        }
                        node_base
                            .depends_on
                            .nodes_with_ref_location
                            .push((dependency_id, location));
                    }
                }
                Err(e) => {
                    // A test whose ref target is merely disabled is excluded silently:
                    // dbt-core reports this at debug level only.
                    if is_test && e.code == ErrorCode::DisabledDependency {
                        has_disabled_or_missing_dependency = true;
                        emit_debug_log_message(format!("{node_unique_id} ({location}): {e}"));
                    } else if (is_test || is_exposure)
                        && let Some((warning, disable)) =
                            downgraded_node_dependency_warning(&e, location.clone())
                    {
                        // A missing target stays a user-visible warning, and the node is
                        // still excluded.
                        has_disabled_or_missing_dependency = disable;
                        emit_warn_log_from_fs_error(warning);
                    } else {
                        // Track this node as having an error (unresolved ref/source)
                        nodes_with_errors.insert(node_unique_id.clone());
                        let err_with_loc = e.with_location(location);
                        emit_error_log_from_fs_error(err_with_loc);
                    }
                }
            };
        }

        // Check sources
        for DbtSourceWrapper { source, location } in node_base.sources.iter() {
            // Source is &Vec<String> (first two elements are source and table)
            let source_name = source[0].clone();
            let table_name = source[1].clone();

            let location = if let Some(location) = location {
                location
                    .clone()
                    .with_file(Arc::new(node_path.to_path_buf()))
            } else {
                CodeLocationWithFile::default()
            };

            match node_resolver.lookup_source(&node_package_name, &source_name, &table_name) {
                Ok((dependency_id, _, _)) => {
                    if !node_base.depends_on.nodes.contains(&dependency_id) {
                        node_base.depends_on.nodes.push(dependency_id.clone());
                    }
                    node_base
                        .depends_on
                        .nodes_with_ref_location
                        .push((dependency_id, location));
                }
                Err(e) => {
                    // For tests and exposures, warn on missing or disabled dependencies instead of erroring
                    if (is_test || is_exposure)
                        && let Some((warning, disable)) =
                            downgraded_node_dependency_warning(&e, location.clone())
                    {
                        // Both cases exclude the test. Unlike its ref path, dbt-core warns for
                        // a disabled source too, so this stays user-visible.
                        has_disabled_or_missing_dependency = disable;
                        emit_warn_log_from_fs_error(warning);
                    } else {
                        // Track this node as having an error (unresolved ref/source)
                        nodes_with_errors.insert(node_unique_id.clone());
                        let err_with_loc = e.with_location(location);
                        emit_error_log_from_fs_error(err_with_loc);
                    }
                }
            };
        }

        // Check functions
        for DbtRef {
            name,
            package,
            version: _,
            location,
        } in node_base.functions.iter()
        {
            let location = if let Some(location) = location {
                location
                    .clone()
                    .with_file(Arc::new(node_path.to_path_buf()))
            } else {
                CodeLocationWithFile::default()
            };

            match node_resolver.lookup_function(package, name, node_package_name_value) {
                Ok((dependency_id, _, _)) => {
                    if !node_base.depends_on.nodes.contains(&dependency_id) {
                        node_base.depends_on.nodes.push(dependency_id.clone());
                    }
                    node_base
                        .depends_on
                        .nodes_with_ref_location
                        .push((dependency_id, location));
                }
                Err(e) => {
                    // Check if this is a disabled dependency error for tests or exposures
                    if (is_test || is_exposure) && e.code == ErrorCode::DisabledDependency {
                        has_disabled_or_missing_dependency = true;
                        let err_with_loc = e.with_location(location);
                        emit_warn_log_from_fs_error(err_with_loc);
                    } else {
                        // Track this node as having an error (unresolved function)
                        nodes_with_errors.insert(node_unique_id.clone());
                        let err_with_loc = e.with_location(location);
                        emit_error_log_from_fs_error(err_with_loc);
                    }
                }
            };
        }

        if is_test && has_disabled_or_missing_dependency {
            tests_to_disable.push(node_unique_id.clone());
        }

        if is_exposure && has_disabled_or_missing_dependency {
            exposures_to_disable.push(node_unique_id);
        }
    }

    // Second pass: dbt-core disables such a test in place, so it stays in `nodes` with
    // `enabled = false`. Mirrors the generic-test path in resolve_data_tests.rs.
    for test_id in &tests_to_disable {
        if let Some(node) = nodes.tests.get_mut(test_id) {
            let test = Arc::make_mut(node);
            test.__base_attr__.enabled = false;
            test.deprecated_config.enabled = Some(false);
        }
    }

    // Exposures still change buckets: dbt-core raises for them, and `retain_schedulable` omits
    // NodeType::Exposure, so one disabled in place would still be selected and scheduled.
    for exposure_id in &exposures_to_disable {
        if let Some(node) = nodes.exposures.remove(exposure_id) {
            disabled_nodes.exposures.insert(exposure_id.clone(), node);
        }
    }

    // Process operations (on_run_start and on_run_end)
    [&mut operations.on_run_start, &mut operations.on_run_end]
        .iter_mut()
        .for_each(|ops| {
            ops.iter_mut().for_each(|operation_spanned| {
                let mut operation = (**operation_spanned).clone();
                let operation_package = operation.__common_attr__.package_name.clone();
                let operation_unique_id = operation.__common_attr__.unique_id.clone();

                // Process refs
                operation.__base_attr__.refs.iter().for_each(|dbt_ref| {
                    let location = dbt_ref.location.as_ref().map_or_else(
                        CodeLocationWithFile::default,
                        |loc| {
                            loc.clone()
                                .with_file(Arc::new(operation.__common_attr__.path.to_path_buf()))
                        },
                    );

                    match node_resolver.lookup_ref(
                        &dbt_ref.package,
                        &dbt_ref.name,
                        &dbt_ref.version.as_ref().map(|v| v.to_string()),
                        &Some(operation_package.clone()),
                    ) {
                        Ok((dependency_id, _, _, _)) => {
                            if !operation
                                .__base_attr__
                                .depends_on
                                .nodes
                                .contains(&dependency_id)
                            {
                                operation
                                    .__base_attr__
                                    .depends_on
                                    .nodes
                                    .push(dependency_id.clone());
                            }
                            operation
                                .__base_attr__
                                .depends_on
                                .nodes_with_ref_location
                                .push((dependency_id, location));
                        }
                        Err(e) => {
                            nodes_with_errors.insert(operation_unique_id.clone());
                            let err_with_loc = e.with_location(location);
                            emit_error_log_from_fs_error(err_with_loc);
                        }
                    }
                });

                // Process sources
                operation
                    .__base_attr__
                    .sources
                    .iter()
                    .filter(|source_wrapper| source_wrapper.source.len() == 2)
                    .for_each(|source_wrapper| {
                        let source_name = &source_wrapper.source[0];
                        let table_name = &source_wrapper.source[1];
                        let location = source_wrapper.location.as_ref().map_or_else(
                            CodeLocationWithFile::default,
                            |loc| {
                                loc.clone().with_file(Arc::new(
                                    operation.__common_attr__.path.to_path_buf(),
                                ))
                            },
                        );

                        match node_resolver.lookup_source(
                            &operation_package,
                            source_name,
                            table_name,
                        ) {
                            Ok((dependency_id, _, _)) => {
                                if !operation
                                    .__base_attr__
                                    .depends_on
                                    .nodes
                                    .contains(&dependency_id)
                                {
                                    operation.__base_attr__.depends_on.nodes.push(dependency_id);
                                }
                            }
                            Err(e) => {
                                nodes_with_errors.insert(operation_unique_id.clone());
                                let err_with_loc = e.with_location(location);
                                emit_error_log_from_fs_error(err_with_loc);
                            }
                        }
                    });

                // Replace with updated operation
                *operation_spanned = operation_spanned.clone().map(|_| operation);
            });
        });

    // Return the set of nodes that had resolution errors
    nodes_with_errors
}

/// Info about a model with a deprecation_date, used by [`check_for_model_deprecations`].
struct DeprecatedModelInfo {
    is_past: bool,
    deprecation_date: String,
    name: String,
    version: Option<String>,
    latest_version: Option<String>,
    package_name: String,
}

/// Check for model deprecations and emit appropriate warnings.
///
/// This implements three warning cases matching dbt-core behavior:
/// 1. DeprecatedModel (I065): Model's own deprecation_date is in the past
/// 2. UpcomingReferenceDeprecation (I066): A model references another model with a future deprecation_date
/// 3. DeprecatedReference (I067): A model references another model with a past deprecation_date
pub fn check_for_model_deprecations(nodes: &Nodes) {
    let mut deprecated_models: BTreeMap<String, DeprecatedModelInfo> = BTreeMap::new();

    for (uid, model) in &nodes.models {
        if let Some(dep_date_str) = &model.__model_attr__.deprecation_date {
            let is_past = parse_deprecation_date(dep_date_str)
                .is_some_and(|deprecation_date| deprecation_date < Utc::now());
            deprecated_models.insert(
                uid.clone(),
                DeprecatedModelInfo {
                    is_past,
                    deprecation_date: dep_date_str.clone(),
                    name: model.__common_attr__.name.clone(),
                    version: model.__model_attr__.version.as_ref().map(|v| v.to_string()),
                    latest_version: model
                        .__model_attr__
                        .latest_version
                        .as_ref()
                        .map(|v| v.to_string()),
                    package_name: model.__common_attr__.package_name.clone(),
                },
            );
        }
    }

    // Case 1: DeprecatedModel - model's own deprecation_date is in the past
    for info in deprecated_models.values() {
        if info.is_past {
            let version_str = info
                .version
                .as_ref()
                .map(|v| format!(".v{v}"))
                .unwrap_or_default();
            let msg = format!(
                "Model {}{} has passed its deprecation date of {}. \
                 This model should be disabled or removed.",
                info.name, version_str, info.deprecation_date
            );
            emit_warn_log_message(ErrorCode::DeprecatedModel, msg);
        }
    }

    // Cases 2 & 3: Check model nodes that reference deprecated models
    for model in nodes.models.values() {
        let child_name = &model.__common_attr__.name;
        for dep_uid in &model.__base_attr__.depends_on.nodes {
            if let Some(info) = deprecated_models.get(dep_uid) {
                let ref_version_str = info
                    .version
                    .as_ref()
                    .map(|v| format!(".v{v}"))
                    .unwrap_or_default();

                if info.is_past {
                    // Case 2: DeprecatedReference (I067)
                    let mut msg = format!(
                        "While compiling '{}': Found a reference to {}{}, \
                         which was deprecated on '{}'. ",
                        child_name, info.name, ref_version_str, info.deprecation_date
                    );
                    if let Some(ref_version) = &info.version {
                        if info
                            .latest_version
                            .as_ref()
                            .is_some_and(|lv| lv != ref_version)
                        {
                            msg.push_str(&format!(
                                "A new version of '{}' is available. Migrate now: \
                                 {{{{ ref('{}', '{}', v='{}') }}}}.",
                                info.name,
                                info.package_name,
                                info.name,
                                info.latest_version.as_ref().unwrap()
                            ));
                        }
                    }
                    emit_warn_log_message(ErrorCode::DeprecatedReference, msg);
                } else {
                    // Case 3: UpcomingReferenceDeprecation (I066)
                    let mut msg = format!(
                        "While compiling '{}': Found a reference to {}{}, \
                         which is slated for deprecation on '{}'. ",
                        child_name, info.name, ref_version_str, info.deprecation_date
                    );
                    if let Some(ref_version) = &info.version {
                        if info
                            .latest_version
                            .as_ref()
                            .is_some_and(|lv| lv != ref_version)
                        {
                            msg.push_str(&format!(
                                "A new version of '{}' is available. Try it out: \
                                 {{{{ ref('{}', '{}', v='{}') }}}}.",
                                info.name,
                                info.package_name,
                                info.name,
                                info.latest_version.as_ref().unwrap()
                            ));
                        }
                    }
                    emit_warn_log_message(ErrorCode::UpcomingReferenceDeprecation, msg);
                }
            }
        }
    }
}

/// Create a FunctionObject from a node (specifically for dbt functions)
pub fn create_function_object_from_node(
    adapter_type: AdapterType,
    node: &dyn InternalDbtNodeAttributes,
) -> FsResult<FunctionObject> {
    let relation = create_relation(
        adapter_type,
        node.database(),
        node.schema(),
        Some(node.base().alias.clone()),
        Some(RelationType::from(node.materialized())),
        node.quoting(),
    )?;

    // Create the qualified function name
    let rendered = relation.render_self_as_str();
    Ok(FunctionObject::new(rendered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::common::{DbtQuoting, NodeDependsOn};
    use dbt_schemas::schemas::manifest::{
        DbtManifest, DbtNode, ManifestMetadata, ManifestModel, nodes_from_dbt_manifest,
    };
    use dbt_schemas::schemas::nodes::{
        CommonAttributes, DbtModel, DbtModelAttr, NodeBaseAttributes,
    };
    use dbt_schemas::state::Operations;
    use std::sync::Arc;

    /// Helper to create a minimal DbtModel with the given name, unique_id, and optional deprecation_date.
    fn make_model(
        unique_id: &str,
        name: &str,
        package_name: &str,
        deprecation_date: Option<&str>,
        depends_on_ids: Vec<String>,
    ) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __common_attr__: CommonAttributes {
                unique_id: unique_id.to_string(),
                name: name.to_string(),
                package_name: package_name.to_string(),
                ..Default::default()
            },
            __base_attr__: NodeBaseAttributes {
                depends_on: NodeDependsOn {
                    nodes: depends_on_ids,
                    ..Default::default()
                },
                ..Default::default()
            },
            __model_attr__: DbtModelAttr {
                deprecation_date: deprecation_date.map(|s| s.to_string()),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    const PACKAGE_SOURCE_ID: &str = "source.source_consumer.landing.source_feed";
    const ROOT_SOURCE_ID: &str = "source.root_project.landing.source_feed";
    const PACKAGE_FUNCTION_ID: &str = "function.source_consumer.shared";
    const ROOT_FUNCTION_ID: &str = "function.root_project.shared";

    fn insert_test_source(
        node_resolver: &mut NodeResolver,
        package_name: &str,
        unique_id: &str,
        status: ModelStatus,
    ) {
        for key in [
            format!("{package_name}.landing.source_feed"),
            "landing.source_feed".to_string(),
        ] {
            node_resolver.sources.entry(key).or_default().push((
                unique_id.to_string(),
                MinijinjaValue::UNDEFINED,
                status,
            ));
        }
    }

    fn insert_test_function(node_resolver: &mut NodeResolver, package_name: &str, unique_id: &str) {
        for key in [format!("{package_name}.shared"), "shared".to_string()] {
            node_resolver.functions.entry(key).or_default().push((
                unique_id.to_string(),
                MinijinjaValue::UNDEFINED,
                ModelStatus::Enabled,
            ));
        }
    }

    fn resolver_with_duplicate_source(package_search_order: PackageSearchOrder) -> NodeResolver {
        let mut node_resolver = NodeResolver {
            root_package_name: "root_project".to_string(),
            package_search_order,
            ..Default::default()
        };
        for (package_name, unique_id) in [
            ("source_consumer", PACKAGE_SOURCE_ID),
            ("root_project", ROOT_SOURCE_ID),
        ] {
            insert_test_source(
                &mut node_resolver,
                package_name,
                unique_id,
                ModelStatus::Enabled,
            );
        }
        node_resolver
    }

    #[test]
    fn databricks_relation_policy_tests() {
        let mut model = make_model(
            "model.root_project.orders",
            "orders",
            "root_project",
            None,
            vec![],
        );
        let base = &mut Arc::make_mut(&mut model).__base_attr__;
        base.adapter = AdapterType::LakeCompute;
        base.database = "Cat".to_string();
        base.schema = "Sch".to_string();
        base.alias = "Tbl".to_string();
        base.effective_propagation_target = Some(AdapterType::Databricks);

        let mut resolver = NodeResolver::default();
        resolver
            .insert_ref(
                model.as_ref(),
                AdapterType::LakeCompute,
                ModelStatus::Enabled,
                false,
            )
            .unwrap();
        let (relation, deferred) = resolver
            .lookup_self_relation("model.root_project.orders", "root_project", "orders", &None)
            .unwrap();
        assert_eq!(relation.to_string(), "cat.sch.tbl");
        assert_eq!(
            relation
                .downcast_object_ref::<RelationObject>()
                .expect("ref should be a relation object")
                .inner()
                .adapter_type(),
            AdapterType::LakeCompute
        );
        assert!(
            relation
                .downcast_object_ref::<RelationObject>()
                .expect("ref should be a relation object")
                .inner()
                .is_table()
        );
        assert!(deferred.is_none());

        let mut snowflake_resolver = NodeResolver::default();
        snowflake_resolver
            .insert_ref(
                model.as_ref(),
                AdapterType::Snowflake,
                ModelStatus::Enabled,
                false,
            )
            .unwrap();
        let (relation, deferred) = snowflake_resolver
            .lookup_self_relation("model.root_project.orders", "root_project", "orders", &None)
            .unwrap();
        assert_eq!(
            relation
                .downcast_object_ref::<RelationObject>()
                .expect("ref should be a relation object")
                .inner()
                .get_canonical_fqn()
                .unwrap()
                .to_string(),
            "cat.sch.tbl"
        );
        assert!(deferred.is_none());
    }

    #[test]
    fn deferred_manifest_databricks_ref_uses_lake_compute_canonical_namespace() {
        let mut manifest = DbtManifest {
            metadata: ManifestMetadata {
                adapter_type: "snowflake".to_string(),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut manifest_model = ManifestModel::default();
        manifest_model.__common_attr__.unique_id = "model.root_project.orders".to_string();
        manifest_model.__common_attr__.name = "orders".to_string();
        manifest_model.__common_attr__.package_name = "root_project".to_string();
        manifest_model.__common_attr__.database = "Cat".to_string();
        manifest_model.__common_attr__.schema = "Sch".to_string();
        manifest_model.__base_attr__.alias = "Tbl".to_string();
        manifest_model.__base_attr__.canonical_relation_adapter = Some(AdapterType::Databricks);

        manifest.nodes = [(
            "model.root_project.orders".to_string(),
            DbtNode::Model(manifest_model),
        )]
        .into_iter()
        .collect();
        let model = nodes_from_dbt_manifest(
            manifest,
            DbtQuoting {
                database: Some(false),
                identifier: Some(false),
                schema: Some(false),
                snowflake_ignore_case: Some(false),
            },
        )
        .models
        .into_iter()
        .next()
        .map(|(_, model)| model)
        .expect("manifest model should be rehydrated");
        assert_eq!(model.node_adapter(), AdapterType::LakeCompute);

        let mut resolver = NodeResolver::default();
        resolver
            .insert_ref(
                model.as_ref(),
                AdapterType::Snowflake,
                ModelStatus::Enabled,
                false,
            )
            .unwrap();
        resolver
            .update_ref_with_deferral(model.as_ref(), AdapterType::Snowflake, true)
            .unwrap();

        let (relation, deferred) = resolver
            .lookup_self_relation("model.root_project.orders", "root_project", "orders", &None)
            .unwrap();
        let deferred = deferred.expect("deferred ref should be populated");
        for value in [relation, deferred] {
            assert_eq!(value.to_string(), "cat.sch.tbl");
            let relation = value
                .downcast_object_ref::<RelationObject>()
                .expect("ref should be a relation object")
                .inner();
            assert_eq!(
                relation.get_canonical_fqn().unwrap().to_string(),
                "cat.sch.tbl"
            );
        }
    }

    #[test]
    fn lookup_source_honors_package_search_order() {
        for (package_search_order, expected) in [
            (PackageSearchOrder::RootFirst, ROOT_SOURCE_ID),
            (PackageSearchOrder::NodePackageFirst, PACKAGE_SOURCE_ID),
        ] {
            let node_resolver = resolver_with_duplicate_source(package_search_order);
            let (unique_id, _, _) = node_resolver
                .lookup_source("source_consumer", "landing", "source_feed")
                .expect("source should resolve");
            assert_eq!(unique_id, expected, "order: {package_search_order:?}");
        }
    }

    #[test]
    fn lookup_source_falls_through_disabled_root_definition() {
        let mut node_resolver = NodeResolver {
            root_package_name: "root_project".to_string(),
            package_search_order: PackageSearchOrder::RootFirst,
            ..Default::default()
        };
        insert_test_source(
            &mut node_resolver,
            "source_consumer",
            PACKAGE_SOURCE_ID,
            ModelStatus::Enabled,
        );
        insert_test_source(
            &mut node_resolver,
            "root_project",
            ROOT_SOURCE_ID,
            ModelStatus::Disabled,
        );

        let (unique_id, _, _) = node_resolver
            .lookup_source("source_consumer", "landing", "source_feed")
            .expect("the package's enabled source should still resolve");

        assert_eq!(unique_id, PACKAGE_SOURCE_ID);
    }

    #[test]
    fn lookup_ref_honors_package_search_order() {
        let package_ref_id = "model.source_consumer.shared";
        let root_ref_id = "model.root_project.shared";

        let resolver = |package_search_order| {
            let mut node_resolver = NodeResolver {
                root_package_name: "root_project".to_string(),
                package_search_order,
                ..Default::default()
            };
            for (package_name, unique_id) in [
                ("source_consumer", package_ref_id),
                ("root_project", root_ref_id),
            ] {
                for key in [format!("{package_name}.shared"), "shared".to_string()] {
                    node_resolver.refs.entry(key).or_default().push((
                        unique_id.to_string(),
                        MinijinjaValue::UNDEFINED,
                        ModelStatus::Enabled,
                        None,
                    ));
                }
            }
            node_resolver
        };

        let lookup = |node_resolver: &NodeResolver| {
            node_resolver
                .lookup_ref(&None, "shared", &None, &Some("source_consumer".to_string()))
                .expect("ref should resolve")
                .0
        };

        assert_eq!(
            lookup(&resolver(PackageSearchOrder::NodePackageFirst)),
            package_ref_id
        );
        assert_eq!(
            lookup(&resolver(PackageSearchOrder::RootFirst)),
            root_ref_id
        );
    }

    #[test]
    fn function_dependency_uses_runtime_search_order() {
        let mut node_resolver = NodeResolver {
            root_package_name: "root_project".to_string(),
            package_search_order: PackageSearchOrder::RootFirst,
            ..Default::default()
        };
        insert_test_function(&mut node_resolver, "source_consumer", PACKAGE_FUNCTION_ID);
        insert_test_function(&mut node_resolver, "root_project", ROOT_FUNCTION_ID);

        let mut model = make_model(
            "model.source_consumer.consumer",
            "consumer",
            "source_consumer",
            None,
            vec![],
        );
        let base = &mut Arc::make_mut(&mut model).__base_attr__;
        base.enabled = true;
        base.functions = vec![DbtRef {
            name: "shared".to_string(),
            package: None,
            version: None,
            location: None,
        }];

        let model_id = model.__common_attr__.unique_id.clone();
        let mut nodes = Nodes::default();
        nodes.models.insert(model_id.clone(), model);
        let mut disabled_nodes = Nodes::default();
        let mut operations = Operations::default();

        let errors = resolve_dependencies(
            &mut nodes,
            &mut disabled_nodes,
            &mut operations,
            &node_resolver,
        );
        assert!(errors.is_empty());

        let runtime_id = node_resolver
            .lookup_function(&None, "shared", &Some("source_consumer".to_string()))
            .expect("function should resolve")
            .0;
        assert_eq!(runtime_id, ROOT_FUNCTION_ID);
        assert_eq!(
            nodes.models[&model_id].__base_attr__.depends_on.nodes,
            vec![runtime_id]
        );
    }

    #[test]
    fn search_packages_mirrors_dbt_core_candidate_order() {
        let mut node_resolver = NodeResolver {
            root_package_name: "root_project".to_string(),
            ..Default::default()
        };

        assert_eq!(
            node_resolver.search_packages(Some("other_package"), Some("source_consumer")),
            vec![Some("other_package".to_string())]
        );
        assert_eq!(node_resolver.search_packages(None, None), vec![None]);
        assert_eq!(
            node_resolver.search_packages(None, Some("root_project")),
            vec![Some("root_project".to_string()), None]
        );
        // Default: the calling node's own package first
        assert_eq!(
            node_resolver.search_packages(None, Some("source_consumer")),
            vec![
                Some("source_consumer".to_string()),
                Some("root_project".to_string()),
                None
            ]
        );

        // dbt-core's order, once the flag is turned off
        node_resolver.package_search_order = PackageSearchOrder::RootFirst;
        assert_eq!(
            node_resolver.search_packages(None, Some("source_consumer")),
            vec![
                Some("root_project".to_string()),
                Some("source_consumer".to_string()),
                None
            ]
        );
    }

    // Flag/env precedence itself is covered by
    // `resolve_require_ref_searches_node_package_before_root` in dbt-common's io_args,
    // which has an injectable env lookup. Here we only pin the mapping onto the enum,
    // and that the `Default` stays dbt-core's order.
    #[test]
    fn package_search_order_maps_resolved_flag() {
        assert_eq!(
            PackageSearchOrder::from_node_package_first(true),
            PackageSearchOrder::NodePackageFirst
        );
        assert_eq!(
            PackageSearchOrder::from_node_package_first(false),
            PackageSearchOrder::RootFirst
        );
        assert_eq!(
            PackageSearchOrder::default(),
            PackageSearchOrder::NodePackageFirst
        );
    }

    #[test]
    fn downgraded_warning_maps_disabled_dependency() {
        let warning = downgraded_node_dependency_warning(
            &FsError::new(
                ErrorCode::DisabledDependency,
                "Attempted to use disabled ref 'x'",
            ),
            CodeLocationWithFile::default(),
        )
        .expect("disabled dependency should be downgraded");

        assert_eq!(warning.0.code, ErrorCode::NodeNotFoundOrDisabled);
        assert!(warning.1);
    }

    #[test]
    fn downgraded_warning_maps_missing_ref_dependency() {
        let warning = downgraded_node_dependency_warning(
            &FsError::new(
                ErrorCode::DependencyNotFound,
                "Ref 'missing_model' not found in project. Searched for 'missing_model'",
            ),
            CodeLocationWithFile::default(),
        )
        .expect("missing ref should be downgraded");

        assert_eq!(warning.0.code, ErrorCode::NodeNotFoundOrDisabled);
        assert!(warning.1);
    }

    #[test]
    fn downgraded_warning_does_not_map_ambiguous_ref_errors() {
        let warning = downgraded_node_dependency_warning(
            &FsError::new(
                ErrorCode::InvalidConfig,
                "Found ambiguous ref('x') pointing to multiple nodes: ['a', 'b']",
            ),
            CodeLocationWithFile::default(),
        );

        assert!(warning.is_none());
    }

    #[test]
    fn test_deprecated_model_warning() {
        // Case 1: Model with past deprecation_date emits DeprecatedModel warning
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.test.my_model".to_string(),
            make_model(
                "model.test.my_model",
                "my_model",
                "test",
                Some("1999-01-01"),
                vec![],
            ),
        );

        // This should not panic and should emit a warning
        check_for_model_deprecations(&nodes);
        // If we get here without panic, the function executed successfully.
        // The warning was emitted via the tracing system.
    }

    #[test]
    fn test_upcoming_reference_deprecation_warning() {
        // Case 2: Model refs another model with future deprecation_date
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.test.my_model".to_string(),
            make_model(
                "model.test.my_model",
                "my_model",
                "test",
                Some("2999-01-01"),
                vec![],
            ),
        );
        nodes.models.insert(
            "model.test.child".to_string(),
            make_model(
                "model.test.child",
                "child",
                "test",
                None,
                vec!["model.test.my_model".to_string()],
            ),
        );

        check_for_model_deprecations(&nodes);
    }

    #[test]
    fn test_deprecated_reference_warning() {
        // Case 3: Model refs another model with past deprecation_date
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.test.my_model".to_string(),
            make_model(
                "model.test.my_model",
                "my_model",
                "test",
                Some("1999-01-01"),
                vec![],
            ),
        );
        nodes.models.insert(
            "model.test.child".to_string(),
            make_model(
                "model.test.child",
                "child",
                "test",
                None,
                vec!["model.test.my_model".to_string()],
            ),
        );

        check_for_model_deprecations(&nodes);
    }

    #[test]
    fn test_no_deprecation_date_no_warnings() {
        // No deprecation_date set: no warnings should be emitted
        let mut nodes = Nodes::default();
        nodes.models.insert(
            "model.test.my_model".to_string(),
            make_model("model.test.my_model", "my_model", "test", None, vec![]),
        );

        check_for_model_deprecations(&nodes);
    }

    // Regression test for https://github.com/dbt-labs/fs/issues/11382:
    // push_or_replace_entry with override_existing=false must not insert a duplicate
    // when the same unique_id is already present. Cross-project publication nodes were
    // registered via from_dbt_nodes() and again during wave resolution, causing the same
    // unique_id to appear twice and triggering "Found ambiguous ref" panics.
    #[test]
    fn test_push_or_replace_entry_no_duplicate_when_override_false() {
        let mut entries: Vec<RefRecord> = Vec::new();
        let relation = MinijinjaValue::UNDEFINED;
        let unique_id = "model.cross_proj.pub_model";

        NodeResolver::push_or_replace_entry(
            &mut entries,
            unique_id,
            &relation,
            ModelStatus::Enabled,
            false,
        );
        NodeResolver::push_or_replace_entry(
            &mut entries,
            unique_id,
            &relation,
            ModelStatus::Enabled,
            false,
        );

        assert_eq!(
            entries.len(),
            1,
            "duplicate unique_id must not be inserted when override_existing=false"
        );
    }

    // Regression test: a disabled and an enabled entry for the same model name but same
    // unique_id must both be retained so ref resolution can correctly find the enabled one.
    #[test]
    fn test_push_or_replace_entry_allows_different_status_for_same_unique_id() {
        let mut entries: Vec<RefRecord> = Vec::new();
        let relation = MinijinjaValue::UNDEFINED;
        let unique_id = "model.test.my_model_2";

        NodeResolver::push_or_replace_entry(
            &mut entries,
            unique_id,
            &relation,
            ModelStatus::Disabled,
            false,
        );
        NodeResolver::push_or_replace_entry(
            &mut entries,
            unique_id,
            &relation,
            ModelStatus::Enabled,
            false,
        );

        assert_eq!(
            entries.len(),
            2,
            "disabled and enabled entries for the same unique_id must both be kept"
        );
    }
}
