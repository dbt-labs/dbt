use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use dbt_common::constants::DBT_GENERIC_TESTS_DIR_NAME;
use dbt_common::path::DbtPath;
use dbt_common::string_utils::maybe_truncate_test_name;
use dbt_common::{CodeLocationWithFile, ErrorCode, FsResult, fs_err, stdfs};
use dbt_jinja_utils::jinja_arg_format::format_value_for_jinja;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::Nodes;
use dbt_schemas::schemas::common::{DbtChecksum, Severity};
use dbt_schemas::schemas::nodes::{DbtTest, TestMetadata};
use dbt_schemas::schemas::profiles::Execute;
use dbt_schemas::schemas::project::{
    DEFAULT_DATA_TEST_ERROR_IF, DEFAULT_DATA_TEST_FAIL_CALC, DEFAULT_DATA_TEST_SEVERITY,
    DEFAULT_DATA_TEST_WARN_IF,
};

// NOTE: This module intentionally mirrors the legacy resolve-phase aggregation logic
// but runs at task time and only considers selected, enabled generic column tests.

#[derive(Debug, Clone)]
pub struct GenericTest {
    pub unique_id: String,
    pub schema: String,
    pub alias: String,
    pub column_name: String,
    pub severity: Option<Severity>,
    pub defined_at: Option<CodeLocationWithFile>,
}

#[derive(Debug, Clone)]
pub struct GenericTestGroup {
    pub unique_id: String,
    pub name: String,
    /// Generic test macro shared by every member of the group, e.g. `not_null`.
    pub macro_name: String,
    /// unique_id of the node the group's tests are attached to.
    pub attached_node: String,
    pub aggregated_test: Arc<DbtTest>,
    pub member_tests: Vec<Arc<DbtTest>>,
    pub tests: Vec<GenericTest>,
}

#[derive(Debug, Clone, Default)]
pub struct GenericTestRelationships {
    // Map from test unique ID to test group name
    pub group_names: HashMap<String, String>,
    // Map from test group name to list of test unique IDs
    pub unique_ids: HashMap<String, Vec<String>>,
    // Map from test group name and normalized column name to GenericTest
    pub tests: HashMap<String, HashMap<String, GenericTest>>,
}

#[derive(Debug, Default, Clone)]
pub struct GenericTestAggregation {
    pub groups: BTreeMap<String, Arc<GenericTestGroup>>,
    pub group_ids: HashMap<String, String>,
    pub relationships: GenericTestRelationships,
}

impl GenericTestAggregation {
    pub fn generic_test_group_for_node(&self, unique_id: &str) -> Option<&Arc<GenericTestGroup>> {
        self.groups.get(unique_id).or_else(|| {
            self.group_ids
                .get(unique_id)
                .and_then(|group_id| self.groups.get(group_id))
        })
    }
}

fn has_safe_data_test_config(test: &DbtTest) -> bool {
    let config = &test.deprecated_config;
    let enabled = config.enabled.is_none_or(|enabled| enabled);
    let safe = config
        .fail_calc
        .as_deref()
        .is_none_or(|fail_calc| fail_calc == DEFAULT_DATA_TEST_FAIL_CALC)
        && config.limit.is_none()
        && config
            .severity
            .as_ref()
            .is_none_or(|severity| severity == &DEFAULT_DATA_TEST_SEVERITY)
        && config
            .error_if
            .as_deref()
            .is_none_or(|error_if| error_if == DEFAULT_DATA_TEST_ERROR_IF)
        && config
            .warn_if
            .as_deref()
            .is_none_or(|warn_if| warn_if == DEFAULT_DATA_TEST_WARN_IF)
        && config.store_failures.is_none()
        && config.store_failures_as.is_none()
        && config.where_.is_none();

    enabled && safe
}

