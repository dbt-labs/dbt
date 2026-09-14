use crate::args::ResolveArgs;
use crate::dbt_project_config::ProjectConfigResolver;
use crate::dbt_project_config::RootProjectConfigs;
use crate::dbt_project_config::disallow_plus_prefix_from_flags;
use crate::dbt_project_config::init_project_config;
use crate::renderer::RenderCtx;
use crate::renderer::RenderCtxInner;
use crate::renderer::SqlFileRenderResult;
use crate::renderer::collect_adapter_identifiers_detect_unsafe;
use crate::renderer::render_unresolved_sql_files;
use crate::resolve::resolve_properties::MinimalPropertiesEntry;
use crate::resolve::resolve_tests::persist_generic_data_tests::format_node_unique_id;
use crate::resolve::resolve_utils::{
    build_unrendered_config, err_resource_name_has_spaces, validate_compute, validate_node_adapter,
};
use crate::utils::RelationComponents;
use crate::utils::extract_resource_config_from_raw_project;
use crate::utils::generate_relation_components;
use crate::utils::get_node_fqn;
use crate::utils::get_original_file_contents;
use crate::utils::get_original_file_path;
use crate::utils::parse_unrendered_config;
use crate::utils::update_node_relation_components;
use crate::validation::check_node_static_analysis;
use dbt_adapter_core::AdapterType;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::constants::DBT_GENERIC_TESTS_DIR_NAME;
use dbt_common::error::AbstractLocation;
use dbt_common::fs_err;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_common::io_utils::try_read_yml_to_str;
use dbt_common::path::DbtPath;
use dbt_common::stdfs;
use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;
use dbt_jinja_utils::jinja_arg_format::format_value_for_jinja;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::JinjaTypeCheckingEventListenerFactory;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::DbtTestAttr;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::IntrospectionKind;
use dbt_schemas::schemas::common::DbtChecksum;
use dbt_schemas::schemas::common::DbtContract;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::common::DbtQuoting;
use dbt_schemas::schemas::common::DocsConfig;
use dbt_schemas::schemas::common::NodeDependsOn;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::common::merge_vec;
use dbt_schemas::schemas::common::normalize_sql;
use dbt_schemas::schemas::nodes::DbtModel;
use dbt_schemas::schemas::nodes::DbtSeed;
use dbt_schemas::schemas::nodes::DbtSnapshot;
use dbt_schemas::schemas::nodes::TestMetadata;
use dbt_schemas::schemas::project::DataTestConfig;
use dbt_schemas::schemas::project::ResolvableConfig;
use dbt_schemas::schemas::project::ResolvedConfig;
use dbt_schemas::schemas::properties::DataTestProperties;
use dbt_schemas::schemas::properties::ModelProperties;
use dbt_schemas::schemas::ref_and_source::DbtRef;
use dbt_schemas::schemas::ref_and_source::DbtSourceWrapper;
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use dbt_schemas::schemas::{
    AdapterAttr, CommonAttributes, DbtTest, InternalDbtNode, NodeBaseAttributes,
};
use dbt_schemas::state::DbtRuntimeConfig;
use dbt_schemas::state::GenericTestAsset;
use dbt_schemas::state::ModelStatus;
use dbt_schemas::state::{DbtAsset, DbtPackage};
use dbt_yaml::Spanned;
use dbt_yaml::Value as YmlValue;
use indexmap::IndexMap;
use md5;
use minijinja::Value;
use serde::de;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::str::FromStr as _;
use std::sync::Arc;

/// Computes the unique_id for a generic test node.
///
/// Follows dbt-core's convention:
///   hash_string = fqn_name + repr(get_hashable_md(test_metadata))
///   unique_id   = "test.<package>.<fqn_name>.<last-10-of-md5(hash_string)>"
///
/// The fqn_name must be the full (non-truncated) name per dbt-core's design:
///   `short_name` (truncated) → compiled file path and alias only
///   `full_name`              → unique_id and FQN
/// See `synthesize_generic_test_names` in dbt-core's generic_test_builders.py.
fn compute_generic_test_unique_id(package_name: &str, test_asset: &GenericTestAsset) -> String {
    // Use the full (non-truncated) name for unique_id, matching dbt-core/Mantle behavior.
    // When truncation occurs, test_name holds the short form and original_name holds the full form.
    let fqn_name = test_asset
        .original_name
        .as_deref()
        .unwrap_or(&test_asset.test_name);
    // Prefer the pre-computed hash from persist_generic_data_tests (uses full kwargs),
    // falling back to a local computation with partial metadata.
    let test_hash = if let Some(hash) = &test_asset.unique_id_hash {
        hash.clone()
    } else {
        const HASH_LENGTH: usize = 10;
        let metadata_repr = build_test_metadata_repr(test_asset);
        let hash_string = format!("{}{}", fqn_name, metadata_repr);
        let hash_hex = format!("{:x}", md5::compute(&hash_string));
        hash_hex[hash_hex.len() - HASH_LENGTH..].to_string()
    };
    format!("test.{}.{}.{}", package_name, fqn_name, test_hash)
}

