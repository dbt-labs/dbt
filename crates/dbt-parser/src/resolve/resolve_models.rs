use crate::args::ResolveArgs;
use crate::dbt_project_config::ProjectConfigResolver;
use crate::dbt_project_config::RootProjectConfigs;
use crate::dbt_project_config::disallow_plus_prefix_from_flags;
use crate::dbt_project_config::init_project_config;
use crate::python_ast::parse_python;
use crate::python_file_info::PythonFileInfo;
use crate::python_validation::validate_python_model;
use crate::python_visitor::analyze_python_file;
use crate::renderer::RenderCtx;
use crate::renderer::RenderCtxInner;
use crate::renderer::SqlFileRenderResult;
use crate::renderer::collect_adapter_identifiers_detect_unsafe;
use crate::renderer::render_unresolved_sql_files;
use crate::resolve::resolve_utils::build_unrendered_config;
use crate::resolve::resolve_utils::err_resource_name_has_spaces;
use crate::resolve::resolve_utils::extract_config_map;
use crate::utils::RelationComponents;
use crate::utils::extract_resource_config_from_raw_project;
use crate::utils::get_node_fqn;
use crate::utils::get_original_file_path;
use crate::utils::get_unique_id;
use crate::utils::parse_unrendered_config;
use crate::utils::update_node_relation_components;
use crate::validation::check_node_static_analysis;

use dbt_adapter::load_catalogs;
use dbt_adapter_core::AdapterType;
use dbt_common::CodeLocationWithFile;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::cancellation::CancellationToken;
use dbt_common::error::AbstractLocation;
use dbt_common::fs_err;
use dbt_common::io_args::StaticAnalysisKind;
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_common::path::DbtPath;
use dbt_common::tokiofs::read_to_string;
use dbt_common::tracing::dbt_emit::emit_error_log_from_fs_error;
use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::listener::JinjaTypeCheckingEventListenerFactory;
use dbt_jinja_utils::node_resolver::NodeResolver;
use dbt_jinja_utils::utils::dependency_package_name_from_ctx;
use dbt_schemas::dbt_utils::resolve_package_quoting;
use dbt_schemas::schemas::CommonAttributes;
use dbt_schemas::schemas::DbtModel;
use dbt_schemas::schemas::DbtModelAttr;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_schemas::schemas::IntrospectionKind;
use dbt_schemas::schemas::NodeBaseAttributes;
use dbt_schemas::schemas::TimeSpine;
use dbt_schemas::schemas::TimeSpinePrimaryColumn;
use dbt_schemas::schemas::common::Access;
use indexmap::IndexMap;

use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::common::DbtQuoting;
use dbt_schemas::schemas::common::FreshnessRules;
use dbt_schemas::schemas::common::ModelFreshnessRules;
use dbt_schemas::schemas::common::NodeDependsOn;
use dbt_schemas::schemas::common::OnSchemaChange;
use dbt_schemas::schemas::common::Versions;
use dbt_schemas::schemas::common::normalize_sql;
use dbt_schemas::schemas::dbt_catalogs::{DbtCatalogs, LoadedCatalogs};
use dbt_schemas::schemas::dbt_column::ColumnInheritanceRules;
use dbt_schemas::schemas::dbt_column::ColumnProperties;
use dbt_schemas::schemas::dbt_column::DbtColumnRef;
use dbt_schemas::schemas::dbt_column::VersionColumnProperties;
use dbt_schemas::schemas::dbt_column::process_columns;
use dbt_schemas::schemas::manifest::semantic_model::NodeRelation;
use dbt_schemas::schemas::nodes::AdapterAttr;
use dbt_schemas::schemas::project::DbtProject;
use dbt_schemas::schemas::project::ModelConfig;
use dbt_schemas::schemas::project::ResolvedModelConfig;
use dbt_schemas::schemas::properties::ModelConstraint;
use dbt_schemas::schemas::properties::ModelFreshness;
use dbt_schemas::schemas::properties::ModelProperties;
use dbt_schemas::schemas::ref_and_source::{DbtRef, DbtSourceWrapper};
use dbt_schemas::schemas::serde::NodeVersion;
use dbt_schemas::state::DbtPackage;
use dbt_schemas::state::DbtRuntimeConfig;
use dbt_schemas::state::GenericTestAsset;
use dbt_schemas::state::ModelStatus;
use dbt_schemas::state::NodeResolverTracker;
use dbt_schemas::state::ResourcePathKind;
use dbt_yaml::Spanned;
use minijinja::MacroSpans;
use minijinja::constants::CURRENT_PATH;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::resolve_properties::MinimalPropertiesEntry;
use super::resolve_tests::persist_generic_data_tests::TestableNodeTrait;
use super::resolve_tests::persist_generic_data_tests::{
    TestUnrenderedConfigs, extract_test_unrendered_configs,
};
use super::resolve_utils::{validate_compute, validate_node_adapter};
use super::validate_models::validate_model;

/// Parses `ref('name')`, `ref('pkg', 'name')`, `ref('name', version=N)`, or
/// `ref('pkg', 'name', version=N)` from a constraint `to:` string (also accepts `v=` alias).
/// Returns `(package, name, version)`. Mirrors dbt-core's `statically_parse_ref_or_source`.
fn parse_ref_from_constraint(to: &str) -> Option<(Option<String>, String, Option<NodeVersion>)> {
    let s = to.trim();
    if !s.starts_with("ref(") || !s.ends_with(')') {
        return None;
    }
    let inner = s[4..s.len() - 1].trim();

    let mut positional: Vec<&str> = Vec::new();
    let mut version: Option<NodeVersion> = None;

    for part in inner.split(',') {
        let part = part.trim();
        if let Some(v) = part
            .strip_prefix("version=")
            .or_else(|| part.strip_prefix("v="))
        {
            let v = v.trim();
            let is_quoted = v.starts_with('\'') || v.starts_with('"');
            let v = v.trim_matches(|c| c == '\'' || c == '"');
            version = Some(if is_quoted {
                NodeVersion::String(v.to_string())
            } else if let Ok(n) = v.parse::<i64>() {
                NodeVersion::Integer(n)
            } else if let Ok(f) = v.parse::<f64>() {
                NodeVersion::Float(f)
            } else {
                NodeVersion::String(v.to_string())
            });
        } else {
            positional.push(part.trim_matches(|c| c == '\'' || c == '"'));
        }
    }

    match positional.as_slice() {
        [name] => Some((None, (*name).to_string(), version)),
        [pkg, name] => Some((Some((*pkg).to_string()), (*name).to_string(), version)),
        _ => None,
    }
}

/// Parses `source('source_name', 'table_name')` from a constraint `to:` string.
fn parse_source_from_constraint(to: &str) -> Option<(String, String)> {
    let s = to.trim();
    if !s.starts_with("source(") || !s.ends_with(')') {
        return None;
    }
    let inner = s[7..s.len() - 1].trim();
    let mut parts = inner.splitn(2, ',');
    let src = parts.next()?.trim().trim_matches(|c| c == '\'' || c == '"');
    let tbl = parts.next()?.trim().trim_matches(|c| c == '\'' || c == '"');
    Some((src.to_string(), tbl.to_string()))
}