fn is_data_test_aggregation_eligible(test: &DbtTest) -> bool {
    get_macro_name(test)
        .is_some_and(|macro_name| matches!(macro_name.as_str(), "unique" | "not_null"))
        && has_safe_data_test_config(test)
}

/// True if the test resolves to dbt's built-in `test_<name>` macro (direct overrides only; dispatch targets not covered).
fn depends_on_builtin_test_macro(test: &DbtTest, name: &str) -> bool {
    // Built-in test macros are keyed `macro.dbt.test_<name>`.
    let builtin = format!("macro.dbt.test_{name}");
    test.__base_attr__.depends_on.macros.contains(&builtin)
}

pub fn is_data_test_static_analysis_eligible(test: &DbtTest) -> bool {
    let Some(metadata) = test.__test_attr__.test_metadata.as_ref() else {
        return false;
    };
    let eligible = matches!(
        metadata.name.as_str(),
        "unique" | "not_null" | "accepted_values"
    ) && depends_on_builtin_test_macro(test, &metadata.name);

    eligible && has_safe_data_test_config(test)
}

fn get_test_group_key(test: &DbtTest) -> Option<(String, String)> {
    if !is_data_test_aggregation_eligible(test) {
        return None;
    }

    let resource_name = test.__test_attr__.attached_node.clone()?;
    let macro_name = get_macro_name(test)?;
    Some((resource_name, macro_name))
}

/// Generates test group name using the same conventions as persist_generic_data_tests.
fn get_group_name(
    macro_name: &str,
    resource_name_display: &str,
    column_names: &[String],
) -> String {
    use regex::Regex;
    use std::sync::LazyLock;

    static CLEAN_REGEX: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"[^0-9a-zA-Z_]+").expect("valid regex"));

    let test_identifier = format!("{macro_name}_{resource_name_display}");

    // Clean column names and join them
    let clean_columns: Vec<String> = column_names
        .iter()
        .map(|col| {
            CLEAN_REGEX
                .replace_all(col.trim_matches('"'), "_")
                .to_string()
        })
        .collect();

    let suffix = clean_columns.join("__");

    maybe_truncate_test_name(&test_identifier, &format!("{test_identifier}_{suffix}"))
}

fn get_macro_name(test: &DbtTest) -> Option<String> {
    let metadata = test.__test_attr__.test_metadata.as_ref()?;
    let macro_name = metadata.name.clone();
    Some(macro_name)
}

fn get_column_name(test: &DbtTest) -> Option<String> {
    let metadata = test.__test_attr__.test_metadata.as_ref()?;
    let column_name = metadata
        .kwargs
        .get("column_name")
        .and_then(|v| v.as_str())?
        .to_string();
    Some(column_name)
}

/// Normalize column name the same way resolve phase did.
pub fn normalize_column_name(column_name: &str) -> String {
    column_name.trim_matches('"').to_ascii_lowercase()
}

fn create_generic_test_relationships(
    test_groups: &BTreeMap<String, Arc<GenericTestGroup>>,
) -> GenericTestRelationships {
    let mut relationships = GenericTestRelationships::default();

    for test_group in test_groups.values() {
        let group_name = test_group.name.clone();

        relationships.unique_ids.insert(
            group_name.clone(),
            test_group
                .tests
                .iter()
                .map(|m| m.unique_id.clone())
                .collect(),
        );

        for test in &test_group.tests {
            relationships
                .group_names
                .insert(test.unique_id.clone(), group_name.clone());

            let column_name = normalize_column_name(&test.column_name);
            relationships
                .tests
                .entry(group_name.clone())
                .or_default()
                .insert(column_name, test.clone());
        }
    }

    relationships
}

