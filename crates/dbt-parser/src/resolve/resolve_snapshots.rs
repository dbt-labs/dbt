use super::resolve_properties::MinimalPropertiesEntry;
use crate::args::ResolveArgs;
use crate::dbt_project_config::{
    ProjectConfigResolver, RootProjectConfigs, disallow_plus_prefix_from_flags,
    init_project_config, strip_resource_paths_from_ref_path,
};
use crate::renderer::{
    RenderCtx, RenderCtxInner, SqlFileRenderResult, collect_adapter_identifiers_detect_unsafe,
    render_unresolved_sql_files,
};
use crate::resolve::resolve_tests::persist_generic_data_tests::TestableNodeTrait;
use crate::resolve::resolve_tests::persist_generic_data_tests::{
    TestUnrenderedConfigs, extract_test_unrendered_configs,
};
use crate::resolve::resolve_utils::{
    build_unrendered_config, err_resource_name_has_spaces, extract_config_map, validate_compute,
    validate_node_adapter,
};
use crate::resolve::yaml_field_utils;
use crate::sql_file_info::SqlFileInfo;
use crate::utils::{
    RelationComponents, extract_resource_config_from_raw_project, get_original_file_path,
    get_snapshot_fqn, parse_unrendered_config, update_node_relation_components,
};
use crate::validation::check_node_static_analysis;
use dbt_adapter_core::AdapterType;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::DBT_SNAPSHOTS_DIR_NAME;
use dbt_common::error::AbstractLocation;
use dbt_common::io_args::{StaticAnalysisKind, StaticAnalysisOffReason};
use dbt_common::path::DbtPath;
use dbt_common::tokiofs;
use dbt_common::tracing::dbt_emit::{emit_error_log_from_fs_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, FsResult, fs_err, stdfs, unexpected_fs_err};
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::DefaultJinjaTypeCheckEventListenerFactory;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::serde::into_typed_with_jinja;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::common::{
    DbtChecksum, DbtQuoting, ModelFreshnessRules, NodeDependsOn,
    conform_normalized_snapshot_raw_code_to_mantle_format, normalize_sql,
};
use dbt_schemas::schemas::dbt_column::process_columns;
use dbt_schemas::schemas::macros::DbtMacro;
use dbt_schemas::schemas::nodes::AdapterAttr;
use dbt_schemas::schemas::project::SnapshotConfig;
use dbt_schemas::schemas::properties::SnapshotProperties;
use dbt_schemas::schemas::ref_and_source::{DbtRef, DbtSourceWrapper};
use dbt_schemas::schemas::{
    CommonAttributes, DbtSnapshot, DbtSnapshotAttr, InternalDbtNode, IntrospectionKind,
    NodeBaseAttributes, NodePathKind,
};
use dbt_schemas::state::{
    DbtAsset, DbtPackage, DbtRuntimeConfig, GenericTestAsset, ModelStatus, NodeResolverTracker,
};
use indexmap::IndexMap;
use minijinja::Value as MinijinjaValue;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