#[allow(
    clippy::cognitive_complexity,
    clippy::expect_fun_call,
    clippy::too_many_arguments
)]
pub async fn resolve_models(
    arg: &ResolveArgs,
    package: &DbtPackage,
    package_quoting: DbtQuoting,
    // Authored quoting per declared adapter name: the adapter's `adapters:` entry
    // in the root dbt_project.yml, plus the top-level `quoting:` block for the
    // default adapter only. Unresolved, so a node's own `+quoting:` still wins.
    // See `authored_quoting_per_adapter`.
    adapter_quoting: &IndexMap<AdapterType, DbtQuoting>,
    root_package: &DbtPackage,
    root_project_configs: &RootProjectConfigs,
    models_properties: &BTreeMap<String, MinimalPropertiesEntry>,
    database: &str,
    schema: &str,
    default_adapter: AdapterType,
    package_name: &str,
    env: Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    runtime_config: Arc<DbtRuntimeConfig>,
    root_runtime_config: Arc<DbtRuntimeConfig>,
    collected_generic_tests: &mut Vec<GenericTestAsset>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    node_resolver: &mut NodeResolver,
    token: &CancellationToken,
    jinja_type_checking_event_listener_factory: Arc<dyn JinjaTypeCheckingEventListenerFactory>,
) -> FsResult<(
    HashMap<String, Arc<DbtModel>>,
    HashMap<String, (String, MacroSpans)>,
    HashMap<String, Arc<DbtModel>>,
)> {
    let mut models: HashMap<String, Arc<DbtModel>> = HashMap::new();
    let mut models_with_execute: HashMap<String, DbtModel> = HashMap::new();
    let mut disabled_models: HashMap<String, Arc<DbtModel>> = HashMap::new();
    let mut node_names = HashSet::new();
    let mut rendering_results: HashMap<String, (String, MacroSpans)> = HashMap::new();
    let dependency_package_name = dependency_package_name_from_ctx(&env, base_ctx);

    let is_dependency = dependency_package_name.is_some();
    // Best-effort raw parse of the root project's `models:` subtree, used only to hydrate
    // dependency package nodes' `unrendered_config` with root overrides (preserving Jinja).
    let raw_local_project_config = extract_resource_config_from_raw_project(
        &package.raw_project_yml,
        "models",
        default_adapter,
    )?;
    let raw_root_project_models_cfg = if is_dependency {
        Some(extract_resource_config_from_raw_project(
            &root_package.raw_project_yml,
            "models",
            default_adapter,
        )?)
    } else {
        None
    };

    let config_resolver = ProjectConfigResolver::build(
        root_project_configs.models.clone(),
        is_dependency,
        || {
            init_project_config(
                &package.dbt_project.models,
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
        Some(default_adapter),
    ));

    let render_ctx = RenderCtx {
        inner: Arc::new(RenderCtxInner {
            args: arg.clone(),
            root_project_name: root_package.dbt_project.name.clone(),
            config_resolver: config_resolver.clone(),
            package_quoting,
            uses_snapshot_fqn: false,
            defer_render_errors_to_compile: true,
            base_ctx: base_ctx.clone(),
            package_name: package_name.to_string(),
            adapter_type: default_adapter,
            database: database.to_string(),
            schema: schema.to_string(),
            resource_paths: package
                .dbt_project
                .model_paths
                .as_ref()
                .unwrap_or(&vec![])
                .clone(),
        }),
        jinja_env: env.clone(),
        runtime_config: runtime_config.clone(),
        root_runtime_config: root_runtime_config.clone(),
    };

    // HACK: strip semantic resources out of all model properties
    // this is because semantic resources have fields that have jinja expressions
    // but should not be rendered (they are hydrated verbatim in manifest.json)
    //
    // This is a hack because we treat models and models.metrics differently in an attempt
    // for only-once parsing of model yaml properties in resolver.rs, which duplicates the knowledge
    // that you must treat them separately, such as the removal of semantic properties here.
    let mut models_properties_sans_semantics: BTreeMap<String, MinimalPropertiesEntry> =
        BTreeMap::new();
    models_properties.iter().for_each(|(model_key, v)| {
        let mut v = v.clone();
        if let Some(m) = v.schema_value.as_mapping_mut() {
            // NOTE: do not remove derived_semantics not because it has jinja
            // but because we want to report any yaml errors that we didn't
            // show in resolve_inner's parsing of model yaml properties
            m.remove("metrics");
        }
        models_properties_sans_semantics.insert(model_key.clone(), v);
    });

    // Snapshot raw schema.yml config blocks before render_unresolved_sql_files nulls out
    // schema_value entries via std::mem::replace. Keyed by model name.
    let raw_schema_yml_configs: BTreeMap<String, BTreeMap<String, dbt_yaml::Value>> =
        models_properties_sans_semantics
            .iter()
            .filter_map(|(key, mpe)| {
                let config_map = extract_config_map(&mpe.schema_value)?;
                Some((key.clone(), config_map))
            })
            .collect();

    // Snapshot raw (unrendered) schema.yml test config blocks in the same window, before
    // render nulls schema_value. Consumed by `persist` to populate generic tests'
    // unrendered_config with their schema.yml config.
    let raw_test_configs: BTreeMap<String, TestUnrenderedConfigs> =
        models_properties_sans_semantics
            .iter()
            .map(|(key, mpe)| {
                (
                    key.clone(),
                    extract_test_unrendered_configs(&mpe.schema_value),
                )
            })
            .collect();

    // Split SQL and Python models for different processing paths
    let (sql_files, python_files): (Vec<_>, Vec<_>) =
        package.model_sql_files.iter().cloned().partition(|asset| {
            asset
                .path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("sql"))
                .unwrap_or(true)
        });

    // Process SQL models through Jinja rendering
    let mut model_sql_resources_map: Vec<SqlFileRenderResult<ModelConfig, ModelProperties>> =
        // FIXME -- this attempts to deserialize the model properties
        // and renders jinja but we shouldn't be doing so with metrics.filter
        render_unresolved_sql_files::<ModelConfig, ModelProperties>(
            &render_ctx,
            &sql_files,
            &mut models_properties_sans_semantics,
            token,
            jinja_type_checking_event_listener_factory.clone(),
        )
        .await?;

    // Process Python models through AST analysis (no Jinja rendering)
    let python_results = process_python_models(
        arg,
        &env,
        base_ctx,
        package_name,
        &package.dbt_project,
        config_resolver,
        python_files,
        &mut models_properties_sans_semantics,
        default_adapter,
    )?;
    model_sql_resources_map.extend(python_results);

    // make deterministic
    model_sql_resources_map.sort_by(|a, b| {
        a.asset
            .path
            .file_name()
            .cmp(&b.asset.path.file_name())
            .then(a.asset.path.cmp(&b.asset.path))
    });

    // Initialize a counter struct to track the version of each model
    let mut duplicates = Vec::new();
    let catalogs = load_catalogs::fetch_catalogs();
    let use_catalogs_v2 = load_catalogs::fetch_use_catalogs_v2();
    let catalogs_state = match catalogs.as_deref() {
        Some(c) if use_catalogs_v2 => LoadedCatalogs::V2(c),
        Some(c) => LoadedCatalogs::V1(c),
        None => LoadedCatalogs::None,
    };

    for SqlFileRenderResult {
        asset: dbt_asset,
        sql_file_info,
        config: model_config_resolved,
        raw_code,
        rendered_sql,
        macro_spans,
        properties: maybe_properties,
        status,
        render_error_deferred,
        patch_path,
        macro_dependencies,
    } in model_sql_resources_map.into_iter()
    {
        let ref_name = dbt_asset.path.file_stem().unwrap().to_str().unwrap();

        if ref_name.contains(' ') {
            return Err(err_resource_name_has_spaces(ref_name, &dbt_asset.path));
        }

        let mut model_config = model_config_resolved;

        // Capture inline SQL config overrides (from `{{ config(...) }}`) separately.
        // This should include only values explicitly set in the SQL file, not inherited defaults.
        let raw_config_call_dict = read_to_string(dbt_asset.base_path.join(&dbt_asset.path))
            .await
            .ok()
            .and_then(|sql| parse_unrendered_config(&sql, false));

        // A model is an ad-hoc inline model iff it lives in the dedicated "" package.
        let is_inline_file = package_name.is_empty();
        if is_inline_file {
            model_config.materialized = DbtMaterialization::Inline;
        }

        let mut model_name = models_properties_sans_semantics
            .get(ref_name)
            .map(|mpe| mpe.name.clone())
            .unwrap_or_else(|| ref_name.to_owned());

        if is_inline_file {
            // Inline nodes should present a stable name for logging and manifest output
            model_name = "inline".to_owned();
        }

        let maybe_version = models_properties_sans_semantics
            .get(ref_name)
            .and_then(|mpe| mpe.version_info.as_ref().map(|v| v.version.clone()));

        let maybe_latest_version = models_properties_sans_semantics
            .get(ref_name)
            .and_then(|mpe| mpe.version_info.as_ref().map(|v| v.latest_version.clone()));

        let name_span = models_properties_sans_semantics
            .get(ref_name)
            .and_then(|mpe| {
                mpe.name_span.is_valid().then(|| {
                    dbt_common::Span::from_serde_span(
                        mpe.name_span.clone(),
                        mpe.relative_path.clone(),
                    )
                })
            })
            .unwrap_or_default();

        let unique_id = get_unique_id(&model_name, package_name, maybe_version.clone(), "model");

        if let Some(state) = &model_config.state {
            ModelFreshnessRules::validate(state.lag_tolerance.as_ref()).map_err(|e| {
                fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => dbt_asset.path.clone(),
                    "{}",
                    e
                )
            })?;
        }

        // Keep track of duplicates (often happens with versioned models)
        if (models.contains_key(&unique_id) || models_with_execute.contains_key(&unique_id))
            && !(status == ModelStatus::Disabled)
        {
            duplicates.push((
                unique_id.clone(),
                model_name.clone(),
                maybe_version.clone(),
                dbt_asset.path.clone(),
            ));
            continue;
        }

        let original_file_path =
            get_original_file_path(&dbt_asset.base_path, &arg.io.in_dir, &dbt_asset.path);

        // Model fqn includes v{version} for versioned models
        let fqn_components = if let Some(version) = &maybe_version {
            vec![model_name.to_owned(), format!("v{}", version)]
        } else {
            vec![model_name.to_owned()]
        };
        let fqn = get_node_fqn(
            package_name,
            dbt_asset.path.to_owned(),
            fqn_components,
            package.dbt_project.model_paths.as_ref().unwrap_or(&vec![]),
        );

        let properties = if let Some(properties) = maybe_properties {
            properties
        } else {
            ModelProperties::empty(model_name.to_owned())
        };

        // Validate model properties (versions, time spine, etc.)
        match validate_model(&properties) {
            Ok(errors) => {
                if !errors.is_empty() {
                    // Show each error individually
                    for error in errors {
                        emit_error_log_from_fs_error(error);
                    }
                    continue;
                }
            }
            Err(e) => {
                emit_error_log_from_fs_error(*e);

                continue;
            }
        }

        let resolved_versioned = resolve_versioned_fields(maybe_version.as_ref(), &properties);
        let model_constraints = resolved_versioned.constraints;
        let model_description = resolved_versioned.description;

        if let Some(raw) = resolved_versioned.invalid_access.as_deref() {
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => dbt_asset.path.clone(),
                "Invalid access type '{}' — must be one of: private, protected, public",
                raw,
            );
            emit_error_log_from_fs_error(*err);
        }

        if resolved_versioned.sibling_access.is_some() {
            emit_warn_log_message(
                ErrorCode::InvalidConfig,
                format!(
                    "Model '{model_name}': the `access` field on a `versions` entry has no effect and is ignored (matching dbt-core). Set `versions[].config.access` instead."
                ),
            );
        }

        // Iterate over metrics and construct the dependencies
        let mut metrics = Vec::new();
        for (metric, package) in sql_file_info.metrics.iter() {
            if let Some(package_str) = package {
                metrics.push(vec![package_str.to_owned(), metric.to_owned()]);
            } else {
                metrics.push(vec![metric.to_owned()]);
            }
        }

        let mut columns = process_columns(
            properties.columns.as_ref(),
            model_config.tags.inner().clone().map(|tags| tags.into()),
        )?;
        let materialized = model_config.materialized.clone();

        if let Some(versions) = &properties.versions {
            let model_config_inner: ModelConfig = model_config.clone().into();
            columns = process_versioned_columns(
                &model_config_inner,
                maybe_version.as_ref(),
                versions,
                columns,
            )?;
        }

        if model_config
            .contract
            .as_ref()
            .is_some_and(|contract| contract.enforced)
            && !materialization_enforces_constraints(&materialized)
            && has_warn_unsupported_constraints(&model_constraints, &columns)
        {
            emit_warn_log_message(
                ErrorCode::UnsupportedConstraintMaterialization,
                format!(
                    "Constraint types are not supported for {materialized} materializations and will be ignored.  Set 'warn_unsupported: false' on this constraint to ignore this warning."
                ),
            );
        }

        if matches!(materialized, DbtMaterialization::Incremental)
            && model_config
                .contract
                .as_ref()
                .is_some_and(|contract| contract.enforced)
            && !matches!(
                model_config.on_schema_change,
                Some(OnSchemaChange::AppendNewColumns) | Some(OnSchemaChange::Fail)
            )
        {
            let osc_str = match model_config.on_schema_change.as_ref() {
                None | Some(OnSchemaChange::Ignore) => "ignore",
                Some(OnSchemaChange::SyncAllColumns) => "sync_all_columns",
                _ => "unknown",
            };
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => dbt_asset.path.clone(),
                "Invalid value for on_schema_change: {}. Models materialized as incremental with contracts enabled must set on_schema_change to 'append_new_columns' or 'fail'",
                osc_str,
            );
            emit_error_log_from_fs_error(*err);
            continue;
        }

        let deprecation_date = resolved_versioned.deprecation_date;

        validate_merge_update_columns_xor(&model_config, &dbt_asset.path)?;
        validate_compute(model_config.compute, &dbt_asset.path)?;
        // `--adapter` overrides the authored `+adapter`, as a flag should. No
        // precondition is checked here: parse resolves every node in the project
        // while only selected nodes run, so one checked here would reject nodes the
        // invocation never touches. See `resolve_compute_write_target`.
        validate_node_adapter(model_config.adapter, &dbt_asset.path)?;
        let resolved_node_adapter = arg.adapter_override.or(model_config.adapter);
        validate_interactive_table_config(
            &model_config,
            status,
            resolved_node_adapter.unwrap_or(default_adapter),
            &dbt_asset.path,
        )?;

        // catalog check: a catalog_name with no match would otherwise silently materialize to the default catalog
        if model_config.enabled
            && let Some(catalog_name) = model_config.catalog_name.as_deref()
            && !catalogs_state.has_catalog_name(catalog_name)?
        {
            let err = match catalogs_state.catalog_names()? {
                None => fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => dbt_asset.path.clone(),
                    "Model specifies catalog_name '{catalog_name}', but no catalogs.yml was found in the project. \
                     Add a catalog named '{catalog_name}' to catalogs.yml, or remove the catalog_name config."
                ),
                Some(names) => fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => dbt_asset.path.clone(),
                    "Model specifies catalog_name '{catalog_name}', which is not defined in catalogs.yml. \
                     Defined catalogs: [{}].", names.join(", ")
                ),
            };
            emit_error_log_from_fs_error(*err);
            continue;
        }

        apply_model_freshness_loaded_at_override(
            model_config.freshness.as_mut(),
            &mut model_config.loaded_at_field,
            &mut model_config.loaded_at_query,
            &model_name,
        )?;

        // A model that sets `database` to a catalog's database (DuckDB ATTACH
        // alias / Snowflake linked `catalog_database`) but no `catalog_name`
        // would otherwise silently materialize as a plain database write and be
        // undercounted in catalog telemetry. Fail fast and make the intent
        // explicit.
        if model_config.enabled && use_catalogs_v2 && model_config.catalog_name.is_none() {
            let model_database = model_config.database.clone().into_inner().unwrap_or(None);
            if let Some(catalogs) = catalogs.as_deref()
                && let Some(database) = model_database
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
            {
                validate_database_not_catalog(
                    &model_name,
                    database,
                    catalogs,
                    resolved_node_adapter.unwrap_or(default_adapter),
                    &dbt_asset.path,
                )?;
            }
        }

        if let Some(freshness) = &model_config.freshness {
            ModelFreshnessRules::validate(freshness.build_after.as_ref()).map_err(|e| {
                fs_err!(
                    code => ErrorCode::InvalidConfig,
                    loc => dbt_asset.path.clone(),
                    "{}",
                    e
                )
            })?;
            // Warn, don't abort, on a partial SLA rule — mirrors `resolve_sources`.
            if let Err(err) = FreshnessRules::validate(freshness.error_after.as_ref()) {
                emit_warn_log_from_fs_error(*err);
            }
            if let Err(err) = FreshnessRules::validate(freshness.warn_after.as_ref()) {
                emit_warn_log_from_fs_error(*err);
            }
        }
        validate_model_freshness_sla(
            model_config.freshness.as_ref(),
            &materialized,
            &dbt_asset.path,
        )?;

        // A model using a custom materialization honors its configured
        // `static_analysis` like any other model. Fusion used to force `off`
        // here, on the theory that a user materialization may persist a schema
        // Fusion cannot see. That silently opted whole projects out of static
        // analysis — even `baseline`, which never needs the analyzed upstream
        // schema. Schema-modifying custom materializations are now the user's
        // to account for (set `static_analysis: off` on those models), the same
        // way introspective queries are handled.
        let static_analysis = model_config.static_analysis.clone();
        check_node_static_analysis(
            &model_config,
            arg.static_analysis,
            unique_id.as_str(),
            dependency_package_name,
        );

        // Hydrate time_spine from model properties
        let mut time_spine: Option<TimeSpine> = None;
        if let Some(props_time_spine) = properties.time_spine.clone() {
            let standard_granularity_column_dimension = properties.columns.clone().unwrap_or_default()
                .into_iter()
                .find(|d| {
                    d.name == props_time_spine.standard_granularity_column.clone()
                }).expect(&format!("Cannot find standard granularity column '{}'. There should have been a validation error.", props_time_spine.standard_granularity_column));

            let primary_column = TimeSpinePrimaryColumn {
                name: props_time_spine.standard_granularity_column.clone(),
                time_granularity: standard_granularity_column_dimension
                    .granularity
                    .unwrap_or_default(),
            };

            // Create a temporary node_relation for the time_spine
            let node_relation = NodeRelation {
                database: Some(database.to_string()),
                schema_name: schema.to_string(),
                alias: model_name.to_string(), // will be updated after relation components are resolved
                relation_name: None,
            };

            time_spine = Some(TimeSpine {
                node_relation,
                primary_column,
                custom_granularities: props_time_spine.custom_granularities.unwrap_or_default(),
            });
        }

        jinja_type_checking_event_listener_factory
            .update_unique_id(&format!("{package_name}.{model_name}"), &unique_id);

        // TODO: In Core, each merge completely overwrites existing keys. We are matching this behavior, but this seems like a bug in Core.
        //
        // For a versioned model the schema.yml snapshot holds the model-level and version-level
        // `config:` blocks deep-merged, both unrendered. dbt-core keeps only the version level
        // unrendered (`core/dbt/contracts/files.py:331-338`) and the model level rendered; we
        // deliberately diverge — see `.agents/state-modified-conformance.md` § "Versioned models".
        //
        // TODO (deferred, not a bug here): a real dbt-core manifest also DOUBLE-APPLIES a
        // version-level list value in this same block — `deep_merge(patch_config_dict,
        // unrendered_version_config)` (dbt-mantle `core/dbt/parser/base.py:421-426`) prepends the
        // version's unrendered hooks/tags onto a list that already contains them. We deliberately
        // do not replicate that duplication. See the `#[ignore]`d marker test
        // `test_versioned_config_hook_duplication_not_replicated_state_modified`
        // (`crates/dbt-cli/tests/dbt_conformance/list/defer_state/test_modified_state.rs`), which
        // pins that Stage 1/Stage 2 already handle a Mantle-produced manifest with the duplicate
        // correctly (suppress-only phantom diff, cleared by Stage 2).
        let unrendered_config = build_unrendered_config(
            &fqn,
            &raw_local_project_config,
            raw_root_project_models_cfg.as_ref(),
            raw_schema_yml_configs.get(ref_name),
            raw_config_call_dict.as_ref(),
            true,
            default_adapter,
        )?;

        // Quoting is resolved here rather than at the package seed, because both
        // remaining layers depend on which adapter the node runs on, and that is
        // only known after the config merge. At this point `model_config.quoting`
        // holds just the `models:`-subtree and model-level `+quoting:` values; the
        // adapter's own config and the adapter type's default go underneath.
        //
        // Written back into the config rather than used locally, so everything
        // downstream sees the same fully-resolved value -- notably
        // `deprecated_config`, which reaches the manifest and the run-cache hash.
        // Leaving unset fields as `None` there would change both.
        // `propagate` comes straight off the node's own config. Unlike
        // `adapter` there is no target default to fall back to and nothing to
        // inherit: an unset `+propagate` means "publish nowhere".
        let selected_propagate: Vec<AdapterType> = model_config
            .propagate
            .clone()
            .map(Into::into)
            .unwrap_or_default();
        let selected_adapter = resolved_node_adapter.unwrap_or(default_adapter);
        model_config.quoting = resolve_package_quoting(
            Some(match adapter_quoting.get(&selected_adapter) {
                Some(authored) => model_config.quoting.filled_from(authored),
                None => model_config.quoting,
            }),
            resolved_node_adapter.unwrap_or(default_adapter),
        );

        // Create the DbtModel with all properties already set
        let mut dbt_model = DbtModel {
            __common_attr__: CommonAttributes {
                name: model_name.to_owned(),
                package_name: package_name.to_owned(),
                path: DbtPath::from(dbt_asset.path.to_owned()),
                name_span,
                original_file_path,
                patch_path: patch_path.as_ref().map(DbtPath::from),
                unique_id: unique_id.clone(),
                fqn,
                // dbt-core: description is always default ''
                description: Some(model_description),
                checksum: sql_file_info.checksum.clone(),
                raw_code: Some(raw_code),
                language: if dbt_asset.is_python() {
                    Some("python".to_string())
                } else {
                    Some("sql".to_string())
                },
                tags: model_config
                    .tags
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                classifiers: model_config
                    .classifiers
                    .inner()
                    .clone()
                    .map(Into::into)
                    .unwrap_or_default(),
                meta: model_config.meta.clone().unwrap_or_default(),
            },
            __base_attr__: NodeBaseAttributes {
                adapter: selected_adapter,
                propagate: selected_propagate,
                database: database.to_string(), // will be updated below
                schema: schema.to_string(),     // will be updated below
                alias: "".to_owned(),           // will be updated below
                relation_name: None,            // will be updated below
                enabled: model_config.enabled,
                compute: model_config.compute,
                extended_model: false,
                persist_docs: model_config.persist_docs.clone(),
                columns,
                depends_on: NodeDependsOn {
                    macros: macro_dependencies,
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
                    .chain(
                        model_constraints
                            .iter()
                            .filter_map(|c| c.to.as_ref())
                            .filter_map(|spanned| {
                                parse_ref_from_constraint(spanned).map(|(pkg, name, version)| {
                                    DbtRef {
                                        name,
                                        package: pkg,
                                        version,
                                        location: Some(CodeLocationWithFile::from(
                                            spanned.span().clone(),
                                        )),
                                    }
                                })
                            }),
                    )
                    .chain(
                        properties
                            .columns
                            .iter()
                            .flatten()
                            .flat_map(|col| col.constraints.iter().flatten())
                            .filter_map(|c| c.to.as_ref())
                            .filter_map(|spanned| {
                                parse_ref_from_constraint(spanned).map(|(pkg, name, version)| {
                                    DbtRef {
                                        name,
                                        package: pkg,
                                        version,
                                        location: Some(CodeLocationWithFile::from(
                                            spanned.span().clone(),
                                        )),
                                    }
                                })
                            }),
                    )
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
                    .chain(
                        model_constraints
                            .iter()
                            .filter_map(|c| c.to.as_ref())
                            .filter_map(|spanned| {
                                parse_source_from_constraint(spanned).map(|(src, tbl)| {
                                    DbtSourceWrapper {
                                        source: vec![src, tbl],
                                        location: Some(CodeLocationWithFile::from(
                                            spanned.span().clone(),
                                        )),
                                    }
                                })
                            }),
                    )
                    .chain(
                        properties
                            .columns
                            .iter()
                            .flatten()
                            .flat_map(|col| col.constraints.iter().flatten())
                            .filter_map(|c| c.to.as_ref())
                            .filter_map(|spanned| {
                                parse_source_from_constraint(spanned).map(|(src, tbl)| {
                                    DbtSourceWrapper {
                                        source: vec![src, tbl],
                                        location: Some(CodeLocationWithFile::from(
                                            spanned.span().clone(),
                                        )),
                                    }
                                })
                            }),
                    )
                    .collect(),
                metrics,
                materialized,
                quoting: model_config
                    .quoting
                    .try_into()
                    .expect("DbtQuoting -> QuotingConfig conversion"),
                quoting_ignore_case: model_config.quoting.snowflake_ignore_case.unwrap_or(false),
                static_analysis_off_reason: (*static_analysis == StaticAnalysisKind::Off)
                    .then_some(StaticAnalysisOffReason::ConfiguredOff),
                static_analysis,
                unrendered_config,
            },
            __model_attr__: DbtModelAttr {
                introspection: if sql_file_info.this {
                    IntrospectionKind::This
                } else {
                    IntrospectionKind::None
                },
                version: maybe_version.map(|v| v.into()),
                latest_version: maybe_latest_version.map(|v| v.into()),
                constraints: model_constraints,
                deprecation_date,
                primary_key: vec![], // applied in resolver.rs -> primary_key_inference.rs
                time_spine,
                // versions[].access has no effect on the resolved node — see ResolvedVersionedFields'
                // doc comment (GT2: dbt-mantle validates it, then unconditionally discards it).
                access: model_config.access.clone().unwrap_or_default(),
                group: model_config.group.clone(),
                contract: model_config.contract.clone(),
                incremental_strategy: model_config.incremental_strategy.clone(),
                freshness: model_config.freshness.clone(),
                state: model_config.state.clone(),
                event_time: model_config.event_time.clone(),
                catalog_name: model_config.catalog_name.clone(),
                table_format: model_config.table_format.clone(),
                sync: model_config.sync.clone(),
                compiled_code: None,
            },
            __adapter_attr__: AdapterAttr::from_config_and_dialect(
                &model_config.__warehouse_specific_config__,
                default_adapter,
            ),
            // Derived from the model config
            deprecated_config: model_config.clone().into(),
            __other__: BTreeMap::new(),
        };

        let components = RelationComponents {
            database: model_config.database.clone().into_inner().unwrap_or(None),
            schema: model_config.schema.clone().into_inner().unwrap_or(None),
            alias: model_config.alias.clone(),
            store_failures: None,
        };

        // update model components using the generate_relation_components function
        update_node_relation_components(
            &mut dbt_model,
            &env,
            &root_package.dbt_project.name,
            package_name,
            base_ctx,
            &components,
            default_adapter,
        )?;

        // Update time_spine node_relation with the resolved relation components
        if dbt_model.__model_attr__.time_spine.is_some() {
            let database = dbt_model.database();
            let schema = dbt_model.schema();
            let alias = dbt_model.alias();
            let relation_name = dbt_model.__base_attr__.relation_name.clone();

            if let Some(ref mut ts) = dbt_model.__model_attr__.time_spine {
                ts.node_relation = NodeRelation {
                    database: Some(database),
                    schema_name: schema,
                    alias,
                    relation_name,
                };
            }
        }
        match node_resolver.insert_ref(&dbt_model, default_adapter, status, false) {
            Ok(_) => (),
            Err(e) => {
                let err_with_loc = e.with_location(dbt_asset.path.clone());
                emit_error_log_from_fs_error(err_with_loc);
            }
        }

        match status {
            ModelStatus::Enabled => {
                // merge them later for the returned models
                if sql_file_info.execute {
                    models_with_execute.insert(unique_id.to_owned(), dbt_model);
                } else {
                    models.insert(unique_id.to_owned(), Arc::new(dbt_model));
                }
                node_names.insert(model_name.to_owned());
                rendering_results.insert(unique_id, (rendered_sql.clone(), macro_spans.clone()));

                if !arg.skip_creating_generic_tests {
                    properties.as_testable().persist(
                        package_name,
                        &root_package.dbt_project.name,
                        collected_generic_tests,
                        test_name_truncations,
                        seen_generic_test_paths,
                        default_adapter,
                        &arg.io,
                        patch_path.as_ref().unwrap_or(&dbt_asset.path),
                        false,
                        &raw_test_configs.get(ref_name).cloned().unwrap_or_default(),
                    )?;
                }
            }
            ModelStatus::Disabled => {
                disabled_models.insert(unique_id.to_owned(), Arc::new(dbt_model));
                if !arg.skip_creating_generic_tests {
                    properties.as_testable().persist(
                        package_name,
                        &root_package.dbt_project.name,
                        collected_generic_tests,
                        test_name_truncations,
                        seen_generic_test_paths,
                        default_adapter,
                        &arg.io,
                        patch_path.as_ref().unwrap_or(&dbt_asset.path),
                        true,
                        &raw_test_configs.get(ref_name).cloned().unwrap_or_default(),
                    )?;
                }
            }
            ModelStatus::ParsingFailed => {
                if render_error_deferred {
                    models.insert(unique_id.to_owned(), Arc::new(dbt_model));
                    node_names.insert(model_name.to_owned());

                    if !arg.skip_creating_generic_tests {
                        properties.as_testable().persist(
                            package_name,
                            &root_package.dbt_project.name,
                            collected_generic_tests,
                            test_name_truncations,
                            seen_generic_test_paths,
                            default_adapter,
                            &arg.io,
                            patch_path.as_ref().unwrap_or(&dbt_asset.path),
                            false,
                            &raw_test_configs.get(ref_name).cloned().unwrap_or_default(),
                        )?;
                    }
                }
            }
        }
    }

    // In incremental and lazy-load runs, model_sql_files only contains changed/new
    // files — unchanged models are skipped by the build cache. Build the full set of
    // known model names from all_paths (always a complete filesystem scan, pre-filter)
    // so we don't fire spurious NoNodeForYamlKey warnings for models that exist on
    // disk but weren't re-parsed this run.
    let all_known_model_names: HashSet<&str> = package
        .all_paths
        .get(&ResourcePathKind::ModelPaths)
        .map(|paths| {
            paths
                .iter()
                .filter_map(|(p, _)| p.as_path().file_stem()?.to_str())
                .collect()
        })
        .unwrap_or_default();

    for (model_name, mpe) in models_properties_sans_semantics.iter() {
        // Skip until we support better error messages for versioned models
        if mpe.version_info.is_some() {
            continue;
        }
        if !mpe.schema_value.is_null() && !all_known_model_names.contains(model_name.as_str()) {
            let err = fs_err!(
                code =>ErrorCode::NoNodeForYamlKey,
                loc => mpe.relative_path.clone(),
                "Unused schema.yml entry for model '{}'",
                model_name,
            );
            emit_warn_log_from_fs_error(*err);
        }
    }

    // Report duplicates
    if !duplicates.is_empty() {
        let mut errs = Vec::new();
        for (_, model_name, maybe_version, path) in duplicates {
            let msg = if let Some(version) = maybe_version {
                format!("Found duplicate model '{model_name}' with version '{version}'")
            } else {
                format!("Found duplicate model '{model_name}'")
            };
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.clone(),
                "{}",
                msg,
            );
            errs.push(err);
        }
        while let Some(err) = errs.pop() {
            if errs.is_empty() {
                return Err(err);
            }
            emit_error_log_from_fs_error(*err);
        }
    }

    // Second pass to capture all identifiers with the appropriate context
    // `models_with_execute` should never have overlapping Arc pointers with `models` and `disabled_models`
    // otherwise make_mut will clone the inner model, and the modifications inside this function call will be lost
    let models_rest = collect_adapter_identifiers_detect_unsafe(
        arg,
        models_with_execute,
        node_resolver,
        env,
        default_adapter,
        package_name,
        &root_package.dbt_project.name,
        runtime_config,
        token,
    )
    .await?;

    models.extend(
        models_rest
            .into_iter()
            .map(|(v, _)| (v.__common_attr__.unique_id.to_string(), Arc::new(v))),
    );
    Ok((models, rendering_results, disabled_models))
}

