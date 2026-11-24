use adbc_core::options::OptionValue;
use dbt_common::{AdapterError, AdapterErrorKind, AdapterResult};
use dbt_schemas::yml_view::YmlView;
use indexmap::IndexMap;
use minijinja::State;
use serde_json::Value;

const QUERY_TAG_OPTION_PREFIX: &str = "databricks.query_tag.";
const DBT_CORE_VERSION: &str = "@@dbt_core_version";
const DBT_MODEL_NAME: &str = "@@dbt_model_name";
const DBT_MATERIALIZED: &str = "@@dbt_materialized";
const MAX_AUTOMATIC_VALUE_CHARS: usize = 128;
const RESERVED_KEYS: [&str; 3] = [DBT_CORE_VERSION, DBT_MODEL_NAME, DBT_MATERIALIZED];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryTags {
    model_name: Option<String>,
    materialized: Option<String>,
    model_tags: IndexMap<String, String>,
}

impl QueryTags {
    fn from_node(
        model_query_tags: Option<&str>,
        model_name: Option<&str>,
        materialized: Option<&str>,
    ) -> AdapterResult<Self> {
        fn truncate_default(value: &str) -> String {
            value.chars().take(MAX_AUTOMATIC_VALUE_CHARS).collect()
        }

        let model_tags = parse_user_tags(model_query_tags, "Model config")?;
        let model_name = model_name.map(truncate_default);
        let materialized = materialized.map(truncate_default);

        Ok(Self {
            model_name,
            materialized,
            model_tags,
        })
    }

    pub fn from_state(state: Option<&State>) -> AdapterResult<Option<Self>> {
        let Some(node) = state.and_then(|state| state.lookup("model", &[])) else {
            return Ok(None);
        };
        let yaml_node = dbt_yaml::to_value(&node).map_err(|error| {
            AdapterError::new(AdapterErrorKind::Configuration, error.to_string())
        })?;
        let Some(node) = QueryTagsNodeView::new(&yaml_node) else {
            return Ok(None);
        };
        let query_tags = Self::from_node(
            node.raw_query_tags(),
            Some(node.name()),
            node.materialized(),
        )?;
        Ok(Some(query_tags))
    }

    pub fn for_each_stmt_option(self, mut f: impl FnMut(String, OptionValue)) {
        let mut emit = |key: &str, value: String| {
            let name = format!("{QUERY_TAG_OPTION_PREFIX}{key}");
            let value = OptionValue::String(value);
            f(name, value);
        };
        if let Some(model_name) = self.model_name {
            emit(DBT_MODEL_NAME, model_name);
        }
        if let Some(materialized) = self.materialized {
            emit(DBT_MATERIALIZED, materialized);
        }
        self.model_tags
            .into_iter()
            .for_each(|(key, value)| emit(&key, value));
    }

    pub fn extend_stmt_options(self, options: &mut IndexMap<String, OptionValue>) {
        self.for_each_stmt_option(|name, value| {
            options.insert(name, value);
        });
    }

    pub fn into_stmt_options(self) -> Vec<(String, OptionValue)> {
        let mut out = IndexMap::with_capacity(self.model_tags.len() + 2);
        self.extend_stmt_options(&mut out);
        out.into_iter().collect()
    }
}

/// Where a node keeps the fields Databricks query tags read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueryTagsNodeKind {
    /// Models, tests and snapshots: typed `databricks_attr`.
    Typed,
    // TODO: Remove these after seeds and unit tests expose typed adapter attributes.
    UnitTest,
    Seed,
}

/// A serialized node, read only for the fields Databricks query tags need.
///
/// This looks the fields up in the `Value` instead of deserializing `DbtModel`,
/// `DbtSeed`, ...: those deserializers are megabytes of code per crate that
/// instantiates them. The node kind is told apart the way probing those types
/// in the order model, unit test, test, snapshot, seed would: by the keys each
/// one requires. `resource_type` is not consulted, and anything with the
/// common node keys and none of the specific ones reads as a seed.
#[derive(Clone, Copy, Debug)]
struct QueryTagsNodeView<'a> {
    node: YmlView<'a>,
    kind: QueryTagsNodeKind,
    name: &'a str,
}

