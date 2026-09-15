//! Testing utilities for databricks relation configs
use crate::AdapterType;
use crate::relation::databricks::config::DatabricksRelationMetadata;
use crate::relation::test_helpers;
use arrow::csv::ReaderBuilder;
use arrow_schema::{DataType, Field, Schema};
use dbt_agate::AgateTable;
use dbt_schemas::schemas::dbt_column::ColumnMask;
use dbt_schemas::schemas::serde::StringOrArrayOfStrings;
use dbt_schemas::schemas::{
    common::PartitionConfig, common::*, dbt_column::DbtColumn, nodes::*, project::*,
};
use dbt_yaml::{Spanned, Value as YmlValue};
use indexmap::IndexMap;
use std::io;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TestModelColumn {
    pub name: String,
    pub comment: Option<String>,
    pub constraints: Vec<Constraint>,
    pub tags: IndexMap<String, String>,
    pub column_mask: Option<ColumnMask>,
}

#[derive(Default)]
pub(crate) struct TestModelConfig {
    pub auto_cluster: bool,
    pub cluster_by: Vec<String>,
    pub columns: Vec<TestModelColumn>,
    /// Defaults to enforced for legacy component tests; set explicitly to `Some(false)` when a
    /// test exercises the contract ownership boundary.
    pub contract_enforced: Option<bool>,
    pub cron: Option<String>,
    pub partition_by: Vec<String>,
    pub persist_column_comments: bool,
    pub persist_relation_comments: bool,
    /// The model's compiled SQL, which the `query` component diffs against the applied view
    /// definition.
    pub query: Option<String>,
    pub relation_comment: Option<String>,
    pub row_filter_function: Option<String>,
    pub row_filter_columns: Vec<String>,
    pub tags: IndexMap<String, String>,
    pub tbl_properties: IndexMap<String, String>,
    pub table_format: Option<String>,
    pub time_zone: Option<String>,
}

pub(crate) fn create_mock_dbt_model(cfg: TestModelConfig) -> DbtModel {
    let persist_docs = PersistDocsConfig {
        relation: Some(cfg.persist_relation_comments),
        columns: Some(cfg.persist_column_comments),
    };

    let base_attrs = NodeBaseAttributes {
        adapter: AdapterType::Snowflake,
        propagate: Vec::new(),
        unrendered_config: Default::default(),
        database: "test_db".to_string(),
        schema: "test_schema".to_string(),
        alias: "test_table".to_string(),
        relation_name: None,
        quoting: dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING,
        quoting_ignore_case: false,
        materialized: DbtMaterialization::Table,
        compute: None,
        static_analysis: Spanned::new(dbt_common::io_args::StaticAnalysisKind::Strict),
        static_analysis_off_reason: None,
        enabled: true,
        extended_model: false,
        persist_docs: Some(persist_docs),
        columns: cfg
            .columns
            .into_iter()
            .map(|c| {
                Arc::new(DbtColumn {
                    name: c.name,
                    description: c.comment,
                    constraints: c.constraints,
                    databricks_tags: Some(
                        c.tags
                            .into_iter()
                            .map(|(k, v)| (k, YmlValue::from(v)))
                            .collect(),
                    ),
                    column_mask: c.column_mask,
                    ..Default::default()
                })
            })
            .collect(),
        refs: vec![],
        sources: vec![],
        functions: vec![],
        metrics: vec![],
        depends_on: NodeDependsOn::default(),
    };

    let wh_config = WarehouseSpecificNodeConfig {
        tblproperties: Some(
            cfg.tbl_properties
                .into_iter()
                .map(|(k, v)| (k, dbt_yaml::Value::from(v)))
                .collect(),
        ),
        partition_by: Some(PartitionConfig::List(cfg.partition_by)),
        liquid_clustered_by: (!cfg.cluster_by.is_empty())
            .then_some(StringOrArrayOfStrings::ArrayOfStrings(cfg.cluster_by)),
        auto_liquid_cluster: Some(cfg.auto_cluster),
        schedule: Some(Schedule::ScheduleConfig(ScheduleConfig {
            cron: cfg.cron,
            time_zone_value: cfg.time_zone,
        })),
        databricks_tags: Some(
            cfg.tags
                .into_iter()
                .map(|(k, v)| (k, YmlValue::from(v)))
                .collect(),
        ),
        row_filter: cfg.row_filter_function.map(|function| RowFilterConfig {
            function: Some(function),
            columns: Some(StringOrArrayOfStrings::ArrayOfStrings(
                cfg.row_filter_columns,
            )),
        }),
        ..Default::default()
    };

    let adapter_attr = AdapterAttr::from_config_and_dialect(&wh_config, AdapterType::Databricks);

    DbtModel {
        deprecated_config: ModelConfig {
            contract: Some(DbtContract {
                enforced: cfg.contract_enforced.unwrap_or(true),
                ..Default::default()
            }),
            table_format: cfg.table_format,
            __warehouse_specific_config__: wh_config,
            ..Default::default()
        },
        __common_attr__: CommonAttributes {
            name: "test_model".to_string(),
            fqn: vec!["test".to_string(), "test_model".to_string()],
            description: cfg.relation_comment,
            ..Default::default()
        },
        __adapter_attr__: adapter_attr,
        __base_attr__: base_attrs,
        __model_attr__: DbtModelAttr {
            compiled_code: cfg.query,
            ..Default::default()
        },
        ..Default::default()
    }
}

// TODO(anna): describe should return column masks
pub(crate) fn create_mock_describe_extended_table<'a>(
    partition_columns: impl IntoIterator<Item = &'a str>,
    rows: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> AgateTable {
    let mut csv_data = "key,value\n".to_string();

    csv_data.push_str("Table,test_table\n");
    csv_data.push_str("Owner,test_user\n");

    for (label, value) in rows.into_iter() {
        csv_data.push_str(&format!("{label},{value}\n"))
    }

    let partition_columns_vec = Vec::from_iter(partition_columns);
    if !partition_columns_vec.is_empty() {
        csv_data.push_str("# Partition Information,\n");
        csv_data.push_str("# col_name,data_type\n");
        for col in partition_columns_vec {
            csv_data.push_str(&format!("{col},string\n"));
        }
        csv_data.push_str(",\n");
    }

    csv_data.push_str("# Detailed Table Information,\n");

    let schema = Arc::new(Schema::new(vec![
        Field::new("key", DataType::Utf8, true),
        Field::new("value", DataType::Utf8, true),
    ]));

    let file = io::Cursor::new(csv_data);
    let mut reader = ReaderBuilder::new(schema)
        .with_header(true)
        .build(file)
        .unwrap();
    let batch = reader.next().unwrap().unwrap();
    AgateTable::from_record_batch(Arc::new(batch))
}

pub(crate) fn run_test_cases(
    test_cases: Vec<test_helpers::TestCase<DatabricksRelationMetadata, TestModelConfig>>,
) {
    test_helpers::run_test_cases(test_cases, create_mock_dbt_model)
}