/// Per-version overrides for a versioned model, resolved against the top-level
/// `ModelProperties` using dbt-core's `ParsedNodePatch` semantics
/// (see `core/dbt/parser/schemas.py`, `versioned_model_patch` construction).
///
/// Override-or-fallback fields only. Fields with other resolution rules stay
/// outside this struct:
///   - `columns` -> `process_versioned_columns` (include/exclude merge)
///   - `config`  -> `VersionInfo.version_config` (deep merge)
///   - `meta`    -> top-level only, no per-version semantics
///   - `data_tests` -> not yet wired (flow through other pipelines)
///
/// `access` is deliberately absent from this struct: dbt-core parses and validates
/// `unparsed_version.access`, then unconditionally discards it — `patch_node_config` always
/// overwrites `node.access` from the config dict, which is never empty because
/// `ModelConfig.access` defaults to `protected` (dbt-mantle `schemas.py:1094`, `base.py:380-383`,
/// `model.py:88-90`). Only the validation survives; see `invalid_access` below.
struct ResolvedVersionedFields {
    description: String,
    constraints: Vec<ModelConstraint>,
    /// Per-version only; no fallback to top-level (dbt-core parity).
    deprecation_date: Option<String>,
    /// Non-empty, non-parseable access string supplied at the version level. `Versions::access`
    /// (`common.rs`) is typed `Option<String>` rather than `Option<Access>` for the same reason:
    /// serde would reject `access: ""` as an unknown variant before we can apply dbt-core's
    /// empty-string-is-None fallthrough semantics.
    invalid_access: Option<String>,
    /// Non-empty version-level `access:` string, valid or not. dbt-core discards this value (see
    /// struct doc above), so its only use here is driving a Fusion-only warning pointing authors
    /// at `versions[].config.access`, the form dbt-core actually honors.
    sibling_access: Option<String>,
}

