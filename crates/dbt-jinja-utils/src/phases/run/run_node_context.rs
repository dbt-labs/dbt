//! This module contains the scope for materializing nodes

use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_common::ErrorCode;
use dbt_common::io_args::IoArgs;
use dbt_common::serde_utils::convert_yml_to_value_map;

use dbt_adapter::column::ColumnStatic;
use dbt_adapter::load_store::ResultStore;
use dbt_common::stdfs;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::NodePathKind;
use dbt_schemas::schemas::telemetry::NodeType;
use dbt_telemetry::ExecutionPhase;
use minijinja::State;
use minijinja::listener::RenderingEventListener;
use minijinja::machinery::Span;
use minijinja::{Error, ErrorKind, Value as MinijinjaValue, value::Object};
use serde::Serialize;

use dbt_jinja_ctx::{
    CompileBaseCtx, HookConfig, JinjaObject, LazyModelWrapper, MacroLookupContext, RunNodeCtx,
    to_jinja_btreemap, to_model_context_map,
};

use super::run_config::RunConfig;
use dbt_schemas::schemas::project::ConfigKeys;

type YmlValue = dbt_yaml::Value;

/// Per-node fields computed from the YAML node config and used to construct
/// the `RunNodeCtx` overlay. Replaces the historical
/// `extend_with_model_context(&mut base_context, ...)` mutator: returning
/// the values lets `build_run_node_context` construct a typed `RunNodeCtx`
/// in one shot rather than incrementally mutating a `BTreeMap`.
struct ModelContextFields {
    this: MinijinjaValue,
    database: String,
    schema: String,
    identifier: String,
    pre_hooks: Option<MinijinjaValue>,
    post_hooks: Option<MinijinjaValue>,
    config: MinijinjaValue,
    model: JinjaObject<LazyModelWrapper>,
    node: JinjaObject<LazyModelWrapper>,
}

fn model_context_alias_types(model: &YmlValue) -> bool {
    model
        .get("config")
        .and_then(|config| config.get("contract"))
        .and_then(|contract| contract.get("alias_types"))
        .and_then(|alias_types| alias_types.as_bool())
        .unwrap_or(true)
}

// reference: https://github.com/dbt-labs/dbt-core/blob/411b53897ea34f9d0bd789a540c5812363e26267/core/dbt/context/providers.py#L1710
fn normalize_model_context_column_data_types(model: &mut YmlValue, adapter_type: AdapterType) {
    if !model_context_alias_types(model) {
        return;
    }

    let YmlValue::Mapping(model_map, _) = model else {
        return;
    };
    let columns_key = YmlValue::string("columns".to_string());
    let Some(YmlValue::Mapping(columns, _)) = model_map.get_mut(&columns_key) else {
        return;
    };

    let column_static = ColumnStatic::new(adapter_type);
    let data_type_key = YmlValue::string("data_type".to_string());
    for column_map in columns.values_mut().filter_map(YmlValue::as_mapping_mut) {
        let Some(data_type) = column_map.get_mut(&data_type_key) else {
            continue;
        };
        let Some(data_type_str) = data_type.as_str().map(ToOwned::to_owned) else {
            continue;
        };

        let translated = column_static.translate_type(&data_type_str);
        if translated != data_type_str {
            *data_type = YmlValue::string(translated);
        }
    }
}

fn normalized_model_context(mut model: YmlValue, adapter_type: AdapterType) -> YmlValue {
    normalize_model_context_column_data_types(&mut model, adapter_type);
    model
}