#[allow(clippy::too_many_arguments, clippy::cognitive_complexity)]
pub async fn resolve_snapshots(
    arg: &ResolveArgs,
    package: &DbtPackage,
    package_quoting: DbtQuoting,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    mut snapshot_properties: BTreeMap<String, MinimalPropertiesEntry>,
    macros: &BTreeMap<String, DbtMacro>,
    database: &str,
    schema: &str,
    default_adapter: AdapterType,
    adapter_quoting: &IndexMap<AdapterType, DbtQuoting>,
    jinja_env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    node_resolver: &mut NodeResolver,
    collected_generic_tests: &mut Vec<GenericTestAsset>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    token: &CancellationToken,
) -> FsResult<(
    HashMap<String, Arc<DbtSnapshot>>,
    HashMap<String, Arc<DbtSnapshot>>,
)> {
    let mut snapshots: HashMap<String, Arc<DbtSnapshot>> = HashMap::new();
    let mut disabled_snapshots: HashMap<String, Arc<DbtSnapshot>> = HashMap::new();
    let jinja_type_checking_event_listener_factory =
        Arc::new(DefaultJinjaTypeCheckEventListenerFactory::default());
    let mut snapshots_with_execute: HashMap<String, DbtSnapshot> = HashMap::new();

    let dependency_package_name = if package.dbt_project.name != root_package.dbt_project.name {
        Some(package.dbt_project.name.as_str())
    } else {
        None
    };

    let is_dependency = dependency_package_name.is_some();
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "snapshots",
        default_adapter,
    )?;
    let raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "snapshots",
            default_adapter,
        )?)
    } else {
        None
    };

    let package_name = package.dbt_project.name.to_owned();

    // Create the `snapshots` directory
    let snapshots_dir = arg.io.out_dir.join(DBT_SNAPSHOTS_DIR_NAME);
    if !snapshots_dir.exists() {
        stdfs::create_dir_all(&snapshots_dir)?;
    }

    // Save snapshots to the `snapshots` directory
    let mut snapshot_files = Vec::new();
    let mut sql_defined_snapshots = Vec::new();
    // Map target path to original macro path for checksum recalculation
    let mut snapshot_original_paths: HashMap<PathBuf, PathBuf> = HashMap::new();
    let default_snapshots_path = vec![DBT_SNAPSHOTS_DIR_NAME.to_string()];
    let mut synthetic_snapshot_macro_uids: HashSet<String> = HashSet::new();
    for (macro_uid, macro_node) in macros {
        if macro_node.package_name == package_name && macro_uid.starts_with("snapshot.") {
            synthetic_snapshot_macro_uids.insert(macro_uid.clone());
            // Write the macro call to the `snapshots` directory
            let macro_call = format!("{{{{ {}() }}}}", macro_node.name);
            let macro_name = macro_node.name.clone();
            let snapshot_name = macro_name
                .strip_prefix("snapshot_")
                .expect("All snapshot macros should start with 'snapshot_'")
                .to_string();

            // Preserve file layout for proper fqn generation
            let original_relative_path = strip_resource_paths_from_ref_path(
                &macro_node.path,
                package
                    .dbt_project
                    .snapshot_paths
                    .as_ref()
                    .unwrap_or(&default_snapshots_path),
            );

            let target_path = PathBuf::from(DBT_SNAPSHOTS_DIR_NAME)
                .join(original_relative_path.with_file_name(format!("{snapshot_name}.sql")));
            let snapshot_path = arg.io.out_dir.join(&target_path);
            if let Some(parent) = snapshot_path.parent() {
                stdfs::create_dir_all(parent)?;
            }
            stdfs::write(snapshot_path, macro_call)?;

            // Track original path for checksum recalculation.
            snapshot_original_paths.insert(
                target_path.clone(),
                macro_node.get_node_path_abs(
                    NodePathKind::Definition,
                    &arg.io.in_dir,
                    &arg.io.out_dir,
                ),
            );

            let original_path = macro_node
                .get_node_definition_path(&arg.io.in_dir, &arg.io.out_dir)
                .into_owned();
            snapshot_files.push(DbtAsset {
                path: target_path.clone(),
                original_path,
                package_name: package_name.clone(),
                base_path: arg.io.out_dir.clone(),
            });
            sql_defined_snapshots.push(target_path);
        }
    }

    // Snapshot raw schema.yml config blocks before schema_values entries are nulled.
    let raw_schema_yml_configs: BTreeMap<String, BTreeMap<String, dbt_yaml::Value>> =
        snapshot_properties
            .iter()
            .filter_map(|(name, mpe)| {
                let config_map = extract_config_map(&mpe.schema_value)?;
                Some((name.clone(), config_map))
            })
            .collect();

    // Raw (unrendered) schema.yml test config blocks, consumed by `persist` to populate
    // generic tests' unrendered_config with their schema.yml config.
    let raw_test_configs: BTreeMap<String, TestUnrenderedConfigs> = snapshot_properties
        .iter()
        .map(|(name, mpe)| {
            (
                name.clone(),
                extract_test_unrendered_configs(&mpe.schema_value),
            )
        })
        .collect();

    // Save snapshot from yml to the `snapshots` directory
    for (snapshot_name, mpe) in snapshot_properties.iter_mut() {
        // if mpe.schema_value
        if !mpe.schema_value.is_null() {
            let mut schema_value =
                std::mem::replace(&mut mpe.schema_value, dbt_yaml::Value::null());
            // `description:` is documentation text and may contain Jinja-like
            // doc snippets the renderer cannot evaluate at parse time
            // (e.g. `{% raw %}{{ ref('x') }}{% endraw %}`). dbt-core never
            // renders descriptions; detach before `into_typed_with_jinja`
            // so the renderer doesn't see the string, then reattach the raw
            // value to the typed struct. Other fields (config, meta, …)
            // still go through Jinja as before.
            let raw_description =
                yaml_field_utils::detach_field_from_mapping(&mut schema_value, "description");
            let mut snapshot: SnapshotProperties = into_typed_with_jinja(
                schema_value,
                false,
                &jinja_env,
                base_ctx,
                &[],
                dependency_package_name,
                true,
            )?;
            if let Some(desc_value) = raw_description
                && let Some(s) = desc_value.as_str()
            {
                snapshot.description = Some(s.to_string());
            }

            if let Some(relation) = &snapshot.relation {
                let relation = relation.to_string();
                // check if the relation matches the pattern of ref(...)
                let relation = if relation.starts_with("ref(") || relation.starts_with("source(") {
                    format!("{{{{ {relation} }}}}")
                } else {
                    relation.to_owned()
                };
                // Write SQL for relation to the `snapshots` directory
                let sql = format!("select * from {relation}");

                // Preserve directory structure from the properties file path
                // This ensures FQN includes directory components for proper selector matching
                let resource_paths: Vec<String> = package.dbt_project.all_source_paths().clone();
                let original_relative_path =
                    strip_resource_paths_from_ref_path(&mpe.relative_path, &resource_paths);
                let target_path = PathBuf::from(DBT_SNAPSHOTS_DIR_NAME)
                    .join(
                        original_relative_path
                            .parent()
                            .unwrap_or_else(|| std::path::Path::new("")),
                    )
                    .join(format!("{snapshot_name}.sql"));
                let snapshot_path = arg.io.out_dir.join(&target_path);
                if let Some(parent) = snapshot_path.parent() {
                    stdfs::create_dir_all(parent)?;
                }
                stdfs::write(&snapshot_path, &sql)?;
                // Compute the original file path relative to in_dir
                // For package YAML snapshots, this includes the package path
                let original_path = get_original_file_path(
                    &package.package_root_path,
                    &arg.io.in_dir,
                    &mpe.relative_path,
                );
                let asset = DbtAsset {
                    path: target_path.clone(),
                    original_path: original_path.to_path_buf(),
                    package_name: package_name.clone(),
                    base_path: arg.io.out_dir.clone(),
                };
                snapshot_files.push(asset.to_owned());
            }
            // Put snapshot back in as it is unused
            let _ = std::mem::replace(
                &mut mpe.schema_value,
                dbt_yaml::to_value(snapshot).map_err(|e| {
                    unexpected_fs_err!("Failed to serialize snapshot properties: {e}")
                })?,
            );
        }
    }

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.snapshots.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.snapshots,
                DbtQuoting::default(),
                dependency_package_name,
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref()),
                default_adapter,
            )
        },
        default_adapter,
    )?
    .with_resolve_defaults((
        arg.static_analysis.unwrap_or_default(),
        root_package.dbt_project.sync.clone(),
    ));

    let render_ctx = RenderCtx {
        inner: Arc::new(RenderCtxInner {
            args: arg.clone(),
            root_project_name: root_package.dbt_project.name.clone(),
            config_resolver,
            package_quoting,
            uses_snapshot_fqn: true,
            defer_render_errors_to_compile: true,
            base_ctx: base_ctx.clone(),
            package_name: package_name.to_string(),
            adapter_type: default_adapter,
            database: database.to_string(),
            schema: schema.to_string(),
            // Must match the resource paths used for the node's own fqn below,
            // or the config the renderer resolves keys off a different path.
            resource_paths: package
                .dbt_project
                .snapshot_paths
                .as_ref()
                .unwrap_or(&default_snapshots_path)
                .clone(),
        }),
        jinja_env: jinja_env.clone(),
        runtime_config: runtime_config.clone(),
        root_runtime_config: root_runtime_config.clone(),
    };

    // Render the snapshots
    let mut snapshot_sql_resources_map =
        render_unresolved_sql_files::<SnapshotConfig, SnapshotProperties>(
            &render_ctx,
            &snapshot_files,
            &mut snapshot_properties,
            token,
            jinja_type_checking_event_listener_factory.clone(),
        )
        .await?;

    // make deterministic
    snapshot_sql_resources_map.sort_by(|a, b| {
        a.asset
            .path
            .file_name()
            .cmp(&b.asset.path.file_name())
            .then(a.asset.path.cmp(&b.asset.path))
    });

    let all_depends_on = jinja_type_checking_event_listener_factory
        .depends_on()
        .clone();

    for SqlFileRenderResult {
        asset: dbt_asset,
        sql_file_info,
        config: mut snapshot_config,
        raw_code,
        macro_spans: _macro_spans,
        properties: maybe_properties,
        status,
        render_error_deferred,
        patch_path,
        ..
    } in snapshot_sql_resources_map.into_iter()
    {
        {
            let error_path = &dbt_asset.original_path;
            let snapshot_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();
            if snapshot_name.contains(' ') {
                return Err(err_resource_name_has_spaces(snapshot_name, error_path));
            }

            // Recalculate checksum from original snapshot file.
            // Without doing this, the checksum will be different from the one from mantle since fusion
            // creates a new file for the snapshot that only contains a function call
            // to the original macro instead of the original macro itself.
            let recalculated_checksum =
                if let Some(original_path) = snapshot_original_paths.get(&dbt_asset.path) {
                    recalculate_snapshot_checksum(arg, original_path, &sql_file_info).await
                } else {
                    // Not a macro-based snapshot, use the checksum from sql_file_info
                    sql_file_info.checksum.clone()
                };

            let properties = if let Some(properties) = maybe_properties {
                properties
            } else {
                SnapshotProperties::empty(snapshot_name.to_owned())
            };
            let snapshot_name_span = snapshot_properties
                .get(snapshot_name)
                .and_then(|mpe| {
                    mpe.name_span.is_valid().then(|| {
                        dbt_common::Span::from_serde_span(
                            mpe.name_span.clone(),
                            mpe.relative_path.clone(),
                        )
                    })
                })
                .unwrap_or_default();

            let unique_id = format!("snapshot.{package_name}.{snapshot_name}");

            let columns = process_columns(
                properties.columns.as_ref(),
                snapshot_config.tags.inner().clone().map(|tags| tags.into()),
            )?;

            // Mirror dbt-core's per-definition-style snapshot fqn so that
            // `state:modified` comparisons against a dbt-core-produced manifest
            // don't see a spurious fqn difference. See `get_snapshot_fqn`.
            let snapshot_paths = package
                .dbt_project
                .snapshot_paths
                .as_ref()
                .unwrap_or(&default_snapshots_path);
            let fqn = get_snapshot_fqn(
                &package_name,
                &dbt_asset.path,
                &dbt_asset.original_path,
                snapshot_name,
                snapshot_paths,
            );

            let static_analysis = snapshot_config.static_analysis.clone();
            check_node_static_analysis(
                &snapshot_config,
                arg.static_analysis,
                &unique_id,
                dependency_package_name,
            );
            validate_compute(snapshot_config.compute, error_path)?;
            // See `resolve_models`: the flag overrides the config. A snapshot
            // selects explicitly -- it has no attached node to inherit from.
            validate_node_adapter(snapshot_config.adapter, error_path)?;
            let resolved_node_adapter = arg.adapter_override.or(snapshot_config.adapter);

            // See `resolve_models`: both remaining quoting layers depend on which
            // adapter the node runs on, which is only known after the config merge.
            // `propagate` comes straight off the node's own config. Unlike
            // `adapter` there is no target default to fall back to and nothing to
            // inherit: an unset `+propagate` means "publish nowhere".
            let selected_propagate: Vec<AdapterType> = snapshot_config
                .propagate
                .clone()
                .map(Into::into)
                .unwrap_or_default();
            let selected_adapter = resolved_node_adapter.unwrap_or(default_adapter);
            snapshot_config.quoting = resolve_package_quoting(
                Some(match adapter_quoting.get(&selected_adapter) {
                    Some(authored) => snapshot_config.quoting.filled_from(authored),
                    None => snapshot_config.quoting,
                }),
                selected_adapter,
            );

            if let Some(state) = &snapshot_config.state {
                ModelFreshnessRules::validate(state.lag_tolerance.as_ref()).map_err(|e| {
                    fs_err!(
                        code => ErrorCode::InvalidConfig,
                        loc => dbt_asset.path.clone(),
                        "{}",
                        e
                    )
                })?;
            }

            let macro_depends_on = all_depends_on
                .get(&format!("{package_name}.{snapshot_name}"))
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|uid| !synthetic_snapshot_macro_uids.contains(uid.as_str()))
                .collect();

            // For block-style snapshots (`{% snapshot foo %}...{% endsnapshot %}`),
            // fs rewrites the on-disk file to a stub macro call `{{ snapshot_foo() }}`
            // and renders that as raw_code. dbt-core writes the verbatim body between
            // the snapshot tags into raw_code instead (its same_body/state:modified
            // selectors depend on this). Re-read the original file here and extract
            // both the unrendered inline config and the snapshot block body in one go.
            let (raw_inline_config, snapshot_block_body) =
                if let Some(original_path) = snapshot_original_paths.get(&dbt_asset.path) {
                    match tokiofs::read_to_string(original_path).await {
                        Ok(sql) => {
                            let body = extract_snapshot_block_body(&sql);
                            let cfg = parse_unrendered_config(&sql, true);
                            (cfg, body)
                        }
                        Err(_) => (None, None),
                    }
                } else {
                    (None, None)
                };

            let unrendered_config = build_unrendered_config(
                &fqn,
                &raw_local_project_config,
                raw_root_project_cfg.as_ref(),
                raw_schema_yml_configs.get(snapshot_name),
                raw_inline_config.as_ref(),
                true,
                default_adapter,
            )?;

            // Create initial snapshot with default values
            let mut dbt_snapshot = DbtSnapshot {
                __common_attr__: CommonAttributes {
                    name: snapshot_name.to_string(),
                    package_name: package_name.clone(),
                    path: DbtPath::from(&dbt_asset.path),
                    name_span: snapshot_name_span,
                    raw_code: Some(snapshot_block_body.unwrap_or(raw_code)),
                    // The original file path where the snapshot was defined
                    // For package snapshots, this includes the package path (e.g., dbt_packages/my_pkg/snapshots/foo.sql)
                    original_file_path: DbtPath::from(&dbt_asset.original_path),
                    unique_id: unique_id.clone(),
                    fqn,
                    description: Some(properties.description.clone().unwrap_or_default()),
                    patch_path: patch_path.as_ref().map(DbtPath::from),
                    checksum: recalculated_checksum,
                    language: Some("sql".to_string()),
                    tags: snapshot_config
                        .tags
                        .inner()
                        .clone()
                        .map(Into::into)
                        .unwrap_or_default(),
                    classifiers: Default::default(),
                    meta: snapshot_config.meta.clone().unwrap_or_default(),
                },
                __base_attr__: NodeBaseAttributes {
                    adapter: selected_adapter,
                    propagate: selected_propagate,
                    database: "".to_owned(), // will be updated below
                    schema: "".to_owned(),   // will be updated below
                    alias: "".to_owned(),    // will be updated below
                    relation_name: None,     // will be updated below
                    columns,
                    depends_on: NodeDependsOn {
                        macros: macro_depends_on,
                        nodes: vec![],
                        nodes_with_ref_location: vec![],
                    },
                    compute: snapshot_config.compute,
                    enabled: snapshot_config.enabled,
                    extended_model: false,
                    persist_docs: snapshot_config.persist_docs.clone(),
                    materialized: snapshot_config.materialized.clone(),
                    quoting: snapshot_config
                        .quoting
                        .try_into()
                        .expect("DbtQuoting -> ResolvedQuoting conversion"),
                    quoting_ignore_case: snapshot_config
                        .quoting
                        .snowflake_ignore_case
                        .unwrap_or(false),
                    static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                        .then_some(StaticAnalysisOffReason::ConfiguredOff),
                    static_analysis,
                    refs: sql_file_info
                        .refs
                        .iter()
                        .map(|(model, project, version, location)| DbtRef {
                            name: model.to_owned(),
                            package: project.to_owned(),
                            version: version.clone(),
                            location: Some(location.with_file(&dbt_asset.path)),
                        })
                        .collect(),
                    unrendered_config,
                    functions: sql_file_info
                        .functions
                        .iter()
                        .map(|(function_name, package, location)| DbtRef {
                            name: function_name.to_owned(),
                            package: package.to_owned(),
                            version: None, // Functions don't have versions
                            location: Some(location.with_file(&dbt_asset.path)),
                        })
                        .collect(),
                    sources: sql_file_info
                        .sources
                        .iter()
                        .map(|(source, table, location)| DbtSourceWrapper {
                            source: vec![source.to_owned(), table.to_owned()],
                            location: Some(location.with_file(&dbt_asset.path)),
                        })
                        .collect(),
                    metrics: vec![],
                },
                __snapshot_attr__: DbtSnapshotAttr {
                    snapshot_meta_column_names: snapshot_config
                        .snapshot_meta_column_names
                        .clone()
                        .unwrap_or_default(),
                    introspection: if sql_file_info.this {
                        IntrospectionKind::This
                    } else {
                        IntrospectionKind::None
                    },
                    sync: snapshot_config.sync.clone(),
                    state: snapshot_config.state.clone(),
                },
                __adapter_attr__: AdapterAttr::from_config_and_dialect(
                    &snapshot_config.__warehouse_specific_config__,
                    default_adapter,
                ),
                deprecated_config: snapshot_config.clone().into(),
                compiled: None,
                compiled_code: None,
                __other__: properties
                    .relation
                    .as_ref()
                    .map(|relation| {
                        let mut other = BTreeMap::new();
                        other.insert(
                            "raw_relation".to_string(),
                            dbt_yaml::Value::String(
                                relation.clone().into_inner(),
                                relation.span().clone(),
                            ),
                        );
                        other
                    })
                    .unwrap_or_default(),
            };

            let components = RelationComponents {
                // For backwards compatibility with target_schema and target_database configs
                database: if snapshot_config.target_database.is_some() {
                    snapshot_config.target_database.clone()
                } else {
                    snapshot_config.database.clone()
                },
                schema: if snapshot_config.target_schema.is_some() {
                    snapshot_config.target_schema.clone()
                } else {
                    snapshot_config.schema.clone()
                },
                alias: snapshot_config.alias.clone(),
                store_failures: None,
            };

            // Update with relation components
            update_node_relation_components(
                &mut dbt_snapshot,
                &jinja_env,
                &root_package.dbt_project.name,
                &package_name,
                base_ctx,
                &components,
                default_adapter,
            )?;

            match node_resolver.insert_ref(&dbt_snapshot, default_adapter, status, false) {
                Ok(_) => (),
                Err(e) => {
                    let err_with_loc = e.with_location(error_path.clone());
                    emit_error_log_from_fs_error(err_with_loc);
                }
            }

            match status {
                ModelStatus::Enabled => {
                    if snapshot_config.unique_key.is_none() || snapshot_config.strategy.is_none() {
                        let e = fs_err!(
                            code => ErrorCode::InvalidConfig,
                            loc => error_path.clone(),
                            "Snapshot '{}' must be configured with a 'strategy' and 'unique_key'",
                            snapshot_name
                        );
                        emit_error_log_from_fs_error(*e);
                    }
                    if sql_file_info.execute && sql_defined_snapshots.contains(&dbt_asset.path) {
                        snapshots_with_execute.insert(unique_id.to_owned(), dbt_snapshot);
                    } else {
                        snapshots.insert(unique_id, Arc::new(dbt_snapshot));
                    }

                    if !arg.skip_creating_generic_tests {
                        properties.as_testable().persist(
                            package_name.as_str(),
                            &root_package.dbt_project.name,
                            collected_generic_tests,
                            test_name_truncations,
                            seen_generic_test_paths,
                            default_adapter,
                            &arg.io,
                            patch_path.as_ref().unwrap_or(&dbt_asset.path),
                            false,
                            &raw_test_configs
                                .get(snapshot_name)
                                .cloned()
                                .unwrap_or_default(),
                        )?;
                    }
                }
                ModelStatus::Disabled => {
                    disabled_snapshots.insert(unique_id, Arc::new(dbt_snapshot));
                }
                ModelStatus::ParsingFailed => {
                    if render_error_deferred {
                        snapshots.insert(unique_id, Arc::new(dbt_snapshot));

                        if !arg.skip_creating_generic_tests {
                            properties.as_testable().persist(
                                package_name.as_str(),
                                &root_package.dbt_project.name,
                                collected_generic_tests,
                                test_name_truncations,
                                seen_generic_test_paths,
                                default_adapter,
                                &arg.io,
                                patch_path.as_ref().unwrap_or(&dbt_asset.path),
                                false,
                                &raw_test_configs
                                    .get(snapshot_name)
                                    .cloned()
                                    .unwrap_or_default(),
                            )?;
                        }
                    }
                }
            }
        }
    }
    for (snapshot_name, mpe) in snapshot_properties.iter() {
        // Skip until we support better error messages for versioned models
        if mpe.version_info.is_some() {
            continue;
        }
        if !mpe.schema_value.is_null() {
            // Validate that the model is not latest and flattened
            let err = fs_err!(
                code => ErrorCode::NoNodeForYamlKey,
                loc => mpe.relative_path.clone(),
                "Unused schema.yml entry for snapshot '{}'",
                snapshot_name,
            );
            emit_warn_log_from_fs_error(*err);
        }
    }
    // Second pass to capture all identifiers with the appropriate context
    // `models_with_execute` should never have overlapping Arc pointers with `models` and `disabled_models`
    // otherwise make_mut will clone the inner model, and the modifications inside this function call will be lost
    let snapshots_rest = collect_adapter_identifiers_detect_unsafe(
        arg,
        snapshots_with_execute,
        node_resolver,
        jinja_env,
        default_adapter,
        package.dbt_project.name.as_str(),
        &root_package.dbt_project.name,
        runtime_config,
        token,
    )
    .await?;

    snapshots.extend(
        snapshots_rest
            .into_iter()
            .map(|(k, _)| (k.__common_attr__.unique_id.clone(), Arc::new(k))),
    );

    Ok((snapshots, disabled_snapshots))
}

