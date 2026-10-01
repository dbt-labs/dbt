use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use arrow_schema::SchemaRef;
use dbt_adapter::adapter::quote_component;
use dbt_adapter::sql_types::TypeOpsFactory;
use dbt_adapter::{Adapter, AdapterResult};
use dbt_adapter::{Column, ColumnBuilder};
use dbt_adapter_core::AdapterType;
use dbt_common::artifact_io::write_artifact_to_file;
use dbt_common::constants::DBT_MANIFEST_JSON;
use dbt_common::io_args::IoArgs;
use dbt_common::path::DbtPath;
use dbt_common::path::get_target_write_path;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::unexpected_err;
use dbt_common::{ErrorCode, FsResult, constants::DBT_COMPILED_DIR_NAME, fs_err, stdfs};
use dbt_dag::schedule::Schedule;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_loader::internal_macro_package_names_for;
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::dbt_column::{DbtColumn, DbtColumnRef};
use dbt_schemas::schemas::relations::base::ComponentName;
use dbt_schemas::schemas::{
    CommonAttributes, InternalDbtNode, InternalDbtNodeAttributes, NodePathKind,
    macros::DbtMacro,
    manifest::{DbtManifest, DbtNode},
    nodes::Nodes,
};
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::RunTasksArgs;
use dbt_telemetry::ArtifactType;
use dbt_telemetry::NodeType;
use minijinja::{State, Value};