#[allow(clippy::too_many_arguments)]
fn build_model_context_fields<S: Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter_type: AdapterType,
    io_args: &IoArgs,
    sql_header: Option<MinijinjaValue>,
) -> ModelContextFields {
    let model = node.serialize();
    let common_attr = node.common();
    let base_attr = node.base();
    let resource_type = node.resource_type();
    // Create a relation for 'this' using config values
    let this_relation = dbt_adapter::relation::RelationObject::new(Arc::from(
        dbt_adapter::relation::do_create_relation(
            adapter_type,
            base_attr.database.clone(),
            base_attr.schema.clone(),
            Some(base_attr.alias.clone()),
            None,
            base_attr.quoting,
        )
        .unwrap(),
    ))
    .into_value();

    let config_yml = dbt_yaml::to_value(deprecated_config).expect("Failed to serialize object");

    // `ModelConfig`/`SeedConfig`/`SnapshotConfig` serialize hooks under the underscored
    // name; `FunctionConfig` doubles as its own manifest type and so uses dbt-core's
    // hyphenated key. Accept either.
    let hooks_of = |underscored: &str, hyphenated: &str| {
        config_yml
            .get(underscored)
            .or_else(|| config_yml.get(hyphenated))
    };

    let pre_hooks = hooks_of("pre_hook", "pre-hook").map(|pre_hook| {
        let values: Vec<HookConfig> = match pre_hook {
            YmlValue::String(_, _) | YmlValue::Mapping(_, _) => {
                parse_hook_item(pre_hook).into_iter().collect()
            }
            YmlValue::Sequence(arr, _) => arr.iter().filter_map(parse_hook_item).collect(),
            YmlValue::Null(_) => vec![],
            _ => {
                emit_warn_log_message(
                    ErrorCode::InvalidConfig,
                    format!("Unknown pre-hook type: {:?}", pre_hook),
                );
                vec![]
            }
        };
        values
            .iter()
            .map(|hook| MinijinjaValue::from_object(hook.clone()))
            .collect::<Vec<MinijinjaValue>>()
            .into()
    });

    let post_hooks = hooks_of("post_hook", "post-hook").map(|post_hook| {
        let values: Vec<HookConfig> = match post_hook {
            YmlValue::String(_, _) | YmlValue::Mapping(_, _) => {
                parse_hook_item(post_hook).into_iter().collect()
            }
            YmlValue::Sequence(arr, _) => arr.iter().filter_map(parse_hook_item).collect(),
            YmlValue::Null(_) => vec![],
            _ => {
                emit_warn_log_message(
                    ErrorCode::InvalidConfig,
                    format!("Unknown post-hook type: {:?}", post_hook),
                );
                vec![]
            }
        };
        values
            .iter()
            .map(|hook| MinijinjaValue::from_object(hook.clone()))
            .collect::<Vec<MinijinjaValue>>()
            .into()
    });

    let mut config_map = convert_yml_to_value_map(config_yml);
    if let Some(sql_header) = sql_header {
        config_map.insert("sql_header".to_string(), sql_header);
    }

    let mut model_map = convert_yml_to_value_map(normalized_model_context(model, adapter_type));

    // We are reading the raw_sql here for snapshots and models
    let raw_sql_path = match resource_type {
        // For snapshots, use path (generated file path) since original_file_path tracks the source
        NodeType::Snapshot => Some(io_args.out_dir.join(common_attr.path.clone())),
        NodeType::Model => Some(io_args.in_dir.join(common_attr.original_file_path.clone())),
        _ => None,
    };
    if let Some(raw_sql_path) = raw_sql_path {
        if let Ok(raw_sql) = stdfs::read_to_string(&raw_sql_path) {
            model_map.insert("raw_sql".to_owned(), MinijinjaValue::from(raw_sql));
        } else {
            emit_warn_log_message(
                ErrorCode::IoError,
                format!("Failed to read raw_sql: {}", raw_sql_path.display()),
            );
        };
    }

    // Get valid config keys based on resource type
    let valid_keys = match resource_type {
        NodeType::Model => dbt_schemas::schemas::project::ModelConfig::valid_field_names(),
        NodeType::Seed => dbt_schemas::schemas::project::SeedConfig::valid_field_names(),
        NodeType::Test => dbt_schemas::schemas::project::DataTestConfig::valid_field_names(),
        NodeType::Snapshot => dbt_schemas::schemas::project::SnapshotConfig::valid_field_names(),
        NodeType::Source => dbt_schemas::schemas::project::SourceConfig::valid_field_names(),
        NodeType::UnitTest => dbt_schemas::schemas::project::UnitTestConfig::valid_field_names(),
        NodeType::Function => dbt_schemas::schemas::project::FunctionConfig::valid_field_names(),
        _ => {
            // For other types, use an empty set to avoid warnings
            std::collections::HashSet::new()
        }
    };

    // Create the lazy wrappers for the model with the compiled path. All three
    // share one mutable map so `{% do model.update(...) %}` in a
    // materialization is observable through `model`, `node` and `config.model`
    // alike.
    let compiled_path =
        node.get_node_path_abs(NodePathKind::Compiled, &io_args.in_dir, &io_args.out_dir);
    let shared_model_map = to_model_context_map(model_map);

    let node_config = RunConfig {
        model_config: config_map,
        model: shared_model_map.clone(),
        model_compiled_path: compiled_path.clone(),
        valid_keys,
    };

    let lazy_model = LazyModelWrapper::new(shared_model_map.clone(), compiled_path.clone());
    let lazy_node = LazyModelWrapper::new(shared_model_map, compiled_path);

    ModelContextFields {
        this: this_relation,
        database: base_attr.database.clone(),
        schema: base_attr.schema.clone(),
        identifier: common_attr.name.clone(),
        pre_hooks,
        post_hooks,
        config: MinijinjaValue::from_object(node_config),
        model: JinjaObject::new(lazy_model),
        node: JinjaObject::new(lazy_node),
    }
}

