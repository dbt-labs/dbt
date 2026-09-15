use dbt_adapter_core::AdapterType;
use dbt_schemas::schemas::dbt_catalogs::DbtCatalogs;
use dbt_schemas::schemas::dbt_catalogs_v2::{CatalogType, PhysicalFormatResolver};
use dbt_schemas::schemas::relations::base::TableFormat;

use dbt_yaml::{Mapping as YmlMapping, Span, Value as YmlValue};
use minijinja::{
    Value,
    value::{Object, ValueKind},
};
use std::collections::BTreeMap;
use std::fmt::Formatter;
use std::path::PathBuf;
use std::sync::Arc;

use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::load_catalogs;

mod catalog_relation_v2;

/// How DuckDB must write a table for this relation's catalog / table format.
///
/// One named state replaces the `supports_stage_create` × `is_iceberg` boolean
/// matrix the macros used to re-derive. Exposed to Jinja as a string-enum via
/// the `duckdb_write_strategy` key so materializations branch on a single value
/// instead of nested booleans.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckDbWriteStrategy {
    /// `CREATE ... AS SELECT` directly (plain DuckDB tables, DuckLake).
    CreateAsSelect,
    /// External Iceberg catalogs (any catalog whose `table_format` is iceberg):
    /// write the target in place — `duckdb__create_table_as` emits an empty
    /// `CREATE` followed by `INSERT`, and the table materialization skips the
    /// temp-table + rename dance entirely, since Iceberg REST attachments do
    /// not support `ALTER ... RENAME`. The iceberg default: empty `CREATE` +
    /// `INSERT` works whether or not the REST catalog supports staged creates.
    DirectCreate,
    /// Iceberg catalog whose user opted in to staged creates with
    /// `stage_create_tables: true` (the duckdb-iceberg#1017 write-compat ATTACH
    /// option, duckdb's upstream default): `CREATE ... AS SELECT` directly
    /// against the target, still skipping the temp-table + rename dance
    /// (renames remain unsupported over Iceberg REST).
    DirectCreateAsSelect,
}

impl DuckDbWriteStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            DuckDbWriteStrategy::CreateAsSelect => "create_as_select",
            DuckDbWriteStrategy::DirectCreate => "direct_create",
            DuckDbWriteStrategy::DirectCreateAsSelect => "direct_create_as_select",
        }
    }
}

const BIGQUERY_DEFAULT_FILE_FORMAT: &str = "default";

const BIGQUERY_ATTR: &str = "bigquery_attr";

const DBX_DEFAULT_TABLE_FORMAT: &str = "default";

const DELTA_TABLE_FORMAT: &str = "delta";
const PARQUET_TABLE_FORMAT: &str = "parquet";
const DATABRICKS_UNITY_CATALOG: &str = "unity";
const DATABRICKS_HIVE_METASTORE: &str = "hive_metastore";

const DATABRICKS_ATTR: &str = "databricks_attr";

// Jinja DDL tends to have comparisons against uppercase strings
// TODO(versufacit): dbt core currently has a notion of the default store as a catalog.
// We may diverge from this. Implemented now for legacy compatibility ahead of Coalesce;
// https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-snowflake/src/dbt/include/snowflake/macros/relations/table/create.sql#L8

const LEGACY_CONFIG_ICEBERG_ATTRIBUTE_ERR: &str = "The external_volume and base_location_* model attributes are not able to \
    be specified on table_format=default models (includes models without an explicit \
    table_format). For other table formats, use catalogs.yml write integrations.";

// A reserved external_volume value, BASE_LOCATION is invalid alongside it
// https://docs.snowflake.com/en/user-guide/tables-iceberg-internal-storage
const SNOWFLAKE_MANAGED_EXTERNAL_VOLUME: &str = "SNOWFLAKE_MANAGED";

const SNOWFLAKE_ATTR: &str = "snowflake_attr";
const DUCKDB_ATTR: &str = "duckdb_attr";
const ADAPTER_PROP_CATALOG_LINKED_DATABASE_TYPE: &str = "catalog_linked_database_type";
const ADAPTER_PROP_USE_UNIFORM: &str = "use_uniform";

#[derive(Debug, Clone, Copy)]
enum LinkedCatalogProvider {
    Glue,
    Unity,
}

impl LinkedCatalogProvider {
    fn is_glue(self) -> bool {
        matches!(self, Self::Glue)
    }

    fn is_unity(self) -> bool {
        matches!(self, Self::Unity)
    }
}

// FIXME: CatalogRelation is a flat struct that every adapter piles onto. Adapter-specific
// egress keys (e.g. duckdb_write_strategy, supports_stage_create) should flow out of a
// per-adapter flat map rather than being baked into shared fields and methods here.
#[derive(Debug, serde::Serialize)]
pub struct CatalogRelation {
    pub adapter_type: AdapterType,

    // identity / routing
    pub catalog_name: Option<String>,
    pub integration_name: Option<String>,

    // type & format
    pub catalog_type: CatalogType,
    pub table_format: TableFormat,

    // normalized SQL options
    pub adapter_properties: BTreeMap<String, String>,

    // metadata helper
    pub is_transient: Option<bool>,

    // Snowflake uses directly
    // Databricks uses as a catalog_relation notion for location_root
    pub external_volume: Option<String>,

    // === Snowflake, BigQuery (biglake), Databricks (unity) — v2 only
    // The physical database/project/catalog that models using this integration should land in.
    // Takes highest priority in generate_database_name over model database config and target.database.
    pub catalog_database: Option<String>,

    // === BigQuery (biglake) — v2 only, LRC (Lakehouse Runtime Catalog)
    // The LRC catalog name for producing a 4-part FQN: `project`.`lakehouse_catalog.namespace`.`table`
    pub lakehouse_catalog: Option<String>,

    // === Snowflake
    // built_in only: synthesized base_location_root and base_location_subpath model attributes
    pub base_location: Option<String>,

    // === Databricks and Bigquery
    pub file_format: Option<String>,
    // TODO: be the owner of tblproperties for model config resolution
}

impl PhysicalFormatResolver for CatalogRelation {
    fn table_format(&self) -> TableFormat {
        self.table_format
    }

    fn catalog_type(&self) -> CatalogType {
        self.catalog_type
    }
}

impl CatalogRelation {
    // safety: v1 uses adapter_properties, v2 uses catalog_database
    pub fn has_catalog_linked_database(&self) -> bool {
        // v1
        self.adapter_properties
            .get("catalog_linked_database")
            .is_some_and(|v| !v.trim().is_empty())
            // v2
            || self.catalog_database.as_deref().is_some_and(|v| !v.trim().is_empty())
                && self.catalog_type.is_catalog_linked()
    }

    // Builder pattern setters - prefer these over introducing a new named
    // `default_catalog_relation_<adapter>_<variant>()` constructor
    pub fn with_table_format(mut self, table_format: TableFormat) -> Self {
        self.table_format = table_format;
        self
    }

    pub fn with_file_format(mut self, file_format: impl Into<String>) -> Self {
        self.file_format = Some(file_format.into());
        self
    }

    pub fn with_adapter_property(
        mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Self {
        self.adapter_properties.insert(key.into(), value.into());
        self
    }

    fn linked_catalog_provider(&self) -> Option<LinkedCatalogProvider> {
        let catalog_name = self.catalog_name.as_deref()?;
        let catalogs = load_catalogs::fetch_catalogs()?;
        let view = catalogs.view_v2().ok()?;
        let catalog = view
            .catalogs
            .iter()
            .find(|catalog| catalog.name == catalog_name)?;

        match catalog.catalog_type {
            CatalogType::Glue => Some(LinkedCatalogProvider::Glue),
            CatalogType::Unity => Some(LinkedCatalogProvider::Unity),
            _ => None,
        }
    }

    pub fn from_model_config_and_catalogs(
        adapter_type: AdapterType,
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<Self> {
        if load_catalogs::fetch_use_catalogs_v2()
            && let Some(catalogs) = catalogs.as_ref()
        {
            return catalog_relation_v2::from_model_config_and_catalogs_v2(
                adapter_type,
                model,
                catalogs.clone(),
            );
        }

        match adapter_type {
            AdapterType::Databricks => {
                Self::from_model_config_and_catalogs_databricks(model, catalogs)
            }
            AdapterType::Snowflake => {
                Self::from_model_config_and_catalogs_snowflake(model, catalogs)
            }
            AdapterType::Bigquery => Self::from_model_config_and_catalogs_bigquery(model, catalogs),
            // Lake compute is DuckDB-backed for relation-building purposes;
            // v1 catalogs.yml never supported per-catalog DuckDB selection either,
            // so this mirrors the DuckDB arm exactly.
            // GizmoSQL is a remote DuckDB and shares DuckDB's macros, which read
            // `duckdb_write_strategy` off this relation.
            AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => {
                Ok(Self::default_catalog_relation_duckdb())
            }
            _ => Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("build_relation_catalog cannot be invoked by an adapter {adapter_type:?}"),
            )),
        }
    }

    // ========
    // Bigquery
    // ========

