use super::utils::{base_tests_inner, column_tests_inner};
use crate::args::ResolveArgs;
use crate::resolve::resolve_utils::extract_config_map;
use dbt_adapter_core::*;
use dbt_common::FsError;
use dbt_common::FsResult;
use dbt_common::constants::DBT_GENERIC_TESTS_DIR_NAME;
use dbt_common::io_args;
use dbt_common::io_args::IoArgs;
use dbt_common::tracing::dbt_emit::{emit_strict_parse_error, emit_warn_log_from_fs_error};
use dbt_common::{ErrorCode, err};
use dbt_common::{fs_err, stdfs};
use dbt_frontend_common::Dialect;
use dbt_jinja_utils::jinja_arg_format::format_value_for_jinja;
use dbt_jinja_utils::serde::check_single_expression_without_whitepsace_control;
use dbt_schemas::schemas::common::Versions;
use dbt_schemas::schemas::common::normalize_quote;
use dbt_schemas::schemas::data_tests::{CustomTest, DataTests};

use dbt_schemas::schemas::dbt_column::ColumnInheritanceRules;
use dbt_schemas::schemas::dbt_column::ColumnProperties;
use dbt_schemas::schemas::dbt_column::VersionColumnProperties;
use dbt_schemas::schemas::project::DataTestConfig;
use dbt_schemas::schemas::properties::Tables;
use dbt_schemas::schemas::properties::{ModelProperties, SeedProperties, SnapshotProperties};
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use dbt_schemas::schemas::serde::yaml_to_fs_error;
use dbt_schemas::state::{DbtAsset, GenericTestAsset};
use dbt_yaml::ShouldBe;
use dbt_yaml::Span;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use itertools::Itertools;
use md5;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::DefaultHasher;
use std::hash::Hash;
use std::hash::Hasher;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::LazyLock;

use dbt_common::string_utils::maybe_truncate_test_name;

#[derive(Debug, Clone)]
pub struct ColumnTestEntry {
    pub quote: bool,
    pub tests: Vec<DataTests>,
    pub tags: Vec<String>,
    pub(super) legacy_syntax_handling: LegacyTestSyntaxHandling,
}

#[derive(Debug, Clone, Copy)]
pub(super) enum LegacyTestSyntaxHandling {
    Strict,
    Warn,
}

/// Raw (unrendered) schema.yml `config:` blocks for a resource's generic tests, captured
/// from the *raw* `schema_value` before Jinja is rendered. Ordered per test so callers can
/// correlate positionally with the (rendered) typed tests that `persist` iterates — the raw
/// and typed test lists have identical shape and order, so index correlation is exact and
/// avoids re-matching by name.
///
/// The typed `DataTestConfig` cannot be reused here: schema.yml Jinja is rendered before
/// deserialization (e.g. `severity: "{{ ... }}"` becomes a `Severity` enum value), so the
/// typed config holds rendered values. `unrendered_config` must preserve authored Jinja, so
/// we source these maps from the raw YAML — mirroring how models/seeds snapshot their own
/// `config:` via `extract_config_map` before render.
#[derive(Debug, Clone, Default)]
pub struct TestUnrenderedConfigs {
    /// Per model/table-level test, in `data_tests`/`tests` order.
    pub model_level: Vec<RawTestConfig>,
    /// Per column name (unquoted) → per-test raw config, in `data_tests`/`tests` order.
    pub columns: BTreeMap<String, Vec<RawTestConfig>>,
}

/// Raw, unrendered config authored on a single generic test entry in schema.yml.
#[derive(Debug, Clone, Default)]
pub struct RawTestConfig {
    /// Merged authored config used for the manifest's `unrendered_config`: explicit `config:`,
    /// then deprecated top-level keys, then `arguments:` keys.
    pub merged: BTreeMap<String, dbt_yaml::Value>,
    /// Keys written literally under `config:`, before any merging.
    pub explicit_config_keys: BTreeSet<String>,
}

/// Extract the raw config authored on a single test entry in schema.yml.
/// Handles the three YAML shapes that deserialize into [`DataTests`]:
/// - a bare string (`- not_null`) → no config → empty map;
/// - the multi-key shape (`{test_name: <t>, config: {...}}`) → `config` at the top level;
/// - the single-key shape (`{not_null: {config: {...}}}`) → `config` nested under the test key.
///
/// Config keys accepted in the deprecated top-level and `arguments:` positions are merged in
/// the same order as test parsing: explicit `config:`, deprecated top-level keys, then keys from
/// `arguments:`. Keeping the raw values here preserves authored Jinja in `unrendered_config`.
/// The keys written literally under `config:` are also recorded, before that merge erases which
/// position each key came from.
fn raw_test_entry_config(entry: &dbt_yaml::Value) -> RawTestConfig {
    let Some(mapping) = entry.as_mapping() else {
        // Bare-string test (or unexpected scalar): no authored config.
        return RawTestConfig::default();
    };
    // Multi-key shape carries an explicit `test_name:` key alongside `config:`.
    let config_holder = if mapping.get("test_name").is_some() {
        entry
    } else {
        // Single-key shape: `{ <test-type>: { config: {...}, ... } }`.
        match mapping.iter().next() {
            Some((_, inner)) => inner,
            None => return RawTestConfig::default(),
        }
    };
    let mut config = extract_config_map(config_holder).unwrap_or_default();
    let explicit_config_keys: BTreeSet<String> = config.keys().cloned().collect();
    let Some(holder_mapping) = config_holder.as_mapping() else {
        return RawTestConfig {
            merged: config,
            explicit_config_keys,
        };
    };

    for key in CONFIG_ARGS {
        if let Some(value) = holder_mapping.get(*key) {
            config.insert((*key).to_string(), value.clone());
        }
    }

    if let Some(arguments) = holder_mapping
        .get("arguments")
        .and_then(dbt_yaml::Value::as_mapping)
    {
        for key in CONFIG_ARGS {
            if let Some(value) = arguments.get(*key) {
                config.insert((*key).to_string(), value.clone());
            }
        }
    }

    RawTestConfig {
        merged: config,
        explicit_config_keys,
    }
}

/// Collect the raw per-test `config:` blocks (in order) from a `data_tests`/`tests` sequence.
fn raw_tests_seq_configs(container: &dbt_yaml::Value) -> Vec<RawTestConfig> {
    let tests = container
        .get("data_tests")
        .or_else(|| container.get("tests"));
    match tests.and_then(|t| t.as_sequence()) {
        Some(seq) => seq.iter().map(raw_test_entry_config).collect(),
        None => Vec::new(),
    }
}

/// Build [`TestUnrenderedConfigs`] from a resource's raw `schema_value` (for sources, the raw
/// table value). Navigates model/table-level `data_tests`/`tests` and `columns[*].data_tests`.
pub fn extract_test_unrendered_configs(schema_value: &dbt_yaml::Value) -> TestUnrenderedConfigs {
    let model_level = raw_tests_seq_configs(schema_value);

    let mut columns: BTreeMap<String, Vec<RawTestConfig>> = BTreeMap::new();
    if let Some(cols) = schema_value.get("columns").and_then(|c| c.as_sequence()) {
        for col in cols {
            let Some(name) = col.get("name").and_then(|n| n.as_str()) else {
                continue;
            };
            columns.insert(name.to_string(), raw_tests_seq_configs(col));
        }
    }

    TestUnrenderedConfigs {
        model_level,
        columns,
    }
}

pub struct TestableNode<'a, T: TestableNodeTrait> {
    inner: &'a T,
}

impl<T: TestableNodeTrait> TestableNode<'_, T> {
    #[allow(clippy::too_many_arguments)]
    pub fn persist(
        &self,
        project_name: &str,
        root_project_name: &str,
        collected_generic_tests: &mut Vec<GenericTestAsset>,
        test_name_truncations: &mut HashMap<String, String>,
        seen_generic_test_paths: &mut HashMap<PathBuf, String>,
        adapter_type: AdapterType,
        io_args: &IoArgs,
        original_file_path: &Path,
        suppress_deprecated_test_validation: bool,
        raw_test_configs: &TestUnrenderedConfigs,
    ) -> FsResult<()> {
        let test_configs: Vec<GenericTestConfig> = self.try_into()?;
        // Process tests for each version (or single resource)
        let mut seen_tests: HashSet<String> = HashSet::new();
        for test_config in test_configs {
            // Handle model-level tests
            if let Some(tests) = &test_config.model_tests {
                for (i, test) in tests.iter().enumerate() {
                    let column_test: DataTests = test.clone();
                    // The raw model-level test list has the same order as the typed one, so
                    // index correlation gives this test's authored schema.yml config.
                    let raw_config = raw_test_configs
                        .model_level
                        .get(i)
                        .cloned()
                        .unwrap_or_default();
                    let test_asset = persist_inner(
                        project_name,
                        root_project_name,
                        &test_config,
                        // A model-level test is attached to no column, even when it passes
                        // a `column_name` kwarg to its macro.
                        None,
                        test.column_name(),
                        &column_test,
                        io_args,
                        original_file_path,
                        &mut seen_tests,
                        test_name_truncations,
                        seen_generic_test_paths,
                        &[],
                        suppress_deprecated_test_validation,
                        LegacyTestSyntaxHandling::Strict,
                        raw_config,
                    )?;
                    collected_generic_tests.push(test_asset);
                }
            }

            // Handle column-level tests
            if let Some(column_tests) = &test_config.column_tests {
                for (column_name, entry) in column_tests {
                    // Raw per-test configs for this column (same order as `entry.tests`).
                    let col_configs = raw_test_configs.columns.get(column_name);
                    for (i, test) in entry.tests.iter().enumerate() {
                        // Need dialect to quote properly
                        let (column_name, should_quote) =
                            normalize_quote(entry.quote, adapter_type, column_name);

                        let quoted_column_name = if should_quote {
                            let q = quote_char(adapter_type).to_string();
                            format!("{}{}{}", q, column_name, q)
                        } else {
                            column_name.to_string()
                        };
                        let raw_config = col_configs
                            .and_then(|v| v.get(i))
                            .cloned()
                            .unwrap_or_default();
                        let test_asset = persist_inner(
                            project_name,
                            root_project_name,
                            &test_config,
                            Some(quoted_column_name.clone()),
                            Some(StringOrArrayOfStrings::String(quoted_column_name)),
                            test,
                            io_args,
                            original_file_path,
                            &mut seen_tests,
                            test_name_truncations,
                            seen_generic_test_paths,
                            &entry.tags,
                            suppress_deprecated_test_validation,
                            entry.legacy_syntax_handling,
                            raw_config,
                        )?;
                        collected_generic_tests.push(test_asset);
                    }
                }
            }
        }

        Ok(())
    }
}

/// Path for a generated generic-test SQL asset. Distinct tests can flatten to the
/// same generated name (e.g. `not_null` on `orders.status_code` vs `orders_status.code`),
/// and the name-keyed default path would let the second test overwrite the first one's
/// SQL file and silently drop it from the manifest. On collision, suffix the kwargs
/// hash (already unique per test, mirroring the `unique_id`) to keep the assets apart.
///
/// Keyed on path -> test_hash (not just path) so that re-deriving the exact same test
/// on a later pass (e.g. versioned models are resolved once per version SQL file, and
/// shared column tests get re-derived identically each time) is idempotent rather than
/// being mistaken for a real collision.
fn generic_test_asset_path(
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    full_name: &str,
    test_hash: &str,
) -> PathBuf {
    let default_path = PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME).join(format!("{full_name}.sql"));
    match seen_generic_test_paths.get(&default_path) {
        None => {
            seen_generic_test_paths.insert(default_path.clone(), test_hash.to_string());
            default_path
        }
        Some(existing_hash) if existing_hash == test_hash => default_path,
        Some(_) => {
            let hashed_path = PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME)
                .join(format!("{full_name}_{test_hash}.sql"));
            seen_generic_test_paths.insert(hashed_path.clone(), test_hash.to_string());
            hashed_path
        }
    }
}

#[allow(clippy::ptr_arg)]
#[allow(clippy::too_many_arguments)]
fn persist_inner(
    project_name: &str,
    root_project_name: &str,
    test_config: &GenericTestConfig,
    column_name: Option<String>,
    column_name_kwarg: Option<StringOrArrayOfStrings>,
    test: &DataTests,
    io_args: &IoArgs,
    original_file_path: &Path,
    seen_tests: &mut HashSet<String>,
    test_name_truncations: &mut HashMap<String, String>,
    seen_generic_test_paths: &mut HashMap<PathBuf, String>,
    column_tags: &[String],
    suppress_deprecated_test_validation: bool,
    legacy_syntax_handling: LegacyTestSyntaxHandling,
    raw_config: RawTestConfig,
) -> FsResult<GenericTestAsset> {
    let RawTestConfig {
        merged: unrendered_schema_config,
        explicit_config_keys,
    } = raw_config;

    // If this is not the root project, we need to pass the project name as a dependency package name
    let dependecy_package_name = if project_name != root_project_name {
        Some(project_name)
    } else {
        None
    };

    let details = get_test_details(
        test,
        test_config,
        column_name_kwarg,
        dependecy_package_name,
        suppress_deprecated_test_validation,
        legacy_syntax_handling,
        &explicit_config_keys,
    )?;

    let TestDetails {
        test_macro_name,
        custom_test_name,
        kwargs,
        namespace,
        config,
        jinja_set_vars,
    } = details;

    let full_name = generate_test_name(
        test_macro_name.as_str(),
        custom_test_name,
        project_name,
        test_config,
        &kwargs,
        namespace.as_ref(),
        &jinja_set_vars,
        test_name_truncations,
    );

    // Generate unique_id hash from UNCLEANED kwargs to match mantle's behavior.
    // Use the original (non-truncated) name for the hash input, since dbt-core/Mantle
    // compute the hash from the full name, not the truncated form.
    let restored_kwargs = restore_jinja_placeholders(&kwargs, &jinja_set_vars);
    let fqn_name_for_hash = test_name_truncations.get(&full_name).unwrap_or(&full_name);
    let test_hash = generate_test_unique_id_hash(
        fqn_name_for_hash,
        &test_macro_name,
        namespace.as_ref(),
        &restored_kwargs,
    );
    let unique_id = format!("{}.{}", full_name, test_hash);

    let path = generic_test_asset_path(seen_generic_test_paths, &full_name, &test_hash);
    let test_file = io_args.out_dir.join(&path);
    let generated_test_sql = generate_test_macro(
        test_macro_name.as_str(),
        &kwargs,
        namespace.as_deref(),
        &config,
        &jinja_set_vars,
    )?;

    // Check for duplicate tests using the unique_id (which includes kwargs hash)
    // rather than just the cleaned name. This matches mantle's behavior where
    // tests with different kwargs get different unique_ids.
    if !seen_tests.insert(unique_id) {
        match &column_name {
            Some(column_name) => {
                return err!(
                    ErrorCode::DbtYamlValidationError,
                    "dbt found two data_tests with the same name \"{}\" on column \"{}\" in \"{}\" in the file \"{}\"",
                    full_name,
                    column_name,
                    test_config.resource_name,
                    original_file_path.display()
                );
            }
            None => {
                return err!(
                    ErrorCode::DbtYamlValidationError,
                    "dbt found two data_tests with the same name \"{}\" in \"{}\" in the file \"{}\"",
                    full_name,
                    test_config.resource_name,
                    original_file_path.display()
                );
            }
        }
    }
    // Explicit test names (`name: customer/id_not_null`) can carry `/` straight into the generic-test SQL path.
    stdfs::create_dir_all(test_file.parent().unwrap())?;
    stdfs::write(&test_file, generated_test_sql)?;
    let dbt_asset = DbtAsset {
        path,
        original_path: original_file_path.to_path_buf(),
        base_path: io_args.out_dir.to_path_buf(),
        package_name: project_name.to_string(),
    };
    let (meta_name, meta_namespace) = (Some(test_macro_name), namespace);
    let column_name_kwarg = kwargs
        .get("column_name")
        .and_then(StringOrArrayOfStrings::from_json_value);
    let combination_of_columns = kwargs
        .get("combination_of_columns")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });
    // Extract model kwarg for test_metadata; the value from get_test_details already
    // includes {{ }} Jinja delimiters (e.g. "{{ get_where_subquery(ref('...')) }}").
    let test_metadata_model = kwargs
        .get("model")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // If the test name was truncated, get the original name from the truncations map
    let original_name = test_name_truncations.get(&full_name).cloned();

    // Convert full kwargs to dbt_yaml::Value for storage in the manifest.
    // Excludes "config" and "_config_raw" which are FS-internal config representations,
    // not macro arguments (mirrors dbt-core which pops config keys before storing kwargs).
    let test_metadata_kwargs: BTreeMap<String, dbt_yaml::Value> = restored_kwargs
        .into_iter()
        .filter(|(k, _)| k.as_str() != "config" && k.as_str() != "_config_raw")
        .filter_map(|(k, v)| {
            serde_json::from_value::<dbt_yaml::Value>(v)
                .ok()
                .map(|yml_v| (k, yml_v))
        })
        .collect();

    let raw_code_config = raw_code_config_from_test_details(&config, &kwargs)?;

    Ok(GenericTestAsset {
        dbt_asset,
        resource_name: test_config.resource_name.clone(),
        version: test_config.version_num.clone(),
        resource_type: test_config.resource_type.clone(),
        source_name: test_config.source_name.clone(),
        test_name: full_name,
        defined_at: test.span().clone().into(),
        test_metadata_name: meta_name,
        test_metadata_namespace: meta_namespace,
        column_name,
        test_metadata_column_name: column_name_kwarg,
        test_metadata_combination_of_columns: combination_of_columns,
        test_metadata_model,
        test_metadata_kwargs,
        raw_code_config,
        original_name,
        unique_id_hash: Some(test_hash),
        column_tags: column_tags.to_vec(),
        unrendered_schema_config,
        description: test.description().map(|s| s.to_string()),
    })
}