impl<'a> QueryTagsNodeView<'a> {
    /// Returns `None` for values that are not a node that carries query tags.
    fn new(yaml_node: &'a dbt_yaml::Value) -> Option<Self> {
        /// Keys only `DbtModel`, `DbtUnitTest`, `DbtTest` and `DbtSnapshot` require.
        const MODEL_KEYS: [&str; 3] = ["access", "constraints", "primary_key"];
        const UNIT_TEST_KEYS: [&str; 3] = ["model", "given", "expect"];
        const TEST_KEYS: [&str; 1] = ["manifest_original_file_path"];
        const SNAPSHOT_KEYS: [&str; 1] = ["snapshot_meta_column_names"];

        let node = YmlView::from_value(yaml_node)?;
        let name = node.get_str("name")?;
        node.get_str("materialized")?;
        let config = node.get_mapping("config")?;
        if !Self::is_str_or_null(config, "query_tags") {
            return None;
        }

        fn has_keys(node: YmlView<'_>, keys: &[&str]) -> bool {
            keys.iter().all(|key| node.contains_key(key))
        }

        // Every config but the unit test's has a typed `materialized`.
        let typed_ok = Self::is_str_or_null(config, "materialized");
        let typed_attr_ok = typed_ok && Self::databricks_attr_ok(node);
        let kind = if typed_attr_ok && has_keys(node, &MODEL_KEYS) {
            QueryTagsNodeKind::Typed
        } else if has_keys(node, &UNIT_TEST_KEYS) {
            QueryTagsNodeKind::UnitTest
        } else if typed_attr_ok && (has_keys(node, &TEST_KEYS) || has_keys(node, &SNAPSHOT_KEYS)) {
            QueryTagsNodeKind::Typed
        } else if typed_ok {
            QueryTagsNodeKind::Seed
        } else {
            return None;
        };
        Some(Self { node, kind, name })
    }

    fn is_str_or_null(mapping: YmlView<'_>, field: &str) -> bool {
        mapping
            .get(field)
            .is_none_or(|value| value.is_null() || value.as_str().is_some())
    }

    /// Whether `databricks_attr` is absent, null, or a mapping whose
    /// `query_tags` is a string or null.
    fn databricks_attr_ok(node: YmlView<'_>) -> bool {
        match node.get("databricks_attr") {
            None => true,
            Some(value) if value.is_null() => true,
            Some(value) => YmlView::from_value(value)
                .is_some_and(|attr| Self::is_str_or_null(attr, "query_tags")),
        }
    }

    fn config(&self) -> Option<YmlView<'a>> {
        self.node.get_mapping("config")
    }

    pub fn name(&self) -> &'a str {
        self.name
    }

    pub fn materialized(&self) -> Option<&'a str> {
        match self.kind {
            QueryTagsNodeKind::Typed => self.node.get_str("materialized"),
            QueryTagsNodeKind::UnitTest => None,
            QueryTagsNodeKind::Seed => Some(
                self.config()
                    .and_then(|config| config.get_str("materialized"))
                    .unwrap_or("seed"),
            ),
        }
    }

    /// Raw, unparsed `query_tags` JSON as configured on the node.
    pub fn raw_query_tags(&self) -> Option<&'a str> {
        match self.kind {
            QueryTagsNodeKind::Typed => self
                .node
                .get_mapping("databricks_attr")
                .and_then(|attr| attr.get_str("query_tags")),
            QueryTagsNodeKind::UnitTest | QueryTagsNodeKind::Seed => self
                .config()
                .and_then(|config| config.get_str("query_tags")),
        }
    }
}