/// Build a Python-like repr of the test metadata for hashing.
/// This matches Mantle's `repr(get_hashable_md(test_metadata))` where:
/// - test_metadata = {"namespace": ..., "name": ..., "kwargs": {...}}
/// - kwargs contains column_name, model, combination_of_columns, etc.
///
/// The keys are sorted alphabetically to match Python's sorted dict behavior.
fn build_test_metadata_repr(asset: &GenericTestAsset) -> String {
    // Build kwargs dict repr - keys must be sorted alphabetically
    let mut kwargs_parts: Vec<String> = Vec::new();

    // column_name (sorted first)
    if let Some(col) = &asset.test_metadata_column_name {
        let col_repr = match col {
            StringOrArrayOfStrings::String(s) => format!("'{s}'"),
            StringOrArrayOfStrings::ArrayOfStrings(cols) => {
                let cols_repr: Vec<String> = cols.iter().map(|c| format!("'{c}'")).collect();
                format!("[{}]", cols_repr.join(", "))
            }
        };
        kwargs_parts.push(format!("'column_name': {col_repr}"));
    }

    // combination_of_columns (sorted second if present)
    if let Some(cols) = &asset.test_metadata_combination_of_columns {
        let cols_repr: Vec<String> = cols.iter().map(|c| format!("'{}'", c)).collect();
        kwargs_parts.push(format!(
            "'combination_of_columns': [{}]",
            cols_repr.join(", ")
        ));
    }

    // model (sorted after column_name/combination_of_columns)
    // Use double quotes if the value contains single quotes, like Python's repr
    if let Some(model) = &asset.test_metadata_model {
        if model.contains('\'') {
            kwargs_parts.push(format!("'model': \"{}\"", model));
        } else {
            kwargs_parts.push(format!("'model': '{}'", model));
        }
    }

    let kwargs_repr = format!("{{{}}}", kwargs_parts.join(", "));

    // Build the full metadata dict repr with sorted keys: kwargs, name, namespace
    let name = asset.test_metadata_name.as_deref().unwrap_or("");
    // In Python, repr(None) is 'None' (unquoted), not "'None'"
    let namespace_repr = match &asset.test_metadata_namespace {
        Some(ns) => format!("'{}'", ns),
        None => "'None'".to_string(),
    };

    // Keys are sorted: kwargs, name, namespace
    format!(
        "{{'kwargs': {}, 'name': '{}', 'namespace': {}}}",
        kwargs_repr, name, namespace_repr
    )
}

fn test_metadata_from_asset(asset: &GenericTestAsset) -> Option<TestMetadata> {
    if let Some(name) = &asset.test_metadata_name {
        let kwargs = if !asset.test_metadata_kwargs.is_empty() {
            asset.test_metadata_kwargs.clone()
        } else {
            // Fallback for assets constructed without test_metadata_kwargs (e.g. unit tests).
            let mut kwargs = BTreeMap::new();
            if let Some(col) = &asset.test_metadata_column_name {
                kwargs.insert("column_name".to_string(), col.to_yaml_value());
            }
            if let Some(cols) = &asset.test_metadata_combination_of_columns {
                let seq = cols
                    .iter()
                    .cloned()
                    .map(YmlValue::string)
                    .collect::<Vec<_>>();
                kwargs.insert(
                    "combination_of_columns".to_string(),
                    YmlValue::Sequence(seq, Default::default()),
                );
            }
            if let Some(model) = &asset.test_metadata_model {
                kwargs.insert("model".to_string(), YmlValue::string(model.clone()));
            }
            kwargs
        };
        return Some(TestMetadata {
            name: name.clone(),
            kwargs,
            namespace: asset.test_metadata_namespace.clone(),
        });
    }
    None
}

fn filter_core_builtin_test_macro_dependencies(
    test_asset: Option<&GenericTestAsset>,
    macros: &mut Vec<String>,
) {
    // This filtering is intentionally not a strictly semantic account of dependencies
    // in the test macro body. A project-defined test with the same name as a built-in
    // retains get_where_subquery even when its macro body does not call that helper,
    // because dbt-core renders the generated model kwarg for every test macro that does
    // not resolve to the exact built-in unique ID. Keep that behavior for manifest parity.
    let Some(test_name) = test_asset.and_then(|asset| asset.test_metadata_name.as_deref()) else {
        return;
    };
    let builtin_macro = match test_name {
        "not_null" => "macro.dbt.test_not_null",
        "unique" => "macro.dbt.test_unique",
        _ => return,
    };
    let test_macro_suffix = format!(".test_{test_name}");
    let is_core_builtin_test = macros
        .iter()
        .any(|macro_id| macro_id.as_str() == builtin_macro)
        && !macros.iter().any(|macro_id| {
            macro_id.as_str() != builtin_macro && macro_id.ends_with(&test_macro_suffix)
        });

    if is_core_builtin_test {
        macros.retain(|macro_id| macro_id != "macro.dbt.get_where_subquery");
    }
}

fn file_key_name_from_asset(asset: &GenericTestAsset) -> Option<String> {
    // Match dbt-core's `{yaml_key}.{name}` for generic tests. Source tests use
    // the source collection name rather than the table name.
    let yaml_key = match asset.resource_type.as_str() {
        "model" => "models",
        "seed" => "seeds",
        "snapshot" => "snapshots",
        "source" => "sources",
        "analysis" => "analyses",
        other => other,
    };
    let name = asset
        .source_name
        .as_deref()
        .unwrap_or(asset.resource_name.as_str());
    Some(format!("{yaml_key}.{name}"))
}

/// Checksum for a data test node, matching dbt-core.
///
/// A singular data test is parsed from its own `tests/*.sql` file, and dbt-core hashes that
/// file's *whitespace-normalized* contents, not the raw bytes:
/// `normalize_file_contents(contents) = " ".join(contents.split())`
/// (`core/dbt/parser/read_files.py:44-48`), then `FileHash.from_contents` over that
/// (`core/dbt/parser/read_files.py:91-96`). This matches how models and snapshots already hash
/// their bodies via `normalize_sql` (`dbt_schemas::schemas::common`).
///
/// A generic data test is synthesized from a `schema.yml` entry and has no file of its own, so
/// dbt-core leaves it at `FileHash.empty()` (`{"name": "none", "checksum": ""}`) — pass `None`.
/// That also keeps generic-test checksums stable across runs when schema names change.
fn data_test_checksum(singular_test_file_contents: Option<&str>) -> DbtChecksum {
    match singular_test_file_contents {
        Some(contents) => DbtChecksum::hash(normalize_sql(contents).as_bytes()),
        None => DbtChecksum::default(),
    }
}