/// Create test aggregation from resolved nodes and the current schedule.
///
/// Only selected, enabled generic column tests are considered.
pub fn create_generic_test_aggregation(
    io: &dbt_common::io_args::IoArgs,
    schedule: &dbt_dag::schedule::Schedule<String>,
    nodes: &Nodes,
    execute: Execute,
) -> FsResult<Option<GenericTestAggregation>> {
    if execute != Execute::Remote {
        return Ok(None);
    }

    // Collect eligible tests keyed by (attached_node, macro_name)
    let mut grouped_tests: HashMap<(String, String), Vec<Arc<DbtTest>>> = HashMap::new();

    for unique_id in &schedule.selected_nodes {
        let Some(test) = nodes.tests.get(unique_id) else {
            continue;
        };
        let Some((resource_name, macro_name)) = get_test_group_key(test) else {
            continue;
        };
        grouped_tests
            .entry((resource_name, macro_name))
            .or_default()
            .push(test.clone());
    }

    let mut groups: BTreeMap<String, Arc<GenericTestGroup>> = BTreeMap::new();
    let mut group_ids = HashMap::new();

    for ((resource_name, macro_name), member_tests) in grouped_tests {
        // Too few to aggregate
        if member_tests.len() < 2 {
            continue;
        }

        let mut tests = Vec::with_capacity(member_tests.len());
        let mut column_names = Vec::new();

        for test in &member_tests {
            let column_name = get_column_name(test.as_ref()).expect("checked");
            column_names.push(column_name.clone());

            let test = GenericTest {
                unique_id: test.common().unique_id.clone(),
                schema: test.base().schema.clone(),
                alias: test.base().alias.clone(),
                column_name: column_name.clone(),
                severity: test.deprecated_config.severity.clone(),
                defined_at: test.defined_at.clone(),
            };
            tests.push(test);
        }

        let group_name = get_group_name(
            &format!("aggregated_{macro_name}"),
            &resource_name.replace('.', "_"),
            &column_names,
        );
        let group_id = format!(
            "test.{}.{}",
            member_tests[0].common().package_name,
            group_name
        );

        // Synthesize (aggregated) group test node
        let aggregated_test =
            create_aggregated_test(&group_name, &group_id, &member_tests[0], &column_names, io)?;

        let group = GenericTestGroup {
            unique_id: group_id.clone(),
            name: group_name.clone(),
            macro_name,
            attached_node: resource_name,
            aggregated_test: Arc::new(aggregated_test),
            tests: tests.clone(),
            member_tests: member_tests.clone(),
        };

        for test in &tests {
            group_ids.insert(test.unique_id.clone(), group_id.clone());
        }

        groups.insert(group_id, Arc::new(group));
    }

    if groups.is_empty() {
        return Ok(None);
    }

    let relationships = create_generic_test_relationships(&groups);

    Ok(Some(GenericTestAggregation {
        groups,
        group_ids,
        relationships,
    }))
}