#[allow(clippy::cognitive_complexity)]
fn update_resolved_states_manifest_with_schemas_and_compiled_sql_core(
    arg: &RunTasksArgs,
    type_ops_factory: &dyn TypeOpsFactory,
    resolved_state: &mut ResolverState,
    mut dbt_manifest: Option<&mut DbtManifest>,
    schema_cache: &Arc<dyn SchemaStoreTrait>,
) -> FsResult<()> {
    // Unwrap the Arc to take ownership of the ResolverState
    let io = &arg.io;

    // Process models
    for (unique_id, model) in resolved_state.nodes.models.iter_mut().filter(|(_, model)| {
        !model.is_extended_model() && !matches!(model.materialized(), DbtMaterialization::External)
    }) {
        let manifest_model = dbt_manifest.as_mut().map(|dbt_manifest| {
            let Some(manifest_model) = dbt_manifest.nodes.get_mut(unique_id) else {
                return unexpected_err!(
                    "Inconsistent manifest: model {} not found in manifest",
                    unique_id
                );
            };
            let DbtNode::Model(manifest_model) = manifest_model else {
                return unexpected_err!(
                    "Inconsistent manifest: model {} not typed DbtNode::Model in manifest",
                    unique_id
                );
            };
            Ok(manifest_model)
        });

        let mut manifest_model = match manifest_model {
            None => None,
            Some(res) => match res {
                Err(e) => return Err(e),
                Ok(manifest_model) => Some(manifest_model),
            },
        };

        let base_mut = match manifest_model.as_mut() {
            None => None,
            Some(manifest_model) => Some(&mut manifest_model.__base_attr__),
        };

        // Only populate compiled_code into the manifest when writing manifest.json or metadata
        // parquet (compile/nodes epoch needs compiled_code and compiled_path).
        // Skipping this avoids reading 5k+ compiled SQL files from disk otherwise.
        // raw_code is already populated at resolve time — see resolve_models.rs.
        if arg.write_json || arg.write_metadata {
            if let Some(base_mut) = base_mut {
                let absolute_path = get_target_write_path(
                    &io.in_dir,
                    &io.out_dir.join(DBT_COMPILED_DIR_NAME),
                    &model.__common_attr__.package_name,
                    &model.__common_attr__.path,
                    &model.__common_attr__.original_file_path,
                );
                if let Ok(compiled_sql) = stdfs::read_to_string(&absolute_path) {
                    let relative_path = stdfs::diff_paths(&absolute_path, &io.in_dir)?;
                    base_mut.compiled_path = Some(relative_path.to_string_lossy().to_string());
                    base_mut.compiled_code = Some(compiled_sql.clone());
                    base_mut.compiled = Some(true);
                }
            }
        }

        if let Some(entry) = schema_cache.get_schema_by_unique_id(unique_id) {
            let columns = update_node_columns(
                resolved_state.adapter_type,
                type_ops_factory,
                &model.__common_attr__,
                &model.__base_attr__.columns,
                entry.inner(),
            )?;

            // Mutate the model in the resolved_state (consumed by dbt-index via --use-index).
            // Do NOT propagate to the manifest — hydrated columns would break state:modified selectors.
            let mut_model = Arc::make_mut(model);
            mut_model.__base_attr__.columns = columns;
        }
    }

    // Process snapshots
    for (unique_id, snapshot) in resolved_state.nodes.snapshots.iter_mut() {
        let manifest_snapshot = dbt_manifest.as_mut().map(|dbt_manifest| {
            let Some(manifest_snapshot) = dbt_manifest.nodes.get_mut(unique_id) else {
                return unexpected_err!(
                    "Inconsistent manifest: snapshot {} not found in manifest",
                    unique_id
                );
            };
            let DbtNode::Snapshot(manifest_snapshot) = manifest_snapshot else {
                return unexpected_err!(
                    "Inconsistent manifest: snapshot {} not typed DbtNode::Snapshot in manifest",
                    unique_id
                );
            };
            Ok(manifest_snapshot)
        });

        let manifest_snapshot = match manifest_snapshot {
            None => None,
            Some(res) => match res {
                Err(e) => return Err(e),
                Ok(manifest_snapshot) => Some(manifest_snapshot),
            },
        };

        let base_mut = match manifest_snapshot {
            None => None,
            Some(manifest_snapshot) => Some(&mut manifest_snapshot.__base_attr__),
        };

        // Only populate compiled_code into the manifest when writing manifest.json or metadata
        // parquet. raw_code is already populated at resolve time — see resolve_snapshots.rs.
        if arg.write_json || arg.write_metadata {
            if let Some(base_mut) = base_mut {
                let absolute_path =
                    snapshot.get_node_path_abs(NodePathKind::Compiled, &io.in_dir, &io.out_dir);
                if let Ok(compiled_sql) = stdfs::read_to_string(&absolute_path) {
                    let relative_path = stdfs::diff_paths(&absolute_path, &io.in_dir)?;
                    base_mut.compiled_path = Some(relative_path.to_string_lossy().to_string());
                    base_mut.compiled_code = Some(compiled_sql);
                    base_mut.compiled = Some(true);
                }
            }
        }
    }

    // Process seeds
    for (unique_id, seed) in resolved_state.nodes.seeds.iter_mut() {
        if let Some(entry) = schema_cache.get_schema_by_unique_id(unique_id) {
            let columns = update_node_columns(
                resolved_state.adapter_type,
                type_ops_factory,
                &seed.__common_attr__,
                &seed.__base_attr__.columns,
                entry.inner(),
            )?;

            // Mutate the seed in the resolved_state (consumed by dbt-index via --use-index).
            // Do NOT propagate to the manifest — hydrated columns would break state:modified selectors.
            let mut_seed = Arc::make_mut(seed);
            mut_seed.__base_attr__.columns = columns;
        }
    }

    // Process sources
    for (unique_id, source) in resolved_state.nodes.sources.iter_mut() {
        if let Some(entry) = schema_cache.get_schema_by_unique_id(unique_id) {
            let columns = update_node_columns(
                resolved_state.adapter_type,
                type_ops_factory,
                &source.__common_attr__,
                &source.__base_attr__.columns,
                entry.inner(),
            )?;

            // Mutate the source in the resolved_state (consumed by dbt-index via --use-index).
            // Do NOT propagate to the manifest — hydrated columns would break state:modified selectors.
            let mut_source = Arc::make_mut(source);
            mut_source.__base_attr__.columns = columns;
        }
    }

    // Process data_tests
    for (unique_id, data_test) in resolved_state.nodes.tests.iter_mut() {
        let manifest_data_test = dbt_manifest.as_mut().map(|dbt_manifest| {
            let Some(manifest_data_test) = dbt_manifest.nodes.get_mut(unique_id) else {
                return unexpected_err!(
                    "Inconsistent manifest: data_test {} not found in manifest",
                    unique_id
                );
            };
            let DbtNode::Test(manifest_data_test) = manifest_data_test else {
                return unexpected_err!(
                    "Inconsistent manifest: data_test {} not typed DbtNode::Test in manifest",
                    unique_id
                );
            };
            Ok(manifest_data_test)
        });

        let manifest_data_test = match manifest_data_test {
            None => None,
            Some(res) => match res {
                Err(e) => return Err(e),
                Ok(manifest_data_test) => Some(manifest_data_test),
            },
        };

        let base_mut = match manifest_data_test {
            None => None,
            Some(manifest_data_test) => Some(&mut manifest_data_test.__base_attr__),
        };

        if arg.write_json || arg.write_metadata {
            if let Some(base_mut) = base_mut {
                let absolute_path = get_target_write_path(
                    &io.in_dir,
                    &io.out_dir.join(DBT_COMPILED_DIR_NAME),
                    &data_test.__common_attr__.package_name,
                    &data_test.__common_attr__.path,
                    &data_test.__common_attr__.original_file_path,
                );
                if let Ok(compiled_sql) = stdfs::read_to_string(&absolute_path) {
                    let relative_path = stdfs::diff_paths(&absolute_path, &io.in_dir)?;
                    base_mut.compiled_path = Some(relative_path.to_string_lossy().to_string());
                    base_mut.compiled_code = Some(compiled_sql.clone());
                    base_mut.compiled = Some(true);
                }
            }
        }
    }

    // Process analyses
    for (unique_id, analysis) in resolved_state.nodes.analyses.iter_mut() {
        let manifest_analysis = dbt_manifest.as_mut().map(|dbt_manifest| {
            let Some(manifest_analysis) = dbt_manifest.nodes.get_mut(unique_id) else {
                return unexpected_err!(
                    "Inconsistent manifest: analysis {} not found in manifest",
                    unique_id
                );
            };
            let DbtNode::Analysis(manifest_analysis) = manifest_analysis else {
                return unexpected_err!(
                    "Inconsistent manifest: analysis {} not typed DbtNode::Analysis in manifest",
                    unique_id
                );
            };
            Ok(manifest_analysis)
        });

        let manifest_analysis = match manifest_analysis {
            None => None,
            Some(res) => match res {
                Err(e) => return Err(e),
                Ok(manifest_analysis) => Some(manifest_analysis),
            },
        };

        let base_mut = match manifest_analysis {
            None => None,
            Some(manifest_analysis) => Some(&mut manifest_analysis.__base_attr__),
        };

        if arg.write_json || arg.write_metadata {
            if let Some(base_mut) = base_mut {
                let absolute_path =
                    analysis.get_node_path_abs(NodePathKind::Compiled, &io.in_dir, &io.out_dir);
                if let Ok(compiled_sql) = stdfs::read_to_string(&absolute_path) {
                    let relative_path = stdfs::diff_paths(&absolute_path, &io.in_dir)?;
                    base_mut.compiled_path = Some(relative_path.to_string_lossy().to_string());
                    base_mut.compiled_code = Some(compiled_sql);
                    base_mut.compiled = Some(true);
                }
            }
        }
    }

    if let Some(dbt_manifest) = dbt_manifest
        && arg.write_json
    {
        write_artifact_to_file(
            dbt_manifest,
            ArtifactType::Manifest,
            &io.out_dir,
            DBT_MANIFEST_JSON,
            &io.in_dir,
        )?;
    }

    Ok(())
}