/// Extract the verbatim body between `{% snapshot ... %}` and `{% endsnapshot %}`
/// tags. Returns None if the input is not a block-style snapshot.
///
/// dbt-core's snapshot parser uses the body (not the wrapping tags) as `raw_code`;
/// fs needs to match so that `state:modified.body`, partial-parse hashing, and
/// `{{ this }}` ref scans against fs-produced manifests behave identically.
fn extract_snapshot_block_body(sql: &str) -> Option<String> {
    static SNAPSHOT_OPEN_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"\{%(-?)\s*snapshot\s+\w+\s*(-?)%\}").unwrap()
    });
    static SNAPSHOT_CLOSE_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"\{%(-?)\s*endsnapshot\s*(-?)%\}").unwrap());

    let open = SNAPSHOT_OPEN_RE.captures(sql)?;
    let open_full = open.get(0).unwrap();
    let body_start = open_full.end();
    let open_trail_dash = !open.get(2).unwrap().as_str().is_empty();

    let close = SNAPSHOT_CLOSE_RE
        .captures_iter(sql)
        .filter(|c| c.get(0).unwrap().start() >= body_start)
        .last()?;
    let close_full = close.get(0).unwrap();
    let close_lead_dash = !close.get(1).unwrap().as_str().is_empty();

    let mut body = &sql[body_start..close_full.start()];
    // Honor Jinja whitespace control: `-%}` on the opening tag and `{%-` on the
    // closing tag strip adjacent whitespace inside the block.
    if open_trail_dash {
        body = body.trim_start();
    }
    if close_lead_dash {
        body = body.trim_end();
    }
    Some(body.to_string())
}

async fn recalculate_snapshot_checksum(
    arg: &ResolveArgs,
    original_path: &PathBuf,
    sql_file_info: &SqlFileInfo<SnapshotConfig>,
) -> DbtChecksum {
    // Read original snapshot file
    let original_absolute_path = arg.io.in_dir.join(original_path);
    match tokiofs::read_to_string(&original_absolute_path).await {
        Ok(original_sql) => {
            // First normalize: remove all whitespace and lowercase
            let normalized_full = normalize_sql(&original_sql);

            let normalized_sql =
                conform_normalized_snapshot_raw_code_to_mantle_format(&normalized_full);
            DbtChecksum::hash(normalized_sql.as_bytes())
        }
        Err(e) => {
            // Fallback to sql_file_info checksum if original file can't be read
            emit_warn_log_from_fs_error(*e);
            sql_file_info.checksum.clone()
        }
    }
}