/// Extend the base context with stateful functions.
///
/// `result_store`, when supplied, is used as-is instead of allocating a fresh
/// [`ResultStore`]. Callers that need to hold onto their own store (e.g. to
/// extract adapter responses after materialization) can build it once and
/// pass it in here rather than re-injecting its closures into the context
/// after this function overwrites them with a throwaway store.
pub fn extend_base_context_stateful_fn(
    base_context: &mut BTreeMap<String, MinijinjaValue>,
    root_project_name: &str,
    packages: BTreeSet<String>,
    result_store: Option<ResultStore>,
) {
    let result_store = result_store.unwrap_or_default();
    base_context.insert(
        "store_result".to_owned(),
        MinijinjaValue::from_function(result_store.store_result()),
    );
    base_context.insert(
        "load_result".to_owned(),
        MinijinjaValue::from_function(result_store.load_result()),
    );
    base_context.insert(
        "store_raw_result".to_owned(),
        MinijinjaValue::from_function(result_store.store_raw_result()),
    );

    // Add submit_python_job context function using a separate helper
    base_context.insert(
        "submit_python_job".to_owned(),
        MinijinjaValue::from_function(submit_python_job_context_fn()),
    );

    let mut packages = packages;
    packages.insert(root_project_name.to_string());

    base_context.insert(
        "context".to_owned(),
        MinijinjaValue::from_object(MacroLookupContext::new(
            root_project_name.to_string(),
            None,
            packages,
        )),
    );
}