    fn from_model_config_and_catalogs_bigquery(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<CatalogRelation> {
        debug_assert!(
            model.kind() != ValueKind::String,
            "Bigquery adapter received a bare string model config; this is unsupported and indicates a parser bug."
        );

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Bigquery).and_then(
                |s| {
                    let t = s.trim();
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        let wants_iceberg = TableFormat::parse(
            Self::get_model_config_value(model, "table_format", AdapterType::Bigquery).as_deref(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
        .is_iceberg();

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            (None, _) if !wants_iceberg => Ok(Self::default_catalog_relation_bigquery()),
            (None, _) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "On Bigquery, table_format=iceberg requires catalogs.yml and a `catalog_name` that selects a write integration.",
            )),
            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),
            (Some(catalog_name), Some(catalogs)) => {
                Self::build_bigquery_with_catalogs(model, catalogs.mapping(), catalog_name)
            }
        }
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/6f89d7ce7e762f3fdf7cf6b48e8372585712f10f/dbt-bigquery/src/dbt/adapters/bigquery/constants.py#L27
    fn default_catalog_relation_bigquery() -> CatalogRelation {
        CatalogRelation {
            adapter_type: AdapterType::Bigquery,
            catalog_name: None,
            integration_name: None,
            catalog_type: CatalogType::BigqueryNative,
            table_format: TableFormat::Default,
            adapter_properties: BTreeMap::new(),
            is_transient: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            file_format: Some(BIGQUERY_DEFAULT_FILE_FORMAT.to_string()),
        }
    }

    pub fn default_catalog_relation_duckdb() -> CatalogRelation {
        CatalogRelation {
            adapter_type: AdapterType::DuckDB,
            catalog_name: None,
            integration_name: None,
            catalog_type: CatalogType::DuckdbNative,
            table_format: TableFormat::Default,
            file_format: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties: BTreeMap::new(),
            is_transient: None,
        }
    }

    fn build_bigquery_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) active integration name
        let integration_name =
            lookup_integration_name(catalogs, catalog_name).ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' missing 'active_write_integration'"),
                )
            })?;

        // 2) resolve the selected write_integration mapping
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) catalog_type must be in YAML (no model override)
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Bigquery).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let catalog_type = match Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })? {
            s if s.eq_ignore_ascii_case("biglake_metastore") => CatalogType::BiglakeMetastore,
            s => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Invalid Bigquery catalog_type '{s}'"),
                ));
            }
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Bigquery);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?;

        // file_format: model > YAML
        let mut file_format =
            Self::get_model_config_value(model, "file_format", AdapterType::Bigquery)
                .or_else(|| Self::yml_str(write_integration, "file_format".to_string()))
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "file_format is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                    )
                })?;
        file_format.make_ascii_lowercase();

        // 6) adapter_properties:
        //    - base_location_root (optional)
        //    - base_location_subpath (optional)
        //    - storage_uri
        //    - connection_id
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props = Self::get_model_adapter_properties(model, AdapterType::Bigquery);

        // base_location_root: model(adapter_properties) > model (legacy) > YAML > default
        let base_location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_root")
                .or_else(|| {
                    Self::get_model_config_value(model, "base_location_root", AdapterType::Bigquery)
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "base_location_root")
                });

        // base_location_subpath: model (adapter_properties) > model (legacy) > default
        if Self::yml_str(write_integration, "base_location_subpath".to_string()).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "base_location_subpath is forbidden by the catalogs.yml schema on write integrations. Model-level base_location_subpath must be used instead, and validation should have already rejected this",
            ));
        }
        let base_location_subpath =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_subpath")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_subpath",
                        AdapterType::Bigquery,
                    )
                });

        let schema = Self::get_model_config_value(model, "schema", AdapterType::Bigquery);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Bigquery)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Bigquery));

        let base_location = Self::build_base_location(
            &base_location_root,
            &base_location_subpath,
            &schema,
            &identifier,
        );

        // external_volume must be in YAML (no model override)
        if Self::get_model_config_value(model, "external_volume", AdapterType::Bigquery).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "external_volume may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let external_volume = Self::yml_str(write_integration, "external_volume".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "external_volume is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        // storage_uri: model (adapter_properties) > model (legacy) > default
        let storage_uri = Self::get_adapter_property(model_adapter_props.as_ref(), "storage_uri")
            .or_else(|| Self::get_model_config_value(model, "storage_uri", AdapterType::Bigquery))
            .unwrap_or_else(|| format!("{external_volume}/{base_location}"));

        let mut adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        adapter_properties.insert("storage_uri".to_owned(), storage_uri);

        Ok(CatalogRelation {
            adapter_type: AdapterType::Bigquery,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            adapter_properties,
            is_transient: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            file_format: Some(file_format),
        })
    }

    // ==========
    // Databricks
    // ==========

    fn from_model_config_and_catalogs_databricks(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<CatalogRelation> {
        debug_assert!(
            model.kind() != ValueKind::String,
            "Databricks adapter received a bare string model config; this is unsupported and indicates a parser bug."
        );

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Databricks).and_then(
                |s| {
                    let t = s.trim();
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        let wants_iceberg = TableFormat::parse(
            Self::get_model_config_value(model, "table_format", AdapterType::Databricks).as_deref(),
        )
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
        .is_iceberg();

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            (None, None) if !wants_iceberg => {
                Ok(Self::default_catalog_relation_databricks_for_model(model))
            }
            (None, None) => Ok(Self::default_catalog_relation_databricks_for_model(model)
                .with_table_format(TableFormat::Iceberg)
                .with_adapter_property(ADAPTER_PROP_USE_UNIFORM, "false")),

            (None, Some(_)) if !wants_iceberg => {
                Ok(Self::default_catalog_relation_databricks_for_model(model))
            }
            (None, Some(_)) => Ok(Self::default_catalog_relation_databricks_for_model(model)
                .with_table_format(TableFormat::Iceberg)
                .with_adapter_property(ADAPTER_PROP_USE_UNIFORM, "false")),

            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),

            (Some(catalog_name), Some(catalogs)) => {
                Self::build_databricks_with_catalogs(model, catalogs.mapping(), catalog_name)
            }
        }
    }

    // https://github.com/databricks/dbt-databricks/blob/ba47ba15fb194e048866f4ce396a7eda71db2596/dbt/adapters/databricks/constants.py
    pub fn default_catalog_relation_databricks() -> CatalogRelation {
        CatalogRelation {
            adapter_type: AdapterType::Databricks,
            catalog_name: None,
            integration_name: None,
            catalog_type: CatalogType::Unity,
            table_format: TableFormat::Default,
            file_format: Some(DELTA_TABLE_FORMAT.to_string()),
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties: BTreeMap::new(),
            is_transient: None,
        }
    }

    // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/catalogs/_unity.py
    fn default_catalog_relation_databricks_for_model(model: &Value) -> CatalogRelation {
        let location_root = Self::get_adapter_property(
            Self::get_model_adapter_properties(model, AdapterType::Databricks).as_ref(),
            "location_root",
        )
        .or_else(|| Self::get_model_config_value(model, "location_root", AdapterType::Databricks))
        .filter(|location_root| !location_root.trim().is_empty());

        CatalogRelation {
            external_volume: location_root
                .and_then(|root| Self::dbx_build_external_volume_for_location(model, &root)),
            ..Self::default_catalog_relation_databricks()
        }
    }

    fn build_databricks_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) active integration name
        let integration_name =
            lookup_integration_name(catalogs, catalog_name).ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' missing 'active_write_integration'"),
                )
            })?;

        // 2) resolve the selected write_integration mapping
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) catalog_type must be in YAML (no model override)
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Databricks).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let raw_catalog_type = Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        let catalog_type = match raw_catalog_type.as_str() {
            s if s.eq_ignore_ascii_case("unity") => CatalogType::Unity,
            s if s.eq_ignore_ascii_case("hive_metastore") => CatalogType::HiveMetastore,
            s => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Invalid Databricks catalog_type '{s}'"),
                ));
            }
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Databricks);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid table_format in catalog '{catalog_name}': {e}"),
            )
        })?;

        // 5) file_format: model > YAML > default(delta)
        let mut file_format =
            Self::get_model_config_value(model, "file_format", AdapterType::Databricks)
                .or_else(|| Self::yml_str(write_integration, "file_format".to_string()))
                .unwrap_or_else(|| String::from(DBX_DEFAULT_TABLE_FORMAT));
        file_format.make_ascii_lowercase();
        let file_format = file_format;

        // 6) adapter_properties:
        //    - UNITY: allow only location_root (optional; non-blank)
        //    - HMS: disallow adapter_properties entirely
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props =
            Self::get_model_adapter_properties(model, AdapterType::Databricks);
        let mut external_volume = None;

        // location_root: model(adapter_properties) > model (legacy) > YAML > default
        let location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "location_root")
                .or_else(|| {
                    Self::get_model_config_value(model, "location_root", AdapterType::Databricks)
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "location_root")
                });

        let adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        if raw_catalog_type.eq_ignore_ascii_case(DATABRICKS_UNITY_CATALOG)
            && let Some(location_root) = location_root
        {
            if location_root.trim().is_empty() {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "adapter_properties.location_root cannot be blank or whitespace",
                ));
            }
            external_volume = Self::dbx_build_external_volume_for_location(model, &location_root);
        } else if raw_catalog_type.eq_ignore_ascii_case(DATABRICKS_HIVE_METASTORE)
            && !adapter_properties.is_empty()
        {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "adapter_properties not allowed for hive_metastore",
            ));
        };
        let external_volume = external_volume;

        Ok(CatalogRelation {
            adapter_type: AdapterType::Databricks,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            file_format: Some(file_format),
            external_volume,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    // centralized reimplementation of https://github.com/databricks/dbt-databricks/blob/53cd1a2c1fcb245ef25ecf2e41249335fd4c8e4b/dbt/adapters/databricks/catalogs/_relation.py#L33
    pub fn dbx_build_external_volume_for_location(
        model: &Value,
        location_root: &str,
    ) -> Option<String> {
        let include_full_name = Self::get_model_config_value(
            model,
            "include_full_name_in_path",
            AdapterType::Databricks,
        )
        .map(|v| v.trim().eq_ignore_ascii_case("true"))
        .unwrap_or(false);

        let mut rel = PathBuf::new();

        if include_full_name {
            if let Some(db) =
                Self::get_model_config_value(model, "database", AdapterType::Databricks)
            {
                rel.push(db);
            }
            if let Some(sc) = Self::get_model_config_value(model, "schema", AdapterType::Databricks)
            {
                rel.push(sc);
            }
        }

        if let Some(id) = Self::get_model_config_value(model, "alias", AdapterType::Databricks) {
            rel.push(id);
        }

        Some(
            PathBuf::from(location_root.trim_end_matches('/'))
                .join(rel)
                .to_string_lossy()
                .replace('\\', "/"),
        )
    }

    // =========
    // Snowflake
    // =========

    pub fn from_model_config_and_catalogs_snowflake(
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<Self> {
        // Special case hack: a plain string means this is the linked database name.
        // You cannot use a string literal anywhere except drop for this feature
        // this function is designed to be used with a model config

        if model.kind() == ValueKind::String {
            let fqn = model.as_str().unwrap().trim();
            let db_only = fqn.split('.').next().unwrap_or(fqn).trim();

            return if let Some(cats) = catalogs.as_ref()
                && Self::cld_exists_in_iceberg_rest(cats.mapping(), db_only)
            {
                Self::build_for_cld_only(model)
            } else {
                Ok(Self::default_catalog_relation_snowflake())
            };
        }

        let model_catalog_name =
            Self::get_model_config_value(model, "catalog_name", AdapterType::Snowflake).and_then(
                |s| {
                    let t = s.trim();
                    // [DELIBERATE CHANGE] Serialization sometimes makes model configs parse a none
                    // value into Some("none"). Unlikely many users will be naming their catalog names 'none'.
                    // TODO: track that down and patch
                    if t.eq_ignore_ascii_case("none") {
                        None
                    } else {
                        Some(t.to_string())
                    }
                },
            );

        match (model_catalog_name.as_deref(), catalogs.as_ref()) {
            // No reconciliation path: only values present on the model config are used.
            // This represents the "legacy" or v1 Iceberg tables/iceberg tables
            // which are Snowflake only and do not use the catalogs.yml.
            (None, _) => Self::build_without_catalogs_yml(model),

            // Catalog-driven path: both catalog_name and catalogs.yml need be present
            (Some(catalog_name), None) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Model specifies catalog_name '{catalog_name}', but catalogs.yml was not found"
                ),
            )),

            (Some(catalog_name), Some(catalogs)) => {
                Self::build_with_catalogs(model, catalogs.mapping(), catalog_name)
            }
        }
    }

    /// Some relations have no configs and attempts incorporate fail.
    ///
    /// This is hack to duplicate core's logic until we have time to architecture a better system.
    fn build_for_cld_only(v: &Value) -> AdapterResult<CatalogRelation> {
        let db_name = v.as_str().unwrap().trim();

        let mut adapter_properties = BTreeMap::new();
        adapter_properties.insert("catalog_linked_database".to_string(), db_name.to_string());

        Ok(CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: None,
            integration_name: None,
            catalog_type: CatalogType::IcebergRest,
            table_format: TableFormat::Iceberg,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: Some(false),
            file_format: None,
        })
    }

    /// Build a legacy model configuration into a catalog relation.
    ///
    /// Helper for building a catalog relation, default or iceberg, for model-configured only
    /// iceberg materializations in Snowflake.
    fn build_without_catalogs_yml(model: &Value) -> AdapterResult<CatalogRelation> {
        if Self::get_model_adapter_properties(model, AdapterType::Snowflake).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "'adapter_properties' may only be specified to override catalogs.yml and cannot be used in a legacy model config",
            ));
        }

        // Core does not functionally permit a manually specified catalog_type in a model config.
        // Prompt the user to adopt catalogs.yml. [DELIBERATE CHANGE]: Core only ignores this silently.
        // This should be an impossible field by YAML strict mode.
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Snowflake).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in catalog entries of catalogs.yml",
            ));
        }

        let transient_spec =
            Self::get_model_config_value(model, "transient", AdapterType::Snowflake);
        let transient_parsed = transient_spec
            .as_ref()
            .map(|s| s.eq_ignore_ascii_case("true"));

        let raw_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Snowflake);
        let table_format = TableFormat::parse(raw_table_format.as_deref()).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("{e}. For other table formats, use catalogs.yml write integrations."),
            )
        })?;

        match table_format {
            // ============================================
            // table_format unspecified or 'default' (legacy path)
            // ============================================
            TableFormat::Default => {
                let external_volume =
                    Self::get_model_config_value(model, "external_volume", AdapterType::Snowflake);
                let base_location_root = Self::get_model_config_value(
                    model,
                    "base_location_root",
                    AdapterType::Snowflake,
                );
                let base_location_subpath = Self::get_model_config_value(
                    model,
                    "base_location_subpath",
                    AdapterType::Snowflake,
                );

                if external_volume.is_some()
                    || base_location_root.is_some()
                    || base_location_subpath.is_some()
                {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        LEGACY_CONFIG_ICEBERG_ATTRIBUTE_ERR,
                    ));
                }

                Ok(CatalogRelation {
                    adapter_type: AdapterType::Snowflake,
                    catalog_name: None,
                    integration_name: None,
                    catalog_type: CatalogType::SnowflakeNative,
                    table_format: TableFormat::Default,
                    adapter_properties: BTreeMap::new(),
                    is_transient: Some(transient_parsed.unwrap_or(true)),
                    external_volume: None,
                    catalog_database: None,
                    lakehouse_catalog: None,
                    base_location: None,
                    file_format: None,
                })
            }

            // ====================================
            // table_format='iceberg' (legacy path)
            // ====================================
            TableFormat::Iceberg => {
                // FIXME(versusfacit): we just swallow transient here for now instead of
                // honoring it. Snowflake actually supports transient iceberg tables when the
                // location is Snowflake managed storage aka SNOWFLAKE_MANAGED. We need to
                // detect that case and stop dropping the value on the floor. See
                // dbt-labs/dbt-core#15427 and
                // https://docs.snowflake.com/en/user-guide/tables-iceberg-internal-storage

                // Leave `external_volume` unset (rather than defaulting it to the literal
                // string "SNOWFLAKE_MANAGED") when the model doesn't configure one: per the
                // docs linked above, Snowflake-managed internal storage means OMITTING
                // `EXTERNAL_VOLUME` from the DDL entirely, not setting it to that string --
                // `SNOWFLAKE_MANAGED` is only meaningful here as a sentinel a user might
                // explicitly write to request internal storage, still recognized by the
                // `base_location` check below.
                let external_volume =
                    Self::get_model_config_value(model, "external_volume", AdapterType::Snowflake);
                let base_location_root = Self::get_model_config_value(
                    model,
                    "base_location_root",
                    AdapterType::Snowflake,
                );
                let base_location_subpath = Self::get_model_config_value(
                    model,
                    "base_location_subpath",
                    AdapterType::Snowflake,
                );

                let schema = Self::get_model_config_value(model, "schema", AdapterType::Snowflake);
                let identifier = Self::get_model_config_value(
                    model,
                    "alias",
                    AdapterType::Snowflake,
                )
                .or_else(|| {
                    Self::get_model_config_value(model, "identifier", AdapterType::Snowflake)
                });

                let base_location = external_volume
                    .as_ref()
                    .filter(|v| {
                        !v.trim()
                            .eq_ignore_ascii_case(SNOWFLAKE_MANAGED_EXTERNAL_VOLUME)
                    })
                    .map(|_| {
                        Self::build_base_location(
                            &base_location_root,
                            &base_location_subpath,
                            &schema,
                            &identifier,
                        )
                    });

                let mut adapter_properties = BTreeMap::new();
                if let Some(v) =
                    Self::get_model_config_value(model, "iceberg_version", AdapterType::Snowflake)
                {
                    adapter_properties.insert("iceberg_version".to_string(), v);
                }

                Ok(CatalogRelation {
                    adapter_type: AdapterType::Snowflake,
                    catalog_name: None,
                    integration_name: None,
                    table_format: TableFormat::Iceberg,
                    catalog_type: CatalogType::SnowflakeBuiltIn,
                    external_volume,
                    catalog_database: None,
                    lakehouse_catalog: None,
                    base_location,
                    adapter_properties,
                    is_transient: Some(false),
                    file_format: None,
                })
            }
        }
    }

    /// Helper for building a catalog relation of any type supported in catalogs.yml
    ///
    /// A catalog write integration holds fallback metadata for model materialization DDL.
    /// Any individual model may override the catalog metadata with their own model configs.
    fn build_with_catalogs(
        model: &Value,
        catalogs: &YmlMapping,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let catalog = find_catalog(catalogs, catalog_name).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })?;

        // 1) identity: catalog comes from MC; integration is the catalog's active one
        let integration_name = lookup_integration_name(catalogs, catalog_name).unwrap_or_default();

        // 2) write integration lookup (may be None)
        let write_integration = Self::lookup_write_integration(catalog, &integration_name);

        // 3) resolve fields: model > write_integration > default/None

        // === catalog_type logic forbids overrides as Core hardcodes in catalogs.yml
        if Self::get_model_config_value(model, "catalog_type", AdapterType::Snowflake).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "catalog_type may only be specified in write integration entries of catalogs.yml",
            ));
        }

        let raw_catalog_type = Self::yml_str(write_integration, "catalog_type".to_owned())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "catalog_type is required by the catalogs.yml schema on every write integration. Validation should have already rejected its absence",
                )
            })?;

        let catalog_type = if raw_catalog_type.eq_ignore_ascii_case("built_in") {
            CatalogType::SnowflakeBuiltIn
        } else if raw_catalog_type.eq_ignore_ascii_case("snowflake") {
            CatalogType::SnowflakeNative
        } else if raw_catalog_type.eq_ignore_ascii_case("iceberg_rest") {
            CatalogType::IcebergRest
        } else {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid Snowflake catalog_type '{raw_catalog_type}'"),
            ));
        };

        let model_table_format =
            Self::get_model_config_value(model, "table_format", AdapterType::Snowflake);
        let yml_table_format = Self::yml_str(write_integration, "table_format".to_string());
        let table_format = TableFormat::parse(
            model_table_format
                .as_deref()
                .or(yml_table_format.as_deref()),
        )
        .map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Invalid table_format in catalog '{catalog_name}': {e}"),
            )
        })?;

        // === Build up the external volume
        let external_volume =
            Self::get_model_config_value(model, "external_volume", AdapterType::Snowflake)
                .or_else(|| Self::yml_str(write_integration, "external_volume".to_string()));

        // === Build up base location
        let yaml_adapter_props = Self::get_yaml_adapter_properties(write_integration);
        let model_adapter_props = Self::get_model_adapter_properties(model, AdapterType::Snowflake);

        // base_location_root: model(adapter_properties) > model (legacy) > YAML > default
        let base_location_root =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_root")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_root",
                        AdapterType::Snowflake,
                    )
                })
                .or_else(|| {
                    Self::get_adapter_property(yaml_adapter_props.as_ref(), "base_location_root")
                });

        // base_location_subpath: model (adapter_properties) > model (legacy) > default
        if Self::yml_str(write_integration, "base_location_subpath".to_string()).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "base_location_subpath is forbidden by the catalogs.yml schema on write integrations. Model-level base_location_subpath must be used instead, and validation should have already rejected this",
            ));
        }
        let base_location_subpath =
            Self::get_adapter_property(model_adapter_props.as_ref(), "base_location_subpath")
                .or_else(|| {
                    Self::get_model_config_value(
                        model,
                        "base_location_subpath",
                        AdapterType::Snowflake,
                    )
                });

        let schema = Self::get_model_config_value(model, "schema", AdapterType::Snowflake);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Snowflake)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Snowflake));

        let base_location = Self::build_base_location(
            &base_location_root,
            &base_location_subpath,
            &schema,
            &identifier,
        );

        // 4) adapter_properties from YAML write_integration.adapter_properties and model config overrides
        let mut adapter_properties =
            Self::merged_adapter_properties(yaml_adapter_props, model_adapter_props);

        // Model-level iceberg_version takes precedence over catalog adapter_properties
        if let Some(v) =
            Self::get_model_config_value(model, "iceberg_version", AdapterType::Snowflake)
        {
            adapter_properties.insert("iceberg_version".to_string(), v);
        }

        // 5) transient handling
        //
        // FIXME(versusfacit): same swallowed transient case as in build_without_catalogs_yml
        // above.

        Ok(CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: Some(integration_name),
            catalog_type,
            table_format,
            external_volume,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: Some(base_location),
            adapter_properties,
            is_transient: Some(false),
            file_format: None,
        })
    }

    // [DELIBERATE CHANGE] Core always has schema and identifier in model config,
    // but we do not apparently. This can subtly change location paths in external volumes.
    // https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-snowflake/src/dbt/adapters/snowflake/parse_model.py#L34
    fn build_base_location(
        root: &Option<String>,
        subpath: &Option<String>,
        schema: &Option<String>,
        identifier: &Option<String>,
    ) -> String {
        // default prefix if not provided
        // see core: https://github.com/dbt-labs/dbt-adapters/blob/80b505709373d0eb027ad0311b16f09c8a4b9bad/dbt-snowflake/src/dbt/adapters/snowflake/parse_model.py#L40
        let prefix = root
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("_dbt");

        let mut parts = vec![prefix.to_string()];

        if let Some(s) = schema.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(s.to_string());
        }
        // https://github.com/dbt-labs/dbt-adapters/blob/80b505709373d0eb027ad0311b16f09c8a4b9bad/dbt-snowflake/src/dbt/adapters/snowflake/parse_model.py#L41C5-L41C57
        if let Some(id) = identifier
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            parts.push(id.to_string());
        }
        if let Some(sp) = subpath.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            parts.push(sp.to_string());
        }

        parts.join("/")
    }

    /// Build the effective `adapter_properties` by combining values from
    /// - catalogs.yml `adapter_properties` (base set), and
    /// - model config values (which override when present).
    ///
    /// The precedence is:
    ///     model_config > catalogs.yml
    fn merged_adapter_properties(
        yaml_adapter_props: Option<BTreeMap<String, String>>,
        model_adapter_props: Option<BTreeMap<String, String>>,
    ) -> BTreeMap<String, String> {
        let mut merged = BTreeMap::new();

        // 1) seed from catalogs.yml
        if let Some(adapter_props) = yaml_adapter_props {
            for (k, v) in adapter_props {
                merged.insert(k, v);
            }
        }

        // 2) overlay model adapter_properties if present
        if let Some(adapter_props) = model_adapter_props {
            for (k, v) in adapter_props {
                merged.insert(k, v);
            }
        }

        merged
    }

    fn yaml_scalar_to_string(v: &YmlValue) -> Option<String> {
        if let Some(b) = v.as_bool() {
            return Some(if b { "true".into() } else { "false".into() });
        }
        if let Some(s) = v.as_str() {
            return Some(s.to_owned());
        }
        if let Some(i) = v.as_i64() {
            return Some(i.to_string());
        }
        if let Some(u) = v.as_u64() {
            return Some(u.to_string());
        }
        debug_assert!(false, "unexpected YAML scalar: {v:?}");
        None
    }

    //
    // === Value Extractors
    //

    // [DELIBERATE CHANGE]: serialization can sometimes serialize None into Some("none")
    // which is not how core reads values in.
    //
    // TODO(anna): At the moment, we don't have the type safety of knowing that `model` has
    // type `DbtModel`, so we try to get the value at both the top level and under `model.config`.
    // Once we can enforce the type of `model`, we won't need these value extractors anymore.
    fn get_model_config_value(
        model: &Value,
        key: &str,
        adapter_type: AdapterType,
    ) -> Option<String> {
        let adapter_attr = match adapter_type {
            AdapterType::Bigquery => BIGQUERY_ATTR,
            AdapterType::Databricks => DATABRICKS_ATTR,
            AdapterType::Snowflake => SNOWFLAKE_ATTR,
            // Lake compute and GizmoSQL model configs surface the same way DuckDB's do.
            AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => DUCKDB_ATTR,
            _ => return None,
        };
        let model_config = if let Ok(adapter_attr) = model.get_attr(adapter_attr)
            && !adapter_attr.is_undefined()
        {
            adapter_attr
        } else {
            model.get_attr("config").ok()?
        };

        let value = match model.get_attr(key) {
            Ok(v) if !v.is_undefined() => v,
            _ => {
                if let Ok(v) = model_config.get_attr(key)
                    && !v.is_undefined()
                {
                    v
                } else {
                    return None;
                }
            }
        };

        if value.is_none() {
            None
        } else {
            Some(value.to_string())
        }
    }

    // TODO(anna): We can remove this once `model` no longer has type `Value`.
    fn get_model_adapter_properties(
        model: &Value,
        adapter_type: AdapterType,
    ) -> Option<BTreeMap<String, String>> {
        let adapter_attr = match adapter_type {
            AdapterType::Bigquery => BIGQUERY_ATTR,
            AdapterType::Databricks => DATABRICKS_ATTR,
            AdapterType::Snowflake => SNOWFLAKE_ATTR,
            // Lake compute and GizmoSQL model configs surface the same way DuckDB's do.
            AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => DUCKDB_ATTR,
            _ => return None,
        };
        let model_config = if let Ok(adapter_attr) = model.get_attr(adapter_attr)
            && !adapter_attr.is_undefined()
        {
            adapter_attr
        } else {
            model.get_attr("config").ok()?
        };

        if let Ok(adapter_properties_val) = model_config.get_attr("adapter_properties") {
            if adapter_properties_val.is_undefined() {
                return None;
            }

            let mut map = BTreeMap::new();
            if let Ok(keys) = adapter_properties_val.try_iter() {
                for key in keys {
                    let key_str = key
                        .as_str()
                        .map(|s| s.to_string())
                        .unwrap_or_else(|| key.to_string());
                    if let Ok(val) = adapter_properties_val.get_item(&key) {
                        let val_str = val
                            .as_str()
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| val.to_string());
                        map.insert(key_str, val_str);
                    }
                }
            }

            Some(map)
        } else {
            None
        }
    }

    fn get_yaml_adapter_properties(
        write_integration: Option<&YmlMapping>,
    ) -> Option<BTreeMap<String, String>> {
        if let Some(YmlValue::Mapping(adapter_props, _)) =
            write_integration.and_then(|m| m.get(key("adapter_properties".to_string())))
        {
            let mut map = BTreeMap::new();
            for (k, v) in adapter_props {
                if let Some(name) = k.as_str()
                    && let Some(s) = Self::yaml_scalar_to_string(v)
                {
                    map.insert(name.to_string(), s);
                }
            }
            Some(map)
        } else {
            None
        }
    }

    fn yml_str(m: Option<&YmlMapping>, k: String) -> Option<String> {
        m.and_then(|mm| mm.get(key(k)))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    #[inline]
    fn get_adapter_property(
        adapter_properties: Option<&BTreeMap<String, String>>,
        key: &str,
    ) -> Option<String> {
        adapter_properties.and_then(|props| props.get(key).cloned())
    }

    #[inline]
    fn lookup_write_integration<'a>(
        catalog: &'a YmlMapping,
        integration_name: &str,
    ) -> Option<&'a YmlMapping> {
        let seq = catalog
            .get(key("write_integrations".to_string()))?
            .as_sequence()?;
        seq.iter().filter_map(|v| v.as_mapping()).find(|m| {
            m.get(key("name".to_string()))
                .or_else(|| m.get(key("integration_name".to_string())))
                .and_then(|v| v.as_str())
                .map(|s| s == integration_name)
                .unwrap_or(false)
        })
    }

    fn map_opt_bool(v: Option<bool>) -> Value {
        match v {
            Some(b) => Value::from(b),
            None => Value::from(()),
        }
    }

    fn map_opt_str(v: Option<String>) -> Value {
        match v.as_deref().map(|s| s.trim()).filter(|t| !t.is_empty()) {
            Some(t) => Value::from(t),
            None => Value::from(()),
        }
    }

    // plain String fields (always defined, but still check empty)
    fn map_str_val(v: &str) -> Value {
        Value::from(v)
    }

    fn map_properties_str(m: &BTreeMap<String, String>, k: &str) -> Value {
        match m.get(k).map(|s| s.trim()).filter(|t| !t.is_empty()) {
            Some(t) => Value::from(t),
            None => Value::from(()),
        }
    }

    fn map_properties_bool(m: &BTreeMap<String, String>, k: &str) -> Value {
        match m.get(k) {
            Some(s) => Value::from(s.trim().eq_ignore_ascii_case("true")),
            None => Value::from(()),
        }
    }

    fn map_properties_u32(m: &BTreeMap<String, String>, k: &str) -> Value {
        match m.get(k).and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(n) => Value::from(n as i64),
            None => Value::from(()),
        }
    }

    // === begin HACK
    /// Returns true if `db_name` appears under any write_integration whose
    /// `catalog_type` is `iceberg_rest` and whose
    /// `adapter_properties.catalog_linked_database` equals `db_name`.
    fn cld_exists_in_iceberg_rest(catalogs: &YmlMapping, db_name: &str) -> bool {
        let Some(seq) = catalogs
            .get(key("catalogs".to_string()))
            .and_then(|v| v.as_sequence())
        else {
            return false;
        };

        for cat in seq.iter().filter_map(|v| v.as_mapping()) {
            let Some(write_integrations) = cat
                .get(key("write_integrations".to_string()))
                .and_then(|v| v.as_sequence())
            else {
                continue;
            };
            for write_integration in write_integrations.iter().filter_map(|v| v.as_mapping()) {
                if let Some(ct) = write_integration
                    .get(key("catalog_type".into()))
                    .and_then(|v| v.as_str())
                    && !ct.eq_ignore_ascii_case("iceberg_rest")
                {
                    continue;
                }
                let adapter_properties = write_integration
                    .get(key("adapter_properties".to_string()))
                    .and_then(|v| v.as_mapping());
                if let Some(cld) = adapter_properties
                    .and_then(|m| m.get(key("catalog_linked_database".to_string())))
                    .and_then(|v| v.as_str())
                    && cld.eq_ignore_ascii_case(db_name)
                {
                    return true;
                }
            }
        }
        false
    }

    /// Build an "empty" catalog relation: everything None/empty, falling back
    /// to the INFO_SCHEMA store and DEFAULT table format.
    pub fn default_catalog_relation_snowflake() -> Self {
        CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: None,
            integration_name: None,
            catalog_type: CatalogType::SnowflakeNative,
            table_format: TableFormat::Default,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties: BTreeMap::new(),
            is_transient: Some(true),
            file_format: None,
        }
    }

    // === end HACK

    pub fn supports_create_or_replace(&self) -> bool {
        match self.adapter_type {
            AdapterType::Databricks => {
                self.table_format.is_iceberg()
                    || self
                        .file_format
                        .as_deref()
                        .is_some_and(|f| f.eq_ignore_ascii_case("delta"))
            }
            _ => unimplemented!(
                "supports_create_or_replace is not implemented for {:?}",
                self.adapter_type
            ),
        }
    }

    // helper for get_value in impl Object
    fn gate_by_adapter(
        &self,
        adapter_types: Vec<AdapterType>,
        value_fetch: impl Fn() -> Value,
    ) -> Value {
        if adapter_types.contains(&self.adapter_type) {
            value_fetch()
        } else {
            Value::from(())
        }
    }
    /// The user's explicit `stage_create_tables` from catalogs.yml
    /// `config.duckdb`, if set — mirrors the duckdb-iceberg#1017
    /// `STAGE_CREATE_TABLES` ATTACH option and steers
    /// [`Self::duckdb_write_strategy`].
    fn stage_create_tables_override(&self) -> Option<bool> {
        self.adapter_properties
            .get("stage_create_tables")
            .map(|v| v.eq_ignore_ascii_case("true"))
    }

    /// Adapter-generic Jinja surface for materializations: whether dbt's write
    /// path stage-creates (`CREATE ... AS SELECT`) for this relation. For
    /// DuckDB this is derived from [`Self::duckdb_write_strategy`] — the FSM is
    /// the single write-path decision point — so the key can never disagree
    /// with the SQL the macros actually emit. (`stage_create_tables` in
    /// catalogs.yml feeds the FSM, not this key directly.)
    pub fn supports_stage_create(&self) -> bool {
        match self.adapter_type {
            AdapterType::DuckDB => matches!(
                self.duckdb_write_strategy(),
                DuckDbWriteStrategy::CreateAsSelect | DuckDbWriteStrategy::DirectCreateAsSelect
            ),
            _ => true,
        }
    }

    /// The single write-path decision for DuckDB materializations. Collapses the
    /// `supports_stage_create` × `table_format == 'iceberg'` boolean matrix the
    /// macros used to re-derive into one named state, exposed to Jinja via the
    /// `duckdb_write_strategy` key.
    ///
    /// TODO(catalog-relation-typed-fields): once non-DuckDB adapters carry typed
    /// eager fields too, this can be precomputed/stored at construction rather
    /// than derived. Tracked alongside the String->enum migration follow-up.
    pub fn duckdb_write_strategy(&self) -> DuckDbWriteStrategy {
        match self.adapter_type {
            AdapterType::DuckDB => {
                // Only true Iceberg catalogs use the direct-create path. DuckLake
                // always writes via the standard CTAS flow, so guard on
                // catalog_type as well as table_format — a stray `table_format`
                // on a DuckLake catalog (which schema validation rejects, but
                // defend in depth) must not route here.
                let is_ducklake = matches!(self.catalog_type, CatalogType::DuckLake);
                if !is_ducklake && self.table_format.is_iceberg() {
                    // `stage_create_tables: true` opts in to staged creates, so
                    // dbt may CTAS the target in place; unset or false stays on
                    // the empty CREATE + INSERT that works under either ATTACH
                    // mode (Horizon rejects staged creates — its preset default
                    // is false).
                    if self.stage_create_tables_override() == Some(true) {
                        DuckDbWriteStrategy::DirectCreateAsSelect
                    } else {
                        DuckDbWriteStrategy::DirectCreate
                    }
                } else {
                    DuckDbWriteStrategy::CreateAsSelect
                }
            }
            // The Jinja key is exposed on every CatalogRelation; only DuckDB
            // materializations consume it, but never let it mislead (a Snowflake
            // iceberg relation is not a duckdb direct-create target).
            _ => DuckDbWriteStrategy::CreateAsSelect,
        }
    }
}