fn resolve_versioned_fields(
    maybe_version: Option<&String>,
    properties: &ModelProperties,
) -> ResolvedVersionedFields {
    let version_match =
        maybe_version
            .zip(properties.versions.as_deref())
            .and_then(|(mv, versions)| {
                versions
                    .iter()
                    .find(|v| v.get_version().as_ref() == Some(mv))
            });

    // dbt-core: `unparsed_version.description or target.description`.
    // Python `or` treats `""` as falsy, so an empty per-version description
    // falls through to the top-level value.
    let description = version_match
        .and_then(|v| v.description.clone().filter(|s| !s.is_empty()))
        .or_else(|| properties.description.clone())
        .unwrap_or_default();

    // dbt-core: `unparsed_version.constraints or target.constraints`. Empty
    // list is falsy in Python -> fall through.
    let constraints = version_match
        .and_then(|v| v.constraints.clone().filter(|c| !c.is_empty()))
        .or_else(|| properties.constraints.clone())
        .unwrap_or_default();

    // dbt-core: top-level `deprecation_date` applies to the unversioned model
    // itself; for versioned children only the per-version value applies (no
    // inheritance from the top-level).
    let deprecation_date = if maybe_version.is_some() {
        version_match.and_then(|v| v.deprecation_date.clone())
    } else {
        properties.deprecation_date.clone()
    }
    .map(|raw| dbt_schemas::schemas::common::normalize_deprecation_date(&raw));

    // dbt-core validates `unparsed_version.access` (raising `InvalidAccessTypeError` on a bad
    // value) and then discards it unconditionally (GT2 — see the struct doc above). Fusion parses
    // it here only to reproduce that validation; the valid value, if any, is never applied.
    let raw_access = version_match.and_then(|v| v.access.clone().filter(|s| !s.is_empty()));
    let invalid_access = raw_access
        .as_deref()
        .and_then(|raw| raw.parse::<Access>().err().map(|()| raw.to_string()));

    ResolvedVersionedFields {
        description,
        constraints,
        deprecation_date,
        invalid_access,
        sibling_access: raw_access,
    }
}

fn process_versioned_columns(
    model_config: &ModelConfig,
    maybe_version: Option<&String>,
    versions: &[Versions],
    columns: Vec<DbtColumnRef>,
) -> Result<Vec<DbtColumnRef>, Box<dbt_common::FsError>> {
    for version in versions.iter() {
        if maybe_version.is_some_and(|v| Some(v) == version.get_version().as_ref())
            // Typed and already Jinja-rendered -- see the doc comment on `Versions::columns`.
            && let Some(version_columns) = version.columns.as_deref()
        {
            // dbt-core: a version with no include/exclude directive defaults to
            // `IncludeExclude(include="*")`, i.e. inherit every model-level column.
            let rules = ColumnInheritanceRules::from_version_column_props(version_columns)
                .unwrap_or_default();

            let version_column_props: Vec<ColumnProperties> = version_columns
                .iter()
                .filter_map(VersionColumnProperties::to_column_properties)
                .collect();
            let version_columns = process_columns(
                Some(&version_column_props),
                model_config.tags.inner().clone().map(|tags| tags.into()),
            )?;

            // dbt-core `UnparsedModelUpdate.get_columns_for_version`: filtered model-level columns
            // first, then the version-local ones. `ParserRef.from_versioned_target` then folds that
            // list into a name-keyed dict, so a version column shadowing a model-level column takes
            // the model-level column's *position* and the version's *value* -- which is exactly what
            // `IndexMap::insert` on an existing key does.
            let mut merged: IndexMap<String, DbtColumnRef> = columns
                .into_iter()
                .filter(|col| rules.should_include_column(&col.name))
                .map(|col| (col.name.clone(), col))
                .collect();
            for col in version_columns {
                merged.insert(col.name.clone(), col);
            }
            return Ok(merged.into_values().collect());
        }
    }

    Ok(columns)
}