pub fn update_columns_from_schemas(
    arg: &RunTasksArgs,
    type_ops_factory: &dyn TypeOpsFactory,
    resolved_state: &mut ResolverState,
    schema_cache: &Arc<dyn SchemaStoreTrait>,
) -> FsResult<()> {
    update_resolved_states_manifest_with_schemas_and_compiled_sql_core(
        arg,
        type_ops_factory,
        resolved_state,
        None,
        schema_cache,
    )
}

pub fn update_resolved_states_manifest_with_schemas_and_compiled_sql(
    arg: &RunTasksArgs,
    type_ops_factory: &dyn TypeOpsFactory,
    resolved_state: &mut ResolverState,
    dbt_manifest: &mut DbtManifest,
    schema_cache: &Arc<dyn SchemaStoreTrait>,
) -> FsResult<()> {
    update_resolved_states_manifest_with_schemas_and_compiled_sql_core(
        arg,
        type_ops_factory,
        resolved_state,
        Some(dbt_manifest),
        schema_cache,
    )
}

/// Updates/fills the column information for each node in the [ResolverState].
/// Returns a new [ResolverState].
pub fn update_resolved_state_node_columns(
    run_task_args: &RunTasksArgs,
    type_ops_factory: &dyn TypeOpsFactory,
    schema_store: &Arc<dyn SchemaStoreTrait>,
    arc_resolved_state: Arc<ResolverState>,
) -> FsResult<ResolverState> {
    let mut resolved_state = (*arc_resolved_state).clone();
    update_columns_from_schemas(
        run_task_args,
        type_ops_factory,
        &mut resolved_state,
        schema_store,
    )?;
    Ok(resolved_state)
}