#[inline]
fn key(key: String) -> YmlValue {
    YmlValue::String(key, Span::default())
}

fn find_catalog<'a>(catalogs: &'a YmlMapping, catalog_name: &str) -> Option<&'a YmlMapping> {
    let seq = catalogs.get(key("catalogs".to_string()))?.as_sequence()?;
    seq.iter().filter_map(|v| v.as_mapping()).find(|m| {
        // match on name or catalog_name
        let n1 = m.get(key("name".to_string())).and_then(|v| v.as_str());
        let n2 = m
            .get(key("catalog_name".to_string()))
            .and_then(|v| v.as_str());
        // backwards compatbility measure for dbt snowflake only
        // see: https://github.com/dbt-labs/dbt-adapters/pull/1134
        let n3 = m.get(key("catalog".to_string())).and_then(|v| v.as_str());
        n1 == Some(catalog_name) || n2 == Some(catalog_name) || n3 == Some(catalog_name)
    })
}

fn lookup_integration_name(catalogs: &YmlMapping, catalog_name: &str) -> Option<String> {
    let cat = find_catalog(catalogs, catalog_name)?;
    cat.get(key("active_write_integration".to_string()))?
        .as_str()
        .map(|s| s.to_string())
}

impl Object for CatalogRelation {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        Some(match key.as_str()? {
            // identity / routing
            "catalog_name" => Self::map_opt_str(self.catalog_name.clone()),
            "integration_name" => Self::map_opt_str(self.integration_name.clone()),

            "catalog_type" => Self::map_str_val(self.catalog_type.as_str()),
            "table_format" => Self::map_str_val(self.physical_table_format().as_str()),
            // ===== DuckDB-specific =====
            "supports_stage_create" => Value::from(self.supports_stage_create()),
            "duckdb_write_strategy" => Value::from(self.duckdb_write_strategy().as_str()),

            // common optional
            "base_location" => Self::map_opt_str(self.base_location.clone()),

            // expose full map
            "adapter_properties" => Value::from_serialize(self.adapter_properties.clone()),

            // === Adapter properties

            // all via adapter_properties
            "max_data_extension_time_in_days" => Self::map_properties_u32(
                &self.adapter_properties,
                "max_data_extension_time_in_days",
            ),

            // BUILT_IN
            "change_tracking" => {
                Self::map_properties_bool(&self.adapter_properties, "change_tracking")
            }
            "data_retention_time_in_days" => {
                Self::map_properties_u32(&self.adapter_properties, "data_retention_time_in_days")
            }
            "storage_serialization_policy" => {
                Self::map_properties_str(&self.adapter_properties, "storage_serialization_policy")
            }

            // BUILT_IN + REST
            "iceberg_version" => {
                Self::map_properties_u32(&self.adapter_properties, "iceberg_version")
            }

            // REST
            "auto_refresh" => Self::map_properties_bool(&self.adapter_properties, "auto_refresh"),
            "catalog_linked_database" => {
                Self::map_properties_str(&self.adapter_properties, "catalog_linked_database")
            }
            "attached_database" => {
                Self::map_properties_str(&self.adapter_properties, "attached_database")
            }
            "catalog_linked_database_type" => Self::map_properties_str(
                &self.adapter_properties,
                ADAPTER_PROP_CATALOG_LINKED_DATABASE_TYPE,
            ),
            "target_file_size" => {
                Self::map_properties_str(&self.adapter_properties, "target_file_size")
            }
            "external_root" => Self::map_properties_str(&self.adapter_properties, "external_root"),

            // v2-only REST surface
            "catalog_database" => self
                .catalog_database
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::UNDEFINED),
            "lakehouse_catalog" => self
                .lakehouse_catalog
                .as_deref()
                .map(Value::from)
                .unwrap_or(Value::UNDEFINED),
            "linked_catalog_provider" => self
                .linked_catalog_provider()
                .map(Value::from_object)
                .unwrap_or_else(|| Value::from(())),

            // === Snowflake
            "is_transient" => self.gate_by_adapter(vec![AdapterType::Snowflake], || {
                Self::map_opt_bool(self.is_transient)
            }),
            "external_volume" => self.gate_by_adapter(vec![AdapterType::Snowflake], || {
                Self::map_opt_str(self.external_volume.clone())
            }),

            // === Databricks
            "file_format" => self.gate_by_adapter(
                vec![
                    AdapterType::Databricks,
                    AdapterType::Bigquery,
                    AdapterType::DuckDB,
                ],
                || Self::map_opt_str(self.file_format.clone()),
            ),
            "location" => self.gate_by_adapter(vec![AdapterType::Databricks], || {
                Self::map_opt_str(self.external_volume.clone())
            }),
            "use_uniform" => self.gate_by_adapter(vec![AdapterType::Databricks], || {
                Self::map_properties_bool(&self.adapter_properties, "use_uniform")
            }),

            // === Bigquery
            "storage_uri" => self.gate_by_adapter(vec![AdapterType::Bigquery], || {
                Self::map_properties_str(&self.adapter_properties, "storage_uri")
            }),
            "connection_id" => self.gate_by_adapter(vec![AdapterType::Bigquery], || {
                Self::map_properties_str(&self.adapter_properties, "connection_id")
            }),

            _ => Value::from(()),
        })
    }

    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State<'_, '_>,
        name: &str,
        _args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "has_catalog_linked_database" => Ok(self
                .gate_by_adapter(vec![AdapterType::Snowflake], || {
                    Value::from(self.has_catalog_linked_database())
                })),
            "supports_create_or_replace" => {
                if load_catalogs::fetch_use_catalogs_v2() {
                    Ok(self.gate_by_adapter(vec![AdapterType::Databricks], || {
                        Value::from(self.supports_create_or_replace())
                    }))
                } else {
                    Err(minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "catalog_relation.supports_create_or_replace() is only available under catalogs v2",
                    ))
                }
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on CatalogRelation: '{name}'"),
            )),
        }
    }

    fn render(self: &Arc<Self>, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CatalogRelation(catalog={}, integration={}, type={}, format={})",
            self.catalog_name.as_deref().unwrap_or("<none>"),
            self.integration_name.as_deref().unwrap_or("<none>"),
            self.catalog_type.as_str(),
            self.table_format.as_str()
        )
    }
}