fn create_aggregated_test(
    test_group_name: &str,
    test_group_id: &str,
    template: &DbtTest,
    columns: &[String],
    io_args: &dbt_common::io_args::IoArgs,
) -> FsResult<DbtTest> {
    let path = PathBuf::from(DBT_GENERIC_TESTS_DIR_NAME).join(format!("{test_group_name}.sql"));
    let absolute_path = io_args.out_dir.join(&path);

    let template_metadata = template
        .__test_attr__
        .test_metadata
        .as_ref()
        .ok_or_else(|| {
            fs_err!(
                ErrorCode::Unexpected,
                "Generic test aggregation requires test metadata"
            )
        })?;
    let model = template_metadata.kwargs.get("model").ok_or_else(|| {
        fs_err!(
            ErrorCode::Unexpected,
            "Generic test aggregation requires test metadata with a model argument"
        )
    })?;
    let mut kwargs = template_metadata.kwargs.clone();
    kwargs.insert(
        "column_names".to_string(),
        dbt_yaml::Value::Sequence(
            columns
                .iter()
                .map(|c| dbt_yaml::Value::string(c.clone()))
                .collect(),
            dbt_yaml::Span::default(),
        ),
    );

    let test_metadata = TestMetadata {
        name: format!("aggregated_{}", template_metadata.name.clone()),
        kwargs,
        namespace: None,
    };

    let raw_code = build_aggregated_raw_code(&test_metadata.name, model, columns, test_group_name)?;

    // write SQL to target so render_sql_instruction can read it
    if let Some(parent) = absolute_path.parent() {
        stdfs::create_dir_all(parent)?;
    }
    stdfs::write(&absolute_path, &raw_code)?;

    let mut test = template.clone();

    test.__common_attr__.name = test_group_name.to_string();
    test.__common_attr__.unique_id = test_group_id.to_string();
    test.__common_attr__.path = DbtPath::from(path);
    test.__common_attr__.original_file_path = DbtPath::from(&absolute_path);
    test.manifest_original_file_path = DbtPath::from(&absolute_path);
    test.__common_attr__.raw_code = Some(raw_code.clone());
    test.__common_attr__.checksum = DbtChecksum::hash(raw_code.trim().as_bytes());
    test.__common_attr__.fqn = vec![
        test.__common_attr__.package_name.clone(),
        "test".to_string(),
        test_group_name.to_string(),
    ];

    // update base attributes
    test.__base_attr__.alias = test_group_name.to_string();
    test.__base_attr__.relation_name = None;
    test.__base_attr__.static_analysis_off_reason = None;

    // metadata
    test.__test_attr__.column_name = None;
    test.__test_attr__.test_metadata = Some(test_metadata);

    Ok(test)
}