/// Replace the statement-result-store closures (`store_result`/`load_result`/
/// `store_raw_result`) in an existing run context with a fresh, isolated
/// [`ResultStore`].
///
/// `build_run_node_context` bakes a single `ResultStore` into the context. That
/// store is fine when a context is used once, but microbatch builds the context
/// once per model and clones it for every batch (the closure `Value`s clone the
/// store's `Arc`, so all batches share one statement registry). With
/// `concurrent_batches = true` the batches run on separate threads and interleave
/// `store`/`load` calls for the same statement name (e.g. `get_columns_in_relation`,
/// or `run_query_statement` on Snowflake), so a batch can `load` a result a peer
/// already consumed and hit `MacroResultAlreadyLoadedError`.
///
/// Calling this on each per-batch context gives every batch its own registry,
/// mirroring dbt-core where each batch runner builds its own model context.
pub fn reset_result_store(context: &mut BTreeMap<String, MinijinjaValue>) -> ResultStore {
    let result_store = ResultStore::default();
    context.insert(
        "store_result".to_owned(),
        MinijinjaValue::from_function(result_store.store_result()),
    );
    context.insert(
        "load_result".to_owned(),
        MinijinjaValue::from_function(result_store.load_result()),
    );
    context.insert(
        "store_raw_result".to_owned(),
        MinijinjaValue::from_function(result_store.store_raw_result()),
    );
    result_store
}

/// Downcast a base context's `builtins` Object back to its concrete
/// `BTreeMap<String, MinijinjaValue>` — same trap as `MACRO_DISPATCH_ORDER`.
fn downcast_builtins_map(builtins: &MinijinjaValue) -> BTreeMap<String, MinijinjaValue> {
    builtins
        .as_object()
        .unwrap()
        .downcast_ref::<BTreeMap<String, MinijinjaValue>>()
        .unwrap()
        .clone()
}

/// Build the per-node run overlay ([`RunNodeCtx`]) with `base: None`.
///
/// Shared core of [`build_run_node_context`] and [`build_run_node_ctx`], which
/// can't delegate to each other (one holds a `&BTreeMap`, the other a
/// `&CompileBaseCtx`). `base_builtins` is that base's `builtins` map — already
/// downcast by each caller via [`downcast_builtins_map`] — extended here with
/// the per-node `RunConfig` before being re-wrapped as a `MinijinjaValue`
/// Object (downstream macro code downcasts it back).
#[allow(clippy::too_many_arguments)]
fn build_run_node_overlay<S: Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter_type: AdapterType,
    agate_table: Option<AgateTable>,
    base_builtins: Option<BTreeMap<String, MinijinjaValue>>,
    io_args: &IoArgs,
    phase: ExecutionPhase,
    sql_header: Option<MinijinjaValue>,
    packages: BTreeSet<String>,
) -> (RunNodeCtx, ResultStore) {
    let common_attr = node.common();
    let resource_type = node.resource_type();

    // Stateful fns: store_result/load_result/store_raw_result/submit_python_job + context.
    // These were `extend_base_context_stateful_fn` mutations into the BTreeMap;
    // pull the same closures + MacroLookupContext into local bindings so we
    // can construct the typed overlay below.
    let result_store = ResultStore::default();
    let store_result = MinijinjaValue::from_function(result_store.store_result());
    let load_result = MinijinjaValue::from_function(result_store.load_result());
    let store_raw_result = MinijinjaValue::from_function(result_store.store_raw_result());
    let submit_python_job = MinijinjaValue::from_function(submit_python_job_context_fn());

    let mut packages_with_root = packages;
    packages_with_root.insert(common_attr.package_name.clone());
    let context_lookup = JinjaObject::new(MacroLookupContext::new(
        common_attr.package_name.clone(),
        None,
        packages_with_root,
    ));

    // Per-node model-specific fields (this/database/schema/identifier, hooks,
    // config, model, node).
    let model_fields =
        build_model_context_fields(node, deprecated_config, adapter_type, io_args, sql_header);

    let write_value = MinijinjaValue::from_object(WriteConfig {
        resource_type: resource_type.as_static_ref().to_string(),
        run_file_path: node.get_node_path_abs(
            NodePathKind::Executable,
            &io_args.in_dir,
            &io_args.out_dir,
        ),
    });

    let load_agate_table = agate_table.map(|agate_table| {
        MinijinjaValue::from_function(move |_args: &[MinijinjaValue]| {
            MinijinjaValue::from_object(agate_table.clone())
        })
    });

    // Builtins overlay: take the caller-provided compile-base map and insert
    // the per-node RunConfig, then re-wrap as an Object below. The map
    // underlying `builtins` MUST be `BTreeMap<String, MinijinjaValue>` exactly
    // (downstream macro code downcasts to that type) — same trap as
    // `MACRO_DISPATCH_ORDER`.
    let mut base_builtins = base_builtins.unwrap_or_default();
    let node_config = model_fields
        .config
        .as_object()
        .unwrap()
        .downcast_ref::<RunConfig>()
        .unwrap()
        .clone();
    base_builtins.insert(
        "config".to_string(),
        MinijinjaValue::from_object(node_config),
    );

    let abs_current_path = node.get_node_path_abs(phase.into(), &io_args.in_dir, &io_args.out_dir);
    let relative_path = abs_current_path
        .strip_prefix(&io_args.out_dir)
        .map(|p| p.to_path_buf())
        .unwrap_or(abs_current_path);

    let ctx = RunNodeCtx {
        base: None,
        this: model_fields.this,
        database: model_fields.database,
        schema: model_fields.schema,
        identifier: model_fields.identifier,
        pre_hooks: model_fields.pre_hooks,
        post_hooks: model_fields.post_hooks,
        config: model_fields.config,
        model: model_fields.model,
        node: model_fields.node,
        connection_name: String::new(),
        store_result,
        load_result,
        store_raw_result,
        submit_python_job,
        context: context_lookup,
        write: write_value,
        load_agate_table,
        builtins: MinijinjaValue::from_object(base_builtins),
        target_package_name: common_attr.package_name.clone(),
        current_path: relative_path.to_string_lossy().into_owned(),
        current_span: MinijinjaValue::from_serialize(Span::default()),
    };

    (ctx, result_store)
}