impl Object for LinkedCatalogProvider {
    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        Some(match key.as_str()? {
            "is_glue" => Value::from(self.is_glue()),
            "is_unity" => Value::from(self.is_unity()),
            _ => Value::from(()),
        })
    }
}

#[cfg(test)]
mod has_catalog_linked_database_tests {
    use super::*;

    fn snowflake(catalog_type: CatalogType) -> CatalogRelation {
        CatalogRelation {
            catalog_type,
            table_format: TableFormat::Iceberg,
            ..CatalogRelation::default_catalog_relation_snowflake()
        }
    }

    #[test]
    fn v1_catalog_linked_database_is_linked() {
        let mut relation = snowflake(CatalogType::IcebergRest);
        relation.adapter_properties.insert(
            "catalog_linked_database".to_string(),
            "MY_LINKED_DB".to_string(),
        );
        assert!(relation.has_catalog_linked_database());
    }

    #[test]
    fn blank_catalog_linked_database_is_not_linked() {
        let mut relation = snowflake(CatalogType::IcebergRest);
        relation
            .adapter_properties
            .insert("catalog_linked_database".to_string(), "  ".to_string());
        assert!(!relation.has_catalog_linked_database());
    }

    #[test]
    fn v2_catalog_database_on_a_linked_catalog_is_linked() {
        let mut relation = snowflake(CatalogType::IcebergRest);
        relation.catalog_database = Some("MY_LINKED_DB".to_string());
        assert!(relation.has_catalog_linked_database());
    }