fn materialization_enforces_constraints(materialized: &DbtMaterialization) -> bool {
    matches!(
        materialized,
        DbtMaterialization::Table | DbtMaterialization::Incremental
    )
}

fn has_warn_unsupported_constraints(
    model_constraints: &[ModelConstraint],
    columns: &[DbtColumnRef],
) -> bool {
    model_constraints
        .iter()
        .any(|constraint| constraint.warn_unsupported != Some(false))
        || columns.iter().any(|column| {
            column
                .constraints
                .iter()
                .any(|constraint| constraint.warn_unsupported != Some(false))
        })
}

fn validate_database_not_catalog(
    model_name: &str,
    database: &str,
    catalogs: &DbtCatalogs,
    adapter_type: AdapterType,
    path: &Path,
) -> FsResult<()> {
    debug_assert!(!database.trim().is_empty());
    let matches = catalogs.catalogs_matching_database(database, adapter_type)?;
    match matches.as_slice() {
        [] => Ok(()),
        [catalog] => Err(fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "{model_name}'s database config, \"{database}\", is associated with catalog \"{catalog}\"; \
             set `catalog_name: {catalog}` explicitly, or change the database."
        )),
        names => {
            let joined = names.join("\", \"");
            Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.to_path_buf(),
                "{model_name}'s database config, \"{database}\", is associated with multiple catalogs (\"{joined}\"); \
                 set `catalog_name` explicitly to one of them, or change the database."
            ))
        }
    }
}

pub fn validate_merge_update_columns_xor(
    model_config: &ResolvedModelConfig,
    path: &Path,
) -> FsResult<()> {
    if model_config.merge_update_columns.is_some() && model_config.merge_exclude_columns.is_some() {
        let err = fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => path.to_path_buf(),
            "merge_update_columns and merge_exclude_columns cannot both be set",
        );
        return Err(err);
    }
    Ok(())
}

fn if_interactive_table(
    model_config: &ResolvedModelConfig,
    check: impl FnOnce() -> FsResult<()>,
) -> FsResult<()> {
    if matches!(
        model_config.materialized,
        DbtMaterialization::InteractiveTable
    ) {
        check()
    } else {
        Ok(())
    }
}

/// `CREATE INTERACTIVE TABLE` without `CLUSTER BY` is rejected by Snowflake (010405). A
/// `cluster_by` naming no usable column renders `cluster by ()` or `cluster by (id, )` — a syntax
/// error (001003) instead.
fn validate_interactive_table_cluster_by(
    model_config: &ResolvedModelConfig,
    path: &Path,
) -> FsResult<()> {
    if_interactive_table(model_config, || {
        let names_a_column = model_config
            .__warehouse_specific_config__
            .cluster_by
            .as_ref()
            .is_some_and(|cluster_by| {
                let fields = cluster_by.fields();
                !fields.is_empty() && fields.iter().all(|field| !field.trim().is_empty())
            });
        if !names_a_column {
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.to_path_buf(),
                "interactive_table models require `cluster_by` to name at least one non-blank column; `CREATE INTERACTIVE TABLE` without `CLUSTER BY`, or with only blank entries, is rejected by Snowflake (010405)",
            );
            return Err(err);
        }
        Ok(())
    })
}

/// Interactive tables have no iceberg variant.
fn validate_interactive_table_table_format(
    model_config: &ResolvedModelConfig,
    path: &Path,
) -> FsResult<()> {
    if_interactive_table(model_config, || {
        if let Some(table_format) = model_config.table_format.as_deref()
            && table_format.eq_ignore_ascii_case("iceberg")
        {
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.to_path_buf(),
                "table_format '{}' is not supported for interactive_table models",
                table_format,
            );
            return Err(err);
        }
        Ok(())
    })
}

fn validate_interactive_table_transient(
    model_config: &ResolvedModelConfig,
    path: &Path,
) -> FsResult<()> {
    if_interactive_table(model_config, || {
        if model_config.__warehouse_specific_config__.transient == Some(true) {
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.to_path_buf(),
                "transient=true is not supported for interactive_table models; `TRANSIENT INTERACTIVE TABLE` is a Snowflake syntax error (001003). Set `transient: false` on this model to override an inherited value",
            );
            return Err(err);
        }
        Ok(())
    })
}

fn validate_interactive_table_target_lag_warehouse(
    model_config: &ResolvedModelConfig,
    path: &Path,
) -> FsResult<()> {
    if_interactive_table(model_config, || {
        let warehouse_config = &model_config.__warehouse_specific_config__;
        // An env var resolving to "" (or whitespace) must not count as a configured warehouse —
        // it would otherwise pass this check and later render an invalid `warehouse =` clause.
        let refresh_warehouse_blank = warehouse_config
            .refresh_warehouse
            .as_deref()
            .is_none_or(|s| s.trim().is_empty());
        let snowflake_warehouse_blank = warehouse_config
            .snowflake_warehouse
            .as_deref()
            .is_none_or(|s| s.trim().is_empty());
        if warehouse_config.target_lag.is_some()
            && refresh_warehouse_blank
            && snowflake_warehouse_blank
        {
            let err = fs_err!(
                code => ErrorCode::InvalidConfig,
                loc => path.to_path_buf(),
                "target_lag requires refresh_warehouse or snowflake_warehouse to be set for interactive_table models (010412)",
            );
            return Err(err);
        }
        Ok(())
    })
}

/// Only for models that will actually be built: `interactive_table` is Snowflake-only, so
/// elsewhere the execution-time "materialization macro not found" error is the correct diagnostic.
pub fn validate_interactive_table_config(
    model_config: &ResolvedModelConfig,
    status: ModelStatus,
    adapter_type: AdapterType,
    path: &Path,
) -> FsResult<()> {
    if status != ModelStatus::Enabled || !matches!(adapter_type, AdapterType::Snowflake) {
        return Ok(());
    }
    validate_interactive_table_cluster_by(model_config, path)?;
    validate_interactive_table_table_format(model_config, path)?;
    validate_interactive_table_transient(model_config, path)?;
    validate_interactive_table_target_lag_warehouse(model_config, path)?;
    Ok(())
}

/// Process Python model files through AST analysis
///
/// Unlike SQL models which go through Jinja rendering, Python models are:
/// 1. Parsed with a Python AST parser
/// 2. Validated for correct structure (model function signature)
/// 3. Analyzed to extract dbt.ref(), dbt.source(), dbt.config() calls
/// 4. Merged with project/properties configs
///
/// Returns SqlFileRenderResult for uniform downstream processing with SQL models
#[allow(clippy::too_many_arguments)]
fn process_python_models(
    arg: &ResolveArgs,
    env: &Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    package_name: &str,
    dbt_project: &DbtProject,
    config_resolver: ProjectConfigResolver<ModelConfig>,
    python_files: Vec<dbt_schemas::state::DbtAsset>,
    models_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    adapter_type: AdapterType,
) -> FsResult<Vec<SqlFileRenderResult<ModelConfig, ModelProperties>>> {
    let mut results = Vec::new();
    let dependency_package_name = dependency_package_name_from_ctx(env.as_ref(), base_ctx);

    for python_asset in python_files {
        // Read and parse Python source
        let absolute_path = python_asset.base_path.join(&python_asset.path);
        // Strip leading/trailing whitespace to match dbt-core's behavior (load_file_contents with strip=True)
        let source = std::fs::read_to_string(&absolute_path)?.trim().to_string();

        let stmts = match parse_python(&source, &python_asset.path) {
            Ok(stmts) => stmts,
            Err(e) => {
                emit_error_log_from_fs_error(*e);
                continue;
            }
        };

        // Validate Python model structure (def model(dbt, session): ...)
        if let Err(e) = validate_python_model(&python_asset.path, &stmts) {
            emit_error_log_from_fs_error(*e);
            continue;
        }

        // Analyze Python AST to extract dbt function calls
        // Use the Python model source to compute the model checksum. This is used by `state:*`
        // selectors (e.g. `state:modified`) when comparing to a deferred/previous-state manifest.
        let checksum =
            dbt_schemas::schemas::common::DbtChecksum::hash(normalize_sql(&source).as_bytes());
        let python_file_info: PythonFileInfo<ModelConfig> = match analyze_python_file(
            &python_asset.path,
            &source,
            &stmts,
            checksum,
            dependency_package_name,
            Some(python_asset.path.clone()),
            adapter_type,
        ) {
            Ok(info) => info,
            Err(e) => {
                emit_error_log_from_fs_error(*e);
                continue;
            }
        };

        // Extract and parse properties from YAML if they exist
        let ref_name = python_asset.path.file_stem().unwrap().to_str().unwrap();
        let (maybe_properties, patch_path) =
            extract_model_properties(env, base_ctx, models_properties, ref_name)?;

        // Merge Python model config with project config and schema.yml properties
        let merged_config = match merge_python_config(
            &python_file_info,
            &python_asset,
            package_name,
            dependency_package_name,
            dbt_project,
            &config_resolver,
            maybe_properties.as_ref(),
            arg,
        ) {
            Ok(config) => config,
            Err(err) => {
                emit_error_log_from_fs_error(*err);
                continue;
            }
        };

        let status = if merged_config.enabled {
            ModelStatus::Enabled
        } else {
            ModelStatus::Disabled
        };

        // Convert to SqlFileRenderResult for uniform downstream processing
        let python_result = SqlFileRenderResult {
            asset: python_asset.clone(),
            config: merged_config,
            sql_file_info: crate::sql_file_info::SqlFileInfo {
                sources: python_file_info.sources,
                static_sources: vec![], // Python models have no dead-branch source discovery
                refs: python_file_info.refs,
                this: false,
                metrics: vec![],
                explicit_config: None,
                tests: vec![],
                macros: vec![],
                materializations: vec![],
                docs: vec![],
                snapshots: vec![],
                functions: vec![],
                checksum: python_file_info.checksum,
                execute: false,
            },
            // Match dbt-core's `load_file_contents(strip=True)` behavior.
            raw_code: source.trim().to_owned(),
            rendered_sql: source.clone(),
            macro_spans: Default::default(),
            properties: maybe_properties,
            status,
            render_error_deferred: false,
            patch_path,
            macro_dependencies: Vec::new(),
        };

        results.push(python_result);
    }

    Ok(results)
}