/// Updates the columns of a node based on the schema fields.
pub fn update_node_columns(
    adapter_type: AdapterType,
    type_ops_factory: &dyn TypeOpsFactory,
    common_attr: &CommonAttributes,
    columns: &[DbtColumnRef],
    schema: &SchemaRef,
) -> FsResult<Vec<DbtColumnRef>> {
    let mut new_columns = vec![];

    let type_ops = type_ops_factory.create(adapter_type);

    // TODO(anna): Let's revisit this and try to extract out the flattening behavior into something like `schema_to_flattened_columns`
    let mut schema_columns = Vec::<Column>::with_capacity(columns.len());
    let builder = ColumnBuilder::new(adapter_type);
    for field in schema.fields() {
        let column = builder.build(field, type_ops.as_ref())?;
        if adapter_type == AdapterType::Bigquery {
            schema_columns.extend(column.flatten());
        } else {
            schema_columns.push(column);
        }
    }

    for column in schema_columns {
        let column_name = column.name();
        let column_type = &column.data_type();

        // Look up the existing column (case-insensitive for adapters like Snowflake
        // that return uppercase column names from schema introspection)
        let existing = columns
            .iter()
            .find(|col| col.name.to_lowercase() == column_name.to_lowercase());

        // If the node has a type, compare it with the schema's type
        if let Some(existing_column) = existing {
            if let Some(existing_type) = &existing_column.data_type
                && existing_type != column_type
            {
                if !type_ops
                    .normalize_and_compare_sql_types(existing_type, column_type)
                    .unwrap_or(false)
                {
                    emit_warn_log_message(
                        ErrorCode::ColumnTypeMismatch,
                        format!(
                            "Column '{}' in node '{}' has a type mismatch. Overriding '{}' with '{}'.",
                            column_name, common_attr.unique_id, existing_type, column_type
                        ),
                    );
                }
            }
        }

        // Create a new column with the schema's type, preserving all YAML-sourced metadata
        let new_column = Arc::new(DbtColumn {
            name: column_name.to_string(),
            data_type: Some(column_type.to_string()),
            description: existing.and_then(|col| col.description.clone()),
            constraints: existing
                .map(|col| col.constraints.clone())
                .unwrap_or_default(),
            meta: existing.map(|col| col.meta.clone()).unwrap_or_default(),
            tags: existing.map(|col| col.tags.clone()).unwrap_or_default(),
            policy_tags: existing.and_then(|col| col.policy_tags.clone()),
            classifiers: existing.and_then(|col| col.classifiers.clone()),
            databricks_tags: existing.and_then(|col| col.databricks_tags.clone()),
            column_mask: existing.and_then(|col| col.column_mask.clone()),
            quote: existing.and_then(|col| col.quote),
            codec: existing.and_then(|col| col.codec.clone()),
            ttl: existing.and_then(|col| col.ttl.clone()),
            deprecated_config: existing
                .map(|col| col.deprecated_config.clone())
                .unwrap_or_default(),
            dimension: existing.and_then(|col| col.dimension.clone()),
            entity: existing.and_then(|col| col.entity.clone()),
            granularity: existing.and_then(|col| col.granularity.clone()),
        });

        new_columns.push(new_column);
    }

    // Update the node's columns
    Ok(new_columns)
}