    #[test]
    fn v2_catalog_database_on_the_managed_catalog_is_not_linked() {
        let mut relation = snowflake(CatalogType::SnowflakeBuiltIn);
        relation.catalog_database = Some("ANALYTICS_ICEBERG".to_string());
        assert!(!relation.has_catalog_linked_database());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use minijinja::Value as JVal;
    use serde_json::json;

    fn adapter_type_to_attr(adapter_type: AdapterType) -> String {
        match adapter_type {
            AdapterType::Snowflake => "snowflake_attr".to_string(),
            AdapterType::Bigquery => "bigquery_attr".to_string(),
            AdapterType::Databricks => "databricks_attr".to_string(),
            _ => panic!("Not yet supported"),
        }
    }

    fn model(adapter_type: AdapterType, config: serde_json::Value) -> JVal {
        let mut model_map = serde_json::Map::new();
        let mut config_map = serde_json::Map::new();
        const TOP_LEVEL_KEYS: [&str; 6] = [
            "catalog_name",
            "table_format",
            "schema",
            "identifier",
            "database",
            "alias",
        ];
        if let serde_json::Value::Object(config) = config {
            for (key, value) in config {
                if TOP_LEVEL_KEYS.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
                    model_map.insert(key, value);
                } else {
                    config_map.insert(key, value);
                }
            }
            model_map.insert(
                adapter_type_to_attr(adapter_type),
                serde_json::Value::Object(config_map),
            );
            JVal::from_serialize(serde_json::Value::Object(model_map))
        } else {
            panic!("Config is not a JSON object");
        }
    }