/// Build a run context as a `BTreeMap` overlaid onto `base_context`.
///
/// Legacy BTreeMap path: builds the typed [`RunNodeCtx`] overlay (`base:
/// None`), serializes it, and `.extend(...)`s onto a clone of `base_context`
/// — the same last-write-wins shadowing the original BTreeMap-based code
/// produced. Callers that already hold a typed [`CompileBaseCtx`] should use
/// [`build_run_node_ctx`] instead, which skips the `to_jinja_btreemap`
/// round-trip.
///
/// TODO: remove once the remaining `&BTreeMap` callers (the `materialize_*`
/// render path, LSP preview) are migrated to [`build_run_node_ctx`]; at that
/// point [`build_run_node_overlay`] folds into `build_run_node_ctx`.
#[allow(clippy::too_many_arguments)]
pub fn build_run_node_context<S: Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter_type: AdapterType,
    agate_table: Option<AgateTable>,
    base_context: &BTreeMap<String, MinijinjaValue>,
    io_args: &IoArgs,
    phase: ExecutionPhase,
    sql_header: Option<MinijinjaValue>,
    packages: BTreeSet<String>,
) -> (BTreeMap<String, MinijinjaValue>, ResultStore) {
    // Downcast the base `builtins` Object to its concrete map here so the
    // shared overlay builder receives a typed map (no downcast on its side).
    let base_builtins = base_context.get("builtins").map(downcast_builtins_map);
    let (overlay, result_store) = build_run_node_overlay(
        node,
        deprecated_config,
        adapter_type,
        agate_table,
        base_builtins,
        io_args,
        phase,
        sql_header,
        packages,
    );

    let mut context = base_context.clone();
    context.extend(to_jinja_btreemap(&overlay));
    (context, result_store)
}