pub fn build_data_test_raw_code(
    test_metadata: Option<TestMetadata>,
    alias: Option<String>,
    raw_code_config: Option<&BTreeMap<String, YmlValue>>,
) -> Option<String> {
    if let Some(test_metadata) = test_metadata {
        let mut test_macro_name = format!("test_{}", test_metadata.name);
        if let Some(namespace) = test_metadata.namespace {
            test_macro_name = format!("{}.{}", namespace, test_macro_name);
        }

        let mut config = raw_code_config.cloned().unwrap_or_default();
        if let Some(alias) = alias {
            config
                .entry("alias".to_string())
                .or_insert_with(|| YmlValue::string(alias));
        }
        let config_str = build_data_test_raw_code_config(&config);

        return Some(format!(
            "{{{{ {test_macro_name}(**_dbt_generic_test_kwargs) }}}}{config_str}"
        ));
    }

    None
}

fn build_data_test_raw_code_config(config: &BTreeMap<String, YmlValue>) -> String {
    const RAW_CODE_CONFIG_ARGS: &[&str] = &[
        "severity",
        "tags",
        "enabled",
        "where",
        "limit",
        "warn_if",
        "error_if",
        "fail_calc",
        "store_failures",
        "store_failures_as",
        "sql_header",
        "meta",
        "database",
        "schema",
        "alias",
    ];

    let args = RAW_CODE_CONFIG_ARGS
        .iter()
        .filter_map(|key| {
            let value = config.get(*key)?;
            let mut value: serde_json::Value = serde_json::to_value(value).ok()?;
            if value.as_array().is_some_and(Vec::is_empty)
                || value.as_object().is_some_and(serde_json::Map::is_empty)
            {
                return None;
            }
            if *key == "severity"
                && let serde_json::Value::String(severity) = &mut value
                && !(severity.contains("{{") && severity.contains("}}"))
            {
                severity.make_ascii_lowercase();
            }
            Some(format!(
                "{key}={}",
                format_value_for_jinja(&value, &BTreeMap::new())
            ))
        })
        .collect::<Vec<_>>();

    if args.is_empty() {
        String::new()
    } else {
        format!("{{{{ config({}) }}}}", args.join(", "))
    }
}

#[allow(clippy::too_many_arguments)]
/// Resolve a test node's top-level `group`.
///
/// For a test attached to a model (`attached_node` is `Some`), the model's
/// own group wins -- matching dbt Core, where an attached test's explicit
/// `config.group` is ignored in favor of the parent model's group (including
/// when the model has none).
///
/// For every other test -- generic tests on sources, and singular tests,
/// both of which have no attached node to inherit from -- dbt Core instead
/// uses the test's own resolved `config.group` directly. Falling through to
/// `None` here (as if there were simply nothing to inherit) silently dropped
/// `config.group` for these nodes, so group-scoped notifications never fired
/// for them. See https://github.com/dbt-labs/dbt-core/issues/15930.
fn resolve_test_group(
    attached_node: Option<&str>,
    models: &BTreeMap<String, Arc<DbtModel>>,
    own_config_group: Option<String>,
) -> Option<String> {
    match attached_node.and_then(|id| models.get(id)) {
        Some(model) => model.__model_attr__.group.clone(),
        None => own_config_group,
    }
}