/// Collects a mapping of catalog -> set of schemas from scheduled nodes.
pub fn get_catalog_schemas(
    nodes: &Nodes,
    schedule: &Schedule<String>,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut catalog_schemas = BTreeMap::new();
    for unique_id in schedule.sorted_nodes.iter() {
        let Some(node) = nodes.get_node(unique_id) else {
            continue;
        };

        let base = node.base();
        if base.relation_name.is_none() {
            continue;
        }
        if node.resource_type() == NodeType::SavedQuery {
            let saved_query = &nodes
                .saved_queries
                .get(&node.common().unique_id)
                .expect("Node should exist");
            for export in saved_query.__saved_query_attr__.exports.iter() {
                let database = export
                    .config
                    .database
                    .clone()
                    .expect("Database should be populated in resolve_saved_queries");
                let schema = export
                    .config
                    .schema_name
                    .clone()
                    .expect("Schema should be populated in resolved_saved_queries");
                catalog_schemas
                    .entry(database)
                    .or_insert_with(BTreeSet::new)
                    .insert(schema);
            }
        } else {
            let catalog = node.base().database.clone();
            let schema = node.base().schema.clone();
            catalog_schemas
                .entry(catalog)
                .or_insert_with(BTreeSet::new)
                .insert(schema);
        }

        catalog_schemas
            .entry(base.database.clone())
            .or_insert_with(BTreeSet::new)
            .insert(base.schema.clone());
    }
    catalog_schemas
}

/// Registers the schemas in the database.
pub fn register_catalog_schemas_remote(
    adapter: &Arc<Adapter>,
    state: &State<'_, '_>,
    catalog_schemas: Vec<(String, String, String)>,
) -> FsResult<()> {
    let metadata_adapter = if let Some(metadata_adapter) = adapter.metadata_adapter() {
        metadata_adapter
    } else {
        emit_warn_log_message(
            ErrorCode::UnsupportedFeature,
            format!(
                "Cannot register databases or schemas in the remote. Adapter '{}' does not support metadata operations.",
                adapter.adapter_type()
            ),
        );
        return Ok(());
    };

    let results = metadata_adapter.create_schemas_if_not_exists(state, catalog_schemas)?;
    for (catalog, schema, unique_id, result) in results {
        if let Err(e) = result {
            let err_string = format!(
                "Failed to create schema '{schema}' in database '{catalog}' in remote for {unique_id}: {e}"
            );

            emit_warn_log_message(ErrorCode::FailedToCreateDatabase, err_string);
        }
    }

    Ok(())
}