/// Build a run context as a typed [`RunNodeCtx`] composed onto `base` via the
/// `RunNodeCtx::base` flatten seam.
///
/// Typed path: the returned overlay carries `base: Some(base.clone())`, so it
/// can be passed straight into `render_named_str` / `Expression::eval`
/// (`S: Serialize`) with no intermediate `to_jinja_btreemap` — the base keys
/// flatten in and the per-node fields shadow them. This is the composition
/// seam the base-context migration moves callers onto incrementally.
#[allow(clippy::too_many_arguments)]
pub fn build_run_node_ctx<S: Serialize>(
    node: &dyn InternalDbtNode,
    deprecated_config: &S,
    adapter_type: AdapterType,
    agate_table: Option<AgateTable>,
    base: &CompileBaseCtx,
    io_args: &IoArgs,
    phase: ExecutionPhase,
    sql_header: Option<MinijinjaValue>,
    packages: BTreeSet<String>,
) -> RunNodeCtx {
    // `CompileBaseCtx::builtins` is a `MinijinjaValue` (Jinja-facing Object
    // slot), so downcast it to the concrete map for the shared overlay builder.
    let base_builtins = downcast_builtins_map(&base.builtins);
    let (mut overlay, _result_store) = build_run_node_overlay(
        node,
        deprecated_config,
        adapter_type,
        agate_table,
        Some(base_builtins),
        io_args,
        phase,
        sql_header,
        packages,
    );
    overlay.base = Some(base.clone());
    overlay
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// When a caller supplies its own `ResultStore`, `extend_base_context_stateful_fn`
    /// must wire that store's closures into the context rather than silently
    /// allocating (and overwriting with) a fresh default one. Seed a result
    /// into the caller's store before extending the context, then confirm
    /// `load_result` on the context can see it.
    #[test]
    fn extend_base_context_stateful_fn_uses_provided_result_store() {
        let store = ResultStore::default();
        store.store_raw_result()(&[MinijinjaValue::from(minijinja::value::Kwargs::from_iter([
            ("name", MinijinjaValue::from("seeded")),
        ]))])
        .unwrap();

        let mut base_context = BTreeMap::new();
        extend_base_context_stateful_fn(
            &mut base_context,
            "test_pkg",
            BTreeSet::new(),
            Some(store),
        );

        let mut env = minijinja::Environment::new();
        env.add_global(
            "load_result",
            base_context.get("load_result").unwrap().clone(),
        );
        let value = env
            .compile_expression("load_result('seeded')")
            .unwrap()
            .eval(minijinja::context!(), &[])
            .unwrap();

        assert!(
            !value.is_none(),
            "expected the caller-provided ResultStore's seeded result to be visible through the context"
        );
    }

    fn column_data_type<'a>(model: &'a YmlValue, column_name: &str) -> &'a str {
        model
            .get("columns")
            .and_then(|columns| columns.get(column_name))
            .and_then(|column| column.get("data_type"))
            .and_then(|data_type| data_type.as_str())
            .expect("column data_type should exist")
    }

    #[test]
    fn bigquery_model_context_alias_types_enabled_normalizes_column_data_types() {
        let mut model = dbt_yaml::to_value(json!({
            "config": {
                "contract": {
                    "alias_types": true
                }
            },
            "columns": {
                "float_col": {"name": "float_col", "data_type": "FLOAT"},
                "integer_col": {"name": "integer_col", "data_type": "INTEGER"},
                "text_col": {"name": "text_col", "data_type": "TEXT"},
                "numeric_col": {"name": "numeric_col", "data_type": "NUMERIC"}
            }
        }))
        .unwrap();

        normalize_model_context_column_data_types(&mut model, AdapterType::Bigquery);

        assert_eq!(column_data_type(&model, "float_col"), "FLOAT64");
        assert_eq!(column_data_type(&model, "integer_col"), "INT64");
        assert_eq!(column_data_type(&model, "text_col"), "STRING");
        assert_eq!(column_data_type(&model, "numeric_col"), "NUMERIC");
    }

    #[test]
    fn bigquery_model_context_alias_types_disabled_preserves_column_data_types() {
        let mut model = dbt_yaml::to_value(json!({
            "config": {
                "contract": {
                    "alias_types": false
                }
            },
            "columns": {
                "float_col": {"name": "float_col", "data_type": "FLOAT"}
            }
        }))
        .unwrap();

        normalize_model_context_column_data_types(&mut model, AdapterType::Bigquery);

        assert_eq!(column_data_type(&model, "float_col"), "FLOAT");
    }

    #[test]
    fn model_context_column_data_type_normalization_is_bigquery_only() {
        let mut model = dbt_yaml::to_value(json!({
            "config": {
                "contract": {
                    "alias_types": true
                }
            },
            "columns": {
                "float_col": {"name": "float_col", "data_type": "FLOAT"}
            }
        }))
        .unwrap();

        normalize_model_context_column_data_types(&mut model, AdapterType::Postgres);

        assert_eq!(column_data_type(&model, "float_col"), "FLOAT");
    }

    #[test]
    fn write_file_creates_nested_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run/pkg/snapshots/my_snap.sql/my_snap.sql");

        write_file(&path, "snapshot", "select 1").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "select 1");
    }

    #[test]
    fn write_file_replaces_stale_flat_file_on_parent_chain() {
        // A pre-#14125 Fusion (or dbt Core v1) left a flat file where we now
        // need a directory: target/run/pkg/snapshots/my_snap.sql is a file, but
        // the nested layout requires it to be a directory. Writing must succeed
        // rather than fail with ENOTDIR. See dbt-core#15692.
        let dir = tempfile::tempdir().unwrap();
        let stale = dir.path().join("run/pkg/snapshots/my_snap.sql");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, "stale flat file").unwrap();

        let nested = stale.join("my_snap.sql");
        write_file(&nested, "snapshot", "select 1").unwrap();

        assert!(stale.is_dir());
        assert_eq!(fs::read_to_string(&nested).unwrap(), "select 1");
    }

    #[test]
    fn write_file_replaces_stale_directory_at_target() {
        // The reverse layout drift: a stale directory sits where we now write a
        // flat file. Writing must succeed rather than fail with EISDIR.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("run/pkg/models/my_model.sql");
        fs::create_dir_all(target.join("my_model.sql").parent().unwrap()).unwrap();
        fs::write(target.join("my_model.sql"), "nested leftover").unwrap();

        write_file(&target, "model", "select 1").unwrap();

        assert!(target.is_file());
        assert_eq!(fs::read_to_string(&target).unwrap(), "select 1");
    }
}