#[derive(Debug, Clone)]
struct TestDetails {
    test_macro_name: String,
    custom_test_name: Option<String>,
    kwargs: BTreeMap<String, Value>,
    namespace: Option<String>,
    config: Option<DataTestConfig>,
    jinja_set_vars: BTreeMap<String, String>,
}

fn get_test_details(
    test: &DataTests,
    test_config: &GenericTestConfig,
    column_name_kwarg: Option<StringOrArrayOfStrings>,
    dependency_package_name: Option<&str>,
    suppress_deprecated_test_validation: bool,
    legacy_syntax_handling: LegacyTestSyntaxHandling,
    explicit_config_keys: &BTreeSet<String>,
) -> FsResult<TestDetails> {
    let mut kwargs = BTreeMap::new();
    let mut config: Option<DataTestConfig> = None;
    let mut jinja_set_vars = BTreeMap::new();

    // Common kwargs for all tests
    // Determine the model string based on the resource type
    let model_string = match test_config.resource_type.as_str() {
        "source" => {
            if let Some(source_name) = &test_config.source_name {
                format!(
                    "source('{}', '{}')",
                    source_name, &test_config.resource_name
                )
            } else {
                return err!(
                    ErrorCode::DbtYamlValidationError,
                    "Source identifiers are missing for a source resource",
                );
            }
        }
        _ => {
            if let Some(ref version_num) = test_config.version_num {
                format!(
                    "ref('{}', version='{}')",
                    &test_config.resource_name, version_num
                )
            } else {
                format!("ref('{}')", &test_config.resource_name)
            }
        }
    };

    kwargs.insert(
        "model".to_string(),
        Value::String(format!("{{{{ get_where_subquery({model_string}) }}}}")),
    );
    if let Some(col) = column_name_kwarg {
        kwargs.insert("column_name".to_string(), col.to_json_value());
    }

    let (test_macro_name, mut custom_test_name, namespace) = match test {
        DataTests::String(test_name) => {
            let (test_macro_name, namespace) = parse_test_name_and_namespace(test_name);
            (test_macro_name, None, namespace)
        }
        DataTests::CustomTest(custom_test) => match custom_test.as_ref() {
            CustomTest::MultiKey(mk) => {
                let (test_name, namespace) = parse_test_name_and_namespace(&mk.test_name);
                let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
                    &mk.arguments,
                    &mk.__deprecated_args_and_configs__,
                    &mk.config,
                    dependency_package_name,
                    suppress_deprecated_test_validation,
                    legacy_syntax_handling,
                    explicit_config_keys,
                )?;
                kwargs.extend(extraction_result.kwargs);
                jinja_set_vars.extend(extraction_result.jinja_set_vars);
                config = extraction_result.config;
                (test_name, mk.name.clone(), namespace)
            }
            CustomTest::SimpleKeyValue(sk) => {
                if sk.len() != 1 {
                    return err!(
                        ErrorCode::DbtYamlValidationError,
                        "Simple key-value custom test must contain exactly one test"
                    );
                }
                let (full_name, inner) = sk.iter().next().unwrap();
                let (test_name, namespace) = parse_test_name_and_namespace(full_name);

                let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
                    &inner.arguments,
                    &inner.__deprecated_args_and_configs__,
                    &inner.config,
                    dependency_package_name,
                    suppress_deprecated_test_validation,
                    legacy_syntax_handling,
                    explicit_config_keys,
                )?;
                kwargs.extend(extraction_result.kwargs);
                jinja_set_vars.extend(extraction_result.jinja_set_vars);
                config = extraction_result.config;
                (test_name, inner.name.clone(), namespace)
            }
        },
    };

    // `name` is reserved in dbt-core generic tests: it names the test node, but is not a macro kwarg.
    // In some YAML shapes it may show up in the parsed args map; we treat it as the custom test name
    // if one wasn't already provided and always remove it from macro kwargs.
    extract_reserved_name_kwarg(&mut custom_test_name, &mut kwargs)?;

    Ok(TestDetails {
        test_macro_name: normalize_test_name(&test_macro_name)?,
        custom_test_name,
        kwargs,
        namespace,
        config,
        jinja_set_vars,
    })
}

fn extract_reserved_name_kwarg(
    custom_test_name: &mut Option<String>,
    kwargs: &mut BTreeMap<String, Value>,
) -> FsResult<()> {
    let Some(v) = kwargs.remove("name") else {
        return Ok(());
    };

    match v {
        Value::String(s) => {
            if custom_test_name.is_none() {
                *custom_test_name = Some(s);
            }
            Ok(())
        }
        other => err!(
            ErrorCode::DbtYamlValidationError,
            "Generic test 'name' must be a string (got {other})"
        ),
    }
}

/// Result of extracting kwargs and jinja variables
#[derive(Debug)]
struct KwargsExtractionResult {
    kwargs: BTreeMap<String, Value>,
    jinja_set_vars: BTreeMap<String, String>,
    config: Option<DataTestConfig>,
}

/// Simplified extraction of kwargs and Jinja variables for strongly typed custom tests.
/// `explicit_config_keys` are the keys written literally under this test's `config:` block.
#[allow(clippy::cognitive_complexity)]
fn extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
    arguments: &Verbatim<Option<dbt_yaml::Value>>,
    deprecated_args_and_configs: &Verbatim<BTreeMap<String, dbt_yaml::Value>>,
    existing_config: &Option<DataTestConfig>,
    dependency_package_name: Option<&str>,
    suppress_deprecated_test_validation: bool,
    legacy_syntax_handling: LegacyTestSyntaxHandling,
    explicit_config_keys: &BTreeSet<String>,
) -> FsResult<KwargsExtractionResult> {
    // Start with existing config
    let mut final_config = existing_config.clone();
    let mut combined_args = BTreeMap::new();
    // Config-like keys that appear under `arguments:` should be treated as config, not macro kwargs.
    // We'll attach them to an embedded `config` kwarg object so `generate_test_macro` emits
    // a `{{ config(...) }}` block rather than passing them into the test macro.
    let mut config_from_arguments: BTreeMap<String, Value> = BTreeMap::new();
    let mut config_from_deprecated = BTreeMap::new();

    // Process arguments parameter
    if let Some(args) = &arguments.0 {
        let json_value = serde_json::to_value(args.clone()).unwrap_or(Value::Null);
        if let Value::Object(map) = json_value {
            for (key, value) in map {
                if CONFIG_ARGS.contains(&key.as_str()) {
                    config_from_arguments.insert(key, value);
                } else {
                    combined_args.insert(key, value);
                }
            }
        }
    }

    // Process deprecated_args_and_configs
    let deprecated = &deprecated_args_and_configs.0;
    if !deprecated.is_empty() {
        let config_keys = extract_config_keys_from_map(deprecated);
        let arg_keys: Vec<String> = deprecated
            .keys()
            .filter(|key| !CONFIG_ARGS.contains(&key.as_str()))
            .cloned()
            .collect();

        let message = if !config_keys.is_empty() && !arg_keys.is_empty() {
            format!(
                "Deprecated test configs: {config_keys:?} and arguments: {arg_keys:?} at top-level detected. Please migrate to the new format: https://docs.getdbt.com/reference/deprecations#missingargumentspropertyingenerictestdeprecation."
            )
        } else if !config_keys.is_empty() {
            format!(
                "Deprecated test configs: {config_keys:?} at top-level detected. Please migrate under the 'config' field."
            )
        } else {
            format!(
                "Deprecated test arguments: {arg_keys:?} at top-level detected. Please migrate to the new format under the 'arguments' field: https://docs.getdbt.com/reference/deprecations#missingargumentspropertyingenerictestdeprecation."
            )
        };

        let schema_error = fs_err!(
            code => ErrorCode::DbtYamlValidationError,
            loc => deprecated.iter().next().map(|(_, v)| v.span().clone()).unwrap_or_default(),
            "{}",
            message
        );

        if !suppress_deprecated_test_validation {
            match legacy_syntax_handling {
                LegacyTestSyntaxHandling::Strict => {
                    emit_strict_parse_error(*schema_error, dependency_package_name);
                }
                LegacyTestSyntaxHandling::Warn => emit_warn_log_from_fs_error(*schema_error),
            }
        }
    }
    for (key, value) in deprecated.clone() {
        let json_value = serde_json::to_value(value.clone()).unwrap_or(Value::Null);

        if CONFIG_ARGS.contains(&key.as_str()) {
            config_from_deprecated.insert(key.clone(), json_value);
        } else {
            // It's an argument, add to combined args
            combined_args.insert(key.clone(), json_value);
        }
    }

    // Merge configs at JSON level, then deserialize once
    if !config_from_deprecated.is_empty() {
        // Convert existing config to JSON if it exists
        let existing_config_json = if let Some(ref existing) = final_config {
            serde_json::to_value(existing).unwrap_or_else(|_| Value::Object(serde_json::Map::new()))
        } else {
            Value::Object(serde_json::Map::new())
        };

        // A conflict requires the key to be written literally under `config:`; `DataTestConfig`
        // serializes some unset fields as `null`/`[]`/`{}`, so the serialized form cannot tell.
        if let Value::Object(existing_map) = &existing_config_json {
            for key in config_from_deprecated.keys() {
                if explicit_config_keys.contains(key)
                    && existing_map.get(key).is_some_and(|v| !v.is_null())
                {
                    return err!(
                        ErrorCode::DbtYamlValidationError,
                        "Test cannot have the same key '{}' at the top-level and in config",
                        key
                    );
                }
            }
        }

        // Merge the JSON objects - deprecated config takes precedence
        let mut merged_config_json = existing_config_json;
        if let Value::Object(ref mut merged_map) = merged_config_json {
            merged_map.extend(config_from_deprecated);
        }

        let merged_config_yaml: dbt_yaml::Value =
            serde_json::from_value(merged_config_json).unwrap();

        // Deserialize the final merged config
        if let Ok(merged_config) = serde::Deserialize::deserialize(merged_config_yaml) {
            final_config = Some(merged_config);
        }
    }

    // Check for reserved "model" argument in combined args
    if combined_args.contains_key("model") {
        return err!(
            ErrorCode::DbtYamlValidationError,
            "Test arguments include \"model\", which is a reserved argument",
        );
    }

    let mut kwargs = BTreeMap::new();
    let mut jinja_set_vars = BTreeMap::new();

    // Process all combined args for jinja vars
    for (key, value) in combined_args {
        let (kwarg_value, jinja_vars) = process_kwarg(&key, &value);
        kwargs.insert(key, kwarg_value);
        for (var_name, var_value) in jinja_vars {
            jinja_set_vars.insert(var_name, var_value);
        }
    }

    // If config-like keys were provided under arguments, merge them into an embedded `config` kwarg
    // so they are emitted via `{{ config(...) }}` and not passed to the test macro.
    if !config_from_arguments.is_empty() {
        match kwargs.remove("config") {
            Some(Value::Object(existing_obj)) => {
                // Preserve explicit `config:` object values by letting them override extracted keys.
                let mut merged = serde_json::Map::new();
                for (k, v) in config_from_arguments {
                    merged.insert(k, v);
                }
                for (k, v) in existing_obj {
                    merged.insert(k, v);
                }
                kwargs.insert("config".to_string(), Value::Object(merged));
            }
            Some(other) => {
                // Unexpected type; keep it as-is and also add extracted config under a proper map.
                // This preserves previous behavior while ensuring config keys still take effect.
                kwargs.insert(
                    "config".to_string(),
                    Value::Object(
                        config_from_arguments
                            .into_iter()
                            .collect::<serde_json::Map<String, Value>>(),
                    ),
                );
                kwargs.insert("_config_raw".to_string(), other);
            }
            None => {
                kwargs.insert(
                    "config".to_string(),
                    Value::Object(
                        config_from_arguments
                            .into_iter()
                            .collect::<serde_json::Map<String, Value>>(),
                    ),
                );
            }
        }
    }

    Ok(KwargsExtractionResult {
        kwargs,
        jinja_set_vars,
        config: final_config,
    })
}

/// Config field names that can appear in test configurations
static CONFIG_ARGS: &[&str] = &[
    "enabled",
    "severity",
    "tags",
    "warn_if",
    "error_if",
    "fail_calc",
    "where",
    "limit",
    "alias",
    "database",
    "schema",
    "group",
    "meta",
    "store_failures",
    "store_failures_as",
    "quoting",
    "static_analysis",
    "sql_header",
];

/// Extract config keys from a BTreeMap, filtering to only include valid config fields
fn extract_config_keys_from_map(deprecated_map: &BTreeMap<String, dbt_yaml::Value>) -> Vec<String> {
    deprecated_map
        .keys()
        .filter(|key| CONFIG_ARGS.contains(&key.as_str()))
        .cloned()
        .collect()
}

/// Helper function to process a kwarg value and detect if it needs a Jinja set block.
/// Returns (kwarg_value, jinja_vars).
fn process_kwarg(key: &str, value: &Value) -> (Value, Vec<(String, String)>) {
    match value {
        Value::String(s) => {
            if needs_jinja_set_block(s) {
                // Generate a unique var name based on the key with a prefix to avoid collisions
                let var_name = format!("dbt_custom_arg_{key}");
                let jinja_var = vec![(var_name.clone(), s.clone())];
                let kwarg_value = Value::String(var_name);
                (kwarg_value, jinja_var)
            } else {
                // For simple values, just use the value directly
                (value.clone(), vec![])
            }
        }
        Value::Array(arr) => {
            // Process each array element
            let mut new_array = Vec::new();
            let mut all_jinja_vars = Vec::new();

            for (idx, elem) in arr.iter().enumerate() {
                let elem_key = format!("{key}_{idx}");
                let (processed_elem, elem_jinja_vars) = process_kwarg(&elem_key, elem);
                new_array.push(processed_elem);
                all_jinja_vars.extend(elem_jinja_vars);
            }

            (Value::Array(new_array), all_jinja_vars)
        }
        Value::Object(obj) => {
            // Process each object value
            let mut new_object = serde_json::Map::new();
            let mut all_jinja_vars = Vec::new();

            for (obj_key, obj_val) in obj {
                let nested_key = format!("{key}_{obj_key}");
                let (processed_val, val_jinja_vars) = process_kwarg(&nested_key, obj_val);
                new_object.insert(obj_key.clone(), processed_val);
                all_jinja_vars.extend(val_jinja_vars);
            }

            (Value::Object(new_object), all_jinja_vars)
        }
        _ => {
            // For other non-string values (numbers, bools, null), use as is
            (value.clone(), vec![])
        }
    }
}