pub async fn resolve_data_tests(
    arg: &ResolveArgs,
    package: &DbtPackage,
    package_quoting: DbtQuoting,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    test_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    database: &str,
    schema: &str,
    default_adapter: AdapterType,
    env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    collected_generic_tests: &[GenericTestAsset],
    node_resolver: &NodeResolver,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
    adapter_quoting: &IndexMap<AdapterType, DbtQuoting>,
    models: &BTreeMap<String, Arc<DbtModel>>,
    disabled_models: &BTreeMap<String, Arc<DbtModel>>,
    seeds: &BTreeMap<String, Arc<DbtSeed>>,
    snapshots: &BTreeMap<String, Arc<DbtSnapshot>>,
) -> FsResult<(HashMap<String, Arc<DbtTest>>, HashMap<String, Arc<DbtTest>>)> {
    let mut nodes: HashMap<String, Arc<DbtTest>> = HashMap::new();
    let mut nodes_with_execute: HashMap<String, DbtTest> = HashMap::new();
    let mut disabled_tests: HashMap<String, Arc<DbtTest>> = HashMap::new();
    let package_name = package.dbt_project.name.as_str();
    let dependency_package_name = dependency_package_name_from_ctx(&env, base_ctx);

    let test_key = if package.dbt_project.data_tests.is_some() {
        "data_tests"
    } else {
        "tests"
    };
    let is_dependency = dependency_package_name.is_some();
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        test_key,
        default_adapter,
    )?;
    let raw_root_project_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            test_key,
            default_adapter,
        )?)
    } else {
        None
    };

    // Create a map of dbt_asset.path.stem to GenericTestAsset for efficient lookup
    let test_path_to_test_asset: HashMap<PathBuf, &GenericTestAsset> = collected_generic_tests
        .iter()
        .map(|test_asset| (test_asset.dbt_asset.path.clone(), test_asset))
        .collect();

    let mut test_assets_to_render = package.test_files.clone();
    test_assets_to_render.extend(
        collected_generic_tests
            .iter()
            .map(|test_asset| test_asset.dbt_asset.clone()),
    );

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.tests.clone(),
        is_dependency,
        || {
            let tests_config = match (
                package.dbt_project.tests.clone(),
                package.dbt_project.data_tests.clone(),
            ) {
                (Some(_), Some(_)) => {
                    unimplemented!("Merge logic for tests and data tests is unimplemented")
                }
                (Some(tests), None) => Some(tests),
                (None, Some(data_tests)) => Some(data_tests),
                (None, None) => None,
            };
            let disallow_plus_prefix =
                disallow_plus_prefix_from_flags(root_package.dbt_project.flags.as_ref());
            init_project_config(
                &tests_config,
                DbtQuoting::default(),
                dependency_package_name,
                disallow_plus_prefix,
                default_adapter,
            )
        },
        default_adapter,
    )?
    .with_resolve_defaults((arg.static_analysis.unwrap_or_default(), arg.store_failures));

    let render_ctx = RenderCtx {
        inner: Arc::new(RenderCtxInner {
            args: arg.clone(),
            root_project_name: root_package.dbt_project.name.clone(),
            config_resolver,
            package_quoting,
            uses_snapshot_fqn: false,
            defer_render_errors_to_compile: true,
            base_ctx: base_ctx.clone(),
            package_name: package_name.to_string(),
            adapter_type: default_adapter,
            database: database.to_string(),
            schema: schema.to_string(),
            // tests can be defined in any yaml config
            resource_paths: package.dbt_project.all_source_paths(),
        }),
        jinja_env: env.clone(),
        runtime_config: runtime_config.clone(),
        root_runtime_config: root_runtime_config.clone(),
    };

    let mut test_sql_resources_map =
        render_unresolved_sql_files::<DataTestConfig, DataTestProperties>(
            &render_ctx,
            &test_assets_to_render,
            test_properties,
            token,
            jinja_type_checking_event_listener_factory.clone(),
        )
        .await?;
    // make deterministic
    test_sql_resources_map.sort_by(|a, b| {
        a.asset
            .path
            .file_name()
            .cmp(&b.asset.path.file_name())
            .then(a.asset.path.cmp(&b.asset.path))
    });

    for SqlFileRenderResult {
        asset: dbt_asset,
        sql_file_info,
        config: mut test_config,
        rendered_sql: _,
        macro_spans: _macro_spans,
        properties: maybe_properties,
        status,
        render_error_deferred,
        patch_path: _,
        ..
    } in test_sql_resources_map.into_iter()
    {
        // Use the custom test name from GenericTestAsset if available, otherwise use the filename.
        // test_name is the truncated form (for file paths); fqn_name is the full form
        // (for unique_id, name field, and FQN), matching dbt-core/Mantle behavior.
        let (test_name, fqn_name) =
            if let Some(test_asset) = test_path_to_test_asset.get(&dbt_asset.path) {
                let short = test_asset.test_name.clone();
                let full = test_asset
                    .original_name
                    .as_ref()
                    .unwrap_or(&test_asset.test_name)
                    .clone();
                (short, full)
            } else {
                let name = dbt_asset
                    .path
                    .file_stem()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                if name.contains(' ') {
                    return Err(err_resource_name_has_spaces(&name, &dbt_asset.path));
                }
                (name.clone(), name)
            };

        let properties = if let Some(properties) = maybe_properties.as_ref() {
            properties
        } else {
            &DataTestProperties::empty(test_name.to_owned())
        };

        // Merge column test tags into the top-level config.
        // Reference: https://github.com/dbt-labs/dbt-core/blob/b783c97eff9cf72e6fc43ef93523b8ec7b029583/core/dbt/parser/schema_generic_tests.py#L368
        let test_tags = test_config.tags.inner().clone().map(|tags| tags.into());
        let column_tags = test_path_to_test_asset
            .get(&dbt_asset.path)
            .map(|asset| asset.column_tags.clone());
        let tags = merge_vec(test_tags, column_tags);

        // To conform to the unique_id format in dbt-core, we need to hash the test name
        // plus the test metadata (namespace, name, kwargs) and append the last 10 characters
        // of the hash to the unique_id.
        // See the `create_test_node` function in
        // https://github.com/dbt-labs/dbt-core/blob/3de3b827bfffdc43845780f484d4d53011f20a37/core/dbt/parser/schema_generic_tests.py#L132
        let unique_id = if let Some(test_asset) = test_path_to_test_asset.get(&dbt_asset.path) {
            // Generic test: unique_id uses the full (non-truncated) name + metadata hash.
            compute_generic_test_unique_id(package_name, test_asset)
        } else {
            // Singular test: no hash suffix — dbt-core's SingularTestParser inherits
            // generate_unique_id(name) from SimpleSQLParser without a hash argument:
            // https://github.com/dbt-labs/dbt-core/blob/29d57865de9aca6372f516dfa52e4b9754b22fe0/core/dbt/parser/base.py#L251
            format!("test.{package_name}.{test_name}")
        };

        // Use the file stem as the lookup key, because that is what the renderer used
        // when it called create_listener — see resolve_model_context.rs where
        // unique_id = "{package_name}.{model_name}" and model_name = file stem.
        // Using fqn_name here caused a key miss when the name was truncated, leaving
        // depends_on.macros empty (dbt-core#15308). The stem usually equals test_name,
        // but diverges for name-collided generic tests whose file carries a hash suffix.
        let renderer_name = dbt_asset
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&test_name);
        jinja_type_checking_event_listener_factory
            .update_unique_id(&format!("{package_name}.{renderer_name}"), &unique_id);
        let mut macro_depends_on =
            jinja_type_checking_event_listener_factory.get_macro_depends_on(&unique_id);
        filter_core_builtin_test_macro_dependencies(
            test_path_to_test_asset.get(&dbt_asset.path).copied(),
            &mut macro_depends_on,
        );

        // Check if this test_name corresponds to any test in our collected tests
        // If so, use the original_file_path from the GenericTestAsset for the fqn construction and original_file_path
        let path_for_fqn = dbt_asset.original_path.clone();

        // singular data tests are only found in test_paths, but generic tests
        // can be found in any directory in all_source_paths
        let fqn = get_node_fqn(
            package_name,
            path_for_fqn,
            vec![fqn_name.clone()],
            &package.dbt_project.all_source_paths(),
        );

        let static_analysis = test_config.static_analysis.clone();
        check_node_static_analysis(
            &test_config,
            arg.static_analysis,
            unique_id.as_str(),
            dependency_package_name,
        );
        validate_compute(test_config.compute, &dbt_asset.path)?;

        let test_asset = test_path_to_test_asset.get(&dbt_asset.path);
        // Match dbt-core's _lookup_attached_node: source tests get no attached_node;
        // model/seed/snapshot tests get the parent's unique_id (with .v<version> for
        // versioned models).
        let attached_node = test_asset.and_then(|ta| {
            if ta.resource_type == "source" {
                None
            } else {
                Some(format_node_unique_id(
                    &ta.resource_type,
                    &ta.dbt_asset.package_name,
                    &ta.resource_name,
                    None,
                    ta.version.as_deref(),
                ))
            }
        });

        // A test runs where its subject runs, so an unset `+adapter` is inherited
        // from `attached_node` -- the same lookup the `group` inheritance below
        // does. `lake_compute` inherits like any other adapter: a test has no
        // materialization to bypass Jinja for, so it executes as a plain read
        // through the LakeCompute adapter (see `execute_test_on_compute_platform`
        // in `compute_platform.rs`), the same way it would on any other adapter.
        let inherited_adapter = attached_node.as_deref().and_then(|id| {
            models
                .get(id)
                .map(|m| m.node_adapter())
                .or_else(|| seeds.get(id).map(|s| s.node_adapter()))
                .or_else(|| snapshots.get(id).map(|s| s.node_adapter()))
        });
        // See `resolve_models`: the flag overrides the config. `None` from both
        // leaves inheritance to fill the gap.
        validate_node_adapter(test_config.adapter, &dbt_asset.path)?;
        let resolved_node_adapter = arg
            .adapter_override
            .or(test_config.adapter)
            .or(inherited_adapter);

        // See `resolve_models`: both remaining quoting layers depend on which
        // adapter the node runs on, which is only known after the config merge.
        let selected_adapter = resolved_node_adapter.unwrap_or(default_adapter);
        test_config.quoting = resolve_package_quoting(
            Some(match adapter_quoting.get(&selected_adapter) {
                Some(authored) => test_config.quoting.filled_from(authored),
                None => test_config.quoting,
            }),
            selected_adapter,
        );

        // NOTE: This says get_original_file_path but for tests this is the path to the generated sql file
        let generated_file_path =
            get_original_file_path(&dbt_asset.base_path, &arg.io.in_dir, &dbt_asset.path);

        let defined_at = test_path_to_test_asset
            .get(&dbt_asset.path)
            .map(|test_asset| test_asset.defined_at.clone());

        let patch_path = &dbt_asset.original_path.clone();

        let is_singular_data_test = defined_at.is_none();

        let manifest_original_file_path = if is_singular_data_test {
            generated_file_path.clone()
        } else {
            DbtPath::from(patch_path)
        };

        // Singular tests are backed by their own `.sql` file, so read its contents once here:
        // both `checksum` (below) and `raw_code` (further down) are derived from these exact
        // bytes, mirroring dbt-core, where a singular test node's checksum and raw_code both
        // come from the same source file. Generic tests have no file of their own.
        let singular_test_file_contents = if is_singular_data_test {
            get_original_file_contents(&arg.io.in_dir, &manifest_original_file_path)
        } else {
            None
        };

        // Populate TestMetadata only for generic data tests (not singular .sql tests)
        // This ensures external tooling (e.g., project-evaluator) correctly classifies tests
        let inferred_test_metadata = if is_singular_data_test {
            None
        } else {
            test_path_to_test_asset
                .get(&dbt_asset.path)
                .and_then(|asset| test_metadata_from_asset(asset))
        };

        // Generic tests carry their `description:` on the GenericTestAsset (from schema.yml);
        // singular tests carry it in their file-level properties. dbt-core surfaces both on
        // the test node's `description`, so mirror that here rather than defaulting to ''.
        let test_description = if is_singular_data_test {
            properties.description.clone().unwrap_or_default()
        } else {
            test_path_to_test_asset
                .get(&dbt_asset.path)
                .and_then(|asset| asset.description.clone())
                .unwrap_or_default()
        };

        // For singular tests, parse the user-written SQL for inline {{ config(...) }}.
        let raw_inline_config = if is_singular_data_test {
            dbt_common::tokiofs::read_to_string(dbt_asset.base_path.join(&dbt_asset.path))
                .await
                .ok()
                .and_then(|sql| parse_unrendered_config(&sql, false))
        } else {
            None
        };

        // For generic tests, the schema.yml `config:` block (`where`, `severity`, `limit`,
        // schema.yml `tags`/`meta`, …) is captured raw (pre-render) on the GenericTestAsset by
        // `persist` and passed here as the `schema` arg, matching how models/seeds/snapshots
        // populate unrendered_config. Singular tests carry no such asset (raw_inline_config
        // covers them).
        let raw_schema_config = test_path_to_test_asset
            .get(&dbt_asset.path)
            .map(|test_asset| &test_asset.unrendered_schema_config);
        let unrendered_config = build_unrendered_config(
            &fqn,
            &raw_local_project_config,
            raw_root_project_cfg.as_ref(),
            raw_schema_config,
            raw_inline_config.as_ref(),
            false,
            default_adapter,
        )?;

        let mut dbt_test = DbtTest {
            defined_at,
            manifest_original_file_path: manifest_original_file_path.clone(),
            __common_attr__: CommonAttributes {
                name: fqn_name.clone(),
                package_name: package_name.to_owned(),
                path: DbtPath::from(dbt_asset.path.to_owned()),
                name_span: dbt_common::Span::default(),
                // original_file_path is a misnomer for tests, it's the path to the generated sql file
                original_file_path: generated_file_path,
                // The patch-path is always set to None in Core:
                // https://github.com/dbt-labs/dbt-mantle/blob/da5abca4f829b167bd1b1d5c6666c12cd8c719c0/core/dbt/parser/schema_generic_tests.py#L119
                patch_path: None,
                unique_id: unique_id.clone(),
                fqn,
                description: Some(test_description),
                checksum: data_test_checksum(singular_test_file_contents.as_deref()),
                // TODO: hydrate for generic + singular tests
                // Examples in Mantle:
                // - Generic test: "{{ test_not_null(**_dbt_generic_test_kwargs) }}"
                // - Generic test with dbt_utils.expression_is_true: "{{ dbt_utils.test_expression_is_true(**_dbt_generic_test_kwargs) }}{{ config(alias=\"dbt_utils_expression_is_true_c_177c20685a18a9071d4a71719e3d9565\") }}"
                // - Singular test: "SELECT 1\nFROM {{ ref('customers') }}\nLIMIT 0"
                raw_code: Some("will_be_updated_below".to_string()),
                language: Some("sql".to_string()),
                tags: tags.unwrap_or_default(),
                classifiers: Default::default(),
                meta: test_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: selected_adapter,
                // A data test never runs on lake compute (see `inherited_adapter` above), so
                // it has nothing to publish: no `+propagate` config exists for this node type.
                propagate: Vec::new(),
                database: database.to_owned(),
                schema: schema.to_owned(),
                alias: "will_be_updated_below".to_owned(),
                relation_name: None,
                static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                    .then_some(StaticAnalysisOffReason::ConfiguredOff),
                static_analysis,
                compute: test_config.compute,
                quoting: test_config
                    .quoting
                    .try_into()
                    .expect("DbtQuoting -> ResolvedQuoting conversion"),
                quoting_ignore_case: test_config.quoting.snowflake_ignore_case.unwrap_or(false),
                materialized: test_config.materialized.clone(),
                enabled: test_config.enabled,
                extended_model: false,
                persist_docs: None,
                columns: vec![],
                depends_on: NodeDependsOn {
                    macros: macro_depends_on,
                    nodes: vec![],
                    nodes_with_ref_location: vec![],
                },
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
                unrendered_config,
            },
            __test_attr__: {
                let group =
                    resolve_test_group(attached_node.as_deref(), models, test_config.group.clone());
                DbtTestAttr {
                    column_name: test_asset.and_then(|ta| ta.column_name.clone()),
                    attached_node,
                    test_metadata: inferred_test_metadata.clone(),
                    file_key_name: test_asset.and_then(|ta| file_key_name_from_asset(ta)),
                    introspection: IntrospectionKind::None,
                    original_name: test_asset.and_then(|ta| ta.original_name.clone()),
                    group,
                    state: test_config.state.clone(),
                }
            },
            __adapter_attr__: AdapterAttr::from_config_and_dialect(
                &test_config.__warehouse_specific_config__,
                default_adapter,
            ),
            deprecated_config: test_config.clone().into(),
            __other__: BTreeMap::new(),
        };

        let components = RelationComponents {
            database: test_config.database.clone(),
            schema: test_config.schema.clone(),
            // When test name was truncated (test_name != fqn_name), use the short form
            // for the alias (table name) per dbt-core convention. dbt-core uses:
            //   short_name → compiled file path and alias only
            //   full_name  → unique_id and FQN
            // Without an explicit alias override, fall back to this short form so the
            // generated CREATE TABLE SQL matches the recorded (and expected) table name.
            alias: test_config.alias.clone().or_else(|| {
                if test_name != fqn_name {
                    Some(test_name.clone())
                } else {
                    None
                }
            }),
            store_failures: Some(test_config.store_failures.unwrap_or(false) || arg.store_failures),
        };

        // Update with relation components
        update_node_relation_components(
            &mut dbt_test,
            &env,
            &root_package.dbt_project.name,
            package_name,
            base_ctx,
            &components,
            default_adapter,
        )?;

        // Mirror dbt-core behavior: when the synthesized name was truncated and the user
        // provided no explicit alias, backfill both config representations with the short
        // name so manifest config.alias and unrendered_config.alias match core.
        if test_config.alias.is_none() && test_name != fqn_name {
            dbt_test.deprecated_config.alias = Some(test_name.clone());
            dbt_test
                .__base_attr__
                .unrendered_config
                .entry("alias".to_string())
                .or_insert_with(|| YmlValue::from(test_name.as_str()));
        }

        dbt_test.__common_attr__.raw_code = if is_singular_data_test {
            singular_test_file_contents
        } else {
            build_data_test_raw_code(
                inferred_test_metadata,
                components.alias.clone(),
                test_path_to_test_asset
                    .get(&dbt_asset.path)
                    .map(|asset| &asset.raw_code_config),
            )
        };

        let parent_is_disabled = dbt_test
            .__test_attr__
            .attached_node
            .as_deref()
            .is_some_and(|id| disabled_models.contains_key(id));

        // match core behavior: store tests for disabled models in nodes (not disabled_nodes) with enabled = false
        // but only when the test itself is not also explicitly disabled
        if parent_is_disabled && status != ModelStatus::Disabled {
            dbt_test.__base_attr__.enabled = false;
            dbt_test.deprecated_config.enabled = Some(false);
            nodes.insert(unique_id, Arc::new(dbt_test));
        } else {
            match status {
                ModelStatus::Enabled => {
                    if sql_file_info.execute {
                        nodes_with_execute.insert(unique_id.to_owned(), dbt_test);
                    } else {
                        nodes.insert(unique_id, Arc::new(dbt_test));
                    }
                }
                ModelStatus::Disabled => {
                    disabled_tests.insert(unique_id, Arc::new(dbt_test));
                }
                ModelStatus::ParsingFailed => {
                    if render_error_deferred {
                        nodes.insert(unique_id, Arc::new(dbt_test));
                    }
                }
            }
        }
    }

    // Second pass to capture all identifiers with the appropriate context
    // `models_with_execute` should never have overlapping Arc pointers with `models` and `disabled_models`
    // otherwise make_mut will clone the inner model, and the modifications inside this function call will be lost
    let tests_rest = collect_adapter_identifiers_detect_unsafe(
        arg,
        nodes_with_execute,
        node_resolver,
        env.clone(),
        default_adapter,
        package.dbt_project.name.as_str(),
        &root_package.dbt_project.name,
        runtime_config,
        token,
    )
    .await?;

    nodes.extend(
        tests_rest
            .into_iter()
            .map(|(k, _)| (k.__common_attr__.unique_id.clone(), Arc::new(k))),
    );

    Ok((nodes, disabled_tests))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::nodes::DbtModelAttr;
    use dbt_schemas::state::DbtAsset;

    fn model_with_group(group: Option<&str>) -> Arc<DbtModel> {
        Arc::new(DbtModel {
            __model_attr__: DbtModelAttr {
                group: group.map(str::to_string),
                ..Default::default()
            },
            ..Default::default()
        })
    }

    /// Regression tests for https://github.com/dbt-labs/dbt-core/issues/15930:
    /// a test with no attached node (generic tests on sources, singular
    /// tests) must fall back to its own resolved `config.group`, not `None`.
    #[test]
    fn resolve_test_group_uses_own_config_when_no_attached_node() {
        let models = BTreeMap::new();
        assert_eq!(
            resolve_test_group(None, &models, Some("test_group".to_string())),
            Some("test_group".to_string())
        );
    }

    #[test]
    fn resolve_test_group_is_none_when_no_attached_node_and_no_own_group() {
        let models = BTreeMap::new();
        assert_eq!(resolve_test_group(None, &models, None), None);
    }

    /// An attached test's own `config.group` is irrelevant: the parent
    /// model's group always wins, matching dbt Core.
    #[test]
    fn resolve_test_group_uses_attached_models_group() {
        let mut models = BTreeMap::new();
        models.insert(
            "model.pkg.stg_thing".to_string(),
            model_with_group(Some("model_group")),
        );
        assert_eq!(
            resolve_test_group(
                Some("model.pkg.stg_thing"),
                &models,
                Some("test_own_group".to_string()),
            ),
            Some("model_group".to_string())
        );
    }

    /// A group-less attached model yields `None`, even if the test declares
    /// its own `config.group` -- reproducing the (correct, matches dbt Core)
    /// "group-less model with explicit test config" row from #15930's table.
    #[test]
    fn resolve_test_group_is_none_when_attached_model_has_no_group() {
        let mut models = BTreeMap::new();
        models.insert("model.pkg.stg_thing".to_string(), model_with_group(None));
        assert_eq!(
            resolve_test_group(
                Some("model.pkg.stg_thing"),
                &models,
                Some("test_own_group".to_string()),
            ),
            None
        );
    }

    /// `attached_node` pointing at a unique_id absent from `models` (e.g. a
    /// disabled or otherwise-excluded model) is treated the same as no
    /// attached node at all: fall back to the test's own group.
    #[test]
    fn resolve_test_group_falls_back_when_attached_model_not_found() {
        let models = BTreeMap::new();
        assert_eq!(
            resolve_test_group(
                Some("model.pkg.missing"),
                &models,
                Some("test_group".to_string()),
            ),
            Some("test_group".to_string())
        );
    }

    #[test]
    fn test_builds_test_metadata_from_asset_single_col() {
        let asset = GenericTestAsset {
            dbt_asset: DbtAsset {
                base_path: PathBuf::new(),
                original_path: PathBuf::from("models/schema.yml"),
                path: PathBuf::from("generic_tests/not_null_customers_id.sql"),
                package_name: "pkg".to_string(),
            },
            resource_name: "customers".to_string(),
            resource_type: "model".to_string(),
            source_name: None,
            test_name: "not_null_customers_id".to_string(),
            defined_at: Default::default(),
            column_name: None,
            test_metadata_name: Some("not_null".to_string()),
            test_metadata_namespace: None,
            test_metadata_column_name: Some(StringOrArrayOfStrings::String("id".to_string())),
            test_metadata_combination_of_columns: None,
            test_metadata_model: None,
            test_metadata_kwargs: BTreeMap::new(),
            raw_code_config: BTreeMap::new(),
            original_name: None,
            unique_id_hash: None,
            version: None,
            column_tags: vec![],
            unrendered_schema_config: BTreeMap::new(),
            description: None,
        };
        let md = test_metadata_from_asset(&asset).expect("metadata");
        assert_eq!(md.name, "not_null");
        assert!(md.namespace.is_none());
        assert_eq!(
            md.kwargs
                .get("column_name")
                .and_then(|v| v.as_str())
                .unwrap(),
            "id"
        );
    }

    /// A singular data test is parsed from its own `.sql` file, so its checksum must be the
    /// sha256 of that file's *whitespace-normalized* contents — the value dbt-core's
    /// `FileHash.from_contents(normalize_file_contents(contents))` produces
    /// (`core/dbt/parser/read_files.py:44-48,91-96`). The expected digest below is computed
    /// independently of this crate:
    ///
    /// ```text
    /// printf %s "select 1 from {{ ref('customers') }} where x < 0" | sha256sum
    /// ```
    #[test]
    fn test_singular_data_test_checksum_is_sha256_of_normalized_file_contents() {
        const SQL: &str = "select 1 from {{ ref('customers') }} where x < 0";
        const EXPECTED_SHA256: &str =
            "0189810ca51b8d0a3a0dd389ed3b616ca34a50d47248d42af82d2cdd16158ff8";

        let in_dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(in_dir.path().join("tests")).expect("mkdir tests");
        let relative_path = PathBuf::from("tests").join("assert_no_negatives.sql");
        // Surrounding whitespace is stripped by dbt-core's `load_file_contents` /
        // `FileHash.from_contents` pair, so it must not change the digest.
        std::fs::write(in_dir.path().join(&relative_path), format!("\n{SQL}\n"))
            .expect("write test file");

        let contents = get_original_file_contents(in_dir.path(), &relative_path)
            .expect("singular test file contents");
        assert_eq!(contents, SQL);

        let checksum = data_test_checksum(Some(&contents));
        assert_eq!(checksum.as_checksum_string(), EXPECTED_SHA256);
        match &checksum {
            DbtChecksum::Object(object) => assert_eq!(object.name, "sha256"),
            other => panic!("expected object-form checksum, got {other:?}"),
        }
    }

    /// The invariant Defect 1 (fs#13735) was missing: two singular test bodies that differ only
    /// in whitespace (blank lines, indentation, newline-vs-space) must hash identically, matching
    /// dbt-core's `normalize_file_contents = " ".join(contents.split())`.
    #[test]
    fn test_singular_data_test_checksum_is_whitespace_invariant() {
        let compact = "select *\nfrom {{ ref('my_model') }}\nwhere id is null";
        let spread = "select *\n\n    from {{ ref('my_model') }}\n\n\n    where id is null";

        let checksum_compact = data_test_checksum(Some(compact));
        let checksum_spread = data_test_checksum(Some(spread));
        assert_eq!(checksum_compact, checksum_spread);

        // Sanity: a genuine content change (not whitespace-only) still hashes differently.
        let different = "select * from {{ ref('my_model') }}";
        let checksum_different = data_test_checksum(Some(different));
        assert_ne!(checksum_compact, checksum_different);
    }

    /// A generic data test is synthesized from `schema.yml` and has no file of its own, so it
    /// keeps dbt-core's `FileHash.empty()`.
    #[test]
    fn test_generic_data_test_checksum_is_empty_file_hash() {
        let checksum = data_test_checksum(None);
        assert_eq!(checksum, DbtChecksum::default());
        match &checksum {
            DbtChecksum::Object(object) => {
                assert_eq!(object.name, "none");
                assert_eq!(object.checksum, "");
            }
            other => panic!("expected object-form checksum, got {other:?}"),
        }
    }

    #[test]
    fn test_raw_code_config_preserves_jinja_severity_expression() {
        let mut config = BTreeMap::new();
        config.insert(
            "severity".to_string(),
            YmlValue::string("{{ env_var('DBT_SEVERITY') }}".to_string()),
        );

        assert_eq!(
            build_data_test_raw_code_config(&config),
            "{{ config(severity=env_var('DBT_SEVERITY')) }}"
        );
    }

    #[test]
    fn test_raw_code_config_normalizes_literal_severity() {
        let mut config = BTreeMap::new();
        config.insert("severity".to_string(), YmlValue::string("WARN".to_string()));

        assert_eq!(
            build_data_test_raw_code_config(&config),
            "{{ config(severity=\"warn\") }}"
        );
    }

    #[test]
    fn test_raw_code_config_normalizes_non_enum_literal_severity() {
        let mut config = BTreeMap::new();
        config.insert(
            "severity".to_string(),
            YmlValue::string("CUSTOM".to_string()),
        );

        assert_eq!(
            build_data_test_raw_code_config(&config),
            "{{ config(severity=\"custom\") }}"
        );
    }

    #[test]
    fn test_unique_id_uses_full_name_when_truncated() {
        // When a generic test name is truncated (>=64 chars), the unique_id must use
        // the full (non-truncated) name to match dbt-core/Mantle behavior.
        // dbt-core's synthesize_generic_test_names returns two names:
        //   short_name → compiled file path and alias only
        //   full_name  → unique_id and FQN
        // Fusion stores the full name in GenericTestAsset.original_name when truncation occurs.
        let truncated_name = "not_null_my_model_with_a_very_l_a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4";
        let full_name =
            "not_null_my_model_with_a_very_long_column_name_that_exceeds_sixty_four_characters";
        assert!(
            full_name.len() >= 64,
            "test setup: full_name must be >=64 chars"
        );
        let asset = GenericTestAsset {
            dbt_asset: DbtAsset {
                base_path: PathBuf::new(),
                original_path: PathBuf::from("models/schema.yml"),
                path: PathBuf::from(format!("generic_tests/{truncated_name}.sql")),
                package_name: "my_project".to_string(),
            },
            resource_name: "my_model".to_string(),
            resource_type: "model".to_string(),
            source_name: None,
            test_name: truncated_name.to_string(),
            defined_at: Default::default(),
            column_name: None,
            test_metadata_name: Some("not_null".to_string()),
            test_metadata_namespace: None,
            test_metadata_column_name: Some(StringOrArrayOfStrings::String(
                "very_long_column_name_that_exceeds_sixty_four_characters".to_string(),
            )),
            test_metadata_combination_of_columns: None,
            test_metadata_model: Some("ref('my_model')".to_string()),
            test_metadata_kwargs: BTreeMap::new(),
            raw_code_config: BTreeMap::new(),
            original_name: Some(full_name.to_string()),
            unique_id_hash: None,
            version: None,
            column_tags: vec![],
            unrendered_schema_config: BTreeMap::new(),
            description: None,
        };

        let unique_id = compute_generic_test_unique_id("my_project", &asset);

        assert!(
            unique_id.contains(full_name),
            "unique_id must use the full (non-truncated) name; got: {unique_id}"
        );
        assert!(
            !unique_id.starts_with(&format!("test.my_project.{truncated_name}.")),
            "unique_id must not use the truncated name; got: {unique_id}"
        );
    }

    #[test]
    fn test_builds_test_metadata_from_asset_combo_cols() {
        let asset = GenericTestAsset {
            dbt_asset: DbtAsset {
                base_path: PathBuf::new(),
                original_path: PathBuf::from("models/schema.yml"),
                path: PathBuf::from(
                    "generic_tests/unique_combination_of_columns_customers_a__b.sql",
                ),
                package_name: "pkg".to_string(),
            },
            resource_name: "customers".to_string(),
            resource_type: "model".to_string(),
            source_name: None,
            test_name: "unique_combination_of_columns_customers_a__b".to_string(),
            defined_at: Default::default(),
            column_name: None,
            test_metadata_name: Some("unique_combination_of_columns".to_string()),
            test_metadata_namespace: None,
            test_metadata_column_name: None,
            test_metadata_combination_of_columns: Some(vec!["a".to_string(), "b".to_string()]),
            test_metadata_model: None,
            test_metadata_kwargs: BTreeMap::new(),
            raw_code_config: BTreeMap::new(),
            original_name: None,
            unique_id_hash: None,
            version: None,
            column_tags: vec![],
            unrendered_schema_config: BTreeMap::new(),
            description: None,
        };
        let md = test_metadata_from_asset(&asset).expect("metadata");
        assert_eq!(md.name, "unique_combination_of_columns");
        let vals: Vec<String> = match md.kwargs.get("combination_of_columns").unwrap() {
            YmlValue::Sequence(arr, _) => arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            _ => panic!("expected sequence for combination_of_columns"),
        };
        assert_eq!(vals, vec!["a", "b"]);
    }
}