fn parse_hook_item(item: &YmlValue) -> Option<HookConfig> {
    match item {
        YmlValue::String(s, _) => Some(HookConfig {
            sql: s.to_string(),
            transaction: true,
        }),
        YmlValue::Mapping(map, _) => {
            let sql = map.get("sql")?.as_str()?.to_string();
            let transaction = map
                .get("transaction")
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            Some(HookConfig { sql, transaction })
        }
        _ => {
            eprintln!("Pre hook unknown type: {item:?}");
            None
        }
    }
}

/// Context function that writes a payload to file
#[derive(Debug)]
pub struct WriteConfig {
    /// The resource type string (see `fusion::node::NodeType`)
    pub resource_type: String,
    /// Absolute target/run path for this node.
    pub run_file_path: PathBuf,
}

impl Object for WriteConfig {
    fn call(
        self: &Arc<Self>,
        _state: &State<'_, '_>,
        args: &[MinijinjaValue],
        _listeners: &[Rc<dyn RenderingEventListener>],
    ) -> Result<MinijinjaValue, Error> {
        if args.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "write function requires payload argument".to_string(),
            ));
        }

        // Extract payload from args
        let payload = match args[0].as_str() {
            Some(s) => s,
            None => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    "Failed to convert payload to string".to_string(),
                ));
            }
        };

        // Write the file
        match write_file(&self.run_file_path, &self.resource_type, payload) {
            Ok(_) => {}
            Err(e) => {
                return Err(Error::new(
                    ErrorKind::InvalidOperation,
                    format!("Failed to write file: {e}"),
                ));
            }
        }

        // Return empty string on success
        Ok(MinijinjaValue::from(""))
    }
}