fn process_config_jinja(
    value: &Value,
    jinja_set_vars: &BTreeMap<String, String>,
) -> (Value, Vec<(String, String)>) {
    fn collect_string_values(value: &Value, values: &mut HashSet<String>) {
        match value {
            Value::String(value) => {
                values.insert(value.clone());
            }
            Value::Array(items) => {
                for item in items {
                    collect_string_values(item, values);
                }
            }
            Value::Object(items) => {
                for item in items.values() {
                    collect_string_values(item, values);
                }
            }
            _ => {}
        }
    }

    fn visit(
        value: &Value,
        next_id: &mut usize,
        reserved_names: &mut HashSet<String>,
    ) -> (Value, Vec<(String, String)>) {
        match value {
            Value::String(value) if value.contains("{{") && value.contains("}}") => {
                let variable_name = loop {
                    let candidate = format!("dbt_custom_config_{next_id}");
                    *next_id += 1;
                    if reserved_names.insert(candidate.clone()) {
                        break candidate;
                    }
                };
                (
                    Value::String(variable_name.clone()),
                    vec![(variable_name, value.clone())],
                )
            }
            Value::Array(values) => {
                let mut variables = Vec::new();
                let values = values
                    .iter()
                    .map(|value| {
                        let (value, nested_variables) = visit(value, next_id, reserved_names);
                        variables.extend(nested_variables);
                        value
                    })
                    .collect();
                (Value::Array(values), variables)
            }
            Value::Object(values) => {
                let mut variables = Vec::new();
                let values = values
                    .iter()
                    .map(|(key, value)| {
                        let (value, nested_variables) = visit(value, next_id, reserved_names);
                        variables.extend(nested_variables);
                        (key.clone(), value)
                    })
                    .collect();
                (Value::Object(values), variables)
            }
            _ => (value.clone(), Vec::new()),
        }
    }

    let mut reserved_names: HashSet<String> = jinja_set_vars.keys().cloned().collect();
    collect_string_values(value, &mut reserved_names);
    let mut next_id = 0;
    visit(value, &mut next_id, &mut reserved_names)
}

/// Reverses `process_kwarg` placeholder substitution so hash/manifest values match
/// dbt-core, which operates on the raw unrendered Jinja string, not `dbt_custom_arg_*`.
fn restore_jinja_placeholders(
    kwargs: &BTreeMap<String, Value>,
    jinja_set_vars: &BTreeMap<String, String>,
) -> BTreeMap<String, Value> {
    kwargs
        .iter()
        .map(|(k, v)| {
            let restored = restore_value(v, jinja_set_vars);
            (k.clone(), restored)
        })
        .collect()
}

fn restore_value(value: &Value, jinja_set_vars: &BTreeMap<String, String>) -> Value {
    match value {
        Value::String(s) => {
            if let Some(original) = jinja_set_vars.get(s) {
                Value::String(original.clone())
            } else {
                value.clone()
            }
        }
        Value::Array(arr) => Value::Array(
            arr.iter()
                .map(|v| restore_value(v, jinja_set_vars))
                .collect(),
        ),
        Value::Object(obj) => Value::Object(
            obj.iter()
                .map(|(k, v)| (k.clone(), restore_value(v, jinja_set_vars)))
                .collect(),
        ),
        _ => value.clone(),
    }
}

/// Determines if a string value needs to be wrapped in a Jinja set block
fn needs_jinja_set_block(value: &str) -> bool {
    // Check for multi-line content
    if value.contains('\n') {
        return true;
    }

    // Check for Jinja expressions
    if value.contains("{{") && value.contains("}}") {
        return true;
    }

    false
}

fn parse_test_name_and_namespace(test_name: &str) -> (String, Option<String>) {
    if let Some((package, test_name)) = test_name.split_once('.') {
        (test_name.to_owned(), Some(package.to_owned()))
    } else {
        (test_name.to_owned(), None)
    }
}

/// Recursively merge two YAML values with rhs overriding lhs on key collisions.
/// - For mappings: merge entries; when both sides have a mapping for a key, merge recursively,
///   otherwise rhs replaces lhs for that key. The resulting mapping keeps the lhs span.
/// - For sequences and scalars (or differing kinds): rhs replaces lhs entirely.
fn merge_yaml_values(lhs: dbt_yaml::Value, rhs: dbt_yaml::Value) -> (bool, dbt_yaml::Value) {
    use dbt_yaml::Value as Y;
    match (lhs, rhs) {
        (Y::Mapping(mut lm, lspan), Y::Mapping(rm, _rspan)) => {
            for (rk, rv) in rm.into_iter() {
                if let Some(existing) = lm.get_mut(&rk) {
                    let (_, merged) = merge_yaml_values(std::mem::take(existing), rv);
                    lm.insert(rk, merged);
                } else {
                    lm.insert(rk, rv);
                }
            }
            (!lm.is_empty(), Y::Mapping(lm, lspan))
        }
        // For sequences/scalars or differing kinds, rhs replaces lhs
        (_l, r) => (true, r),
    }
}

fn raw_code_config_from_test_details(
    config: &Option<DataTestConfig>,
    kwargs: &BTreeMap<String, Value>,
) -> FsResult<BTreeMap<String, dbt_yaml::Value>> {
    let (_, merged) = merged_test_config(config, kwargs)?;
    match merged {
        dbt_yaml::Value::Mapping(map, _) => Ok(map
            .into_iter()
            .filter_map(|(k, v)| k.as_str().map(|s| (s.to_string(), v)))
            .collect()),
        _ => Ok(BTreeMap::new()),
    }
}

fn merged_test_config(
    config: &Option<DataTestConfig>,
    kwargs: &BTreeMap<String, Value>,
) -> FsResult<(bool, dbt_yaml::Value)> {
    let passed_in_cfg = if let Some(cfg) = config {
        let cfg_val = dbt_yaml::to_value(cfg).map_err(|e| yaml_to_fs_error(e, None))?;
        dbt_schemas::schemas::serialization_utils::serialize_with_mode(
            &cfg_val,
            dbt_schemas::schemas::serialization_utils::SerializationMode::OmitNone,
        )
    } else {
        dbt_yaml::Value::Mapping(dbt_yaml::Mapping::new(), Span::default())
    };

    let embedded_cfg = match kwargs.get("config") {
        Some(Value::Object(obj)) => {
            serde_json::from_value(Value::Object(obj.clone())).map_err(|e| {
                fs_err!(
                    ErrorCode::DbtYamlValidationError,
                    "Failed to convert embedded config: {}",
                    e
                )
            })?
        }
        _ => dbt_yaml::Value::Mapping(dbt_yaml::Mapping::new(), Span::default()),
    };

    Ok(merge_yaml_values(passed_in_cfg, embedded_cfg))
}

static CLEAN_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[^0-9a-zA-Z_]+").expect("valid regex"));

/// Narrow sanitizer for the test-name segments that become a filename
/// (`source_name` / `resource_name`). The synthesized name is used directly
/// as `target/generic_tests/<name>.sql`, so only path separators must be
/// rewritten — anything else is left alone to avoid changing test names /
/// `unique_id`s for inputs that already worked (e.g. `prod.events`,
/// `my-source`).
static PATH_UNSAFE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[/\\]").expect("valid regex"));

/// Generates a unique hash for a generic test based on uncleaned kwargs.
/// This matches mantle's behavior where the unique_id includes a hash of the
/// test metadata (namespace, name, kwargs) WITHOUT cleaning, ensuring that
/// tests with different expressions (e.g., '> 0' vs '= 0') get different
/// unique_ids even if their cleaned names would be identical.
///
/// https://github.com/dbt-labs/dbt-core/blob/2ef17b836e39d1b4c7a55f14b448a2254378302e/core/dbt/parser/schema_generic_tests.py#L104-L117
fn generate_test_unique_id_hash(
    fqn_name: &str,
    test_macro_name: &str,
    namespace: Option<&String>,
    kwargs: &BTreeMap<String, Value>,
) -> String {
    const HASH_LENGTH: usize = 10;

    // Mantle builds test_metadata as:
    //   metadata = {"namespace": builder.namespace, "name": builder.name, "kwargs": builder.args}
    // Then computes:
    //   hashable_metadata = repr(get_hashable_md(test_metadata))
    //   hash_string = name + hashable_metadata
    //   test_hash = md5(hash_string)[-HASH_LENGTH:]
    //
    // get_hashable_md recursively processes:
    //   - dicts: sorted by keys, recursively processed
    //   - lists: recursively processed
    //   - other: str(value)

    // Build the test_metadata dict structure matching mantle
    let hashable_metadata = build_hashable_metadata_repr(test_macro_name, namespace, kwargs);

    // hash_string = name + hashable_metadata (name is fqn_name in mantle)
    let hash_string = format!("{}{}", fqn_name, hashable_metadata);

    // Compute MD5 hash and take last HASH_LENGTH characters
    let digest = md5::compute(hash_string.as_bytes());
    let hash_hex = format!("{:x}", digest);
    hash_hex[hash_hex.len() - HASH_LENGTH..].to_string()
}

/// Builds a Python repr()-like string of the test metadata dict.
/// https://github.com/dbt-labs/dbt-core/blob/2ef17b836e39d1b4c7a55f14b448a2254378302e/core/dbt/parser/schema_generic_tests.py#L104-L113
fn build_hashable_metadata_repr(
    test_macro_name: &str,
    namespace: Option<&String>,
    kwargs: &BTreeMap<String, Value>,
) -> String {
    use std::fmt::Write;

    // Build the metadata dict: {"kwargs": {...}, "name": "...", "namespace": "..."}
    let mut out = String::new();
    out.push_str("{'kwargs': ");

    // "kwargs" comes first alphabetically
    write_value_to_hashable_repr(
        &mut out,
        &Value::Object(kwargs.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
    );

    // "name" comes second
    let _ = write!(out, ", 'name': '{}'", test_macro_name);

    // "namespace" comes third (None is still represented)
    match namespace {
        Some(ns) => {
            let _ = write!(out, ", 'namespace': '{}'", ns);
        }
        None => out.push_str(", 'namespace': 'None'"),
    }

    out.push('}');
    out
}

/// Writes a serde_json Value as a Python repr()-like string to a buffer.
/// https://github.com/dbt-labs/dbt-core/blob/2ef17b836e39d1b4c7a55f14b448a2254378302e/core/dbt/parser/schema_generic_tests.py#L104-L113
fn write_value_to_hashable_repr(out: &mut String, value: &Value) {
    use std::fmt::Write;

    match value {
        Value::Object(map) => {
            // Sort keys alphabetically (serde_json::Map may not be sorted)
            let mut sorted_entries: Vec<_> = map.iter().collect();
            sorted_entries.sort_by(|a, b| a.0.cmp(b.0));

            out.push('{');
            for (i, (k, v)) in sorted_entries.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                let _ = write!(out, "'{}': ", k);
                write_value_to_hashable_repr(out, v);
            }
            out.push('}');
        }
        Value::Array(arr) => {
            out.push('[');
            for (i, v) in arr.iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                write_value_to_hashable_repr(out, v);
            }
            out.push(']');
        }
        Value::String(s) => {
            // Match Python's repr(): use double quotes when the string contains single quotes,
            // otherwise use single quotes.
            if s.contains('\'') {
                let _ = write!(out, "\"{}\"", s);
            } else {
                let _ = write!(out, "'{}'", s);
            }
        }
        // dbt-core's get_hashable_md recursively wraps every leaf primitive with `str()`
        // before `repr()`-ing the metadata, so leaves appear in the hash input as quoted
        // Python strings (e.g. `'True'`, `'-90'`, `'None'`) — not raw types. Match that.
        Value::Number(n) => {
            let _ = write!(out, "'{}'", n);
        }
        Value::Bool(b) => {
            out.push_str(if *b { "'True'" } else { "'False'" });
        }
        Value::Null => out.push_str("'None'"),
    }
}

//https://github.com/dbt-labs/dbt-core/blob/31881d2a3bea030e700e9df126a3445298385698/core/dbt/parser/generic_test_builders.py#L26
/// Generates a test name and alias for a generic test.
///
/// * `test_name` - Name of the test (e.g. "unique", "not_null", etc)
/// * `is_custom_test_name` - Whether a custom name was provided for this test
#[allow(clippy::too_many_arguments)]
fn generate_test_name(
    test_macro_name: &str,
    custom_test_name: Option<String>,
    project_name: &str,
    test_config: &GenericTestConfig,
    kwargs: &BTreeMap<String, Value>,
    package_name: Option<&String>,
    jinja_set_vars: &BTreeMap<String, String>,
    test_name_truncations: &mut HashMap<String, String>,
) -> String {
    // If a custom test name is provided, use it directly for the display name
    if let Some(custom_test_name) = custom_test_name {
        return custom_test_name;
    }

    // Flatten args (excluding 'model' and config args)
    let mut flat_args = Vec::new();
    for (arg_name, arg_val) in kwargs.iter().sorted_by(|a, b| a.0.cmp(b.0)) {
        // Skip 'model' argument
        if arg_name == "model" {
            continue;
        }

        // Skip nested 'config' section to match dbt-core behavior
        if arg_name == "config" {
            continue;
        }

        let actual_value = restore_value(arg_val, jinja_set_vars);

        // Match dbt-core's `str(value)` semantics for leaf primitives: Python renders
        // `True`/`False`/`None`, not the JSON forms `true`/`false`/`null`. This affects
        // the synthesized test name (and therefore the unique_id hash) — diverging here
        // breaks `state:modified` parity against Mantle.
        let render = |v: &Value| -> String {
            match v {
                Value::Bool(true) => "True".to_string(),
                Value::Bool(false) => "False".to_string(),
                Value::Null => "None".to_string(),
                _ => v.to_string(),
            }
        };
        let parts = match &actual_value {
            Value::Object(map) => map.values().map(&render).collect::<Vec<_>>(),
            Value::Array(arr) => arr.iter().map(&render).collect(),
            _ => vec![render(&actual_value)],
        };

        flat_args.extend(parts);
    }

    // Clean args to only allow alphanumeric and underscore
    let clean_flat_args: Vec<String> = flat_args
        .iter()
        .map(|arg| {
            // `Value::to_string()` may escape newlines into the two-character sequence `\n`.
            // Treat both escaped and literal newlines as whitespace so we don't end up with
            // artifacts like `_nand_` from `\nand`.
            let normalized = arg
                .trim_matches('"')
                .replace("\\r\\n", " ")
                .replace("\\n", " ")
                .replace("\\r", " ")
                .replace(['\n', '\r'], " ");
            CLEAN_REGEX.replace_all(&normalized, "_").to_string()
        })
        .collect();

    // Join args with double underscores - empty string if no args
    let suffix = if !clean_flat_args.is_empty() {
        clean_flat_args.join("__")
    } else {
        String::new()
    };

    // Build the test name from here.
    //
    // The synthesized name is also used as the SQL filename written under
    // `target/generic_tests/`, so path separators in source / resource names
    // (e.g. `raw/data/table`) would otherwise be interpreted as directory
    // boundaries and cause the write to fail. Sanitize only those — every
    // other character is preserved so test names and `unique_id`s for
    // already-working inputs (e.g. `prod.events`, `my-source`) don't drift.
    let (prefix, resource_name) = match &test_config.source_name {
        Some(source_name) => {
            // handles the test from a source model
            let safe_source = PATH_UNSAFE_REGEX.replace_all(source_name, "_");
            let safe_resource = PATH_UNSAFE_REGEX.replace_all(&test_config.resource_name, "_");
            (
                format!("source_{test_macro_name}"),
                format!("{safe_source}_{safe_resource}"),
            )
        }
        None => (
            test_macro_name.to_string(),
            PATH_UNSAFE_REGEX
                .replace_all(&test_config.resource_name, "_")
                .into_owned(),
        ),
    };

    let test_identifier = match &test_config.version_num {
        Some(version_num) => format!("{prefix}_{resource_name}_v{version_num}"),
        None => format!("{prefix}_{resource_name}"),
    };

    // In dbt-core, namespaced tests (e.g. `elementary.schema_changes_from_baseline`) include the
    // namespace prefix in the synthesized name. When truncation applies, the preserved prefix
    // should also include that namespace (Mantle behavior).
    let identifier_for_name = match package_name {
        Some(pkg_name) if pkg_name != project_name => format!("{pkg_name}_{test_identifier}"),
        _ => test_identifier,
    };

    let result = format!("{identifier_for_name}_{suffix}");

    // dbt-core truncates the test name to 63 characters, if the
    // full name is too long. This is done by including the first
    // 30 identifying chars plus a 32-character hash of the full contents
    // See the function `synthesize_generic_test_name` in `dbt-core`:
    // https://github.com/dbt-labs/dbt-core/blob/9010537499980743503ed3b462eb1952be4d2b38/core/dbt/parser/generic_test_builders.py
    let truncated = maybe_truncate_test_name(&identifier_for_name, &result);
    if truncated != result {
        // First write wins to guard against churn if called multiple times.
        test_name_truncations
            .entry(truncated.clone())
            .or_insert_with(|| result.clone());
    }
    truncated
}