    fn model_deprecated_config(config: serde_json::Value) -> JVal {
        let mut model_map = serde_json::Map::new();
        const TOP_LEVEL_KEYS: [&str; 6] = [
            "catalog_name",
            "table_format",
            "schema",
            "identifier",
            "database",
            "alias",
        ];
        if let serde_json::Value::Object(config) = config {
            model_map.insert(
                "config".to_owned(),
                serde_json::Value::Object(config.clone()),
            );
            for (key, value) in config {
                if TOP_LEVEL_KEYS.iter().any(|k| k.eq_ignore_ascii_case(&key)) {
                    model_map.insert(key, value);
                }
            }
            JVal::from_serialize(serde_json::Value::Object(model_map))
        } else {
            panic!("Config is not a JSON object");
        }
    }

    fn s(s: &str) -> YmlValue {
        YmlValue::String(s.to_owned(), Span::default())
    }
    fn boolv(b: bool) -> YmlValue {
        YmlValue::Bool(b, Span::default())
    }
    fn i64v(n: i64) -> YmlValue {
        YmlValue::Number(n.into(), Span::default())
    }
    fn u64v(n: u64) -> YmlValue {
        YmlValue::Number(n.into(), Span::default())
    }
    fn mapping(entries: &[(&str, YmlValue)]) -> YmlMapping {
        let mut m = YmlMapping::new();
        for (k, v) in entries {
            m.insert(s(k), v.clone());
        }
        m
    }
    fn map(entries: &[(&str, YmlValue)]) -> YmlValue {
        let mut m = YmlMapping::new();
        for (k, v) in entries {
            m.insert(s(k), v.clone());
        }
        YmlValue::Mapping(m, Span::default())
    }
    fn seq(items: &[YmlValue]) -> YmlValue {
        YmlValue::Sequence(items.to_vec(), Span::default())
    }

    /// Build a valid catalogs.yml mapping for a single catalog/integration.
    fn catalogs_yaml_one(
        catalog_name: &str,
        win: &str,
        catalog_type: &str,
        table_format: &str,
        extra_integration_fields: &[(&str, YmlValue)],
    ) -> YmlMapping {
        let mut wi = mapping(&[
            ("name", s(win)),
            ("catalog_type", s(catalog_type)),
            ("table_format", s(table_format)),
        ]);
        for (k, v) in extra_integration_fields {
            wi.insert(s(k), v.clone());
        }
        let cat = mapping(&[
            ("name", s(catalog_name)),
            ("active_write_integration", s(win)),
            (
                "write_integrations",
                seq(&[YmlValue::Mapping(wi, Span::default())]),
            ),
        ]);
        mapping(&[("catalogs", seq(&[YmlValue::Mapping(cat, Span::default())]))])
    }

    //
    // --- legacy config (no catalogs.yml) ---
    //

    #[test]
    fn legacy_default_implied_ok_and_forbids_external_and_base_location_fields() {
        // default implied
        let conf = json!({ "schema": "S", "identifier": "I" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.catalog_type, CatalogType::SnowflakeNative);
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.adapter_properties.is_empty());
        }