/// Extract model properties from YAML schema files
///
/// Consumes the schema_value from models_properties to mark it as "used"
/// and prevent "Unused schema.yml entry" warnings
fn extract_model_properties(
    env: &Arc<JinjaEnv>,
    base_ctx: &BTreeMap<String, minijinja::Value>,
    models_properties: &mut BTreeMap<String, MinimalPropertiesEntry>,
    ref_name: &str,
) -> FsResult<(Option<ModelProperties>, Option<PathBuf>)> {
    if let Some(mpe) = models_properties.get_mut(ref_name)
        && !mpe.schema_value.is_null()
    {
        // Consume the schema_value by replacing it with null
        // This marks the entry as "used" to prevent unused warnings
        let schema_value = std::mem::replace(&mut mpe.schema_value, dbt_yaml::Value::null());
        let mut base_ctx = base_ctx.clone();
        base_ctx.insert(
            CURRENT_PATH.to_string(),
            minijinja::Value::from(mpe.relative_path.to_string_lossy().to_string()),
        );
        let properties = dbt_jinja_utils::serde::into_typed_with_jinja::<ModelProperties, _>(
            schema_value,
            false,
            env,
            &base_ctx,
            &[],
            dependency_package_name_from_ctx(env, &base_ctx),
            true,
        )?;
        return Ok((Some(properties), Some(mpe.relative_path.clone())));
    }
    Ok((None, None))
}

/// Warn when config.get() accesses keys that exist in config.meta
fn check_config_get_on_meta_keys(config: &ResolvedModelConfig, path: &Path) {
    let Some(meta) = &config.meta else {
        return;
    };
    let Some(config_keys) = &config.config_keys_used else {
        return;
    };
    for key in config_keys.iter().filter(|key| meta.contains_key(*key)) {
        emit_warn_log_from_fs_error(*fs_err!(
            code => ErrorCode::Generic,
            loc => path.to_path_buf(),
            "The key '{}' was accessed using dbt.config.get('{}'), \
            but was detected as a custom config under 'meta'. \
            Please use dbt.config.meta_get('{}') instead of dbt.config.get('{}') \
            to access the custom config value.",
            key, key, key, key
        ));
    }
}

/// Merge Python model config with project config and schema.yml properties
///
/// Python models collect config from dbt.config() calls during AST analysis.
/// These need to be merged with:
/// 1. Project-level config (from dbt_project.yml)
/// 2. Schema.yml properties config (if present)
#[allow(clippy::too_many_arguments)]
fn merge_python_config(
    python_file_info: &PythonFileInfo<ModelConfig>,
    python_asset: &dbt_schemas::state::DbtAsset,
    package_name: &str,
    dependency_package_name: Option<&str>,
    dbt_project: &DbtProject,
    config_resolver: &ProjectConfigResolver<ModelConfig>,
    maybe_properties: Option<&ModelProperties>,
    arg: &ResolveArgs,
) -> FsResult<ResolvedModelConfig> {
    let model_name = python_asset
        .path
        .file_stem()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    let unique_id = get_unique_id(&model_name, package_name, None, "model");

    let fqn = get_node_fqn(
        package_name,
        python_asset.path.clone(),
        vec![model_name],
        dbt_project.model_paths.as_ref().unwrap_or(&vec![]),
    );

    // Config precedence (highest to lowest):
    // 1. root overlay (dependency packages only)
    // 2. dbt.config() in Python file
    // 3. config: in schema.yml
    // 4. dbt_project.yml

    // For Python models, we always apply materialized="table" as the default at the properties
    // layer. See https://github.com/dbt-labs/dbt-core/blob/34bb3f94dde716a3f9c36481d2ead85c211075dd/core/dbt/parser/base.py#L338
    let mut properties_config = maybe_properties
        .and_then(|p| p.config.clone())
        .unwrap_or_default();
    if properties_config.materialized.is_none() {
        properties_config.materialized = Some(DbtMaterialization::Table);
    }

    let python_config = *python_file_info.config.clone();
    // Capture static_analysis BEFORE apply_resolve_defaults so we can distinguish an
    // explicitly-set value from one inherited from the CLI --static-analysis flag.
    let pre_defaults_config = config_resolver
        .with_configs_and_root_overlay(&fqn, &[Some(&properties_config), Some(&python_config)]);
    let merged_config = config_resolver.resolve_with_overrides(
        &fqn,
        &fqn,
        &[Some(&properties_config), Some(&python_config)],
        |c| {
            if !python_file_info.config_keys_used.is_empty() {
                c.config_keys_used = Some(python_file_info.config_keys_used.clone());
                c.config_keys_defaults = Some(python_file_info.config_keys_defaults.clone());
            }
            if !python_file_info.meta_keys_used.is_empty() {
                c.meta_keys_used = Some(python_file_info.meta_keys_used.clone());
                c.meta_keys_defaults = Some(python_file_info.meta_keys_defaults.clone());
            }
            // Python models always have static_analysis turned off
            c.static_analysis = Some(Spanned::new(StaticAnalysisKind::Off));
        },
    );

    if let Some(spanned) = pre_defaults_config.static_analysis {
        crate::validation::warn_python_static_analysis(spanned.into_inner(), &unique_id);
    }

    check_node_static_analysis(
        &merged_config,
        arg.static_analysis,
        &unique_id,
        dependency_package_name,
    );

    check_config_get_on_meta_keys(&merged_config, &python_asset.path);

    let mat = merged_config.materialized.clone();
    if mat != DbtMaterialization::Table && mat != DbtMaterialization::Incremental {
        let err = fs_err!(
            code => ErrorCode::InvalidConfig,
            loc => python_asset.path.to_path_buf(),
            "Invalid materialization '{}' for Python model. Only 'table' or 'incremental' are allowed.",
            mat,
        );
        return Err(err);
    }

    Ok(merged_config)
}

/// Reconciles `loaded_at_field` / `loaded_at_query` set inside `freshness` with
/// the same keys set as its siblings, mirroring `apply_freshness_loaded_at_override`
/// on the source path: the nested value wins and clears its peer.
///
/// Unlike sources, the freshness runner reads these off `freshness` for models, so
/// the resolved pair is written back to both places.
/// Validates `warn_after` / `error_after` against the materialization.
/// `build_after` is a scheduling rule, not an SLA, and is never checked here.
fn validate_model_freshness_sla(
    freshness: Option<&ModelFreshness>,
    materialized: &DbtMaterialization,
    path: &Path,
) -> FsResult<()> {
    let Some(freshness) = freshness.filter(|f| f.has_sla()) else {
        return Ok(());
    };

    match materialized {
        DbtMaterialization::Ephemeral => Err(fs_err!(
            code => ErrorCode::FreshnessConfigInvalid,
            loc => path.to_path_buf(),
            "freshness cannot be configured on an ephemeral model because nothing is materialized \
             to measure freshness against; change the materialization or remove the freshness config",
        )),
        // Their metadata tracks definition changes, not data landing.
        // Empty-or-absent, not merely absent: `loaded_at_field: ""` reaches the
        // runtime as the same `""` sentinel as an unset field, so it would be
        // routed into the metadata-batch path this guard exists to forbid.
        DbtMaterialization::View | DbtMaterialization::External
            if freshness
                .loaded_at_field
                .as_deref()
                .unwrap_or("")
                .is_empty()
                && freshness
                    .loaded_at_query
                    .as_deref()
                    .unwrap_or("")
                    .is_empty() =>
        {
            Err(fs_err!(
                code => ErrorCode::FreshnessConfigInvalid,
                loc => path.to_path_buf(),
                "freshness on a {materialized} requires 'loaded_at_field' or 'loaded_at_query' \
                 because a {materialized}'s relation metadata reflects when its definition last \
                 changed, not how recent its data is",
            ))
        }
        _ => Ok(()),
    }
}