/// Represents test configuration for a model version
#[derive(Debug, Clone)]
struct GenericTestConfig {
    resource_type: String,
    resource_name: String,
    version_num: Option<String>,
    model_tests: Option<Vec<DataTests>>,
    column_tests: Option<BTreeMap<String, ColumnTestEntry>>,
    source_name: Option<String>,
}

/// Generates the Jinja macro call for a generic test
#[allow(clippy::too_many_arguments)]
fn generate_test_macro(
    test_macro_name: &str,
    kwargs: &BTreeMap<String, Value>,
    namespace: Option<&str>,
    config: &Option<DataTestConfig>,
    jinja_set_vars: &BTreeMap<String, String>,
) -> FsResult<String> {
    let (non_empty, cfg_json) = merged_test_config(config, kwargs)?;
    let cfg_serde: Value = serde_json::to_value(&cfg_json).map_err(|e| {
        fs_err!(
            ErrorCode::DbtYamlValidationError,
            "Failed to serialize config: {}",
            e
        )
    })?;
    let (cfg_serde, config_jinja_vars) = process_config_jinja(&cfg_serde, jinja_set_vars);

    let mut render_jinja_set_vars = jinja_set_vars.clone();
    render_jinja_set_vars.extend(config_jinja_vars);

    // Add Jinja set blocks at the beginning of the file
    let mut sql = String::new();
    for (var_name, var_value) in &render_jinja_set_vars {
        let set_val = if check_single_expression_without_whitepsace_control(var_value) {
            format!(
                "{{% set {} = {} %}}\n\n",
                var_name,
                &var_value[2..var_value.len() - 2].trim()
            )
        } else {
            format!("{{% set {var_name} -%}}\n{var_value}\n{{%- endset %}}\n\n")
        };
        sql.push_str(&set_val);
    }

    // ── serialize & emit the config block ────────────────
    // Compute the config block emit ahead of the macro call so we can place it
    // *after* the macro call below (matching dbt-Core). dbt's `config()` is
    // resolution-order sensitive — multiple calls merge with last-call-wins —
    // so when a custom test macro internally calls `{{ config(severity=...) }}`
    // and the user also supplies an embedded `config:` block, Core's order
    // (macro call then embedded config) lets the embedded block win. Fusion
    // previously emitted the embedded block first, which inverted precedence.
    //
    // Config strings can themselves contain Jinja. Route those values through
    // generated set variables so they render before `config(...)` receives them.
    let config_emit = if non_empty {
        Some(format_value_for_jinja(&cfg_serde, &render_jinja_set_vars))
    } else {
        None
    };

    // Build test macro call with namespace
    // dbt allows referencing a macro of test_<name> using just <name> in data_tests
    // via the qualified_name prefix using 'test_'
    let qualified_name = if let Some(ns) = namespace {
        format!("{ns}.test_{test_macro_name}")
    } else {
        format!("test_{test_macro_name}")
    };

    // Format all kwargs, handling ref calls specially
    // Exclude an embedded 'config' kwarg as it is emitted via config(...) below
    // Exclude reserved 'name' kwarg (used to name the test node, not passed to the macro).
    let formatted_args: Vec<String> = kwargs
        .iter()
        .filter(|(k, _)| k.as_str() != "config" && k.as_str() != "name")
        .map(|(k, v)| {
            let value_str = format_value_for_jinja(v, jinja_set_vars);
            format!("{k}={value_str}")
        })
        .collect();
    sql.push_str(&format!(
        "{{{{ {}({}) }}}}",
        qualified_name,
        formatted_args.join(", ")
    ));
    if let Some(config_str) = config_emit {
        sql.push_str(&format!("{{{{ config({config_str}) }}}}"));
    }
    Ok(sql)
}

impl<T> TryFrom<&TestableNode<'_, T>> for Vec<GenericTestConfig>
where
    T: TestableNodeTrait,
{
    // TODO this is currently infallible, we could implement From instead
    type Error = Box<FsError>;

    fn try_from(value: &TestableNode<T>) -> Result<Self, Self::Error> {
        let base = GenericTestConfig {
            resource_type: value.inner.resource_type().to_owned(),
            resource_name: value.inner.resource_name().to_owned(),
            version_num: None,
            model_tests: value.inner.base_tests()?,
            column_tests: value.inner.column_tests()?,
            source_name: value.inner.source_name(),
        };
        if let Some(versions) = value.inner.versions() {
            collect_versioned_model_tests(&base, versions)
        } else {
            Ok(vec![base])
        }
    }
}