/// Write a file to disk
fn write_file(full_path: &Path, resource_type: &str, payload: &str) -> Result<(), Error> {
    // Check if model is a Macro or SourceDefinition
    if resource_type == "macro" || resource_type == "source" {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            "Macros and sources cannot be written to disk",
        ));
    }

    // Reconcile a target/ left behind by a different artifact layout — e.g.
    // dbt Core v1, or a pre-#14125 Fusion that wrote snapshot run artifacts as
    // a flat file where we now write a nested directory (or vice versa). Without
    // this, a stale file sitting where we now need a directory fails the write
    // below with ENOTDIR ("Not a directory"), and a stale directory sitting
    // where we now write a flat file fails with EISDIR ("Is a directory").
    // See https://github.com/dbt-labs/dbt-core/issues/15692.
    if let Some(parent) = full_path.parent()
        && !parent.is_dir()
    {
        // Parent is missing, or a stale file on the parent chain (from an older
        // layout) sits where we now need a directory. Remove the deepest such
        // file so create_dir_all can rebuild the tree, then create it.
        for ancestor in parent.ancestors() {
            if ancestor.is_file() {
                if let Err(e) = fs::remove_file(ancestor) {
                    return Err(Error::new(
                        ErrorKind::InvalidOperation,
                        format!("Failed to remove stale file {}: {}", ancestor.display(), e),
                    ));
                }
                break;
            }
        }
        if let Err(e) = fs::create_dir_all(parent) {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("Failed to create directory {}: {}", parent.display(), e),
            ));
        }
    }

    // A stale directory sitting where we now write a flat file fails with EISDIR.
    if full_path.is_dir()
        && let Err(e) = fs::remove_dir_all(full_path)
    {
        return Err(Error::new(
            ErrorKind::InvalidOperation,
            format!(
                "Failed to remove stale directory {}: {}",
                full_path.display(),
                e
            ),
        ));
    }

    match fs::write(full_path, payload) {
        Ok(_) => Ok(()),
        Err(e) => Err(Error::new(
            ErrorKind::InvalidOperation,
            format!("Failed to write to {}: {}", full_path.display(), e),
        )),
    }
}

/// Returns the function used for the submit_python_job context.
fn submit_python_job_context_fn()
-> impl Fn(&State, &[MinijinjaValue]) -> Result<MinijinjaValue, Error> + Copy {
    |state: &State, args: &[MinijinjaValue]| {
        // Parse arguments: submit_python_job(parsed_model, compiled_code)
        if args.len() != 2 {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                format!("submit_python_job expects 2 arguments, got {}", args.len()),
            ));
        }
        let parsed_model = &args[0];
        let compiled_code = args[1].as_str().ok_or_else(|| {
            Error::new(
                ErrorKind::InvalidOperation,
                "compiled_code must be a string",
            )
        })?;

        // Note(Ani):
        // dbt-core validates:
        //   - macro_stack.depth == 2
        //   - call_stack[1] == "macro.dbt.statement"
        //   - "materialization" in call_stack[0]
        //
        // In fusion, we shouldn't need to do this because this funciton is only registered in the run node context
        // so if a user tries to use it outside of a statement.sql macro, in a materialization macro, it will fail earlier due to an unrecongized function call.

        // Get adapter from context and call submit_python_job
        let adapter = state
            .lookup("adapter", &[])
            .ok_or_else(|| Error::new(ErrorKind::UndefinedError, "adapter not found in context"))?;
        adapter.call_method(
            state,
            "submit_python_job",
            &[parsed_model.clone(), MinijinjaValue::from(compiled_code)],
            &[],
        )
    }
}