fn build_aggregated_raw_code(
    test_name: &str,
    model: &dbt_yaml::Value,
    columns: &[String],
    alias: &str,
) -> FsResult<String> {
    let model_json = serde_json::to_value(model).map_err(|e| {
        fs_err!(
            ErrorCode::Unexpected,
            "Failed to serialize aggregated test model argument: {}",
            e
        )
    })?;
    if model_json.is_object() {
        return Err(fs_err!(
            ErrorCode::Unexpected,
            "Aggregated test arguments do not support object values"
        ));
    }
    let column_names_json = serde_json::to_value(columns).map_err(|e| {
        fs_err!(
            ErrorCode::Unexpected,
            "Failed to serialize aggregated test column_names argument: {}",
            e
        )
    })?;
    let jinja_set_vars = BTreeMap::new();
    let model_arg = format_value_for_jinja(&model_json, &jinja_set_vars);
    let column_names_arg = format_value_for_jinja(&column_names_json, &jinja_set_vars);
    let alias_arg = serde_json::to_string(alias).expect("string serialization should not fail");

    Ok(format!(
        "{{{{ test_{test_name}(model={model_arg}, column_names={column_names_arg}) }}}}{{{{ config(alias={alias_arg}) }}}}",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    use dbt_schemas::schemas::common::StoreFailuresAs;
    use dbt_schemas::schemas::nodes::{DbtTestAttr, Nodes};

    fn test_node(unique_id: &str, macro_name: &str, column_name: &str) -> DbtTest {
        let mut test = DbtTest::default();
        test.__common_attr__.unique_id = unique_id.to_string();
        test.__common_attr__.name = unique_id.to_string();
        test.__common_attr__.package_name = "pkg".to_string();
        test.__base_attr__.schema = "dbt_test__audit".to_string();
        test.__base_attr__.alias = unique_id.replace('.', "_");
        // Represent a built-in generic test: its resolved macro is dbt's `test_<name>`.
        test.__base_attr__.depends_on.macros = vec![format!("macro.dbt.test_{macro_name}")];
        test.__test_attr__ = DbtTestAttr {
            column_name: Some(column_name.to_string()),
            attached_node: Some("model.pkg.orders".to_string()),
            test_metadata: Some(TestMetadata {
                name: macro_name.to_string(),
                kwargs: BTreeMap::from([
                    (
                        "column_name".to_string(),
                        dbt_yaml::Value::string(column_name.to_string()),
                    ),
                    (
                        "model".to_string(),
                        dbt_yaml::Value::string(
                            "{{ get_where_subquery(ref('orders')) }}".to_string(),
                        ),
                    ),
                ]),
                namespace: None,
            }),
            ..Default::default()
        };
        test
    }

    fn resolved_default_config(test: &mut DbtTest) {
        test.deprecated_config.enabled = Some(true);
        test.deprecated_config.severity = Some(DEFAULT_DATA_TEST_SEVERITY.clone());
        test.deprecated_config.fail_calc = Some(DEFAULT_DATA_TEST_FAIL_CALC.to_string());
        test.deprecated_config.error_if = Some(DEFAULT_DATA_TEST_ERROR_IF.to_string());
        test.deprecated_config.warn_if = Some(DEFAULT_DATA_TEST_WARN_IF.to_string());
    }

    fn schedule_and_nodes(tests: Vec<DbtTest>) -> (dbt_dag::schedule::Schedule<String>, Nodes) {
        let mut schedule = dbt_dag::schedule::Schedule::default();
        let mut nodes = Nodes::default();

        for test in tests {
            let unique_id = test.common().unique_id.clone();
            schedule.selected_nodes.insert(unique_id.clone());
            nodes.tests.insert(unique_id, Arc::new(test));
        }

        (schedule, nodes)
    }

    #[test]
    fn aggregated_raw_code_quotes_column_names_but_preserves_model_expression() {
        let template = test_node("test.pkg.not_null_orders_id", "not_null", "id");
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let io = dbt_common::io_args::IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };
        let columns = vec!["id".to_string(), "order_total".to_string()];

        let test = create_aggregated_test(
            "aggregated_not_null_orders",
            "test.pkg.aggregated_not_null_orders",
            &template,
            &columns,
            &io,
        )
        .expect("aggregated test");

        assert_eq!(
            test.__common_attr__.raw_code.as_deref(),
            Some(
                "{{ test_aggregated_not_null(model=get_where_subquery(ref('orders')), column_names=[\"id\",\"order_total\"]) }}{{ config(alias=\"aggregated_not_null_orders\") }}"
            )
        );
    }

    #[test]
    fn aggregated_raw_code_escapes_alias_as_string_literal() {
        let template = test_node("test.pkg.not_null_orders_id", "not_null", "id");
        let template_metadata = template
            .__test_attr__
            .test_metadata
            .as_ref()
            .expect("test metadata");
        let model = template_metadata.kwargs.get("model").expect("model");
        let columns = ["id".to_string()];
        let raw_code = build_aggregated_raw_code(
            "aggregated_not_null",
            model,
            &columns,
            "aggregated_not_null_orders\" }}{{ var('secret') }}{{ \"",
        )
        .expect("raw code");

        assert_eq!(
            raw_code,
            "{{ test_aggregated_not_null(model=get_where_subquery(ref('orders')), column_names=[\"id\"]) }}{{ config(alias=\"aggregated_not_null_orders\\\" }}{{ var('secret') }}{{ \\\"\") }}"
        );
    }

    /// Group iteration order reaches `Schedule::sorted_nodes` and therefore the order of
    /// console result lines, so it must not depend on how the groups were inserted.
    #[test]
    fn aggregation_groups_iterate_in_sorted_group_id_order() {
        let mut tests = Vec::new();
        for model in ["orders", "customers", "payments", "products"] {
            for macro_name in ["not_null", "unique"] {
                for column in ["id", "code"] {
                    let mut test = test_node(
                        &format!("test.pkg.{macro_name}_{model}_{column}"),
                        macro_name,
                        column,
                    );
                    test.__test_attr__.attached_node = Some(format!("model.pkg.{model}"));
                    resolved_default_config(&mut test);
                    tests.push(test);
                }
            }
        }

        let (schedule, nodes) = schedule_and_nodes(tests);
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let io = dbt_common::io_args::IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let aggregation = create_generic_test_aggregation(&io, &schedule, &nodes, Execute::Remote)
            .expect("aggregation")
            .expect("aggregated groups");

        // 4 models x 2 macros, each with 2 member columns.
        let group_ids: Vec<String> = aggregation.groups.keys().cloned().collect();
        assert_eq!(group_ids.len(), 8);

        let mut sorted_group_ids = group_ids.clone();
        sorted_group_ids.sort();
        assert_eq!(
            group_ids, sorted_group_ids,
            "groups must iterate in sorted group-id order"
        );
    }

    #[test]
    fn resolved_default_config_tests_aggregate() {
        for macro_name in ["not_null", "unique"] {
            let mut test_a = test_node(
                &format!("test.pkg.{macro_name}_orders_id"),
                macro_name,
                "id",
            );
            let mut test_b = test_node(
                &format!("test.pkg.{macro_name}_orders_email"),
                macro_name,
                "email",
            );
            resolved_default_config(&mut test_a);
            resolved_default_config(&mut test_b);
            assert!(is_data_test_aggregation_eligible(&test_a));
            assert!(is_data_test_aggregation_eligible(&test_b));

            let (schedule, nodes) = schedule_and_nodes(vec![test_a, test_b]);
            let temp_dir = tempfile::tempdir().expect("temp dir");
            let io = dbt_common::io_args::IoArgs {
                out_dir: temp_dir.path().to_path_buf(),
                ..Default::default()
            };

            let aggregation =
                create_generic_test_aggregation(&io, &schedule, &nodes, Execute::Remote)
                    .expect("aggregation")
                    .expect("aggregated group");
            assert_eq!(aggregation.groups.len(), 1);

            let group = aggregation.groups.values().next().expect("group");
            assert_eq!(group.member_tests.len(), 2);
            assert!(group.name.starts_with(&format!("aggregated_{macro_name}_")));

            let aggregated_sql = temp_dir
                .path()
                .join(DBT_GENERIC_TESTS_DIR_NAME)
                .join(format!("{}.sql", group.name));
            assert!(aggregated_sql.exists());
        }
    }

    #[test]
    fn custom_config_tests_are_not_aggregatable() {
        let cases: Vec<(&str, fn(&mut DbtTest))> = vec![
            ("severity", |test| {
                test.deprecated_config.severity = Some(Severity::Warn)
            }),
            ("fail_calc", |test| {
                test.deprecated_config.fail_calc = Some("sum(failures)".to_string())
            }),
            ("error_if", |test| {
                test.deprecated_config.error_if = Some("> 10".to_string())
            }),
            ("warn_if", |test| {
                test.deprecated_config.warn_if = Some("> 0".to_string())
            }),
            ("limit", |test| test.deprecated_config.limit = Some(1)),
            ("where", |test| {
                test.deprecated_config.where_ = Some("id is not null".to_string())
            }),
            ("store_failures", |test| {
                test.deprecated_config.store_failures = Some(false)
            }),
            ("store_failures_as", |test| {
                test.deprecated_config.store_failures_as = Some(StoreFailuresAs::Table)
            }),
        ];

        for (name, mutate) in cases {
            let mut test = test_node(
                &format!("test.pkg.not_null_orders_{name}"),
                "not_null",
                name,
            );
            resolved_default_config(&mut test);
            mutate(&mut test);
            assert!(
                !is_data_test_aggregation_eligible(&test),
                "{name} should make the test ineligible"
            );
        }
    }

    #[test]
    fn accepted_values_is_only_eligible_for_static_analysis() {
        let mut test = test_node(
            "test.pkg.accepted_values_orders_status",
            "accepted_values",
            "status",
        );
        resolved_default_config(&mut test);

        assert!(is_data_test_static_analysis_eligible(&test));
        assert!(!is_data_test_aggregation_eligible(&test));
        assert!(get_test_group_key(&test).is_none());
    }

    #[test]
    fn custom_config_prevents_accepted_values_static_analysis() {
        let cases: Vec<(&str, fn(&mut DbtTest))> = vec![
            ("severity", |test| {
                test.deprecated_config.severity = Some(Severity::Warn)
            }),
            ("fail_calc", |test| {
                test.deprecated_config.fail_calc = Some("sum(failures)".to_string())
            }),
            ("error_if", |test| {
                test.deprecated_config.error_if = Some("> 10".to_string())
            }),
            ("warn_if", |test| {
                test.deprecated_config.warn_if = Some("> 0".to_string())
            }),
            ("limit", |test| test.deprecated_config.limit = Some(1)),
            ("where", |test| {
                test.deprecated_config.where_ = Some("status is not null".to_string())
            }),
            ("store_failures", |test| {
                test.deprecated_config.store_failures = Some(false)
            }),
            ("store_failures_as", |test| {
                test.deprecated_config.store_failures_as = Some(StoreFailuresAs::Table)
            }),
        ];

        for (name, mutate) in cases {
            let mut test = test_node(
                &format!("test.pkg.accepted_values_orders_{name}"),
                "accepted_values",
                "status",
            );
            resolved_default_config(&mut test);
            mutate(&mut test);
            assert!(
                !is_data_test_static_analysis_eligible(&test),
                "{name} should make the test ineligible"
            );
        }
    }

    #[test]
    fn static_analysis_eligibility_requires_builtin_test_macro() {
        // Build a safe-config test node whose resolved macro dependency is `dep`.
        // Note: namespace is no longer consulted; the depends_on id alone decides.
        let make = |name: &str, dep: String| {
            let mut test = test_node("test.pkg.orders_status", name, "status");
            resolved_default_config(&mut test);
            test.__base_attr__.depends_on.macros = vec![dep];
            test
        };

        for name in ["accepted_values", "unique", "not_null"] {
            // 1. Built-in macro (macro.dbt.test_<name>) is eligible.
            let builtin = make(name, format!("macro.dbt.test_{name}"));
            assert!(
                is_data_test_static_analysis_eligible(&builtin),
                "built-in {name} should be eligible"
            );

            // 2. Root-project override (no package prefix) is not eligible.
            let overridden = make(name, format!("macro.my_project.test_{name}"));
            assert!(
                !is_data_test_static_analysis_eligible(&overridden),
                "overridden {name} should not be eligible"
            );
        }

        // 3. Namespaced package version (e.g. dbt_utils) is not eligible.
        let namespaced = make(
            "accepted_values",
            "macro.dbt_utils.test_accepted_values".to_string(),
        );
        assert!(
            !is_data_test_static_analysis_eligible(&namespaced),
            "namespaced accepted_values should not be eligible"
        );

        // 4. Empty depends_on.macros: conservative fallback, not eligible.
        let mut empty = make("accepted_values", String::new());
        empty.__base_attr__.depends_on.macros = Vec::new();
        assert!(
            !is_data_test_static_analysis_eligible(&empty),
            "unconfirmable built-in should not be eligible"
        );
    }

    #[test]
    fn custom_config_prevents_group_creation() {
        let mut test_a = test_node("test.pkg.unique_orders_id", "unique", "id");
        let mut test_b = test_node("test.pkg.unique_orders_email", "unique", "email");
        resolved_default_config(&mut test_a);
        resolved_default_config(&mut test_b);
        test_b.deprecated_config.warn_if = Some("> 0".to_string());

        let (schedule, nodes) = schedule_and_nodes(vec![test_a, test_b]);
        let temp_dir = tempfile::tempdir().expect("temp dir");
        let io = dbt_common::io_args::IoArgs {
            out_dir: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let aggregation = create_generic_test_aggregation(&io, &schedule, &nodes, Execute::Remote)
            .expect("aggregation");
        assert!(aggregation.is_none());
    }
}