fn apply_model_freshness_loaded_at_override(
    freshness: Option<&mut ModelFreshness>,
    loaded_at_field: &mut Option<String>,
    loaded_at_query: &mut Option<String>,
    model_name: &str,
) -> FsResult<()> {
    let Some(freshness) = freshness else {
        return Ok(());
    };

    match (
        freshness.loaded_at_field.clone(),
        freshness.loaded_at_query.clone(),
    ) {
        (Some(_), Some(_)) => {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "loaded_at_field and loaded_at_query cannot be set at the same time on model `{}`",
                model_name
            ));
        }
        (Some(field), None) => {
            *loaded_at_field = Some(field);
            *loaded_at_query = Some(String::new());
        }
        (None, Some(query)) => {
            *loaded_at_field = Some(String::new());
            *loaded_at_query = Some(query);
        }
        // Nothing nested: the siblings stand.
        (None, None) => {}
    }

    freshness.loaded_at_field = loaded_at_field.clone();
    freshness.loaded_at_query = loaded_at_query.clone();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_model_freshness_loaded_at_override, parse_ref_from_constraint,
        parse_source_from_constraint, validate_database_not_catalog, validate_model_freshness_sla,
    };
    use dbt_adapter_core::AdapterType;
    use dbt_common::{ErrorCode, FsResult};
    use dbt_schemas::schemas::common::{
        DbtMaterialization, FreshnessPeriod, FreshnessRules, ModelFreshnessRules,
    };
    use dbt_schemas::schemas::dbt_catalogs::DbtCatalogs;
    use dbt_schemas::schemas::properties::ModelFreshness;
    use dbt_schemas::schemas::serde::NodeVersion;
    use std::path::Path;

    fn sla_freshness() -> ModelFreshness {
        ModelFreshness {
            warn_after: Some(FreshnessRules {
                count: Some(24),
                period: Some(FreshnessPeriod::hour),
            }),
            ..Default::default()
        }
    }

    fn validate_sla(
        freshness: Option<&ModelFreshness>,
        materialized: DbtMaterialization,
    ) -> FsResult<()> {
        validate_model_freshness_sla(freshness, &materialized, Path::new("models/m.sql"))
    }

    #[test]
    fn ephemeral_model_with_sla_is_rejected() {
        let err = validate_sla(Some(&sla_freshness()), DbtMaterialization::Ephemeral)
            .expect_err("ephemeral models cannot carry a freshness SLA");

        assert_eq!(err.code, ErrorCode::FreshnessConfigInvalid);
        assert!(
            err.to_string().contains("nothing is materialized"),
            "message should explain why: {err}"
        );
    }

    #[test]
    fn view_model_with_sla_and_no_loaded_at_is_rejected() {
        let err = validate_sla(Some(&sla_freshness()), DbtMaterialization::View)
            .expect_err("views need an explicit loaded-at source");

        assert_eq!(err.code, ErrorCode::FreshnessConfigInvalid);
        assert!(
            err.to_string().contains("loaded_at_field"),
            "message should name the remedy: {err}"
        );
    }

    #[test]
    fn external_model_with_sla_and_no_loaded_at_is_rejected() {
        let err = validate_sla(Some(&sla_freshness()), DbtMaterialization::External)
            .expect_err("external tables need an explicit loaded-at source");

        assert_eq!(err.code, ErrorCode::FreshnessConfigInvalid);
        assert!(
            err.to_string().contains("loaded_at_field"),
            "message should name the remedy: {err}"
        );
    }

    #[test]
    fn external_model_with_loaded_at_field_is_accepted() {
        let freshness = ModelFreshness {
            loaded_at_field: Some("updated_at".to_string()),
            ..sla_freshness()
        };
        assert!(validate_sla(Some(&freshness), DbtMaterialization::External).is_ok());

        let with_query = ModelFreshness {
            loaded_at_query: Some("select max(updated_at) from {{ this }}".to_string()),
            ..sla_freshness()
        };
        assert!(validate_sla(Some(&with_query), DbtMaterialization::External).is_ok());
    }

    #[test]
    fn view_model_with_loaded_at_field_is_accepted() {
        let freshness = ModelFreshness {
            loaded_at_field: Some("updated_at".to_string()),
            ..sla_freshness()
        };
        assert!(validate_sla(Some(&freshness), DbtMaterialization::View).is_ok());

        let with_query = ModelFreshness {
            loaded_at_query: Some("select max(updated_at) from {{ this }}".to_string()),
            ..sla_freshness()
        };
        assert!(validate_sla(Some(&with_query), DbtMaterialization::View).is_ok());
    }

    #[test]
    fn view_and_external_models_reject_empty_string_loaded_at() {
        for materialized in [DbtMaterialization::View, DbtMaterialization::External] {
            let empty_field = ModelFreshness {
                loaded_at_field: Some(String::new()),
                ..sla_freshness()
            };
            assert!(
                validate_sla(Some(&empty_field), materialized.clone()).is_err(),
                "{materialized} with an empty loaded_at_field should be rejected like an absent one"
            );

            let empty_query = ModelFreshness {
                loaded_at_query: Some(String::new()),
                ..sla_freshness()
            };
            assert!(
                validate_sla(Some(&empty_query), materialized.clone()).is_err(),
                "{materialized} with an empty loaded_at_query should be rejected like an absent one"
            );
        }
    }

    #[test]
    fn table_like_model_with_sla_and_no_loaded_at_is_accepted() {
        for materialized in [
            DbtMaterialization::Table,
            DbtMaterialization::Incremental,
            DbtMaterialization::MaterializedView,
            DbtMaterialization::DynamicTable,
        ] {
            assert!(
                validate_sla(Some(&sla_freshness()), materialized.clone()).is_ok(),
                "{materialized} should fall back to adapter metadata"
            );
        }
    }

    #[test]
    fn build_after_only_is_never_validated() {
        let build_after_only = ModelFreshness {
            build_after: Some(ModelFreshnessRules {
                count: Some(1),
                period: Some(FreshnessPeriod::day),
                updates_on: None,
            }),
            ..Default::default()
        };

        // Even the otherwise-rejected materializations must pass.
        for materialized in [DbtMaterialization::Ephemeral, DbtMaterialization::View] {
            assert!(validate_sla(Some(&build_after_only), materialized.clone()).is_ok());
        }
        assert!(validate_sla(None, DbtMaterialization::Ephemeral).is_ok());
    }

    /// Mirrors `test_freshness_loaded_at_field_overrides_top_level_query` on the
    /// source path: nested wins and clears its sibling peer.
    #[test]
    fn nested_loaded_at_field_overrides_sibling_query() {
        let mut freshness = ModelFreshness {
            loaded_at_field: Some("FRESHNESS_LOADED_AT".to_string()),
            ..sla_freshness()
        };
        let mut field = Some(String::new());
        let mut query = Some("select max(src_loaded_at) from m".to_string());

        apply_model_freshness_loaded_at_override(
            Some(&mut freshness),
            &mut field,
            &mut query,
            "my_model",
        )
        .unwrap();

        assert_eq!(field.as_deref(), Some("FRESHNESS_LOADED_AT"));
        assert_eq!(query.as_deref(), Some(""));
        assert_eq!(
            freshness.loaded_at_field.as_deref(),
            Some("FRESHNESS_LOADED_AT")
        );
        assert_eq!(freshness.loaded_at_query.as_deref(), Some(""));
    }

    /// Mirrors `test_freshness_loaded_at_query_overrides_top_level_field`.
    #[test]
    fn nested_loaded_at_query_overrides_sibling_field() {
        let mut freshness = ModelFreshness {
            loaded_at_query: Some("select max(freshness_loaded_at) from m".to_string()),
            ..sla_freshness()
        };
        let mut field = Some("SRC_LOADED_AT".to_string());
        let mut query = Some(String::new());

        apply_model_freshness_loaded_at_override(
            Some(&mut freshness),
            &mut field,
            &mut query,
            "my_model",
        )
        .unwrap();

        assert_eq!(field.as_deref(), Some(""));
        assert_eq!(
            query.as_deref(),
            Some("select max(freshness_loaded_at) from m")
        );
    }

    /// The sibling keys are the shape the RFC documents, so they must reach
    /// `freshness` — that is where the freshness runner reads them for models.
    #[test]
    fn sibling_loaded_at_field_is_mirrored_onto_freshness() {
        let mut freshness = sla_freshness();
        let mut field = Some("SIBLING_LOADED_AT".to_string());
        let mut query = None;

        apply_model_freshness_loaded_at_override(
            Some(&mut freshness),
            &mut field,
            &mut query,
            "my_model",
        )
        .unwrap();

        assert_eq!(
            freshness.loaded_at_field.as_deref(),
            Some("SIBLING_LOADED_AT")
        );
    }

    /// Mirrors `test_freshness_loaded_at_field_and_query_conflict_errors`.
    #[test]
    fn nested_loaded_at_field_and_query_conflict_errors() {
        let mut freshness = ModelFreshness {
            loaded_at_field: Some("LOADED_AT".to_string()),
            loaded_at_query: Some("select max(loaded_at) from m".to_string()),
            ..sla_freshness()
        };
        let mut field = None;
        let mut query = None;

        let err = apply_model_freshness_loaded_at_override(
            Some(&mut freshness),
            &mut field,
            &mut query,
            "my_model",
        )
        .expect_err("nested freshness peers should be mutually exclusive");
        assert!(
            err.to_string()
                .contains("loaded_at_field and loaded_at_query cannot be set at the same time"),
            "error must name the conflict; got: {err}"
        );
    }

    fn catalogs_from_yaml(yaml: &str) -> DbtCatalogs {
        let value: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
        let span = value.span().clone();
        let mapping = value
            .as_mapping()
            .expect("top-level must be a mapping")
            .clone();
        DbtCatalogs::new(mapping, span)
    }

    #[test]
    fn single_match_errors() {
        let catalogs = catalogs_from_yaml(
            r#"
catalogs:
  - name: my_glue
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://glue.example.com"
"#,
        );
        let err = validate_database_not_catalog(
            "my_model",
            "my_glue",
            &catalogs,
            AdapterType::DuckDB,
            Path::new("models/m.sql"),
        )
        .expect_err("database matching a catalog name should error");
        let msg = err.to_string();
        assert!(msg.contains("my_glue"), "message was: {msg}");
        assert!(msg.contains("catalog_name: my_glue"), "message was: {msg}");
    }

    #[test]
    fn ambiguous_multi_catalog_match_lists_all() {
        let catalogs = catalogs_from_yaml(
            r#"
catalogs:
  - name: cat_a
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://a.example.com"
        catalog_database: "shared"
  - name: cat_b
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://b.example.com"
        catalog_database: "shared"
"#,
        );
        let err = validate_database_not_catalog(
            "my_model",
            "shared",
            &catalogs,
            AdapterType::DuckDB,
            Path::new("models/m.sql"),
        )
        .expect_err("ambiguous match should error");
        let msg = err.to_string();
        assert!(msg.contains("multiple catalogs"), "message was: {msg}");
        assert!(msg.contains("cat_a"), "message was: {msg}");
        assert!(msg.contains("cat_b"), "message was: {msg}");
    }

    #[test]
    fn test_parse_ref_single_arg() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_model')"),
            Some((None, "my_model".to_string(), None))
        );
    }

    #[test]
    fn test_parse_ref_double_quoted() {
        assert_eq!(
            parse_ref_from_constraint(r#"ref("my_model")"#),
            Some((None, "my_model".to_string(), None))
        );
    }

    #[test]
    fn test_parse_ref_two_args() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_pkg', 'my_model')"),
            Some((Some("my_pkg".to_string()), "my_model".to_string(), None))
        );
    }

    #[test]
    fn test_parse_ref_with_whitespace() {
        assert_eq!(
            parse_ref_from_constraint("  ref( 'my_model' )  "),
            Some((None, "my_model".to_string(), None))
        );
    }

    #[test]
    fn test_parse_ref_version_kwarg_integer() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_model', version=2)"),
            Some((None, "my_model".to_string(), Some(NodeVersion::Integer(2))))
        );
    }

    #[test]
    fn test_parse_ref_v_kwarg_alias() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_model', v=1)"),
            Some((None, "my_model".to_string(), Some(NodeVersion::Integer(1))))
        );
    }

    #[test]
    fn test_parse_ref_version_kwarg_string() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_model', version='1.0')"),
            Some((
                None,
                "my_model".to_string(),
                Some(NodeVersion::String("1.0".to_string()))
            ))
        );
    }

    #[test]
    fn test_parse_ref_two_args_with_version() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_pkg', 'my_model', version=3)"),
            Some((
                Some("my_pkg".to_string()),
                "my_model".to_string(),
                Some(NodeVersion::Integer(3))
            ))
        );
    }

    /// A quoted version must stay a string even when it looks numeric. This guards
    /// the `is_quoted` branch: `version="2"` is `String("2")`, distinct from the
    /// unquoted `version=2` which is `Integer(2)`. The existing `'1.0'` test would
    /// pass either way, so it does not exercise this branch.
    #[test]
    fn test_parse_ref_version_kwarg_quoted_integer() {
        assert_eq!(
            parse_ref_from_constraint(r#"ref('my_model', version="2")"#),
            Some((
                None,
                "my_model".to_string(),
                Some(NodeVersion::String("2".to_string()))
            ))
        );
    }

    /// Unquoted non-integral version parses as a float (mirrors dbt-core's numeric
    /// coercion), distinct from the quoted string form.
    #[test]
    fn test_parse_ref_version_kwarg_unquoted_float() {
        assert_eq!(
            parse_ref_from_constraint("ref('my_model', version=1.5)"),
            Some((None, "my_model".to_string(), Some(NodeVersion::Float(1.5))))
        );
    }

    #[test]
    fn test_parse_ref_not_a_ref() {
        assert_eq!(parse_ref_from_constraint("some_schema.some_table"), None);
    }

    #[test]
    fn test_parse_ref_source_call_not_a_ref() {
        assert_eq!(parse_ref_from_constraint("source('src', 'tbl')"), None);
    }

    #[test]
    fn test_parse_source_basic() {
        assert_eq!(
            parse_source_from_constraint("source('my_source', 'my_table')"),
            Some(("my_source".to_string(), "my_table".to_string()))
        );
    }

    #[test]
    fn test_parse_source_double_quoted() {
        assert_eq!(
            parse_source_from_constraint(r#"source("my_source", "my_table")"#),
            Some(("my_source".to_string(), "my_table".to_string()))
        );
    }

    #[test]
    fn test_parse_source_with_whitespace() {
        assert_eq!(
            parse_source_from_constraint("  source( 'my_source' , 'my_table' )  "),
            Some(("my_source".to_string(), "my_table".to_string()))
        );
    }

    #[test]
    fn test_parse_source_not_a_source() {
        assert_eq!(parse_source_from_constraint("some_schema.some_table"), None);
    }

    #[test]
    fn test_parse_source_ref_call_not_a_source() {
        assert_eq!(parse_source_from_constraint("ref('my_model')"), None);
    }

    #[test]
    fn test_parse_source_missing_second_arg() {
        assert_eq!(parse_source_from_constraint("source('my_source')"), None);
    }
}

/// Tests for the resolve-time `interactive_table` config validations and the gate that
/// restricts them to models that will actually be built on Snowflake.
#[cfg(test)]
mod interactive_table_validation_tests {
    use super::{
        AdapterType, DbtMaterialization, ModelConfig, ModelStatus, ResolvedModelConfig, Spanned,
        StaticAnalysisKind, validate_interactive_table_cluster_by,
        validate_interactive_table_config, validate_interactive_table_table_format,
        validate_interactive_table_target_lag_warehouse, validate_interactive_table_transient,
    };
    use dbt_common::FsResult;
    use dbt_schemas::schemas::common::{ClusterConfig, DbtQuoting};
    use dbt_schemas::schemas::project::{ResolvableConfig, WarehouseSpecificNodeConfig};
    use std::path::PathBuf;

    fn test_path() -> PathBuf {
        PathBuf::from("models/my_model.sql")
    }

    /// Each test mutates a single field off of this baseline.
    fn valid_interactive_table_config() -> ModelConfig {
        ModelConfig {
            quoting: Some(DbtQuoting::default()),
            static_analysis: Some(Spanned::new(StaticAnalysisKind::Off)),
            materialized: Some(DbtMaterialization::InteractiveTable),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                cluster_by: Some(ClusterConfig::String("id".to_string())),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn resolve(cfg: ModelConfig) -> ResolvedModelConfig {
        cfg.finalize()
    }

    fn assert_rejects_with(result: FsResult<()>, expected_substring: &str) {
        let err = result.expect_err("expected a validation error");
        let message = err.to_string();
        assert!(
            message.contains(expected_substring),
            "error message '{message}' did not contain expected text '{expected_substring}'"
        );
    }

    #[test]
    fn interactive_table_without_cluster_by_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by = None;
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_cluster_by(&resolved, &test_path()),
            "require `cluster_by`",
        );
    }

    #[test]
    fn interactive_table_with_cluster_by_is_valid() {
        let resolved = resolve(valid_interactive_table_config());
        assert!(validate_interactive_table_cluster_by(&resolved, &test_path()).is_ok());
    }

    // The four tests below each cover one shape of a `cluster_by` that is set but names no
    // usable column. Each renders a malformed `cluster by` clause, so each has to be rejected
    // exactly as an absent `cluster_by` is.
    #[test]
    fn interactive_table_with_empty_cluster_by_list_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by = Some(ClusterConfig::List(vec![]));
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_cluster_by(&resolved, &test_path()),
            "name at least one non-blank column",
        );
    }

    #[test]
    fn interactive_table_with_empty_cluster_by_string_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by = Some(ClusterConfig::String(String::new()));
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_cluster_by(&resolved, &test_path()),
            "name at least one non-blank column",
        );
    }

    #[test]
    fn interactive_table_with_whitespace_only_cluster_by_string_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by =
            Some(ClusterConfig::String("   ".to_string()));
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_cluster_by(&resolved, &test_path()),
            "name at least one non-blank column",
        );
    }

    // A blank entry alongside a real column is rejected too: the rendered clause would be
    // `cluster by (id, )`, which is malformed even though one column name is present.
    #[test]
    fn interactive_table_with_blank_cluster_by_list_entry_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by = Some(ClusterConfig::List(vec![
            "id".to_string(),
            "  ".to_string(),
        ]));
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_cluster_by(&resolved, &test_path()),
            "name at least one non-blank column",
        );
    }

    // Positive control for the list variant: rejecting empty and blank shapes must not reject a
    // list that names real columns.
    #[test]
    fn interactive_table_with_multi_column_cluster_by_list_is_valid() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.cluster_by = Some(ClusterConfig::List(vec![
            "id".to_string(),
            "val".to_string(),
        ]));
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_cluster_by(&resolved, &test_path()).is_ok());
    }

    #[test]
    fn interactive_table_with_iceberg_table_format_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.table_format = Some("iceberg".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_table_format(&resolved, &test_path()),
            "table_format 'iceberg' is not supported",
        );
    }

    #[test]
    fn interactive_table_with_iceberg_table_format_uppercase_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.table_format = Some("ICEBERG".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_table_format(&resolved, &test_path()),
            "table_format 'ICEBERG' is not supported",
        );
    }

    #[test]
    fn interactive_table_with_transient_true_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.transient = Some(true);
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_transient(&resolved, &test_path()),
            "transient=true is not supported",
        );
    }

    #[test]
    fn interactive_table_with_transient_false_is_valid() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.transient = Some(false);
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_transient(&resolved, &test_path()).is_ok());
    }

    #[test]
    fn interactive_table_with_target_lag_and_no_warehouse_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_target_lag_warehouse(&resolved, &test_path()),
            "target_lag requires refresh_warehouse or snowflake_warehouse",
        );
    }

    // target_lag with only refresh_warehouse is valid. The rule follows the merged
    // refresh_warehouse-then-snowflake_warehouse semantic, not snowflake_warehouse alone.
    #[test]
    fn interactive_table_with_target_lag_and_refresh_warehouse_is_valid() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        cfg.__warehouse_specific_config__.refresh_warehouse = Some("wh".to_string());
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_target_lag_warehouse(&resolved, &test_path()).is_ok());
    }

    #[test]
    fn interactive_table_with_target_lag_and_snowflake_warehouse_is_valid() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        cfg.__warehouse_specific_config__.snowflake_warehouse = Some("wh".to_string());
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_target_lag_warehouse(&resolved, &test_path()).is_ok());
    }

    #[test]
    fn interactive_table_with_target_lag_and_blank_refresh_warehouse_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        cfg.__warehouse_specific_config__.refresh_warehouse = Some("".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_target_lag_warehouse(&resolved, &test_path()),
            "target_lag requires refresh_warehouse or snowflake_warehouse",
        );
    }

    #[test]
    fn interactive_table_with_target_lag_and_whitespace_only_refresh_warehouse_errors() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        cfg.__warehouse_specific_config__.refresh_warehouse = Some("   ".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_target_lag_warehouse(&resolved, &test_path()),
            "target_lag requires refresh_warehouse or snowflake_warehouse",
        );
    }

    #[test]
    fn interactive_table_with_target_lag_and_blank_refresh_warehouse_falls_back_to_snowflake_warehouse_is_valid()
     {
        // A blank refresh_warehouse must not block falling back to a valid snowflake_warehouse.
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        cfg.__warehouse_specific_config__.refresh_warehouse = Some("".to_string());
        cfg.__warehouse_specific_config__.snowflake_warehouse = Some("wh".to_string());
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_target_lag_warehouse(&resolved, &test_path()).is_ok());
    }

    // Regression guard: a non-interactive model violating every rule above must pass
    // every validator, since each is a no-op outside `materialized: interactive_table`.
    #[test]
    fn non_interactive_table_model_violating_every_rule_is_valid() {
        // cluster_by is intentionally left unset.
        let cfg = ModelConfig {
            quoting: Some(DbtQuoting::default()),
            static_analysis: Some(Spanned::new(StaticAnalysisKind::Off)),
            materialized: Some(DbtMaterialization::Table),
            table_format: Some("iceberg".to_string()),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                transient: Some(true),
                target_lag: Some("1 minute".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve(cfg);
        assert!(validate_interactive_table_cluster_by(&resolved, &test_path()).is_ok());
        assert!(validate_interactive_table_table_format(&resolved, &test_path()).is_ok());
        assert!(validate_interactive_table_transient(&resolved, &test_path()).is_ok());
        assert!(validate_interactive_table_target_lag_warehouse(&resolved, &test_path()).is_ok());
    }

    /// Violates all four interactive_table rules at once: no `cluster_by`, an iceberg
    /// `table_format`, `transient: true`, and a `target_lag` with no warehouse.
    fn config_violating_every_rule() -> ModelConfig {
        ModelConfig {
            quoting: Some(DbtQuoting::default()),
            static_analysis: Some(Spanned::new(StaticAnalysisKind::Off)),
            materialized: Some(DbtMaterialization::InteractiveTable),
            table_format: Some("iceberg".to_string()),
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                transient: Some(true),
                target_lag: Some("1 minute".to_string()),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    // A disabled model is never built, so the gate must skip validation regardless of how
    // badly its config is malformed.
    #[test]
    fn disabled_model_skips_validation_on_snowflake() {
        let resolved = resolve(config_violating_every_rule());
        assert!(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Disabled,
                AdapterType::Snowflake,
                &test_path(),
            )
            .is_ok()
        );
    }

    // A model that failed to parse is discarded before it would ever be built; piling a
    // config error on top of a parse failure would be wrong, so the gate must skip it too.
    #[test]
    fn parsing_failed_model_skips_validation_on_snowflake() {
        let resolved = resolve(config_violating_every_rule());
        assert!(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::ParsingFailed,
                AdapterType::Snowflake,
                &test_path(),
            )
            .is_ok()
        );
    }

    // interactive_table is Snowflake-only; on any other adapter the execution-time
    // "materialization macro not found" error is the correct diagnostic, so the gate must
    // skip validation even for an enabled model.
    #[test]
    fn enabled_model_skips_validation_on_non_snowflake_adapter() {
        let resolved = resolve(config_violating_every_rule());
        assert!(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Enabled,
                AdapterType::Postgres,
                &test_path(),
            )
            .is_ok()
        );
    }

    // Positive control: an enabled model on Snowflake must still be validated.
    #[test]
    fn enabled_model_on_snowflake_is_validated() {
        let resolved = resolve(config_violating_every_rule());
        assert_rejects_with(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Enabled,
                AdapterType::Snowflake,
                &test_path(),
            ),
            "require `cluster_by`",
        );
    }

    // One rule violated at a time: `enabled_model_on_snowflake_is_validated` violates all four,
    // so `?` short-circuits on `cluster_by` before the later checks run.
    #[test]
    fn enabled_model_on_snowflake_has_table_format_validated() {
        let mut cfg = valid_interactive_table_config();
        cfg.table_format = Some("iceberg".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Enabled,
                AdapterType::Snowflake,
                &test_path(),
            ),
            "table_format 'iceberg' is not supported",
        );
    }

    #[test]
    fn enabled_model_on_snowflake_has_transient_validated() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.transient = Some(true);
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Enabled,
                AdapterType::Snowflake,
                &test_path(),
            ),
            "transient=true is not supported",
        );
    }

    #[test]
    fn enabled_model_on_snowflake_has_target_lag_warehouse_validated() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.target_lag = Some("1 minute".to_string());
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_config(
                &resolved,
                ModelStatus::Enabled,
                AdapterType::Snowflake,
                &test_path(),
            ),
            "target_lag requires refresh_warehouse or snowflake_warehouse",
        );
    }

    // The inherited-value remedy is part of the message contract: a project-wide
    // `+transient: true` reaches a model whose own file never mentions the key, so the error
    // has to say what to set and where.
    #[test]
    fn transient_error_names_the_remedy() {
        let mut cfg = valid_interactive_table_config();
        cfg.__warehouse_specific_config__.transient = Some(true);
        let resolved = resolve(cfg);
        assert_rejects_with(
            validate_interactive_table_transient(&resolved, &test_path()),
            "Set `transient: false` on this model",
        );
    }
}