/// Computes catalogs and schemas of selected nodes in order to create missing ones.
/// This is the same as `get_catalog_schemas`, but includes the unique id of an associated node.
pub fn get_catalog_schemas_and_ids(
    nodes: &Nodes,
    schedule: &Schedule<String>,
) -> BTreeMap<String, BTreeMap<String, String>> {
    let mut catalog_schemas = BTreeMap::new();
    for unique_id in schedule.selected_nodes.iter() {
        let Some(node) = nodes.get_node(unique_id) else {
            continue;
        };

        let base = node.base();
        if base.relation_name.is_none() {
            continue;
        }
        if node.resource_type() == NodeType::SavedQuery {
            let saved_query = &nodes
                .saved_queries
                .get(&node.common().unique_id)
                .expect("Node should exist");
            for export in saved_query.__saved_query_attr__.exports.iter() {
                let database = export
                    .config
                    .database
                    .clone()
                    .expect("Database should be populated in resolve_saved_queries");
                let schema = export
                    .config
                    .schema_name
                    .clone()
                    .expect("Schema should be populated in resolved_saved_queries");
                catalog_schemas
                    .entry(database)
                    .or_insert_with(BTreeMap::new)
                    .insert(schema, unique_id.clone());
            }
        } else {
            let catalog = base.database.clone();
            let schema = base.schema.clone();
            catalog_schemas
                .entry(catalog)
                .or_insert_with(BTreeMap::new)
                .insert(schema, unique_id.clone());
        }
    }
    catalog_schemas
}

/// Filters schemas by ones that don't already exist.
pub fn filter_missing_schemas(
    adapter: &Arc<Adapter>,
    state: &State,
    catalog_schemas: &BTreeMap<String, BTreeMap<String, String>>,
) -> AdapterResult<Vec<(String, String, String)>> {
    let catalog_schema_count = catalog_schemas
        .iter()
        .fold(0, |acc, (_, schemas)| acc + schemas.len());
    let mut missing_catalog_schemas = Vec::with_capacity(catalog_schema_count);
    for (catalog, schemas) in catalog_schemas {
        let quoted_catalog = quote_component(
            adapter.adapter_type(),
            &adapter.engine().quoting(),
            catalog,
            ComponentName::Database,
        );
        match adapter.list_schemas_typed(state, &quoted_catalog) {
            Ok(schemas_values) => {
                let existing_schemas = BTreeSet::<String>::from_iter(
                    schemas_values.into_iter().map(|s| s.to_lowercase()),
                );
                missing_catalog_schemas.extend(schemas.iter().filter_map(|(schema, unique_id)| {
                    if existing_schemas.contains(&schema.to_lowercase()) {
                        None
                    } else {
                        Some((catalog.clone(), schema.clone(), unique_id.clone()))
                    }
                }));
            }
            // Fail fast on this Snowflake driver error,
            // this indicates that the driver times out on the http request from the server
            // after default number if retries in gosnowflake
            // TODO: see if we can represent irrecoverable errors like this one using ErrorKind etc.
            // we cannot currently because the ErrorKind::Driver is too inclusive that contain errors we want to ignore
            Err(err)
                if err.to_string().contains("context deadline exceeded")
                    && adapter.adapter_type() == AdapterType::Snowflake =>
            {
                return Err(err);
            }
            Err(_) => {
                missing_catalog_schemas.extend(schemas.iter().map(|(schema, unique_id)| {
                    (catalog.clone(), schema.clone(), unique_id.clone())
                }));
            }
        }
    }

    Ok(missing_catalog_schemas)
}