// Given a model def from a properties file, and a list of versions,
// collect all the tests for each version and return a map of versioned model names to test configs
fn collect_versioned_model_tests(
    base_test_config: &GenericTestConfig,
    versions: &[Versions],
) -> FsResult<Vec<GenericTestConfig>> {
    let mut version_tests = vec![];
    // For each version, merge base tests with version-specific tests
    for version in versions {
        let Some(version_suffix) = version.get_version() else {
            return err!(
                ErrorCode::InvalidConfig,
                "Version '{:?}' does not meet the required format",
                version.v
            );
        };

        // Start with base tests but set the version number
        let mut version_config = base_test_config.clone();
        version_config.version_num = Some(version_suffix.to_string());

        // In properties files, version tests may be specified via either `tests` or `data_tests`.
        // Treat them equivalently (same as model-level and column-level tests).
        if let Some(version_tests) =
            base_tests_inner(version.tests.as_deref(), version.data_tests.as_deref())?
        {
            version_config.model_tests = Some(version_tests);
        }

        // Handle version-specific column tests and inheritance
        if let Some(columns) = version.columns.as_deref() {
            let mut column_tests = if let Some(inheritance_rules) =
                ColumnInheritanceRules::from_version_column_props(columns)
            {
                // Apply inheritance rules
                base_test_config
                    .column_tests
                    .as_ref()
                    .map(|base_column_tests| {
                        base_column_tests
                            .iter()
                            .filter_map(|(col_name, tests)| {
                                if inheritance_rules.should_include_column(col_name) {
                                    Some((col_name.clone(), tests.clone()))
                                } else {
                                    None
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                // No inheritance rules specified - inherit all column tests
                base_test_config.column_tests.clone().unwrap_or_default()
            };

            // Then handle any explicit column test definitions. Directive-only entries
            // (`include:`/`exclude:`, no `name:`) yield `None` and are skipped here -- they were
            // already consumed as inheritance rules above.
            for col in columns
                .iter()
                .filter_map(VersionColumnProperties::to_column_properties)
            {
                if col.tests.is_some() && col.data_tests.is_some() {
                    return err!(
                        ErrorCode::InvalidConfig,
                        "Cannot have both 'tests' and 'data_tests' defined"
                    );
                }
                // In properties files, column tests may be specified via either `tests` or
                // `data_tests`. Treat them equivalently (same as non-versioned columns).
                if let Some(tests) = col.tests.as_ref().or(col.data_tests.as_ref()) {
                    let tags = col
                        .config
                        .as_ref()
                        .and_then(|c| c.tags.clone())
                        .map(|t| t.into())
                        .unwrap_or_default();
                    column_tests.insert(
                        col.name.clone(),
                        ColumnTestEntry {
                            quote: col.quote.unwrap_or(false),
                            tests: tests.clone(),
                            tags,
                            // Version-local column tests were ignored before they were collected
                            // from the typed `versions[].columns` field. Keep their legacy syntax
                            // warning-only so discovering the tests does not break existing projects.
                            legacy_syntax_handling: LegacyTestSyntaxHandling::Warn,
                        },
                    );
                }
            }

            // Always assign even when empty: a version with `columns` that excludes
            // all testable columns should produce zero tests, not fall back to the
            // base config (which still carries the excluded column tests from the clone).
            version_config.column_tests = Some(column_tests);
        } else {
            // No columns section at all - inherit all column tests
            version_config.column_tests = base_test_config.column_tests.clone();
        }

        // Use versioned name as key
        version_tests.push(version_config);
    }
    Ok(version_tests)
}

/// Format a node `unique_id` the way dbt-core does.
///
/// Shapes (mirrors `RefableLookup.get_unique_id` and source unique_id format):
/// - versioned model:    `model.<pkg>.<name>.v<version>`
/// - source:             `source.<pkg>.<source_name>.<name>`
/// - everything else:    `<resource_type>.<pkg>.<name>`
///
/// `version` takes precedence over `source_name` (versioned sources don't exist).
pub fn format_node_unique_id(
    resource_type: &str,
    package_name: &str,
    resource_name: &str,
    source_name: Option<&str>,
    version: Option<&str>,
) -> String {
    if let Some(v) = version {
        format!("{resource_type}.{package_name}.{resource_name}.v{v}")
    } else if let Some(s) = source_name {
        format!("{resource_type}.{package_name}.{s}.{resource_name}")
    } else {
        format!("{resource_type}.{package_name}.{resource_name}")
    }
}

/// The minimal info we need to generate generic tests for a single dbt resource.
pub trait TestableNodeTrait {
    /// "model", "seed", "snapshot", or "source".
    fn resource_type(&self) -> &str;

    fn resource_name(&self) -> &str;

    fn unique_id(&self, project_name: &str, version: Option<&str>) -> String {
        format_node_unique_id(
            self.resource_type(),
            project_name,
            self.resource_name(),
            self.source_name().as_deref(),
            version,
        )
    }

    /// For _Tables from _Sources, return its corresponding source name.
    /// For everything else, return None.
    fn source_name(&self) -> Option<String> {
        None
    }

    /// Top-level tests (equivalent to "tests" or "data_tests").
    fn base_tests(&self) -> FsResult<Option<Vec<DataTests>>>;

    /// Columns, each with optional tests.
    fn column_tests(&self) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>>;

    /// Versions for models, or None for everything else.
    fn versions(&self) -> Option<&[Versions]> {
        None
    }

    fn as_testable(&self) -> TestableNode<'_, Self>
    where
        Self: Sized,
    {
        TestableNode { inner: self }
    }
}

impl TestableNodeTrait for ModelProperties {
    fn resource_type(&self) -> &str {
        "model"
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn base_tests(&self) -> FsResult<Option<Vec<DataTests>>> {
        base_tests_inner(self.tests.as_deref(), self.data_tests.as_deref())
    }

    fn column_tests(&self) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>> {
        column_tests_inner(&self.columns)
    }

    fn versions(&self) -> Option<&[Versions]> {
        self.versions.as_deref()
    }
}

impl TestableNodeTrait for SeedProperties {
    fn resource_type(&self) -> &str {
        "seed"
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn base_tests(&self) -> FsResult<Option<Vec<DataTests>>> {
        base_tests_inner(self.tests.as_deref(), self.data_tests.as_deref())
    }

    fn column_tests(&self) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>> {
        column_tests_inner(&self.columns)
    }
}

impl TestableNodeTrait for SnapshotProperties {
    fn resource_type(&self) -> &str {
        "snapshot"
    }

    fn resource_name(&self) -> &str {
        &self.name
    }

    fn base_tests(&self) -> FsResult<Option<Vec<DataTests>>> {
        base_tests_inner(self.tests.as_deref(), self.data_tests.as_deref())
    }

    fn column_tests(&self) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>> {
        column_tests_inner(&self.columns)
    }
}

/// _Tables doesn't know its source, so we wrap it in a struct that does.
pub struct TestableTable<'a> {
    pub source_name: String,
    pub table: &'a Tables,
}

impl TestableNodeTrait for TestableTable<'_> {
    fn resource_type(&self) -> &str {
        "source"
    }

    fn resource_name(&self) -> &str {
        &self.table.name
    }

    fn source_name(&self) -> Option<String> {
        Some(self.source_name.clone())
    }

    fn base_tests(&self) -> FsResult<Option<Vec<DataTests>>> {
        base_tests_inner(
            self.table.tests.as_deref(),
            self.table.data_tests.as_deref(),
        )
    }

    fn column_tests(&self) -> FsResult<Option<BTreeMap<String, ColumnTestEntry>>> {
        column_tests_inner(&self.table.columns)
    }
}

/// Normalizes a test name following the existing dbt behavior
/// https://github.com/dbt-labs/dbt-core/blob/main/core/dbt/parser/generic_test_builders.py#L121-L122
fn normalize_test_name(input: &str) -> FsResult<String> {
    let name_pattern = Regex::new(r"^([a-zA-Z_][0-9a-zA-Z_]*)+").expect("Valid test name pattern");
    name_pattern
        .captures(input)
        .and_then(|caps| caps.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| fs_err!(ErrorCode::InvalidConfig, "Invalid test name: {}", input))
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_schemas::schemas::data_tests::{CustomTestInner, CustomTestMultiKey};
    use serde_json::Value;
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn test_generic_test_asset_path_disambiguates_name_collisions() {
        // `not_null` on `orders.status_code` and on `orders_status.code` both flatten
        // to the same generated name; the second asset must not reuse the first path.
        let mut seen = HashMap::new();
        let first = generic_test_asset_path(&mut seen, "not_null_orders_status_code", "aaaaaaaaaa");
        let second =
            generic_test_asset_path(&mut seen, "not_null_orders_status_code", "bbbbbbbbbb");
        assert_eq!(
            first,
            PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME).join("not_null_orders_status_code.sql")
        );
        assert_eq!(
            second,
            PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME)
                .join("not_null_orders_status_code_bbbbbbbbbb.sql")
        );

        // Non-colliding names keep the plain (hash-free) path.
        let other = generic_test_asset_path(&mut seen, "not_null_orders_id", "cccccccccc");
        assert_eq!(
            other,
            PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME).join("not_null_orders_id.sql")
        );

        // Re-deriving the exact same test (same full_name AND same test_hash) is
        // idempotent, not a collision: versioned models resolve shared column tests
        // once per version SQL file, re-deriving identical paths on each pass.
        let rederived = generic_test_asset_path(&mut seen, "not_null_orders_id", "cccccccccc");
        assert_eq!(rederived, other);
    }

    #[test]
    fn test_format_node_unique_id_shapes() {
        // Versioned model wins over source_name (versioned sources don't exist).
        assert_eq!(
            format_node_unique_id("model", "pkg", "m", None, Some("1")),
            "model.pkg.m.v1"
        );
        // Unversioned model.
        assert_eq!(
            format_node_unique_id("model", "pkg", "m", None, None),
            "model.pkg.m"
        );
        // Source: source_name slot.
        assert_eq!(
            format_node_unique_id("source", "pkg", "tbl", Some("src"), None),
            "source.pkg.src.tbl"
        );
        // Seed / snapshot fall through to the default shape.
        assert_eq!(
            format_node_unique_id("seed", "pkg", "s", None, None),
            "seed.pkg.s"
        );
    }

    #[test]
    fn test_no_double_quoting() {
        // Test case 1: Already double-quoted string
        let mut kwargs1 = BTreeMap::new();
        kwargs1.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs1.insert(
            "arg1".to_string(),
            Value::String("\"already quoted\"".to_string()),
        );

        // Test case 2: Already single-quoted string
        let mut kwargs2 = BTreeMap::new();
        kwargs2.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs2.insert(
            "arg1".to_string(),
            Value::String("'already quoted'".to_string()),
        );

        // Test case 3: Unquoted string that should get quotes
        let mut kwargs3 = BTreeMap::new();
        kwargs3.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs3.insert(
            "arg1".to_string(),
            Value::String("needs quotes".to_string()),
        );

        // Test case 4: ref call that shouldn't get quotes
        let mut kwargs4 = BTreeMap::new();
        kwargs4.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs4.insert(
            "arg1".to_string(),
            Value::String("ref('other_model')".to_string()),
        );

        // Test case 5: source call that shouldn't get quotes
        let mut kwargs5 = BTreeMap::new();
        kwargs5.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs5.insert(
            "arg1".to_string(),
            Value::String("source('src', 'tbl')".to_string()),
        );

        let test_name = "unique";
        let namespace = None;
        let jinja_set_vars = BTreeMap::new();

        let result1 =
            generate_test_macro(test_name, &kwargs1, namespace, &None, &jinja_set_vars).unwrap();
        let result2 =
            generate_test_macro(test_name, &kwargs2, namespace, &None, &jinja_set_vars).unwrap();
        let result3 =
            generate_test_macro(test_name, &kwargs3, namespace, &None, &jinja_set_vars).unwrap();
        let result4 =
            generate_test_macro(test_name, &kwargs4, namespace, &None, &jinja_set_vars).unwrap();
        let result5 =
            generate_test_macro(test_name, &kwargs5, namespace, &None, &jinja_set_vars).unwrap();

        // Verify results - note that BTreeMap sorts keys alphabetically, so arg1 comes before model
        assert_eq!(
            result1,
            "{{ test_unique(arg1=\"\\\"already quoted\\\"\", model=ref('my_model')) }}"
        );
        assert_eq!(
            result2,
            "{{ test_unique(arg1=\"'already quoted'\", model=ref('my_model')) }}"
        );
        assert_eq!(
            result3,
            "{{ test_unique(arg1=\"needs quotes\", model=ref('my_model')) }}"
        );
        assert_eq!(
            result4,
            "{{ test_unique(arg1=ref('other_model'), model=ref('my_model')) }}"
        );
        assert_eq!(
            result5,
            "{{ test_unique(arg1=source('src', 'tbl'), model=ref('my_model')) }}"
        );

        // Test for no triple or quadruple quotes
        assert!(!result1.contains("\"\"\""));
        assert!(!result1.contains("\"\"\"\""));
        assert!(!result2.contains("'''"));
        assert!(!result2.contains("''''"));
    }

    #[test]
    fn test_jinja_set_var_extraction() {
        // Create a test input with a complex SQL query containing Jinja
        let mut test_args = serde_json::Map::new();
        test_args.insert(
            "model_column".to_string(),
            Value::String("num_in_motion_network_disruption_events".to_string()),
        );
        test_args.insert(
            "model_agg_type".to_string(),
            Value::String("sum".to_string()),
        );
        test_args.insert(
            "model_filter".to_string(),
            Value::String("event_date_utc >= DATEADD(DAY, -7, DATE_TRUNC('DAY', CONVERT_TIMEZONE('UTC', CURRENT_TIMESTAMP)))".to_string()),
        );

        // This is the complex SQL query with Jinja that should be extracted
        let complex_sql = "SELECT MD5( CONCAT( DATE_TRUNC('day', event_timestamp_utc), COALESCE(asset_external_id, '-99'), COALESCE(device_serial, '-99'),
            COALESCE(deployment_id, -99), COALESCE(app_version, '-99'), COALESCE(app_name, '-99'), COALESCE(android_os_version, '-99'), COALESCE(tablet_brand,
            '-99'), COALESCE(tablet_model, '-99'), COALESCE(last_heartbeat_cvd_esn, '-99'), COALESCE(last_heartbeat_cvd_type, '-99') ) )   AS pseudo_dbt_id,
            COUNT(DISTINCT event_timestamp_utc) AS num_events FROM {{ ref('connection_events_staging') }} WHERE DATE_TRUNC('DAY', event_timestamp_utc) >=
            DATEADD(DAY, -7, DATE_TRUNC('DAY', CONVERT_TIMEZONE('UTC', CURRENT_TIMESTAMP))) AND last_heartbeat_speed > 0 AND event_name = 'NetworkChange' and
            state = 'DISCONNECTED' GROUP BY pseudo_dbt_id";

        test_args.insert(
            "upstream_model_cte".to_string(),
            Value::String(complex_sql.to_string()),
        );

        test_args.insert(
            "upstream_column".to_string(),
            Value::String("num_events".to_string()),
        );
        test_args.insert(
            "upstream_agg_type".to_string(),
            Value::String("sum".to_string()),
        );
        test_args.insert("upstream_filter".to_string(), Value::Null);
        test_args.insert("severity".to_string(), Value::String("warn".to_string()));

        // Process the args using the new simplified function
        // Convert serde_json::Map to BTreeMap for the new function
        let test_args_btree: BTreeMap<String, Value> = test_args.into_iter().collect();

        // Convert to dbt_yaml::Value using the to_value function
        let yaml_value = dbt_yaml::to_value(&test_args_btree).unwrap();
        let verbatim_wrapper = Verbatim::from(Some(yaml_value));
        let empty_deprecated = Verbatim::from(BTreeMap::new());
        let existing_config = None;
        let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
            &verbatim_wrapper,
            &empty_deprecated,
            &existing_config,
            None,
            false,
            LegacyTestSyntaxHandling::Strict,
            &BTreeSet::new(),
        )
        .unwrap();
        let kwargs = extraction_result.kwargs;
        let jinja_set_vars = extraction_result.jinja_set_vars;

        // Verify that the complex SQL was extracted
        assert!(
            !jinja_set_vars.is_empty(),
            "No Jinja set vars were extracted"
        );

        // Find the upstream_model_cte variable
        let extracted_var_name = kwargs.get("upstream_model_cte").and_then(|v| v.as_str());
        assert!(
            extracted_var_name.is_some(),
            "upstream_model_cte value should be a string variable reference"
        );

        let var_name = extracted_var_name.unwrap();
        assert!(
            jinja_set_vars.contains_key(var_name),
            "upstream_model_cte variable {var_name} not found in set vars"
        );

        let extracted_sql = jinja_set_vars.get(var_name).unwrap();
        assert_eq!(
            extracted_sql, complex_sql,
            "Extracted SQL doesn't match original"
        );

        // Verify that simple args were not extracted
        assert!(
            !jinja_set_vars.values().any(|v| v == "sum"),
            "Simple value 'sum' should not be extracted to a set var"
        );
    }

    #[test]
    fn test_generate_test_name_with_set_vars() {
        // Create test inputs
        let test_macro_name = "upstream_column_comparison";
        let project_name = "my_project";
        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "my_model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        // Create kwargs with a reference to a set variable
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs.insert(
            "column_name".to_string(),
            Value::String("my_column".to_string()),
        );

        // This is the variable reference that should be replaced with its actual value
        let set_var_name = "dbt_parser_upstream_model_cte_12345";
        kwargs.insert(
            "upstream_model_cte".to_string(),
            Value::String(set_var_name.to_string()),
        );

        // Create the set variables map with the original SQL
        let mut jinja_set_vars = BTreeMap::new();
        let original_sql = "SELECT * FROM staging WHERE complex_condition";
        jinja_set_vars.insert(set_var_name.to_string(), original_sql.to_string());

        // Generate the test name
        let mut test_name_truncations = HashMap::new();
        let test_name = generate_test_name(
            test_macro_name,
            None,
            project_name,
            &test_config,
            &kwargs,
            None,
            &jinja_set_vars,
            &mut test_name_truncations,
        );

        // Verify that the test name does not contain the variable name
        // and that the original SQL is truncated from the final test name.
        assert!(
            !test_name.contains("SELECT"),
            "The original SQL should be truncated from the final test name"
        );
        assert!(
            !test_name.contains(set_var_name),
            "Test name should not contain the set variable name"
        );

        // Also test with an empty set vars map to ensure it still works
        let empty_set_vars = BTreeMap::new();
        let mut test_name_truncations = HashMap::new();
        let test_name_no_vars = generate_test_name(
            test_macro_name,
            None,
            project_name,
            &test_config,
            &kwargs,
            None,
            &empty_set_vars,
            &mut test_name_truncations,
        );

        // set vars part of the name is truncated from the final test name due to length
        assert!(
            !test_name_no_vars.contains(set_var_name),
            "Set var name should be truncated from the final test name"
        );
    }

    #[test]
    fn test_version_column_exclude_suppresses_inherited_tests() {
        // Regression test for https://github.com/dbt-labs/dbt-fusion/issues/1666
        //
        // When a versioned model uses `columns.exclude` to omit a column from a
        // specific version, any tests defined on that column in the base model
        // must NOT appear in that version's column_tests.
        //
        // The bug: after filtering, if every testable column is excluded the
        // resulting BTreeMap is empty.  The old guard `if !is_empty()` skipped
        // the assignment, leaving the cloned base config (with all tests) intact.

        use dbt_schemas::schemas::common::Versions;
        use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
        use dbt_yaml::Verbatim;

        // Build a base config: one column "cost_center_bkey" with a `unique` test.
        let unique_test = DataTests::String(Spanned::from("unique".to_string()));
        let mut base_col_tests = BTreeMap::new();
        base_col_tests.insert(
            "cost_center_bkey".to_string(),
            ColumnTestEntry {
                quote: false,
                tests: vec![unique_test],
                tags: vec![],
                legacy_syntax_handling: LegacyTestSyntaxHandling::Strict,
            },
        );
        let base_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "cost_centers".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: Some(base_col_tests),
            source_name: None,
        };

        // Helper: build the `columns` list of a version spec, carrying only the
        // include/exclude directive entry.
        let make_columns = |include: &str, exclude: Vec<&str>| -> Vec<VersionColumnProperties> {
            vec![VersionColumnProperties {
                include: Some(StringOrArrayOfStrings::String(include.to_string())),
                exclude: (!exclude.is_empty())
                    .then(|| exclude.iter().map(|s| (*s).to_string()).collect()),
                ..Default::default()
            }]
        };

        let make_version = |v: i64, columns: Vec<VersionColumnProperties>| -> Versions {
            Versions {
                v: dbt_yaml::Value::Number(dbt_yaml::Number::from(v), Span::zero()),
                deprecation_date: None,
                defined_in: None,
                description: None,
                access: None,
                config: Verbatim::from(None),
                constraints: None,
                data_tests: None,
                tests: None,
                columns: Some(columns),
                __additional_properties__: Verbatim::from(HashMap::new()),
            }
        };

        // v1: include all (no exclusions) — test must be present
        let v1 = make_version(1, make_columns("all", vec![]));
        // v2: include all, exclude cost_center_bkey — test must be absent
        let v2 = make_version(2, make_columns("all", vec!["cost_center_bkey"]));

        let result =
            collect_versioned_model_tests(&base_config, &[v1, v2]).expect("should not fail");

        assert_eq!(result.len(), 2, "expected one config per version");

        let v1_cfg = result
            .iter()
            .find(|c| c.version_num.as_deref() == Some("1"))
            .unwrap();
        assert!(
            v1_cfg
                .column_tests
                .as_ref()
                .map(|m| m.contains_key("cost_center_bkey"))
                .unwrap_or(false),
            "v1 should inherit the unique test for cost_center_bkey"
        );

        let v2_cfg = result
            .iter()
            .find(|c| c.version_num.as_deref() == Some("2"))
            .unwrap();
        assert!(
            !v2_cfg
                .column_tests
                .as_ref()
                .map(|m| m.contains_key("cost_center_bkey"))
                .unwrap_or(false),
            "v2 must not have a test for cost_center_bkey (it is excluded)"
        );
    }

    /// dbt-core renames a version's `tests:` to `data_tests:` in
    /// `SchemaParser.validate_data_tests` (`core/dbt/parser/schemas.py`, the `# versioned models`
    /// branch) before `get_tests_for_version` ever runs, so the alias is honored at the version
    /// level exactly like it is at the model and column levels. See dbt-labs/fs#14416.
    #[test]
    fn test_version_model_tests_legacy_alias_honored() {
        let base_config = versioned_model_base_config();
        let version = version_with_model_tests(
            Some(vec![DataTests::String(Spanned::from(
                "not_null".to_string(),
            ))]),
            None,
        );

        let result = collect_versioned_model_tests(&base_config, &[version]).unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].version_num.as_deref(), Some("1"));
        let model_tests = result[0]
            .model_tests
            .as_ref()
            .expect("version-level `tests` should create model tests");
        assert_eq!(model_tests.len(), 1);
        assert_eq!(model_tests[0].test_name(), Some("not_null"));
    }

    /// dbt-core's `validate_and_rename` raises `ValidationError: Invalid test config: cannot have
    /// both 'tests' and 'data_tests' defined` for a version carrying both keys, so Fusion errors
    /// too rather than silently picking one.
    #[test]
    fn test_version_model_tests_both_keys_is_an_error() {
        let base_config = versioned_model_base_config();
        let version = version_with_model_tests(
            Some(vec![DataTests::String(Spanned::from(
                "not_null".to_string(),
            ))]),
            Some(vec![DataTests::String(Spanned::from("unique".to_string()))]),
        );

        let err = collect_versioned_model_tests(&base_config, &[version])
            .expect_err("a version with both `tests` and `data_tests` must be rejected");

        assert!(
            err.to_string()
                .contains("Cannot have both 'tests' and 'data_tests' defined"),
            "unexpected error: {err}"
        );
    }

    /// dbt-core guards the rename with `if data.get("tests"):`, so an empty `tests:` list is falsy
    /// and never becomes `data_tests`. `get_tests_for_version` then sees `data_tests is None` and
    /// falls back to the model-level tests, so an empty `tests:` list must NOT override them.
    #[test]
    fn test_version_model_empty_tests_list_inherits_model_level() {
        let base_config = versioned_model_base_config_with_model_tests();
        let version = version_with_model_tests(Some(vec![]), None);

        let result = collect_versioned_model_tests(&base_config, &[version]).unwrap();

        assert_eq!(result.len(), 1);
        let model_tests = result[0]
            .model_tests
            .as_ref()
            .expect("an empty `tests:` list should leave the inherited model-level tests in place");
        assert_eq!(model_tests.len(), 1);
        assert_eq!(model_tests[0].test_name(), Some("not_null"));
    }

    /// An empty `data_tests:` list is a real value, not an absent one: dbt-core checks
    /// `data_tests is not None`, so `[]` overrides the model-level tests with nothing.
    #[test]
    fn test_version_model_empty_data_tests_list_overrides_model_level() {
        let base_config = versioned_model_base_config_with_model_tests();
        let version = version_with_model_tests(None, Some(vec![]));

        let result = collect_versioned_model_tests(&base_config, &[version]).unwrap();

        assert_eq!(result.len(), 1);
        let model_tests = result[0]
            .model_tests
            .as_ref()
            .expect("an empty `data_tests:` list should still replace the inherited tests");
        assert!(
            model_tests.is_empty(),
            "expected the inherited tests to be overridden with nothing; got {model_tests:?}"
        );
    }

    /// A falsy `tests:` never reaches dbt-core's both-keys check, so pairing an empty `tests:` list
    /// with a real `data_tests:` is allowed and the `data_tests:` value is used.
    #[test]
    fn test_version_model_empty_tests_list_with_data_tests_is_not_an_error() {
        let base_config = versioned_model_base_config();
        let version = version_with_model_tests(
            Some(vec![]),
            Some(vec![DataTests::String(Spanned::from("unique".to_string()))]),
        );

        let result = collect_versioned_model_tests(&base_config, &[version])
            .expect("an empty `tests:` list must not trigger the both-keys error");

        assert_eq!(result.len(), 1);
        let model_tests = result[0]
            .model_tests
            .as_ref()
            .expect("version `data_tests` should be used");
        assert_eq!(model_tests.len(), 1);
        assert_eq!(model_tests[0].test_name(), Some("unique"));
    }

    fn versioned_model_base_config_with_model_tests() -> GenericTestConfig {
        GenericTestConfig {
            model_tests: Some(vec![DataTests::String(Spanned::from(
                "not_null".to_string(),
            ))]),
            ..versioned_model_base_config()
        }
    }

    fn versioned_model_base_config() -> GenericTestConfig {
        GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "orders".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        }
    }

    fn version_with_model_tests(
        tests: Option<Vec<DataTests>>,
        data_tests: Option<Vec<DataTests>>,
    ) -> Versions {
        use dbt_yaml::Verbatim;

        Versions {
            v: dbt_yaml::Value::Number(dbt_yaml::Number::from(1), Span::zero()),
            deprecation_date: None,
            defined_in: None,
            description: None,
            access: None,
            config: Verbatim::from(None),
            constraints: None,
            data_tests,
            tests,
            columns: None,
            __additional_properties__: Verbatim::from(HashMap::new()),
        }
    }

    #[test]
    fn test_generate_test_name_with_custom_test_name() {
        // Create test inputs
        let custom_test_name = "custom_test_name";
        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "my_model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let test_name_no_vars = generate_test_name(
            "test_macro_name",
            Some(custom_test_name.to_string()),
            "project_name",
            &test_config,
            &BTreeMap::new(),
            None,
            &BTreeMap::new(),
            &mut HashMap::new(),
        );

        assert_eq!(
            test_name_no_vars, custom_test_name,
            "Test name should exactly match the custom test name when provided"
        );
    }

    #[test]
    fn test_generate_test_name_sanitizes_path_unsafe_chars_in_source_table() {
        // A source table name containing `/` would otherwise leak path
        // separators into the synthesized test name and the generic test
        // SQL filename, producing missing intermediate directories on write.
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String(
                "{{ get_where_subquery(source('special_chars_source', 'raw/data/table')) }}"
                    .to_string(),
            ),
        );
        kwargs.insert("column_name".to_string(), Value::String("id".to_string()));

        let test_config = GenericTestConfig {
            resource_type: "source".to_string(),
            resource_name: "raw/data/table".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: Some("special_chars_source".to_string()),
        };

        let name = generate_test_name(
            "not_null",
            None,
            "project_name",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut HashMap::new(),
        );

        assert!(
            !name.contains('/'),
            "synthesized test name must not contain `/`: {name}"
        );
        assert_eq!(
            name, "source_not_null_special_chars_source_raw_data_table_id",
            "slashes in the source table name should be normalized to `_`"
        );
    }

    #[test]
    fn test_generate_test_name_preserves_non_path_special_chars() {
        // Hyphens, dots, etc. previously produced working filenames; the
        // path-separator-only sanitizer must leave them alone so existing
        // test names and `unique_id`s do not drift.
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("{{ get_where_subquery(ref('weird.name-model')) }}".to_string()),
        );
        kwargs.insert("column_name".to_string(), Value::String("id".to_string()));

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "weird.name-model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let name = generate_test_name(
            "unique",
            None,
            "project_name",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut HashMap::new(),
        );

        assert_eq!(name, "unique_weird.name-model_id");
    }

    #[test]
    fn test_generate_test_name_sanitizes_windows_path_separator() {
        // Backslashes are path separators on Windows. Treat them like `/`
        // so the same source table name behaves identically across platforms.
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String(
                "{{ get_where_subquery(source('special_chars_source', 'raw\\data\\table')) }}"
                    .to_string(),
            ),
        );
        kwargs.insert("column_name".to_string(), Value::String("id".to_string()));

        let test_config = GenericTestConfig {
            resource_type: "source".to_string(),
            resource_name: "raw\\data\\table".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: Some("special_chars_source".to_string()),
        };

        let name = generate_test_name(
            "not_null",
            None,
            "project_name",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut HashMap::new(),
        );

        assert!(!name.contains('\\'));
        assert_eq!(
            name,
            "source_not_null_special_chars_source_raw_data_table_id"
        );
    }

    #[test]
    fn test_generate_test_name_excludes_config_from_name() {
        // Arrange: exact kwargs structure provided by the user
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('codes_metrics_daily'))".to_string()),
        );
        kwargs.insert(
            "combination_of_columns".to_string(),
            Value::Array(vec![
                Value::String("flowcode_id".to_string()),
                Value::String("report_date".to_string()),
            ]),
        );
        let mut config_obj = serde_json::Map::new();
        config_obj.insert("error_if".to_string(), Value::String(">1000".to_string()));
        config_obj.insert("severity".to_string(), Value::String("warn".to_string()));
        config_obj.insert("warn_if".to_string(), Value::String(">0".to_string()));
        config_obj.insert(
            "where".to_string(),
            Value::String(
                "report_date >= current_date -  {{ env_var('DBT_CUSTOM_INCREMENT') }}".to_string(),
            ),
        );
        kwargs.insert("config".to_string(), Value::Object(config_obj));

        let test_macro_name = "unique_combination_of_columns";
        let project_name = "my_project";
        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "codes_metrics_daily".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };
        let jinja_set_vars = BTreeMap::new();

        // Act
        let mut test_name_truncations = HashMap::new();
        let generated = generate_test_name(
            test_macro_name,
            None,
            project_name,
            &test_config,
            &kwargs,
            None,
            &jinja_set_vars,
            &mut test_name_truncations,
        );

        // Assert: ensure config fields are excluded and only combination_of_columns contribute
        assert!(
            generated == "unique_combination_of_columns__44f4ef42665e203902f74bb7d4b98bb7",
            "Expected test name to include only combination_of_columns values in suffix, got: {generated}"
        );
        for disallowed in [
            "warn",
            "_1000",
            "_0",
            "env_var",
            "current_date",
            "where",
            "warn_if",
            "error_if",
        ] {
            assert!(
                !generated.contains(disallowed),
                "Generated name should not contain config-derived token '{disallowed}': {generated}"
            );
        }
    }

    #[test]
    fn test_generate_test_name_treats_escaped_newlines_as_whitespace() {
        // Regression: `Value::to_string()` may escape newlines into `\n`, which previously
        // yielded `_nand_` from `\nand` after CLEAN_REGEX sanitization. We want `_and_`.

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "my_model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs.insert(
            "compare_row_condition".to_string(),
            // Represents a multi-line condition serialized as an escaped newline.
            Value::String("x = 1\\nand y = 2".to_string()),
        );

        let mut test_name_truncations = HashMap::new();
        let generated = generate_test_name(
            "dbt_expectations_expect_table_aggregation_to_equal_other_table",
            None,
            "my_project",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        // The returned name may be truncated to <=63 chars (dbt-core behavior).
        // When that happens, the un-truncated full name is stored in `test_name_truncations`.
        let full = test_name_truncations
            .get(&generated)
            .cloned()
            .unwrap_or_else(|| generated.clone());

        assert!(
            !full.contains("_nand_"),
            "Full generated name should not contain `_nand_`: {full}"
        );
        assert!(
            full.contains("x_1_and_y_2"),
            "Full generated name should treat escaped newlines as whitespace: {full}"
        );
    }

    #[test]
    fn test_generate_test_name_renders_primitives_as_python_str() {
        // Regression: `serde_json::Value::{Bool, Null}.to_string()` produces JSON-style
        // forms (`"true"`, `"false"`, `"null"`), but dbt-core uses Python's `str(value)`
        // which produces (`"True"`, `"False"`, `"None"`) when building the synthesized
        // test name. The wrong form changes both the displayed name and the trailing
        // unique_id hash (since the hash is computed over the name), breaking
        // `state:modified` parity against Mantle-produced manifests.
        // Cover all three branches of the kwargs match in `generate_test_name`:
        //   - bare leaf       -> `_ => actual_value.to_string()`
        //   - leaf in Object  -> `Value::Object(map) => map.values().map(.to_string())`
        //   - leaf in Array   -> `Value::Array(arr) => arr.iter().map(.to_string())`

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "my_model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        // ── bare leaf kwargs (bool + null) ────────────────────────────────
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs.insert("inclusive".to_string(), Value::Bool(true));
        kwargs.insert("strict".to_string(), Value::Bool(false));
        kwargs.insert("missing".to_string(), Value::Null);

        let mut test_name_truncations = HashMap::new();
        let generated = generate_test_name(
            "accepted_range",
            None,
            "my_project",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        assert!(
            generated.contains("True"),
            "Bare bool kwarg should render as Python `True`, got: {generated}"
        );
        assert!(
            generated.contains("False"),
            "Bare bool kwarg should render as Python `False`, got: {generated}"
        );
        assert!(
            generated.contains("None"),
            "Bare null kwarg should render as Python `None`, got: {generated}"
        );

        // ── leaves nested in Value::Object ────────────────────────────────
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        let mut constraint = serde_json::Map::new();
        constraint.insert("strict".to_string(), Value::Bool(false));
        constraint.insert("default".to_string(), Value::Null);
        kwargs.insert("constraint".to_string(), Value::Object(constraint));

        let mut test_name_truncations = HashMap::new();
        let generated = generate_test_name(
            "accepted_range",
            None,
            "my_project",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        assert!(
            generated.contains("False"),
            "Bool inside Value::Object should render as Python `False`, got: {generated}"
        );
        assert!(
            generated.contains("None"),
            "Null inside Value::Object should render as Python `None`, got: {generated}"
        );

        // ── leaves inside Value::Array ────────────────────────────────────
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs.insert(
            "values".to_string(),
            Value::Array(vec![Value::Bool(true), Value::Bool(false), Value::Null]),
        );

        let mut test_name_truncations = HashMap::new();
        let generated = generate_test_name(
            "accepted_range",
            None,
            "my_project",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        assert!(
            generated.contains("True") && generated.contains("False") && generated.contains("None"),
            "Leaves inside Value::Array should render as Python `True`/`False`/`None`, got: {generated}"
        );
    }

    #[test]
    fn test_generate_test_name_with_name_longer_than_63_chars() {
        //This test is to ensure that if the generated test name is longer than 63 characters
        // it will be truncated to 30 characters and an md5 hash will be added to the end
        // to create a unique name that is 63 characters or less.
        use serde_json::json;
        // Create test inputs
        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "my_model_with_a_long_name_beyond_64_chars_and_some_other_chars_aa"
                .to_string(),
            version_num: None,
            model_tests: Some(vec![DataTests::CustomTest(
                CustomTest::MultiKey(Box::new(CustomTestMultiKey {
                    arguments: Verbatim::from(Some(
                        dbt_yaml::to_value(json!({
                            "column_names": ["id"]
                        }))
                        .unwrap(),
                    )),
                    column_name: None,
                    config: None,
                    __deprecated_args_and_configs__: Verbatim::from(BTreeMap::new()),
                    name: Some("noop?=p+:".to_string()),
                    test_name: "noop?=p+:".to_string(),
                    description: None,
                }))
                .into(),
            )]),
            column_tests: None,
            source_name: None,
        };
        let mut kwargs = BTreeMap::new();

        // The "id" in the "model_tests" above goes in tandem with the "id" in vector that
        // forms the value part of the "column_names" key in the kwargs hashmap so that
        // "id" gets added to the generated test name.
        kwargs.insert(
            "column_names".to_string(),
            Value::Array(vec![Value::String("id".to_string())]),
        );

        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('main'))".to_string()),
        );

        let mut test_name_truncations = HashMap::new();
        let test_name_no_vars = generate_test_name(
            "test_macro_name",
            None,
            "project_name",
            &test_config,
            &kwargs,
            None,
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        // The generated test name will initially be over 64 characters and have the
        // "id" column name in it at the end and so the `generate_test_name` will
        // truncate to the first 30 characters and add an md5 hash
        // to create a unique name that is 63 characters.
        assert!(
            test_name_no_vars.contains("test_macro_name_my_model_with__"),
            "Test name should contain only the first 30 characters of the original generated name"
        );
        assert!(
            test_name_no_vars.len() <= 63,
            "Test name should be 63 characters or less"
        );
        assert!(
            !test_name_no_vars.contains("id"),
            "Test name should not contain the 'id' column name after truncation"
        );
        // Verify the truncations map records the full original name, which is the name
        // that dbt-core/Mantle use for unique_id construction.
        let original_name = test_name_truncations
            .get(&test_name_no_vars)
            .expect("truncated name should be recorded in test_name_truncations");
        assert!(
            original_name.len() >= 64,
            "Original name should be the untruncated form (>=64 chars), got: {original_name}"
        );
        assert!(
            original_name.contains("id"),
            "Original name should contain 'id' which was truncated away, got: {original_name}"
        );
    }

    #[test]
    fn test_needs_jinja_set_block() {
        // Multiline content
        assert!(
            needs_jinja_set_block("line1\nline2"),
            "Multiline content should need a set block"
        );

        // Content with Jinja expression
        assert!(
            needs_jinja_set_block("SELECT * FROM {{ ref('model') }}"),
            "Content with Jinja expression should need a set block"
        );

        // Simple string without Jinja
        assert!(
            !needs_jinja_set_block("simple string"),
            "Simple string should not need a set block"
        );

        // Unbalanced Jinja brackets shouldn't trigger (single opening bracket)
        assert!(
            !needs_jinja_set_block("Text with { one bracket"),
            "Text with single bracket should not need a set block"
        );

        // Unbalanced Jinja brackets shouldn't trigger (single closing bracket)
        assert!(
            !needs_jinja_set_block("Text with } one bracket"),
            "Text with single bracket should not need a set block"
        );
    }

    #[test]
    fn test_normalize_test_name_valid_cases() {
        let input_expected_pairs = vec![
            ("test", "test"),
            ("_test", "_test"),
            ("test_name", "test_name"),
            ("test+extra", "test"),
            ("valid::invalid", "valid"),
            ("name=with=equals", "name"),
            ("test+++", "test"),
        ];

        for (input, expected) in input_expected_pairs {
            match normalize_test_name(input) {
                Ok(result) => assert_eq!(
                    result, expected,
                    "Input '{input}' should normalize to '{expected}', got '{result}'"
                ),
                Err(e) => panic!("Expected success for input '{input}', got error: {e:?}"),
            }
        }
    }

    #[test]
    fn test_normalize_test_name_invalid_cases() {
        let invalid_cases = vec![
            "", "+test", "123test", "=test", ":test", "+++", "::::", "====", " test", "\ntest",
        ];

        for input in invalid_cases {
            assert!(normalize_test_name(input).is_err());
        }
    }

    #[test]
    fn test_generate_test_macro_embedded_config_in_kwargs() {
        // Arrange: config parameter is None, but kwargs contains a 'config' object
        let mut kwargs = BTreeMap::new();
        // minimal model kwarg so the macro call formats correctly
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );

        let mut cfg = serde_json::Map::new();
        cfg.insert("error_if".to_string(), Value::String("!= 0".to_string()));
        cfg.insert("warn_if".to_string(), Value::String("> 0".to_string()));
        cfg.insert(
            "fail_calc".to_string(),
            Value::String("count(*)".to_string()),
        );
        cfg.insert(
            "alias".to_string(),
            Value::String("my_test_alias".to_string()),
        );
        kwargs.insert("config".to_string(), Value::Object(cfg));

        let jinja_set_vars = BTreeMap::new();

        // Act
        let sql =
            generate_test_macro("accepted_values", &kwargs, None, &None, &jinja_set_vars).unwrap();

        // Assert: a config(...) block is emitted from kwargs.config
        assert!(
            sql.contains("{{ config("),
            "Expected emitted config(...) block when config is embedded in kwargs"
        );
        assert!(
            sql.contains("\"error_if\"")
                && sql.contains("\"warn_if\"")
                && sql.contains("\"fail_calc\""),
            "Expected serialized config JSON keys to be present in config(...)"
        );

        // Assert: the macro invocation excludes 'config=' kwarg (filtered out)
        assert!(
            !sql.contains("config="),
            "Embedded config must not be passed as a macro kwarg; it should be emitted only via config(...)"
        );

        // Assert: macro call prefix matches expected form
        assert!(
            sql.contains("{{ test_accepted_values("),
            "Expected default-qualified macro call '{{ test_accepted_values(...) }}'"
        );
        // And the model kwarg remains present
        assert!(
            sql.contains("model=ref('my_model')"),
            "Expected model kwarg to be present in macro call"
        );
    }

    #[test]
    fn test_generate_test_macro_merges_param_and_kwargs_config() {
        use serde_json::json;
        // Arrange: both parameter config and embedded kwargs config are present
        // Parameter config (base)
        let param_cfg: DataTestConfig = serde_json::from_value(json!({
            "warn_if": "!= 0",
            "error_if": "!= 0",
            "alias": "base_alias",
            "__warehouse_specific_config__": {}
        }))
        .unwrap();

        // Kwargs config (overlay) should override warn_if and add fail_calc
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        let mut overlay = serde_json::Map::new();
        overlay.insert("warn_if".to_string(), Value::String("> 5".to_string()));
        overlay.insert(
            "fail_calc".to_string(),
            Value::String("count(*)".to_string()),
        );
        kwargs.insert("config".to_string(), Value::Object(overlay));

        // Act
        let sql = generate_test_macro(
            "accepted_values",
            &kwargs,
            None,
            &Some(param_cfg),
            &BTreeMap::new(),
        )
        .unwrap();

        // Assert: config(...) emitted and contains merged values:
        // - warn_if from kwargs (> 5) overrides param (!= 0)
        // - error_if from param remains
        // - fail_calc from kwargs appears
        // - alias from param remains
        assert!(sql.contains("{{ config("), "config(...) should be emitted");
        assert!(
            sql.contains("\"warn_if\":\"> 5\""),
            "warn_if from kwargs should override base: {sql}"
        );
        assert!(
            sql.contains("\"error_if\":\"!= 0\""),
            "error_if from param should be preserved: {sql}"
        );
        assert!(
            sql.contains("\"fail_calc\":\"count(*)\""),
            "fail_calc from kwargs should be present: {sql}"
        );
        assert!(
            sql.contains("\"alias\":\"base_alias\""),
            "alias from param should be preserved: {sql}"
        );

        // Assert: macro invocation excludes config= and retains model kwarg
        assert!(
            !sql.contains("config="),
            "Merged config must not be passed as macro kwarg"
        );
        assert!(
            sql.contains("{{ test_accepted_values(") && sql.contains("model=ref('my_model')"),
            "Macro call should be well-formed and include model kwarg"
        );
    }

    #[test]
    fn test_generate_test_macro_renders_jinja_inside_config_strings() {
        use serde_json::json;

        let config: DataTestConfig = serde_json::from_value(json!({
            "fail_calc": "dbt_custom_config_0",
            "where": "dim_partner_tier_sk != '{{ var('missing_surrogate_key') }}'",
            "__warehouse_specific_config__": {}
        }))
        .unwrap();
        let kwargs = BTreeMap::from([(
            "model".to_string(),
            Value::String(
                "get_where_subquery(ref('fct_partner_list_historic_states'))".to_string(),
            ),
        )]);

        let sql = generate_test_macro(
            "relationships",
            &kwargs,
            None,
            &Some(config),
            &BTreeMap::new(),
        )
        .unwrap();

        assert!(
            sql.contains(
                "{% set dbt_custom_config_1 -%}\n\
                 dim_partner_tier_sk != '{{ var('missing_surrogate_key') }}'\n\
                 {%- endset %}"
            ),
            "nested config Jinja should render through a set block: {sql}"
        );
        assert!(
            sql.contains("\"where\":dbt_custom_config_1"),
            "config(...) should receive the rendered set variable: {sql}"
        );
        assert!(
            sql.contains("\"fail_calc\":\"dbt_custom_config_0\""),
            "literal config values must not collide with generated variables: {sql}"
        );
    }

    #[test]
    fn test_extract_kwargs_moves_config_keys_out_of_arguments() {
        // Arrange: config-like keys under `arguments` should not become macro kwargs; they should
        // be routed into an embedded `config` object so `generate_test_macro` emits `{{ config(...) }}`.
        let args_yaml = dbt_yaml::to_value(serde_json::json!({
            "values": ["APPAREL", "HEADWEAR"],
            "error_if": ">1"
        }))
        .unwrap();
        let arguments = Verbatim::from(Some(args_yaml));
        let deprecated = Verbatim::from(BTreeMap::new());
        let existing_config: Option<DataTestConfig> = None;
        // Act
        let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
            &arguments,
            &deprecated,
            &existing_config,
            None,
            false,
            LegacyTestSyntaxHandling::Strict,
            &BTreeSet::new(),
        )
        .unwrap();

        // Assert: error_if is not a macro kwarg
        assert!(
            !extraction_result.kwargs.contains_key("error_if"),
            "error_if must not be passed to the test macro as a kwarg"
        );
        // Assert: error_if is captured as embedded config
        let cfg = extraction_result
            .kwargs
            .get("config")
            .expect("expected embedded config object")
            .as_object()
            .expect("embedded config must be an object");
        assert_eq!(
            cfg.get("error_if"),
            Some(&serde_json::Value::String(">1".to_string())),
            "error_if should be moved into embedded config"
        );
        // And the original argument remains
        assert!(
            extraction_result.kwargs.contains_key("values"),
            "values should remain a macro kwarg"
        );
    }

    /// Drives the top-level-vs-`config:` conflict guard from the two authored YAML positions.
    /// `explicit_config_keys` lists the keys written literally under `config:`; the deprecation
    /// log is suppressed so only the guard is under test.
    fn extract_with_conflict_guard(
        deprecated_yaml: &str,
        config_yaml: &str,
        explicit_config_keys: &[&str],
    ) -> FsResult<KwargsExtractionResult> {
        let arguments: Verbatim<Option<dbt_yaml::Value>> = Verbatim::from(None);
        let deprecated = Verbatim::from(
            dbt_yaml::from_str::<BTreeMap<String, dbt_yaml::Value>>(deprecated_yaml).unwrap(),
        );
        let config_value: dbt_yaml::Value = dbt_yaml::from_str(config_yaml).unwrap();
        let existing_config: Option<DataTestConfig> =
            Some(serde::Deserialize::deserialize(config_value).unwrap());
        let explicit_config_keys: BTreeSet<String> = explicit_config_keys
            .iter()
            .map(|k| (*k).to_string())
            .collect();
        extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
            &arguments,
            &deprecated,
            &existing_config,
            None,
            true,
            LegacyTestSyntaxHandling::Strict,
            &explicit_config_keys,
        )
    }

    /// Asserts the guard rejects `key` as authored in both the top-level and `config:` positions.
    fn assert_same_key_conflict(deprecated_yaml: &str, config_yaml: &str, key: &str) {
        let err = extract_with_conflict_guard(deprecated_yaml, config_yaml, &[key])
            .expect_err("a key authored under `config:` must conflict with the top-level key");
        assert!(
            err.to_string().contains(&format!("same key '{key}'")),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn test_empty_config_block_does_not_conflict_with_deprecated_tags() {
        // `config: {}` authors no keys, so a deprecated top-level `tags` is not a duplicate.
        // `tags` serializes as `[]` when unset, which must not read as an authored value.
        extract_with_conflict_guard("tags: [legacy_tag]", "{}", &[])
            .expect("an empty `config:` block must not conflict with a top-level key");
    }

    #[test]
    fn test_empty_config_block_does_not_conflict_with_deprecated_meta() {
        // Same for `meta`, which serializes as `{}` when unset.
        extract_with_conflict_guard("meta: {owner: alice}", "{}", &[])
            .expect("an empty `config:` block must not conflict with a top-level key");
    }

    #[test]
    fn test_explicit_config_tags_conflicts_with_deprecated_tags() {
        assert_same_key_conflict("tags: [legacy_tag]", "tags: [cfg_tag]", "tags");
    }

    #[test]
    fn test_empty_list_config_tags_conflicts_with_deprecated_tags() {
        // dbt Core errors here too: `tags` is literally present under `config:`.
        assert_same_key_conflict("tags: [legacy_tag]", "tags: []", "tags");
    }

    #[test]
    fn test_null_config_tags_conflicts_with_deprecated_tags() {
        // dbt Core errors here too: `tags` is present under `config:` with a null value.
        assert_same_key_conflict("tags: [legacy_tag]", "tags:", "tags");
    }

    #[test]
    fn test_generate_test_macro_excludes_reserved_name_kwarg() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "column_list".to_string(),
            Value::Array(vec![
                Value::String("a".to_string()),
                Value::String("b".to_string()),
            ]),
        );
        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('metapack_dpd'))".to_string()),
        );
        kwargs.insert(
            "name".to_string(),
            Value::String("metapack_dpd_column_name_order_is_correct".to_string()),
        );

        let sql = generate_test_macro(
            "expect_table_columns_to_match_ordered_list",
            &kwargs,
            Some("dbt_expectations"),
            &None,
            &BTreeMap::new(),
        )
        .unwrap();

        assert!(
            sql.contains("{{ dbt_expectations.test_expect_table_columns_to_match_ordered_list("),
            "Expected namespaced macro call, got: {sql}"
        );
        assert!(
            sql.contains("column_list=[\"a\",\"b\"]"),
            "Expected column_list kwarg to be present, got: {sql}"
        );
        assert!(
            sql.contains("model=get_where_subquery(ref('metapack_dpd'))"),
            "Expected model kwarg to be present, got: {sql}"
        );
        assert!(
            !sql.contains("name="),
            "Reserved 'name' must not be passed as a macro kwarg, got: {sql}"
        );
    }

    #[test]
    fn test_jinja_extraction_from_arrays() {
        // Test that Jinja expressions inside arrays are properly extracted
        let mut test_args = serde_json::Map::new();
        test_args.insert(
            "values".to_string(),
            Value::Array(vec![
                Value::String("{% raw %}{{true}}{% endraw %}".to_string()),
                Value::Null,
                Value::String("regular_string".to_string()),
            ]),
        );
        test_args.insert(
            "another_arg".to_string(),
            Value::String("{{ var('myvar', 'baz') }}-bar".to_string()),
        );

        // Convert to the format expected by the extraction function
        let test_args_btree: BTreeMap<String, Value> = test_args.into_iter().collect();
        let yaml_value = dbt_yaml::to_value(&test_args_btree).unwrap();
        let verbatim_wrapper = Verbatim::from(Some(yaml_value));
        let empty_deprecated = Verbatim::from(BTreeMap::new());
        let existing_config = None;
        let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
            &verbatim_wrapper,
            &empty_deprecated,
            &existing_config,
            None,
            false,
            LegacyTestSyntaxHandling::Strict,
            &BTreeSet::new(),
        )
        .unwrap();

        // Check that both the array element and the string arg had their Jinja extracted
        assert!(
            extraction_result.jinja_set_vars.len() >= 2,
            "Expected at least 2 Jinja set vars to be extracted, got: {}",
            extraction_result.jinja_set_vars.len()
        );

        // Check that the array was modified to use variable references
        let values_kwarg = extraction_result.kwargs.get("values").unwrap();
        if let Value::Array(arr) = values_kwarg {
            // First element should be a variable reference
            if let Value::String(s) = &arr[0] {
                assert!(
                    s.starts_with("dbt_custom_arg_values_0"),
                    "Expected first array element to be replaced with variable reference, got: {s}"
                );
                // And that variable should map to the original Jinja expression
                assert!(
                    extraction_result
                        .jinja_set_vars
                        .get(s)
                        .map(|v| v == "{% raw %}{{true}}{% endraw %}")
                        .unwrap_or(false),
                    "Variable reference should map to original Jinja expression"
                );
            }
            // Second element should remain null
            assert_eq!(arr[1], Value::Null);
            // Third element should remain a regular string
            assert_eq!(arr[2], Value::String("regular_string".to_string()));
        } else {
            panic!("Expected values kwarg to be an array");
        }

        // Test that generate_test_macro properly handles the array with variable references
        let mut kwargs = extraction_result.kwargs;
        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('my_model'))".to_string()),
        );

        let sql = generate_test_macro(
            "accepted_values",
            &kwargs,
            None,
            &None,
            &extraction_result.jinja_set_vars,
        )
        .unwrap();

        // Should have the set blocks with whitespace control
        assert!(
            sql.contains("{% set dbt_custom_arg_values_0 -%}"),
            "Expected set block with whitespace control for array element, got: {sql}"
        );
        assert!(
            sql.contains("{% raw %}{{true}}{% endraw %}"),
            "Expected original Jinja expression in set block, got: {sql}"
        );
        assert!(
            sql.contains("{%- endset %}"),
            "Expected endset with whitespace control, got: {sql}"
        );

        // The macro call should have the variable reference without quotes
        // and null values converted to "none" for proper Jinja syntax.
        assert!(
            sql.contains("values=[dbt_custom_arg_values_0,none,\"regular_string\"]"),
            "Expected array with variable reference (unquoted), none, and quoted string, got: {sql}"
        );
    }

    #[test]
    fn test_jinja_set_block_whitespace_control() {
        // Test that the generated set blocks use whitespace control to avoid newlines
        let mut jinja_set_vars = BTreeMap::new();
        jinja_set_vars.insert(
            "dbt_custom_arg_values_0".to_string(),
            "{% raw %}{{true}}{% endraw %}".to_string(),
        );

        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('one'))".to_string()),
        );
        kwargs.insert("column_name".to_string(), Value::String("id".to_string()));
        kwargs.insert(
            "values".to_string(),
            Value::Array(vec![
                Value::String("dbt_custom_arg_values_0".to_string()),
                Value::Null,
            ]),
        );

        let sql =
            generate_test_macro("accepted_values", &kwargs, None, &None, &jinja_set_vars).unwrap();

        // Verify that the set block uses whitespace control (-%} and {%-)
        assert!(
            sql.contains("{% set dbt_custom_arg_values_0 -%}"),
            "Set block should use trailing whitespace control (-%}}), got: {sql}"
        );
        assert!(
            sql.contains("{%- endset %}"),
            "Set block should use leading whitespace control ({{%-), got: {sql}"
        );

        // The generated SQL should have the format:
        // {% set dbt_custom_arg_values_0 -%}
        // {% raw %}{{true}}{% endraw %}
        // {%- endset %}
        //
        // {{ test_accepted_values(...) }}
        //
        // When this is rendered by Jinja, the whitespace control will strip
        // the newlines, so the value of dbt_custom_arg_values_0 will be
        // "{{true}}" without leading/trailing newlines
    }

    #[test]
    fn test_jinja_extraction_from_objects() {
        // Test that Jinja expressions inside objects are properly extracted and formatted
        let mut test_args = serde_json::Map::new();
        let mut nested_obj = serde_json::Map::new();
        nested_obj.insert(
            "query".to_string(),
            Value::String("SELECT * FROM {{ ref('model') }}".to_string()),
        );
        nested_obj.insert("limit".to_string(), Value::Number(100.into()));
        test_args.insert("config_obj".to_string(), Value::Object(nested_obj));

        // Convert to the format expected by the extraction function
        let test_args_btree: BTreeMap<String, Value> = test_args.into_iter().collect();
        let yaml_value = dbt_yaml::to_value(&test_args_btree).unwrap();
        let verbatim_wrapper = Verbatim::from(Some(yaml_value));
        let empty_deprecated = Verbatim::from(BTreeMap::new());
        let existing_config = None;
        let extraction_result = extract_kwargs_and_jinja_vars_and_dep_kwarg_and_configs(
            &verbatim_wrapper,
            &empty_deprecated,
            &existing_config,
            None,
            false,
            LegacyTestSyntaxHandling::Strict,
            &BTreeSet::new(),
        )
        .unwrap();

        // Check that the jinja expression in the object was extracted
        assert!(
            !extraction_result.jinja_set_vars.is_empty(),
            "Expected Jinja set vars to be extracted from nested object"
        );

        // Test that generate_test_macro properly handles objects with variable references
        let mut kwargs = extraction_result.kwargs;
        kwargs.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('my_model'))".to_string()),
        );

        let sql = generate_test_macro(
            "custom_test",
            &kwargs,
            None,
            &None,
            &extraction_result.jinja_set_vars,
        )
        .unwrap();

        // Should have the set block for the nested query
        assert!(
            sql.contains("{% set dbt_custom_arg_config_obj_query -%}"),
            "Expected set block for nested object query, got: {sql}"
        );

        // The macro call should have the object formatted with the variable reference unquoted
        // Note: BTreeMap iterates in sorted key order, so "limit" comes before "query"
        assert!(
            sql.contains("config_obj={\"limit\":100,\"query\":dbt_custom_arg_config_obj_query}"),
            "Expected object with variable reference (unquoted) and regular value, got: {sql}"
        );
    }

    #[test]
    fn test_generate_test_macro_does_not_escape_curly_braces_in_string_kwargs() {
        // Curly braces are common in regex quantifiers (e.g. {1,2}) and should be preserved.
        // They are not special inside a quoted Jinja string literal; escaping them mutates the
        // argument value and changes the downstream compiled SQL.
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('my_model')".to_string()),
        );
        kwargs.insert(
            "column_name".to_string(),
            Value::String("postcode".to_string()),
        );
        kwargs.insert(
            "regex".to_string(),
            Value::String("^[A-Z]{1,2}\\\\d{1,2}[A-Z]?$".to_string()),
        );

        let sql = generate_test_macro(
            "expect_column_values_to_match_regex",
            &kwargs,
            Some("dbt_expectations"),
            &None,
            &BTreeMap::new(),
        )
        .unwrap();

        // We expect backslashes and quotes to be escaped for the Jinja string literal,
        // but curly braces must remain unescaped.
        assert!(
            sql.contains("regex=\"^[A-Z]{1,2}\\\\\\\\d{1,2}[A-Z]?$\""),
            "Expected regex kwarg to preserve braces while escaping backslashes, got: {sql}"
        );
        assert!(
            !sql.contains("\\{") && !sql.contains("\\}"),
            "Curly braces should not be backslash-escaped in generated macro args, got: {sql}"
        );
    }

    #[test]
    fn test_write_value_to_hashable_repr_string() {
        let mut out = String::new();
        write_value_to_hashable_repr(&mut out, &Value::String("hello".to_string()));
        assert_eq!(out, "'hello'");
    }

    #[test]
    fn test_write_value_to_hashable_repr_number() {
        // dbt-core's `get_hashable_md` wraps every leaf primitive in `str()` before
        // `repr()`-ing the metadata, so numbers appear as quoted strings in the hash input.
        let mut out = String::new();
        write_value_to_hashable_repr(&mut out, &Value::Number(42.into()));
        assert_eq!(out, "'42'");
    }

    #[test]
    fn test_write_value_to_hashable_repr_bool() {
        // Match Python's `repr(str(True))` / `repr(str(False))` — quoted strings.
        let mut out = String::new();
        write_value_to_hashable_repr(&mut out, &Value::Bool(true));
        assert_eq!(out, "'True'");

        out.clear();
        write_value_to_hashable_repr(&mut out, &Value::Bool(false));
        assert_eq!(out, "'False'");
    }

    #[test]
    fn test_write_value_to_hashable_repr_null() {
        // Match Python's `repr(str(None))` — `'None'` (quoted string).
        let mut out = String::new();
        write_value_to_hashable_repr(&mut out, &Value::Null);
        assert_eq!(out, "'None'");
    }

    #[test]
    fn test_write_value_to_hashable_repr_array() {
        // Numbers inside arrays are also leaf primitives, so they get quoted.
        let mut out = String::new();
        write_value_to_hashable_repr(
            &mut out,
            &Value::Array(vec![
                Value::String("a".to_string()),
                Value::Number(1.into()),
            ]),
        );
        assert_eq!(out, "['a', '1']");
    }

    #[test]
    fn test_write_value_to_hashable_repr_object_sorted_keys() {
        // Keys should be sorted alphabetically
        let mut map = serde_json::Map::new();
        map.insert("zebra".to_string(), Value::String("z".to_string()));
        map.insert("apple".to_string(), Value::String("a".to_string()));
        map.insert("mango".to_string(), Value::String("m".to_string()));

        let mut out = String::new();
        write_value_to_hashable_repr(&mut out, &Value::Object(map));
        assert_eq!(out, "{'apple': 'a', 'mango': 'm', 'zebra': 'z'}");
    }

    #[test]
    fn test_build_hashable_metadata_repr_with_namespace() {
        let mut kwargs = BTreeMap::new();
        kwargs.insert("expression".to_string(), Value::String("> 0".to_string()));

        let repr = build_hashable_metadata_repr(
            "expression_is_true",
            Some(&"dbt_utils".to_string()),
            &kwargs,
        );

        // Should have sorted keys: kwargs, name, namespace
        assert!(repr.starts_with("{'kwargs': "));
        assert!(repr.contains("'name': 'expression_is_true'"));
        assert!(repr.contains("'namespace': 'dbt_utils'"));
    }

    #[test]
    fn test_build_hashable_metadata_repr_without_namespace() {
        let kwargs = BTreeMap::new();
        let repr = build_hashable_metadata_repr("unique", None, &kwargs);

        assert!(repr.contains("'namespace': 'None'"));
    }

    #[test]
    fn test_generate_test_unique_id_hash_matches_dbt_core() {
        // Regression: pin the hash output for `dbt_utils.accepted_range` against a
        // Mantle-produced value. This caught two parity bugs:
        //   1. boolean arg lowercased in fqn_name (`__true__` vs `__True__`)
        //   2. leaf primitives in the hashable-metadata repr emitted as raw types
        //      (`'inclusive': True`) instead of dbt-core's stringified form
        //      (`'inclusive': 'True'`).
        // Inputs mirror the YAML:
        //   - dbt_utils.accepted_range:
        //       arguments: { min_value: -90, max_value: 90, inclusive: true }
        // applied to source SWIMPLY_SWIMPLY_PROD.POOLS column `latitude`.
        let fqn_name =
            "dbt_utils_source_accepted_range_SWIMPLY_SWIMPLY_PROD_POOLS_latitude__True__90___90";

        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "column_name".to_string(),
            Value::String("latitude".to_string()),
        );
        kwargs.insert("min_value".to_string(), Value::Number((-90).into()));
        kwargs.insert("max_value".to_string(), Value::Number(90.into()));
        kwargs.insert("inclusive".to_string(), Value::Bool(true));
        kwargs.insert(
            "model".to_string(),
            Value::String(
                "{{ get_where_subquery(source('SWIMPLY_SWIMPLY_PROD', 'POOLS')) }}".to_string(),
            ),
        );

        let namespace = "dbt_utils".to_string();
        let hash =
            generate_test_unique_id_hash(fqn_name, "accepted_range", Some(&namespace), &kwargs);

        assert_eq!(
            hash, "1a34381004",
            "Hash must match Mantle/dbt-core for replay parity. Got: {hash}"
        );
    }

    #[test]
    fn test_generate_test_unique_id_hash_different_expressions() {
        // This is the key test: expressions '> 0' and '= 0' should produce different hashes
        // even though they would clean to the same value '_0'
        let mut kwargs_gt = BTreeMap::new();
        kwargs_gt.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('model'))".to_string()),
        );
        kwargs_gt.insert(
            "column_name".to_string(),
            Value::String("effective_conc".to_string()),
        );
        kwargs_gt.insert("expression".to_string(), Value::String("> 0".to_string()));

        let mut kwargs_eq = BTreeMap::new();
        kwargs_eq.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('model'))".to_string()),
        );
        kwargs_eq.insert(
            "column_name".to_string(),
            Value::String("effective_conc".to_string()),
        );
        kwargs_eq.insert("expression".to_string(), Value::String("= 0".to_string()));

        let namespace = Some("dbt_utils".to_string());
        let full_name = "dbt_utils_expression_is_true_model__0";

        let hash_gt = generate_test_unique_id_hash(
            full_name,
            "expression_is_true",
            namespace.as_ref(),
            &kwargs_gt,
        );
        let hash_eq = generate_test_unique_id_hash(
            full_name,
            "expression_is_true",
            namespace.as_ref(),
            &kwargs_eq,
        );

        assert_ne!(
            hash_gt, hash_eq,
            "Hashes should differ for '> 0' vs '= 0' expressions"
        );
        assert_eq!(hash_gt.len(), 10, "Hash should be 10 characters");
        assert_eq!(hash_eq.len(), 10, "Hash should be 10 characters");
    }

    #[test]
    fn test_generate_test_unique_id_hash_same_kwargs_same_hash() {
        // Same kwargs should produce same hash
        let mut kwargs = BTreeMap::new();
        kwargs.insert(
            "model".to_string(),
            Value::String("ref('model')".to_string()),
        );
        kwargs.insert("column_name".to_string(), Value::String("col".to_string()));

        let hash1 = generate_test_unique_id_hash("test_name", "unique", None, &kwargs);
        let hash2 = generate_test_unique_id_hash("test_name", "unique", None, &kwargs);

        assert_eq!(hash1, hash2, "Same kwargs should produce same hash");
    }

    #[test]
    fn test_duplicate_detection_with_different_expressions() {
        // Simulate what happens with two expression_is_true tests with '> 0' and '= 0'
        // Both clean to '_0' in the test name, but should have different unique_ids

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "stg_model".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        // Test 1: expression '> 0'
        let mut kwargs1 = BTreeMap::new();
        kwargs1.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('stg_model'))".to_string()),
        );
        kwargs1.insert(
            "column_name".to_string(),
            Value::String("effective_conc".to_string()),
        );
        kwargs1.insert("expression".to_string(), Value::String("> 0".to_string()));

        // Test 2: expression '= 0'
        let mut kwargs2 = BTreeMap::new();
        kwargs2.insert(
            "model".to_string(),
            Value::String("get_where_subquery(ref('stg_model'))".to_string()),
        );
        kwargs2.insert(
            "column_name".to_string(),
            Value::String("effective_conc".to_string()),
        );
        kwargs2.insert("expression".to_string(), Value::String("= 0".to_string()));

        let namespace = Some("dbt_utils".to_string());
        let mut test_name_truncations = HashMap::new();

        // Generate test names (these will be the same due to cleaning)
        let name1 = generate_test_name(
            "expression_is_true",
            None,
            "my_project",
            &test_config,
            &kwargs1,
            namespace.as_ref(),
            &BTreeMap::new(),
            &mut test_name_truncations,
        );
        let name2 = generate_test_name(
            "expression_is_true",
            None,
            "my_project",
            &test_config,
            &kwargs2,
            namespace.as_ref(),
            &BTreeMap::new(),
            &mut test_name_truncations,
        );

        // Names should be identical (both clean to '_0')
        assert_eq!(name1, name2, "Cleaned names should be identical");

        // But unique_ids should differ
        let hash1 = generate_test_unique_id_hash(
            &name1,
            "expression_is_true",
            namespace.as_ref(),
            &kwargs1,
        );
        let hash2 = generate_test_unique_id_hash(
            &name2,
            "expression_is_true",
            namespace.as_ref(),
            &kwargs2,
        );

        let unique_id1 = format!("{}.{}", name1, hash1);
        let unique_id2 = format!("{}.{}", name2, hash2);

        assert_ne!(
            unique_id1, unique_id2,
            "Unique IDs should differ even when cleaned names are the same"
        );

        // Verify that using unique_id for duplicate detection would NOT flag these as duplicates
        let mut seen_tests = HashSet::new();
        assert!(
            seen_tests.insert(unique_id1),
            "First test should be inserted"
        );
        assert!(
            seen_tests.insert(unique_id2),
            "Second test should also be inserted (not a duplicate)"
        );
    }

    #[test]
    fn test_versioned_ref_is_quoted_in_persisted_generic_tests() {
        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "turmoenster".to_string(),
            version_num: Some("1_1".to_string()),
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let details = get_test_details(
            &DataTests::String("not_null".to_string().into()),
            &test_config,
            None,
            None,
            false,
            LegacyTestSyntaxHandling::Strict,
            &BTreeSet::new(),
        )
        .unwrap();

        let model = details
            .kwargs
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap();
        assert_eq!(
            model, "{{ get_where_subquery(ref('turmoenster', version='1_1')) }}",
            "Version should be emitted as a quoted string to avoid Jinja interpreting 1_1 as numeric 11"
        );
        assert!(
            !model.contains("version=1_1"),
            "Unquoted v=1_1 would be parsed by Jinja as the numeric literal 11"
        );
    }

    #[test]
    fn test_persist_creates_intermediate_dirs_for_explicit_test_name_with_slash() {
        let out_dir = tempfile::tempdir().unwrap();
        let io_args = IoArgs {
            out_dir: out_dir.path().to_path_buf(),
            ..Default::default()
        };

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "customers".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let test = DataTests::CustomTest(
            CustomTest::MultiKey(Box::new(CustomTestMultiKey {
                test_name: "not_null".to_string(),
                name: Some("customer/id_not_null".to_string()),
                description: None,
                config: None,
                column_name: None,
                arguments: Verbatim::from(None),
                __deprecated_args_and_configs__: Verbatim::from(BTreeMap::new()),
            }))
            .into(),
        );

        let test_asset = persist_inner(
            "project_name",
            "project_name",
            &test_config,
            Some("id".to_string()),
            Some(StringOrArrayOfStrings::String("id".to_string())),
            &test,
            &io_args,
            Path::new("models/schema.yml"),
            &mut HashSet::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            &[],
            false,
            LegacyTestSyntaxHandling::Strict,
            RawTestConfig::default(),
        )
        .expect("persist_inner should create missing intermediate directories, not fail");

        assert_eq!(test_asset.test_name, "customer/id_not_null");
        let written_path = out_dir
            .path()
            .join(DBT_GENERIC_TESTS_DIR_NAME)
            .join("customer/id_not_null.sql");
        assert!(
            written_path.is_file(),
            "expected generated test SQL at {}",
            written_path.display()
        );
    }

    #[test]
    fn test_persist_inner_carries_generic_test_description() {
        let out_dir = tempfile::tempdir().unwrap();
        let io_args = IoArgs {
            out_dir: out_dir.path().to_path_buf(),
            ..Default::default()
        };

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "orders".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let test = DataTests::CustomTest(
            CustomTest::MultiKey(Box::new(CustomTestMultiKey {
                test_name: "unique".to_string(),
                name: None,
                description: Some("Order id must be unique across all orders.".to_string()),
                config: None,
                column_name: None,
                arguments: Verbatim::from(None),
                __deprecated_args_and_configs__: Verbatim::from(BTreeMap::new()),
            }))
            .into(),
        );

        let test_asset = persist_inner(
            "project_name",
            "project_name",
            &test_config,
            Some("id".to_string()),
            Some(StringOrArrayOfStrings::String("id".to_string())),
            &test,
            &io_args,
            Path::new("models/schema.yml"),
            &mut HashSet::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            &[],
            false,
            LegacyTestSyntaxHandling::Strict,
            RawTestConfig::default(),
        )
        .expect("persist_inner should succeed");

        assert_eq!(
            test_asset.description,
            Some("Order id must be unique across all orders.".to_string())
        );
    }

    #[test]
    fn test_persist_inner_bare_string_test_has_no_description() {
        // The `- unique` bare form has no description slot at all; a generic test
        // authored this way must not silently inherit some other value.
        let out_dir = tempfile::tempdir().unwrap();
        let io_args = IoArgs {
            out_dir: out_dir.path().to_path_buf(),
            ..Default::default()
        };

        let test_config = GenericTestConfig {
            resource_type: "model".to_string(),
            resource_name: "orders".to_string(),
            version_num: None,
            model_tests: None,
            column_tests: None,
            source_name: None,
        };

        let test = DataTests::String("unique".to_string().into());

        let test_asset = persist_inner(
            "project_name",
            "project_name",
            &test_config,
            Some("id".to_string()),
            Some(StringOrArrayOfStrings::String("id".to_string())),
            &test,
            &io_args,
            Path::new("models/schema.yml"),
            &mut HashSet::new(),
            &mut HashMap::new(),
            &mut HashMap::new(),
            &[],
            false,
            LegacyTestSyntaxHandling::Strict,
            RawTestConfig::default(),
        )
        .expect("persist_inner should succeed");

        assert_eq!(test_asset.description, None);
    }
}