fn parse_user_tags(
    query_tags: Option<&str>,
    source: &str,
) -> AdapterResult<IndexMap<String, String>> {
    fn config_error(message: impl Into<String>) -> AdapterError {
        AdapterError::new(AdapterErrorKind::Configuration, message.into())
    }

    let Some(query_tags) = query_tags.filter(|value| !value.is_empty()) else {
        return Ok(IndexMap::new());
    };

    let object: IndexMap<String, Value> = serde_json::from_str(query_tags).map_err(|error| {
        if error.classify() == serde_json::error::Category::Data {
            config_error("query_tags must be a JSON object (dictionary)")
        } else {
            config_error(format!("Invalid JSON in query_tags: {error}"))
        }
    })?;

    {
        let is_reserved = |key: &String| -> bool { RESERVED_KEYS.contains(&key.as_str()) };
        let has_reserved = object.keys().any(is_reserved);
        if has_reserved {
            let mut reserved = object
                .keys()
                .filter(|k| is_reserved(k))
                .cloned()
                .collect::<Vec<_>>();
            reserved.sort();
            let message = format!(
                "{source}: Cannot use reserved query tag keys: {}. Reserved keys are: {}",
                reserved.join(", "),
                RESERVED_KEYS.join(", ")
            );
            return Err(config_error(message));
        }
    }

    object
        .into_iter()
        .map(|(key, value)| {
            let Value::String(value) = value else {
                let message =format!(
                    "{source}: query_tags values must be strings for key '{key}'. Only string values are supported."
                );
                return Err(config_error(message));
            };
            Ok((key, value))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::Options;

    use super::{QUERY_TAG_OPTION_PREFIX, QueryTags, QueryTagsNodeView};
    use adbc_core::options::OptionValue;
    use dbt_common::AdapterResult;
    use dbt_schemas::schemas::{
        AdapterAttr, DbtAnalysis, DbtExposure, DbtFunction, DbtModel, DbtSeed, DbtSnapshot,
        DbtSource, DbtTest, DbtUnitTest, InternalDbtNode, common::DbtMaterialization,
        manifest::DbtOperation, nodes::DatabricksAttr,
    };
    use dbt_yaml::{Mapping, Value as YmlValue};
    use minijinja::{Environment, State, Value as MinijinjaValue};
    use serde::Deserialize;
    use std::sync::Arc;

    fn databricks_statement_options(state: &State) -> AdapterResult<Options> {
        let query_tags = QueryTags::from_state(Some(state))?.unwrap_or_default();
        let opts = query_tags.into_stmt_options();
        Ok(opts)
    }

    fn query_tags_from_yaml_node(yaml_node: &dbt_yaml::Value) -> AdapterResult<QueryTags> {
        match QueryTagsNodeView::new(yaml_node) {
            Some(node) => QueryTags::from_node(
                node.raw_query_tags(),
                Some(node.name()),
                node.materialized(),
            ),
            None => Ok(QueryTags::default()),
        }
    }

    /// The implementation before `QueryTagsNodeView`, which probes full node
    /// types with serde. Kept as the reference the view is compared against.
    fn legacy_query_tags_from_yaml_node(yaml_node: &dbt_yaml::Value) -> AdapterResult<QueryTags> {
        macro_rules! tags_for_node {
            ($node_type:ty, $node:ident) => {
                if let Ok($node) = <$node_type>::deserialize(yaml_node) {
                    let query_tags = $node
                        .__adapter_attr__
                        .databricks_attr
                        .as_deref()
                        .and_then(|attr| attr.query_tags.as_deref());
                    return QueryTags::from_node(
                        query_tags,
                        Some(&$node.__common_attr__.name),
                        Some(&$node.__base_attr__.materialized.to_string()),
                    );
                }
            };
        }

        // TODO: Remove this path after seeds and unit tests expose typed adapter attributes.
        macro_rules! tags_for_node_deprecated {
            ($node_type:ty, $node:ident, $materialized:expr) => {
                if let Ok($node) = <$node_type>::deserialize(yaml_node) {
                    let query_tags = $node
                        .deprecated_config
                        .__warehouse_specific_config__
                        .query_tags
                        .as_deref();
                    let materialized: Option<String> = $materialized;
                    return QueryTags::from_node(
                        query_tags,
                        Some(&$node.__common_attr__.name),
                        materialized.as_deref(),
                    );
                }
            };
        }

        tags_for_node!(DbtModel, model);
        tags_for_node_deprecated!(DbtUnitTest, unit_test, None);
        tags_for_node!(DbtTest, test);
        tags_for_node!(DbtSnapshot, snapshot);
        tags_for_node_deprecated!(
            DbtSeed,
            seed,
            Some(
                seed.deprecated_config
                    .materialized
                    .as_ref()
                    .unwrap_or(&DbtMaterialization::Seed)
                    .to_string()
            )
        );

        Ok(QueryTags::default())
    }

    fn string_option<'a>(options: &'a [(String, OptionValue)], name: &str) -> Option<&'a str> {
        options.iter().find_map(|(option_name, value)| {
            if option_name == name
                && let OptionValue::String(value) = value
            {
                Some(value.as_str())
            } else {
                None
            }
        })
    }

    #[test]
    fn emits_only_node_driver_statement_options() {
        let options = QueryTags::from_node(
            Some(r#"{"team":"model"}"#),
            Some("orders"),
            Some("incremental"),
        )
        .unwrap()
        .into_stmt_options();

        assert_eq!(options.len(), 3);
        assert_eq!(
            options
                .iter()
                .map(|(name, _)| name.as_str())
                .collect::<Vec<_>>(),
            [
                "databricks.query_tag.@@dbt_model_name",
                "databricks.query_tag.@@dbt_materialized",
                "databricks.query_tag.team",
            ]
        );

        assert_eq!(
            string_option(&options, &format!("{QUERY_TAG_OPTION_PREFIX}team")),
            Some("model")
        );
        assert_eq!(
            string_option(
                &options,
                &format!("{QUERY_TAG_OPTION_PREFIX}@@dbt_model_name")
            ),
            Some("orders")
        );
    }

    #[test]
    fn preserves_query_tag_insertion_order() {
        let options = QueryTags::from_node(
            Some(r#"{"m_model":"last","a_shared":"model"}"#),
            Some("orders"),
            Some("incremental"),
        )
        .unwrap()
        .into_stmt_options();

        let names = options
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "databricks.query_tag.@@dbt_model_name",
                "databricks.query_tag.@@dbt_materialized",
                "databricks.query_tag.m_model",
                "databricks.query_tag.a_shared",
            ]
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.a_shared"),
            Some("model")
        );
    }

    #[test]
    fn missing_state_and_operations_inherit_database_defaults_without_statement_options() {
        assert!(
            QueryTags::from_state(None)
                .unwrap()
                .unwrap_or_default()
                .into_stmt_options()
                .is_empty()
        );

        let mut operation = DbtOperation::default();
        operation.__common_attr__.name = "on-run-start-0".to_string();
        let yaml = dbt_yaml::to_value(operation).unwrap();

        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_stmt_options();
        assert!(options.is_empty());

        let internal = dbt_yaml::from_str("internal: true").unwrap();
        assert!(
            query_tags_from_yaml_node(&internal)
                .unwrap()
                .into_stmt_options()
                .is_empty()
        );
    }

    fn tagged_seed() -> DbtSeed {
        let mut seed = DbtSeed::default();
        seed.__common_attr__.name = "orders_seed".to_string();
        seed.__base_attr__.materialized = DbtMaterialization::Table;
        seed.deprecated_config.materialized = Some(DbtMaterialization::Seed);
        seed.deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"resource"}"#.to_string());
        seed
    }

    fn model_global(seed: DbtSeed) -> MinijinjaValue {
        MinijinjaValue::from_serialize(dbt_yaml::to_value(seed).unwrap())
    }

    #[test]
    fn databricks_statement_options_tag_the_node_in_state() {
        let mut env = Environment::new();
        env.add_global("model", model_global(tagged_seed()));
        let state = State::new_for_env(&env);

        let options = databricks_statement_options(&state).unwrap();

        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name"),
            Some("orders_seed")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_materialized"),
            Some("seed")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("resource")
        );
    }

    fn databricks_attr(query_tags: &str) -> AdapterAttr {
        AdapterAttr::default().with_databricks_attr(Some(Arc::new(DatabricksAttr {
            query_tags: Some(query_tags.to_string()),
            ..Default::default()
        })))
    }

    fn assert_resource_tags(
        yaml: dbt_yaml::Value,
        resource_name: &str,
        materialized: Option<&str>,
    ) {
        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_stmt_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("resource")
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name"),
            Some(resource_name)
        );
        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_materialized"),
            materialized
        );
    }

    #[test]
    fn non_model_resources_emit_resource_query_tags() {
        let mut test = DbtTest::default();
        test.__common_attr__.name = "accepted_values_orders".to_string();
        test.__base_attr__.materialized = DbtMaterialization::Test;
        test.__adapter_attr__ = databricks_attr(r#"{"team":"resource"}"#);
        test.deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"deprecated"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(test).unwrap(),
            "accepted_values_orders",
            Some("test"),
        );

        let mut snapshot = DbtSnapshot::default();
        snapshot.__common_attr__.name = "orders_snapshot".to_string();
        snapshot.__base_attr__.materialized = DbtMaterialization::Snapshot;
        snapshot.__adapter_attr__ = databricks_attr(r#"{"team":"resource"}"#);
        snapshot
            .deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"deprecated"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(snapshot).unwrap(),
            "orders_snapshot",
            Some("snapshot"),
        );

        assert_resource_tags(
            dbt_yaml::to_value(tagged_seed()).unwrap(),
            "orders_seed",
            Some("seed"),
        );

        let mut unit_test = DbtUnitTest::default();
        unit_test.__common_attr__.name = "orders_unit_test".to_string();
        unit_test
            .deprecated_config
            .__warehouse_specific_config__
            .query_tags = Some(r#"{"team":"resource"}"#.to_string());
        assert_resource_tags(
            dbt_yaml::to_value(unit_test).unwrap(),
            "orders_unit_test",
            None,
        );
    }

    #[test]
    fn model_state_reads_rendered_query_tags() {
        let mut model = DbtModel::default();
        model.__common_attr__.name = "orders".to_string();
        model.__adapter_attr__ = databricks_attr(r#"{"team":"model"}"#);
        let yaml = dbt_yaml::to_value(model).unwrap();

        let options = query_tags_from_yaml_node(&yaml)
            .unwrap()
            .into_stmt_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.team"),
            Some("model")
        );
    }

    #[test]
    fn rejects_invalid_shapes_and_non_string_values() {
        let non_object =
            QueryTags::from_node(Some(r#"["not","an","object"]"#), None, None).unwrap_err();
        assert!(non_object.message().contains("must be a JSON object"));

        let non_string =
            QueryTags::from_node(Some(r#"{"cost_center":3000}"#), None, None).unwrap_err();
        assert!(non_string.message().contains("values must be strings"));
    }

    #[test]
    fn rejects_reserved_node_query_tag_keys() {
        for source in [
            r#"{"@@dbt_core_version":"override"}"#,
            r#"{"@@dbt_model_name":"override"}"#,
            r#"{"@@dbt_materialized":"override"}"#,
        ] {
            let model_error = QueryTags::from_node(Some(source), None, None).unwrap_err();
            assert!(model_error.message().contains("reserved query tag keys"));
        }

        let error = QueryTags::from_node(
            Some(r#"{"@@dbt_materialized":"override","@@dbt_core_version":"override"}"#),
            None,
            None,
        )
        .unwrap_err();
        assert!(
            error.message().contains(
                "Cannot use reserved query tag keys: @@dbt_core_version, @@dbt_materialized"
            ),
            "{}",
            error.message()
        );
    }

    #[test]
    fn truncates_automatic_node_values_to_128_characters() {
        let model_name = format!("{}::", "x".repeat(127));
        let options = QueryTags::from_node(None, Some(&model_name), Some("table"))
            .unwrap()
            .into_stmt_options();

        assert_eq!(
            string_option(&options, "databricks.query_tag.@@dbt_model_name")
                .unwrap()
                .chars()
                .count(),
            128
        );
    }

    /// Serialized default nodes of every kind, both as the Jinja context sees
    /// them (`InternalDbtNode::serialize`, with `resource_type`) and as plain
    /// serde output (without it).
    fn base_nodes() -> Vec<(String, YmlValue)> {
        let mut nodes = Vec::new();
        macro_rules! push {
            ($($node_type:ty),* $(,)?) => {$(
                let node = <$node_type>::default();
                nodes.push((
                    format!("{}/serialize", stringify!($node_type)),
                    InternalDbtNode::serialize(&node),
                ));
                nodes.push((
                    format!("{}/to_value", stringify!($node_type)),
                    dbt_yaml::to_value(&node).unwrap(),
                ));
            )*};
        }
        push!(
            DbtModel,
            DbtSeed,
            DbtTest,
            DbtSnapshot,
            DbtUnitTest,
            DbtOperation,
            DbtSource,
            DbtAnalysis,
            DbtFunction,
            DbtExposure,
        );
        nodes
    }

    fn string(value: &str) -> YmlValue {
        YmlValue::string(value.to_string())
    }

    /// Sets `key` to `value`, or removes it for `None`.
    fn set(mapping: &mut Mapping, key: &str, value: Option<YmlValue>) {
        match value {
            Some(value) => {
                mapping.insert(string(key), value);
            }
            None => {
                mapping.remove(key);
            }
        }
    }

    fn set_nested(mapping: &mut Mapping, outer: &str, key: &str, value: Option<YmlValue>) {
        if !matches!(mapping.get(outer), Some(YmlValue::Mapping(..))) {
            if value.is_none() {
                return;
            }
            mapping.insert(string(outer), YmlValue::mapping(Mapping::new()));
        }
        if let Some(YmlValue::Mapping(inner, _)) = mapping.get_mut(outer) {
            set(inner, key, value);
        }
    }

    fn query_tags_samples() -> Vec<(&'static str, Option<YmlValue>)> {
        vec![
            ("absent", None),
            ("null", Some(YmlValue::null())),
            ("valid", Some(string(r#"{"team":"x","cost":"y"}"#))),
            ("empty", Some(string(""))),
            ("empty_object", Some(string("{}"))),
            ("invalid_json", Some(string("{not json"))),
            ("array", Some(string(r#"["a","b"]"#))),
            ("reserved", Some(string(r#"{"@@dbt_model_name":"x"}"#))),
            ("non_string_value", Some(string(r#"{"cost":3000}"#))),
            ("yaml_number", Some(YmlValue::number(5i64.into()))),
            (
                "yaml_mapping",
                Some(YmlValue::mapping(Mapping::from_iter([(
                    string("team"),
                    string("x"),
                )]))),
            ),
        ]
    }

    fn materialized_samples() -> Vec<(String, Option<YmlValue>)> {
        let mut samples: Vec<(String, Option<YmlValue>)> = [
            "snapshot",
            "seed",
            "view",
            "table",
            "incremental",
            "materialized_view",
            "external",
            "test",
            "ephemeral",
            "unit",
            "analysis",
            "function",
            "metric_view",
            "streaming_table",
            "dynamic_table",
            "interactive_table",
            "inline",
        ]
        .into_iter()
        .map(|m| (m.to_string(), Some(string(m))))
        .collect();
        samples.push(("custom".to_string(), Some(string("my_custom_mat"))));
        samples.push(("absent".to_string(), None));
        samples.push(("null".to_string(), Some(YmlValue::null())));
        samples.push((
            "yaml_number".to_string(),
            Some(YmlValue::number(3i64.into())),
        ));
        samples
    }

    /// `resource_type` as the Jinja context sets it, removed, or naming
    /// another kind: the legacy probing ignores it, so the view must too.
    fn resource_type_samples() -> Vec<(&'static str, Option<YmlValue>)> {
        vec![
            ("keep", None),
            ("remove", None),
            ("model", Some(string("model"))),
            ("seed", Some(string("seed"))),
            ("unit_test", Some(string("unit_test"))),
        ]
    }

    /// Generates node values by mutating serialized nodes of every kind.
    fn generated_cases() -> Vec<(String, YmlValue)> {
        let mut cases = Vec::new();
        let mut push = |label: String, base: &YmlValue, edit: &dyn Fn(&mut Mapping)| {
            let mut value = base.clone();
            if let YmlValue::Mapping(mapping, _) = &mut value {
                edit(mapping);
            }
            cases.push((label, value));
        };

        for (base_label, base) in base_nodes() {
            let base_label = &base_label;
            push(format!("{base_label} as is"), &base, &|_| {});

            for (rt_label, resource_type) in resource_type_samples() {
                let edit_resource_type = |m: &mut Mapping| match rt_label {
                    "keep" => {}
                    "remove" => set(m, "resource_type", None),
                    _ => set(m, "resource_type", resource_type.clone()),
                };

                // Interactions between the typed and the deprecated sources.
                for (attr_label, attr_tags) in query_tags_samples() {
                    for (config_label, config_tags) in query_tags_samples() {
                        for has_attr in [false, true] {
                            push(
                                format!(
                                    "{base_label} rt={rt_label} attr={has_attr}/{attr_label} config.query_tags={config_label}"
                                ),
                                &base,
                                &|m| {
                                    edit_resource_type(m);
                                    set(m, "name", Some(string("orders")));
                                    if has_attr {
                                        set_nested(
                                            m,
                                            "databricks_attr",
                                            "query_tags",
                                            attr_tags.clone(),
                                        );
                                        if attr_tags.is_none() {
                                            set(
                                                m,
                                                "databricks_attr",
                                                Some(YmlValue::mapping(Mapping::new())),
                                            );
                                        }
                                    } else {
                                        set(m, "databricks_attr", None);
                                    }
                                    set_nested(m, "config", "query_tags", config_tags.clone());
                                },
                            );
                        }
                    }
                }

                // Materialization, top level and in the deprecated config.
                let materialized_grid = if matches!(rt_label, "keep" | "remove") {
                    materialized_samples()
                } else {
                    Vec::new()
                };
                for (top_label, top) in materialized_grid.clone() {
                    for (config_label, config) in materialized_grid.clone() {
                        push(
                            format!(
                                "{base_label} rt={rt_label} materialized={top_label} config.materialized={config_label}"
                            ),
                            &base,
                            &|m| {
                                edit_resource_type(m);
                                set(m, "name", Some(string("orders")));
                                set(m, "materialized", top.clone());
                                set_nested(m, "config", "materialized", config.clone());
                                set_nested(
                                    m,
                                    "databricks_attr",
                                    "query_tags",
                                    Some(string(r#"{"team":"attr"}"#)),
                                );
                                set_nested(
                                    m,
                                    "config",
                                    "query_tags",
                                    Some(string(r#"{"team":"config"}"#)),
                                );
                            },
                        );
                    }
                }

                // Names.
                for (name_label, name) in [
                    ("absent", None),
                    ("empty", Some(string(""))),
                    ("long", Some(string(&"x".repeat(300)))),
                    ("unicode", Some(string("pedidos_ção_日本"))),
                    ("number", Some(YmlValue::number(7i64.into()))),
                    ("null", Some(YmlValue::null())),
                ] {
                    push(
                        format!("{base_label} rt={rt_label} name={name_label}"),
                        &base,
                        &|m| {
                            edit_resource_type(m);
                            set(m, "name", name.clone());
                        },
                    );
                }

                // Keys the shape fallback looks at, removed one at a time.
                for key in [
                    "config",
                    "materialized",
                    "given",
                    "access",
                    "manifest_original_file_path",
                    "databricks_attr",
                    "unique_id",
                    "fqn",
                    "checksum",
                ] {
                    push(
                        format!("{base_label} rt={rt_label} without={key}"),
                        &base,
                        &|m| {
                            edit_resource_type(m);
                            set(m, "name", Some(string("orders")));
                            set(m, key, None);
                        },
                    );
                }
            }
        }
        cases
    }

    /// Values that are not serialized nodes at all.
    fn handwritten_cases() -> Vec<(String, YmlValue)> {
        [
            "null",
            "3",
            "orders",
            "[]",
            "[1, 2]",
            "{}",
            "internal: true",
            "name: orders",
            "name: orders\nmaterialized: table",
            "name: orders\nmaterialized: table\nconfig: {}",
            "name: orders\nmaterialized: table\nconfig: {query_tags: '{\"team\":\"x\"}'}",
            "name: orders\nresource_type: model",
            "name: orders\nresource_type: model\nmaterialized: table",
            "name: orders\nresource_type: seed\nconfig: {materialized: table}",
            "name: orders\nresource_type: unit_test\ngiven: []",
            "name: orders\ndatabricks_attr: {query_tags: '{\"team\":\"x\"}'}",
            "name: orders\ngiven: []\nconfig: {query_tags: '{\"team\":\"x\"}'}",
        ]
        .into_iter()
        .map(|yaml| (format!("yaml {yaml:?}"), dbt_yaml::from_str(yaml).unwrap()))
        .collect()
    }

    fn outcome(result: AdapterResult<QueryTags>) -> Result<QueryTags, String> {
        result.map_err(|error| error.message().to_string())
    }

    /// Keys every node type the legacy code probes requires, but the view
    /// does not check for. `name`, `materialized` and `config` are left out:
    /// the view still requires them.
    const LEGACY_COMMON_KEYS: [&str; 17] = [
        "unique_id",
        "package_name",
        "fqn",
        "path",
        "original_file_path",
        "name_span",
        "checksum",
        "tags",
        "meta",
        "schema",
        "alias",
        "quoting",
        "quoting_ignore_case",
        "adapter",
        "static_analysis",
        "enabled",
        "depends_on",
    ];

    /// The legacy result for `value` with any missing [`LEGACY_COMMON_KEYS`]
    /// filled in from `template`. The view accepts nodes without those keys
    /// and reads none of them, so it must agree with legacy on the filled value.
    fn relaxed_legacy(value: &YmlValue, template: &Mapping) -> Result<QueryTags, String> {
        let mut filled = value.clone();
        if let YmlValue::Mapping(mapping, _) = &mut filled {
            for key in LEGACY_COMMON_KEYS {
                if !mapping.contains_key(key) {
                    mapping.insert(string(key), template.get(key).unwrap().clone());
                }
            }
        }
        outcome(legacy_query_tags_from_yaml_node(&filled))
    }

    #[test]
    fn view_matches_legacy_deserialization() {
        let cases = generated_cases()
            .into_iter()
            .chain(handwritten_cases())
            .collect::<Vec<_>>();

        let template = InternalDbtNode::serialize(&DbtSeed::default());
        let template = template.as_mapping().unwrap();

        let mut mismatches = Vec::new();
        for (label, value) in &cases {
            let legacy = relaxed_legacy(value, template);
            let view = outcome(query_tags_from_yaml_node(value));
            if legacy != view {
                mismatches.push(format!("{label}\n  legacy: {legacy:?}\n  view:   {view:?}"));
            }
        }

        assert!(
            mismatches.is_empty(),
            "{} of {} cases differ; first ones:\n{}",
            mismatches.len(),
            cases.len(),
            mismatches
                .iter()
                .take(30)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }
}