pub fn typecheck_macros(
    resolver_state: &ResolverState,
    env: Arc<JinjaEnv>,
    jinja_typechecking_listener_factory: Arc<
        dyn dbt_jinja_utils::listener::JinjaTypeCheckingEventListenerFactory,
    >,
    arg: &RunTasksArgs,
) -> FsResult<()> {
    // Internal-package macros are embedded in the binary and never present on disk in Embedded
    // mode. Skip them — they're stable and pre-tested. After the manifest parity fix,
    // original_file_path is package-relative and no longer starts with "dbt_internal_packages",
    // so we identify internal macros by package_name instead.
    // Union over every adapter the target declares, not just the default one:
    // otherwise a borrowed package's macros would not be recognised as internal.
    let internal_pkgs =
        internal_macro_package_names_for(resolver_state.dbt_profile.adapter_types());
    let is_internal = |m: &&DbtMacro| internal_pkgs.contains(&m.package_name);

    let all_files = {
        let mut seen = BTreeSet::new();
        resolver_state
            .macros
            .macros
            .values()
            .filter(|m| !is_internal(m))
            .filter_map(|m| {
                let path =
                    m.get_node_path_abs(NodePathKind::Definition, &arg.io.in_dir, &arg.io.out_dir);
                seen.insert(path.clone()).then(|| DbtPath::from(path))
            })
            .collect::<Vec<_>>()
    };

    let mut content_cache: HashMap<DbtPath, String> = HashMap::new();

    let noqa_comments = collect_noqa_comments(&arg.io, &all_files, &mut content_cache)?;

    for m in resolver_state.macros.macros.values() {
        if is_internal(&m) {
            continue;
        }
        let relative_file_path = m.original_file_path.clone();
        let absolute_file_path = DbtPath::from(m.get_node_path_abs(
            NodePathKind::Definition,
            &arg.io.in_dir,
            &arg.io.out_dir,
        ));
        let content = if let Some(content) = content_cache.get(&absolute_file_path) {
            content.clone()
        } else {
            let content = stdfs::read_to_string(absolute_file_path.as_path()).map_err(|e| {
                fs_err!(
                    ErrorCode::Generic,
                    "Failed to read file {}: {}",
                    &absolute_file_path.display(),
                    e
                )
            })?;
            content_cache.insert(absolute_file_path.clone(), content.clone());
            content
        };
        let (content, offset) = if let Some(span) = m.span {
            let start = span.start_offset as usize;
            let end = span.end_offset as usize;
            let sliced_content = content.get(start..end).ok_or_else(|| {
                fs_err!(
                    ErrorCode::Generic,
                    "Invalid span offsets ({}..{}) for file {} (content length: {})",
                    start,
                    end,
                    &absolute_file_path.display(),
                    content.len()
                )
            })?;
            (
                sliced_content.to_string(),
                dbt_common::CodeLocationWithFile::new(
                    span.start_line,
                    span.start_col,
                    span.start_offset,
                    relative_file_path.to_path_buf(),
                ),
            )
        } else {
            return Err(fs_err!(
                ErrorCode::Generic,
                "Span of macro not found: {}",
                &absolute_file_path.display()
            ));
        };
        let _ = dbt_jinja_utils::typecheck::typecheck(
            &arg.io,
            env.clone(),
            &noqa_comments,
            jinja_typechecking_listener_factory.clone(),
            Some(m.package_name.clone()),
            &env.env.get_root_package_name(),
            Value::from_dyn_object(env.env.get_dbt_and_adapters_namespaces()),
            &relative_file_path.clone(),
            &content,
            &offset,
            &m.unique_id.clone(),
            resolver_state.adapter_type,
            false,
        );
    }
    Ok(())
}

fn collect_noqa_comments(
    io: &IoArgs,
    files: &Vec<DbtPath>,
    content_cache: &mut HashMap<DbtPath, String>,
) -> FsResult<HashMap<DbtPath, HashSet<u32>>> {
    let mut noqa_comments = HashMap::new();

    for file in files {
        let absolute_file_path = DbtPath::from(io.in_dir.join(file.as_path()));
        let content = if let Some(content) = content_cache.get(&absolute_file_path) {
            content.clone()
        } else {
            let content = stdfs::read_to_string(absolute_file_path.as_path())?;
            content_cache.insert(absolute_file_path.clone(), content.clone());
            content
        };
        let noqa_lines = noqa_comments
            .entry(absolute_file_path.clone())
            .or_insert_with(HashSet::new);
        for (line_number, line) in content.lines().enumerate() {
            if line.contains("-- noqa") {
                noqa_lines.insert(line_number as u32 + 1);
            }
        }
    }
    Ok(noqa_comments)
}

/// Write decompiled SQL to out_dir/decompiled/{path}
/// Mirrors dbt's compiled output structure. Path should be the common.path from the node
/// (e.g., "models/staging/stg_users.sql")
pub fn write_decompiled_sql(out_dir: &Path, relative_path: &Path, sql: &str) {
    let decompiled_path = out_dir.join("decompiled").join(relative_path);

    if let Some(parent) = decompiled_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&decompiled_path, sql);
}