        // forbidden on DEFAULT (implied)
        for (k, v) in [
            ("external_volume", "EV"),
            ("base_location_root", "root"),
            ("base_location_subpath", "sub"),
        ] {
            let conf = json!({ k: v });
            let ms = [
                model(AdapterType::Snowflake, conf.clone()),
                model_deprecated_config(conf),
            ];
            for m in ms {
                let err = CatalogRelation::build_without_catalogs_yml(&m).unwrap_err();
                assert!(
                    format!("{err}").contains("not able to be specified on table_format=default")
                );
            }
        }
    }

    #[test]
    fn legacy_default_explicit_ok_and_forbids_externals() {
        let conf = json!({ "table_format": "DEFAULT" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.catalog_type, CatalogType::SnowflakeNative);
        }

        let conf = json!({ "table_format": "DEFAULT", "external_volume": "EV" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_without_catalogs_yml(&m).unwrap_err();
            assert!(format!("{err}").contains("not able to be specified on table_format=default"));
        }
    }

    #[test]
    fn legacy_iceberg_sets_built_in_and_synthesizes_base_location() {
        let conf = json!({
            "table_format": "ICEBERG",
            "external_volume": "EV",
            "base_location_root": "_root",
            "base_location_subpath": "sub",
            "schema": "SCH",
            "identifier": "ID"
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.external_volume.as_deref(), Some("EV"));
            assert_eq!(r.base_location.as_deref(), Some("_root/SCH/ID/sub"));
        }
    }

    #[test]
    fn legacy_iceberg_unconfigured_external_volume_omits_both_clauses() {
        let conf = json!({
            "table_format": "ICEBERG",
            "schema": "SCH",
            "identifier": "ID"
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            // No `external_volume` configured -- leave it unset so the macro omits
            // the clause entirely, rather than emitting the literal (bogus)
            // `external_volume = 'SNOWFLAKE_MANAGED'`, which Snowflake rejects as a
            // nonexistent/unauthorized volume.
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
        }
    }

    #[test]
    fn legacy_iceberg_explicit_snowflake_managed_external_volume_omits_base_location() {
        for ev in [
            "SNOWFLAKE_MANAGED",
            "snowflake_managed",
            "  Snowflake_Managed  ",
        ] {
            let conf = json!({
                "table_format": "ICEBERG",
                "external_volume": ev,
                "base_location_root": "_root",
                "base_location_subpath": "sub",
                "schema": "SCH",
                "identifier": "ID"
            });
            let ms = [
                model(AdapterType::Snowflake, conf.clone()),
                model_deprecated_config(conf),
            ];
            for m in ms {
                let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
                assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
                assert_eq!(r.table_format, TableFormat::Iceberg);
                assert_eq!(r.external_volume.as_deref(), Some(ev));
                assert!(
                    r.base_location.is_none(),
                    "base_location must be omitted for external_volume={ev:?}"
                );
            }
        }
    }

    #[test]
    fn legacy_only_default_or_iceberg_allowed() {
        let conf = json!({ "table_format": "PARQUET" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_without_catalogs_yml(&m).unwrap_err();
            assert!(format!("{err}").contains("Unsupported table_format 'PARQUET'"));
            assert!(format!("{err}").contains(&TableFormat::opts_display()));
        }
    }

    #[test]
    fn legacy_catalog_type_forbidden_at_model_level() {
        let conf = json!({ "catalog_type": "BUILT_IN" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_without_catalogs_yml(&m).unwrap_err();
            assert!(
                format!("{err}").contains(
                    "catalog_type may only be specified in catalog entries of catalogs.yml"
                )
            );
        }
    }

    #[test]
    fn legacy_adapter_properties_blocked_and_transient_ignored() {
        // adapter_properties blocked
        let conf = json!({ "adapter_properties": { "x": "y" } });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_without_catalogs_yml(&m).unwrap_err();
            assert!(format!("{err}").contains("'adapter_properties' may only be specified"));
        }

        // transient is ignored (no error, no effect)
        let conf = json!({ "transient": true });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(r.table_format, TableFormat::Default);
            assert!(r.adapter_properties.is_empty());
        }
    }

    //
    // --- base location ---
    //

    #[test]
    fn base_location_defaults_and_order() {
        assert_eq!(
            CatalogRelation::build_base_location(&None, &None, &None, &None),
            "_dbt"
        );
        assert_eq!(
            CatalogRelation::build_base_location(&None, &None, &Some("S".into()), &None),
            "_dbt/S"
        );
        assert_eq!(
            CatalogRelation::build_base_location(
                &None,
                &None,
                &Some("S".into()),
                &Some("I".into())
            ),
            "_dbt/S/I"
        );
        assert_eq!(
            CatalogRelation::build_base_location(
                &Some("_root".into()),
                &Some("sub".into()),
                &Some("S".into()),
                &Some("I".into())
            ),
            "_root/S/I/sub"
        );
    }

    //
    // --- from_model_config_and_catalogs orchestration
    //

    #[test]
    fn from_model_no_catalog_name_uses_legacy_path() {
        let conf = json!({});
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Snowflake, &m, None)
                    .unwrap();
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.catalog_type, CatalogType::SnowflakeNative);
        }
    }

    #[test]
    fn from_model_catalog_name_without_catalogs_errors() {
        let conf = json!({ "catalog_name": "CAT" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Snowflake, &m, None)
                    .unwrap_err();
            assert!(format!("{err}").contains("catalog_name 'CAT'"));
            assert!(format!("{err}").contains("catalogs.yml was not found"));
        }
    }

    #[test]
    fn from_model_catalog_name_string_none_is_treated_as_absent() {
        // "none" (any case) treated as not provided -> legacy
        let conf = json!({ "catalog_name": "None" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Snowflake, &m, None)
                    .unwrap();
            assert_eq!(r.table_format, TableFormat::Default);
            assert!(r.catalog_name.is_none());
        }
    }

    //
    // --- catalogs.yml reconciliation ---
    //

    #[test]
    fn catalogs_reconciliation_model_overrides_and_merging() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "BUILT_IN",
            "ICEBERG",
            &[
                ("external_volume", s("EV_YAML")),
                (
                    "adapter_properties",
                    map(&[
                        ("change_tracking", boolv(true)),
                        ("target_file_size", u64v(128)),
                        ("storage_serialization_policy", s("SNAPPY")),
                        ("base_location_root", s("_root_yaml")),
                    ]),
                ),
            ],
        );

        let conf = json!({
            "catalog_name": "CAT",
            "table_format": "ICEBERG",
            "schema": "S",
            "identifier": "I",
            "external_volume": "EV_MODEL",
            "base_location_subpath": "sub_model",
            "adapter_properties": { "storage_serialization_policy": "ZSTD" }
        });

        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert_eq!(r.catalog_name.as_deref(), Some("CAT"));
            assert_eq!(r.integration_name.as_deref(), Some("WIN"));
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            assert_eq!(r.table_format, TableFormat::Iceberg);

            // precedence: model > catalogs.yml
            assert_eq!(r.external_volume.as_deref(), Some("EV_MODEL"));
            assert_eq!(r.base_location.as_deref(), Some("_root_yaml/S/I/sub_model"));

            // merged adapter_properties; model override wins
            assert_eq!(
                r.adapter_properties
                    .get("change_tracking")
                    .map(|s| s.as_str()),
                Some("true")
            );
            assert_eq!(
                r.adapter_properties
                    .get("target_file_size")
                    .map(|s| s.as_str()),
                Some("128")
            );
            assert_eq!(
                r.adapter_properties
                    .get("storage_serialization_policy")
                    .map(|s| s.as_str()),
                Some("ZSTD")
            );
        }
    }

    #[test]
    fn catalogs_iceberg_flow_is_respected() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "BUILT_IN",
            "ICEBERG",
            &[
                ("external_volume", s("EV")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("_root"))]),
                ),
            ],
        );
        let conf = json!({ "catalog_name": "CAT", "schema": "S", "identifier": "I", "base_location_subpath": "sub" });

        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.external_volume.as_deref(), Some("EV"));
            assert_eq!(r.base_location.as_deref(), Some("_root/S/I/sub"));
        }
    }

    #[test]
    fn catalogs_bad_table_format_in_model_override_is_rejected() {
        let cats = catalogs_yaml_one("CAT", "WIN", "BUILT_IN", "DEFAULT", &[]);
        let conf = json!({ "catalog_name": "CAT", "table_format": "FANCY" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap_err();
            assert!(format!("{err}").contains("Unsupported table_format 'FANCY'"));
            assert!(format!("{err}").contains(&TableFormat::opts_display()));
        }
    }

    #[test]
    fn catalogs_model_cannot_override_catalog_type() {
        let cats = catalogs_yaml_one("CAT", "WIN", "INFO_SCHEMA", "DEFAULT", &[]);
        let conf = json!({ "catalog_name": "CAT", "catalog_type": "BUILT_IN" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap_err();
            assert!(format!("{err}").contains(
                "catalog_type may only be specified in write integration entries of catalogs.yml"
            ));
        }
    }

    #[test]
    fn model_root_override_trims() {
        let bl = CatalogRelation::build_base_location(
            &Some("   root_with_spaces   ".into()),
            &None,
            &Some("S".into()),
            &Some("I".into()),
        );
        assert_eq!(bl, "root_with_spaces/S/I");
    }

    #[test]
    fn yaml_scalar_normalization_bool_i64_u64() {
        assert_eq!(
            CatalogRelation::yaml_scalar_to_string(&boolv(true)),
            Some("true".into())
        );
        assert_eq!(
            CatalogRelation::yaml_scalar_to_string(&i64v(-5)),
            Some("-5".into())
        );
        assert_eq!(
            CatalogRelation::yaml_scalar_to_string(&u64v(42)),
            Some("42".into())
        );
    }

    #[test]
    fn fallback_base_location_defaults_to_dbt() {
        // no root/subpath in model or yaml
        let bl = CatalogRelation::build_base_location(
            &None,
            &None,
            &Some("S".into()),
            &Some("I".into()),
        );
        assert_eq!(bl, "_dbt/S/I");
    }

    //
    // --- is transient reconciliation ---
    //
    #[test]
    fn legacy_default_transient_unspecified_defaults_true() {
        let conf = json!({ "table_format": "DEFAULT" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(r.is_transient.unwrap());
        }
    }

    #[test]
    fn legacy_default_transient_false_explicit() {
        let conf = json!({ "table_format": "DEFAULT", "transient": false });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn legacy_default_transient_true_explicit() {
        let conf = json!({ "table_format": "DEFAULT", "transient": true });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(r.is_transient.unwrap());
        }
    }

    #[test]
    fn legacy_iceberg_transient_false_is_ignored() {
        let conf = json!({ "table_format": "ICEBERG", "transient": false });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn legacy_iceberg_transient_true_is_ignored() {
        let conf = json!({ "table_format": "ICEBERG", "transient": true });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn legacy_iceberg_unspecified_transient_defaults_false() {
        let conf = json!({ "table_format": "ICEBERG" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn catalogs_iceberg_unspecified_transient_defaults_false() {
        let cats = catalogs_yaml_one("CAT", "WIN", "BUILT_IN", "ICEBERG", &[]);
        let conf = json!({ "catalog_name": "CAT" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn catalogs_iceberg_transient_true_is_ignored() {
        let cats = catalogs_yaml_one("CAT", "WIN", "BUILT_IN", "ICEBERG", &[]);
        let conf = json!({ "catalog_name": "CAT", "transient": true });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    #[test]
    fn catalogs_iceberg_transient_false_is_ignored() {
        let cats = catalogs_yaml_one("CAT", "WIN", "BUILT_IN", "ICEBERG", &[]);
        let conf = json!({ "catalog_name": "CAT", "transient": false });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert!(!r.is_transient.unwrap());
        }
    }

    // FIXME(versusfacit): this write integration uses an external volume, not Snowflake managed
    // storage, so Snowflake would reject transient here. We currently swallow it and pass
    // anyway instead of erroring. See the FIXME in build_with_catalogs.
    #[test]
    fn catalogs_iceberg_transient_true_with_external_volume_incorrectly_passes() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "BUILT_IN",
            "ICEBERG",
            &[("external_volume", s("ext_vol"))],
        );
        let conf = json!({ "catalog_name": "CAT", "transient": true });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert!(!r.is_transient.unwrap());
            assert_eq!(r.external_volume.as_deref(), Some("ext_vol"));
        }
    }

    //
    // --- Databricks ---
    //

    #[test]
    fn dbx_default_relation_without_catalogs_ok() {
        let conf = json!({});
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();

            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert!(r.adapter_properties.is_empty());
            assert!(r.catalog_name.is_none());
            assert!(r.integration_name.is_none());
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_default_relation_without_catalogs_honors_location_root() {
        let conf = json!({
            "location_root": "s3://bucket/root",
            "database": "db",
            "schema": "sc",
            "alias": "a",
        });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();

            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.external_volume.as_deref(), Some("s3://bucket/root/a"));
        }
    }

    #[test]
    fn dbx_default_relation_without_catalogs_honors_include_full_name_in_path() {
        let conf = json!({
            "location_root": "s3://bucket/root/",
            "include_full_name_in_path": true,
            "database": "db",
            "schema": "sc",
            "alias": "a",
        });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();

            assert_eq!(
                r.external_volume.as_deref(),
                Some("s3://bucket/root/db/sc/a")
            );
        }
    }

    #[test]
    fn dbx_default_relation_without_catalogs_treats_blank_location_root_as_unset() {
        let conf = json!({ "location_root": "   ", "alias": "a" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();

            assert!(r.external_volume.is_none());
        }
    }

    #[test]
    fn dbx_default_relation_with_unselected_catalogs_honors_location_root() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "unity",
            "DEFAULT",
            &[("file_format", s("delta"))],
        );
        let conf = json!({ "location_root": "s3://bucket/root", "alias": "a" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.external_volume.as_deref(), Some("s3://bucket/root/a"));
        }
    }

    #[test]
    fn dbx_iceberg_without_catalogs_returns_managed_default() {
        let conf = json!({ "table_format": "ICEBERG" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();
            assert!(r.catalog_name.is_none());
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn dbx_iceberg_without_catalogs_honors_location_root() {
        let conf = json!({
            "table_format": "ICEBERG",
            "location_root": "s3://bucket/root",
            "database": "db",
            "schema": "sc",
            "alias": "a",
        });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap();

            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.external_volume.as_deref(), Some("s3://bucket/root/a"));
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn dbx_iceberg_with_unselected_catalogs_honors_location_root() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "unity",
            "DEFAULT",
            &[("file_format", s("delta"))],
        );
        let conf = json!({
            "table_format": "ICEBERG",
            "location_root": "s3://bucket/root/",
            "include_full_name_in_path": true,
            "database": "db",
            "schema": "sc",
            "alias": "a",
        });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(
                r.external_volume.as_deref(),
                Some("s3://bucket/root/db/sc/a")
            );
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn dbx_with_catalogs_but_no_catalog_name_defaults_when_not_iceberg() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "unity",
            "DEFAULT",
            &[("file_format", s("delta"))],
        );
        let conf = json!({});
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_iceberg_with_catalog_name_but_no_catalogs_yml_still_errors() {
        let conf = json!({ "table_format": "ICEBERG", "catalog_name": "UC" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Databricks, &m, None)
                    .unwrap_err();
            let msg = format!("{err}");
            assert!(msg.contains("catalog_name"));
            assert!(msg.contains("catalogs.yml"));
        }
    }

    #[test]
    fn dbx_with_catalogs_but_no_catalog_name_iceberg_returns_managed_default() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "unity",
            "ICEBERG",
            &[("file_format", s("delta"))],
        );
        let conf = json!({ "table_format": "ICEBERG" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();
            assert!(r.catalog_name.is_none());
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert!(r.external_volume.is_none());
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn dbx_unity_minimal_iceberg_ok() {
        let cats = catalogs_yaml_one(
            "UC",
            "WIN",
            "unity",
            "ICEBERG",
            &[
                ("file_format", s("delta")),
                (
                    "adapter_properties",
                    map(&[("location_root", s("/Volumes/org/lake"))]),
                ),
            ],
        );
        let conf = json!({ "catalog_name": "UC" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("UC"));
            assert_eq!(r.integration_name.as_deref(), Some("WIN"));
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert_eq!(
                r.adapter_properties
                    .get("location_root")
                    .map(|s| s.as_str()),
                Some("/Volumes/org/lake")
            );
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_unity_location_root_blank_rejected() {
        let cats = catalogs_yaml_one(
            "UC",
            "WIN",
            "unity",
            "ICEBERG",
            &[
                ("file_format", s("delta")),
                ("adapter_properties", map(&[("location_root", s("   "))])),
            ],
        );
        let conf = json!({ "catalog_name": "UC" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            assert!(format!("{err}").contains("location_root cannot be blank"));
        }
    }

    #[test]
    fn dbx_unity_model_can_set_file_format_delta_when_yaml_omits() {
        let cats = catalogs_yaml_one("UC", "WIN", "unity", "ICEBERG", &[]);
        let conf = json!({ "catalog_name": "UC", "file_format": "delta" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert_eq!(r.table_format, TableFormat::Iceberg);
        }
    }

    #[test]
    fn dbx_hms_iceberg_delta_ok_per_adapter_surface() {
        let cats = catalogs_yaml_one(
            "HMS",
            "WIN",
            "hive_metastore",
            "ICEBERG",
            &[("file_format", s("delta"))],
        );
        let conf = json!({ "catalog_name": "HMS" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::HiveMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert!(r.adapter_properties.is_empty());
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_hms_forbids_adapter_properties() {
        let cats = catalogs_yaml_one(
            "HMS",
            "WIN",
            "hive_metastore",
            "ICEBERG",
            &[
                ("file_format", s("delta")),
                (
                    "adapter_properties",
                    map(&[("location_root", s("/mnt/should_not_be_here"))]),
                ),
            ],
        );
        let conf = json!({ "catalog_name": "HMS" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            assert!(format!("{err}").contains("adapter_properties not allowed for hive_metastore"));
        }
    }

    #[test]
    fn dbx_hms_default_hudi_ok() {
        let cats = catalogs_yaml_one(
            "HMS",
            "WIN",
            "hive_metastore",
            "DEFAULT",
            &[("file_format", s("hudi"))],
        );
        let conf = json!({ "catalog_name": "HMS" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::HiveMetastore);
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.file_format.as_deref(), Some("hudi"));
            assert!(r.adapter_properties.is_empty());
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_hms_default_parquet_ok() {
        let cats = catalogs_yaml_one(
            "HMS",
            "WIN",
            "hive_metastore",
            "DEFAULT",
            &[("file_format", s("parquet"))],
        );
        let conf = json!({ "catalog_name": "HMS" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::HiveMetastore);
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.adapter_properties.is_empty());
            assert!(r.is_transient.is_none());
        }
    }

    #[test]
    fn dbx_hms_model_overrides_integration_file_format_ok() {
        let cats = catalogs_yaml_one(
            "HMS",
            "WIN",
            "hive_metastore",
            "DEFAULT",
            &[("file_format", s("delta"))],
        );
        let conf = json!({ "catalog_name": "HMS", "file_format": "parquet" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::HiveMetastore);
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
        }
    }

    #[test]
    fn dbx_unity_does_not_clobber_model_file_format_when_valid_delta() {
        let cats = catalogs_yaml_one(
            "UC",
            "WIN",
            "unity",
            "ICEBERG",
            &[("file_format", s("parquet"))],
        );
        let conf = json!({ "catalog_name": "UC", "file_format": "DELTA" });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
        }
    }

    #[test]
    fn dbx_model_cannot_override_catalog_type_unity_to_hms() {
        let cats = catalogs_yaml_one(
            "UC",
            "WIN",
            "unity",
            "DEFAULT",
            &[("file_format", s("delta"))],
        );
        let conf = json!({
            "catalog_name": "UC",
            "catalog_type": "hive_metastore"
        });

        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Databricks,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            let msg = format!("{err}");
            assert!(msg.contains("catalog_type"));
        }
    }

    //
    // --- Bigquery ---
    //

    #[test]
    fn bigquery_default_relation_without_catalogs_ok() {
        let conf = json!({});
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Bigquery, &m, None)
                    .unwrap();

            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(
                r.file_format,
                Some(BIGQUERY_DEFAULT_FILE_FORMAT.to_string())
            );
            assert_eq!(r.catalog_type, CatalogType::BigqueryNative);
            assert!(r.adapter_properties.is_empty());
            assert!(r.catalog_name.is_none());
            assert!(r.integration_name.is_none());
            assert!(r.is_transient.is_none());
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
        }
    }

    #[test]
    fn bigquery_default_relation_without_catalogs_errors() {
        let conf = json!({"catalog_name": "catalog"});
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err =
                CatalogRelation::from_model_config_and_catalogs(AdapterType::Bigquery, &m, None)
                    .unwrap_err();

            assert!(err.message().contains("Model specifies catalog_name"));
            assert!(err.message().contains("catalogs.yml was not found"));
        }
    }

    #[test]
    fn bigquery_with_catalogs_but_no_catalog_name_defaults_when_not_iceberg() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({});
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(
                r.file_format,
                Some(BIGQUERY_DEFAULT_FILE_FORMAT.to_string())
            );
            assert_eq!(r.catalog_type, CatalogType::BigqueryNative);
            assert!(r.adapter_properties.is_empty());
            assert!(r.catalog_name.is_none());
            assert!(r.integration_name.is_none());
            assert!(r.is_transient.is_none());
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
        }
    }

    #[test]
    fn bigquery_with_catalogs_no_catalog_name_iceberg_format_errors() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({"table_format": "iceberg"});
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            assert!(
                err.message()
                    .contains("table_format=iceberg requires catalogs.yml")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_missing_catalog_name_errors() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({"catalog_name": "missing"});
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            assert!(err.message().contains("not found in catalogs.yml"));
        }
    }

    #[test]
    fn bigquery_with_catalogs_minimal_biglake_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/_dbt/schema_name/identifier_name")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_root_at_model_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("root"))]),
                ),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "base_location_root": "not_root",
            "adapter_properties": {
                "base_location_root": "root"
            }
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/schema_name/identifier_name")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_root_at_model_legacy_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("root"))]),
                ),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "base_location_root": "root",
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/schema_name/identifier_name")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_subpath_at_model_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("root"))]),
                ),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "adapter_properties": {
                "base_location_subpath": "subpath",
            }
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/schema_name/identifier_name/subpath")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_subpath_at_model_legacy_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("root"))]),
                ),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "base_location_subpath": "subpath",
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/schema_name/identifier_name/subpath")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_root_and_subpath_at_model_legacy_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "base_location_root": "root",
            "base_location_subpath": "subpath"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/schema_name/identifier_name/subpath")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_remove_base_root_at_model_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
                (
                    "adapter_properties",
                    map(&[("base_location_root", s("root"))]),
                ),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "adapter_properties": { "base_location_root": "" }
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/_dbt/schema_name/identifier_name")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_external_volume_err() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "external_volume": "gs://other_bucket"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let err = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap_err();

            assert!(err.message().contains(
                "external_volume may only be specified in write integration entries of catalogs.yml"
            ));
        }
    }

    // --- iceberg_version ---

    #[test]
    fn iceberg_version_from_model_config_legacy_path() {
        let conf = json!({
            "table_format": "ICEBERG",
            "external_volume": "EV",
            "schema": "S",
            "identifier": "I",
            "iceberg_version": 3,
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert_eq!(
                r.adapter_properties
                    .get("iceberg_version")
                    .map(|s| s.as_str()),
                Some("3")
            );
        }
    }

    #[test]
    fn iceberg_version_absent_from_model_config_legacy_path() {
        let conf = json!({
            "table_format": "ICEBERG",
            "external_volume": "EV",
            "schema": "S",
            "identifier": "I",
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_without_catalogs_yml(&m).unwrap();
            assert!(!r.adapter_properties.contains_key("iceberg_version"));
        }
    }

    #[test]
    fn iceberg_version_model_config_overrides_catalog_adapter_properties() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "BUILT_IN",
            "ICEBERG",
            &[
                ("external_volume", s("EV")),
                ("adapter_properties", map(&[("iceberg_version", i64v(1))])),
            ],
        );
        let conf = json!({
            "catalog_name": "CAT",
            "schema": "S",
            "identifier": "I",
            "iceberg_version": 3,
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            // model-level iceberg_version=3 overrides catalog adapter_properties iceberg_version=1
            assert_eq!(
                r.adapter_properties
                    .get("iceberg_version")
                    .map(|s| s.as_str()),
                Some("3")
            );
        }
    }

    #[test]
    fn iceberg_version_falls_back_to_catalog_adapter_properties() {
        let cats = catalogs_yaml_one(
            "CAT",
            "WIN",
            "BUILT_IN",
            "ICEBERG",
            &[
                ("external_volume", s("EV")),
                ("adapter_properties", map(&[("iceberg_version", i64v(3))])),
            ],
        );
        let conf = json!({
            "catalog_name": "CAT",
            "schema": "S",
            "identifier": "I",
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::build_with_catalogs(&m, &cats, "CAT").unwrap();
            assert_eq!(
                r.adapter_properties
                    .get("iceberg_version")
                    .map(|s| s.as_str()),
                Some("3")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_storage_uri_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "adapter_properties": {
                "storage_uri": "gs://other_bucket/other/path",
            }
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://other_bucket/other/path")
            );
        }
    }

    #[test]
    fn bigquery_with_catalogs_biglake_override_connection_ok() {
        let cats = catalogs_yaml_one(
            "cat_name",
            "wi_name",
            "biglake_metastore",
            "iceberg",
            &[
                ("file_format", s("parquet")),
                ("external_volume", s("gs://bucket")),
            ],
        );
        let conf = json!({
            "catalog_name": "cat_name",
            "schema": "schema_name",
            "identifier": "identifier_name",
            "adapter_properties": {
                "connection_id": "cool_connection",
            }
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];
        for m in ms {
            let r = CatalogRelation::from_model_config_and_catalogs(
                AdapterType::Bigquery,
                &m,
                Some(Arc::new(DbtCatalogs::new(cats.clone(), Default::default()))),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("cat_name"));
            assert_eq!(r.integration_name.as_deref(), Some("wi_name"));
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert!(r.is_transient.is_none());
            assert_eq!(
                r.adapter_properties
                    .get("connection_id")
                    .map(|s| s.as_str()),
                Some("cool_connection")
            );
        }
    }
}
