use dbt_adapter_core::AdapterType;
use dbt_common::string_utils::try_parse_bool_str;
use dbt_schemas::schemas::dbt_catalogs::{
    CatalogSpecView, CatalogType, DbtCatalogsView, FileFormat, PhysicalFormatResolver, UniformMode,
};
use dbt_schemas::schemas::dbt_catalogs_deprecated::DbtCatalogs;
use dbt_schemas::schemas::relations::base::TableFormat;

use dbt_yaml as yml;
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
use crate::metadata::duckdb::CatalogSpecDuckDbExt;

mod catalog_relation_deprecated;

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
    // safety: the deprecated builders use adapter_properties, the plain ones use catalog_database
    pub fn has_catalog_linked_database(&self) -> bool {
        // deprecated
        self.adapter_properties
            .get("catalog_linked_database")
            .is_some_and(|v| !v.trim().is_empty())
            // plain
            || self.catalog_database.as_deref().is_some_and(|v| !v.trim().is_empty())
                && self.catalog_type.is_catalog_linked()
    }

    pub fn lakehouse_catalog(&self) -> Option<&str> {
        debug_assert_eq!(self.adapter_type, AdapterType::Bigquery);
        self.lakehouse_catalog.as_deref()
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
        let view = catalogs.view().ok()?;
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
        // A bare string is a distinct request shape (Snowflake's "drop a
        // catalog-linked database by name" caller, see `drop.sql`), not a
        // model config at all -- it means something different regardless of
        // whether catalogs.yml is on the deprecated or active schema, so it
        // must be intercepted here, before the deprecated/active branch,
        // rather than inside each per-adapter resolver. Previously this
        // dispatched into the deprecated/active branch first, so the active
        // path (which has no concept of this call shape) received bare
        // strings and hit an unreachable-in-theory `debug_assert!`. See
        // dbt-labs/fs#<TODO: file number>.
        if model.kind() == ValueKind::String {
            return Self::from_linked_database_name(adapter_type, model, catalogs);
        }

        if !load_catalogs::fetch_use_catalogs_v2() || catalogs.is_none() {
            return Self::deprecated_from_model_config_and_catalogs(adapter_type, model, catalogs);
        }

        from_model_config_and_catalogs_default(adapter_type, model, catalogs.unwrap())
    }

    /// Resolves a catalog relation from a bare database name, used only by
    /// Snowflake's `drop_relation` macro (`relations/table/drop.sql`) to
    /// determine whether the database being dropped from is a catalog-linked
    /// database (CLD, e.g. Glue) before deciding how to drop. This is a
    /// distinct request shape from a model config and is handled uniformly
    /// here regardless of the active catalogs.yml version -- CLD-linked-
    /// database detection currently has no active-schema (`DbtCatalogsView`)
    /// implementation, so it always resolves against the deprecated view.
    fn from_linked_database_name(
        adapter_type: AdapterType,
        model: &Value,
        catalogs: Option<Arc<DbtCatalogs>>,
    ) -> AdapterResult<Self> {
        debug_assert_eq!(
            model.kind(),
            ValueKind::String,
            "from_linked_database_name called with a non-string model config"
        );

        if adapter_type != AdapterType::Snowflake {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!(
                    "build_catalog_relation received a bare database name, but catalog-linked \
                     database detection is only supported for Snowflake, not {adapter_type:?}"
                ),
            ));
        }

        let fqn = model.as_str().unwrap_or_default().trim();
        let db_only = fqn.split('.').next().unwrap_or(fqn).trim();

        if let Some(cats) = catalogs.as_ref()
            && Self::cld_exists_in_iceberg_rest(cats.mapping(), db_only)
        {
            Self::build_for_cld_only(model)
        } else {
            Ok(Self::default_catalog_relation_snowflake())
        }
    }

    // ========
    // Bigquery
    // ========

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

    // ==========
    // Databricks
    // ==========

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
            // Lake compute model configs surface the same way DuckDB's do.
            AdapterType::DuckDB | AdapterType::LakeCompute => DUCKDB_ATTR,
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
            // Lake compute model configs surface the same way DuckDB's do.
            AdapterType::DuckDB | AdapterType::LakeCompute => DUCKDB_ATTR,
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

const FIELD_CATALOG_NAME: &str = "catalog_name";
const FIELD_CATALOG: &str = "catalog";
const FIELD_CATALOG_TYPE: &str = "catalog_type";
const FIELD_TABLE_FORMAT: &str = "table_format";
const FIELD_EXTERNAL_VOLUME: &str = "external_volume";
const FIELD_BASE_LOCATION_ROOT: &str = "base_location_root";
const FIELD_BASE_LOCATION_SUBPATH: &str = "base_location_subpath";
const FIELD_TRANSIENT: &str = "transient";
const FIELD_CHANGE_TRACKING: &str = "change_tracking";
const FIELD_DATA_RETENTION_TIME_IN_DAYS: &str = "data_retention_time_in_days";
const FIELD_STORAGE_SERIALIZATION_POLICY: &str = "storage_serialization_policy";
const FIELD_ICEBERG_VERSION: &str = "iceberg_version";
const FIELD_FILE_FORMAT: &str = "file_format";
const FIELD_LOCATION_ROOT: &str = "location_root";
const FIELD_USE_UNIFORM: &str = "use_uniform";
const FIELD_AUTO_REFRESH: &str = "auto_refresh";
const FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS: &str = "max_data_extension_time_in_days";
const FIELD_TARGET_FILE_SIZE: &str = "target_file_size";

// bigquery
const FIELD_STORAGE_URI: &str = "storage_uri";
const FIELD_CONNECTION_ID: &str = "connection_id";

// databricks
const ADAPTER_PROP_LOCATION_ROOT: &str = "location_root";

const FIELD_CATALOG_DATABASE: &str = "catalog_database";
const FIELD_LAKEHOUSE_CATALOG: &str = "lakehouse_catalog";
const ADAPTER_PROP_AUTO_REFRESH: &str = "auto_refresh";
const ADAPTER_PROP_MAX_DATA_EXTENSION_TIME_IN_DAYS: &str = "max_data_extension_time_in_days";
const ADAPTER_PROP_TARGET_FILE_SIZE: &str = "target_file_size";
const ADAPTER_PROP_EXTERNAL_ROOT: &str = "external_root";

// YAML parsing quirk: a model config with a blank `catalog:` key produces the
// string "none" rather than null. Without this sentinel, that would resolve to a
// catalog lookup for a catalog literally named "none" and fail confusingly.
// Catch it early and treat it as absent (no catalog routing).
const MODEL_NONE_SENTINEL: &str = "none";

// Separate fn: lets tests bypass the flag gate directly.
fn from_model_config_and_catalogs_default(
    adapter_type: AdapterType,
    model: &Value,
    catalogs: Arc<DbtCatalogs>,
) -> AdapterResult<CatalogRelation> {
    // V2 relation building assumes a structured model config object. If a bare
    // string still reaches this layer, that is a caller bug rather than a
    // supported v2 input shape.
    debug_assert!(
        model.kind() != ValueKind::String,
        "catalogs.yml v2 received a bare string model config; this is unsupported and indicates a parser bug."
    );

    if CatalogRelation::get_model_adapter_properties(model, adapter_type).is_some() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "catalogs.yml v2 supports top-level model overrides only; model adapter_properties are not supported",
        ));
    }

    let catalog_name = match adapter_type {
        AdapterType::Databricks => {
            let model_catalog_name = model_catalog_name(model, AdapterType::Databricks);
            let wants_iceberg = TableFormat::parse(
                CatalogRelation::get_model_config_value(
                    model,
                    FIELD_TABLE_FORMAT,
                    AdapterType::Databricks,
                )
                .as_deref(),
            )
            .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
            .is_iceberg();

            match model_catalog_name {
                None if !wants_iceberg => {
                    return Ok(
                        CatalogRelation::default_catalog_relation_databricks_for_model(model),
                    );
                }
                None => {
                    let use_uniform =
                        parse_model_bool(model, FIELD_USE_UNIFORM, AdapterType::Databricks)?
                            .unwrap_or(false);
                    let relation = CatalogRelation::default_catalog_relation_databricks()
                        .with_table_format(TableFormat::Iceberg)
                        .with_file_format(if use_uniform {
                            DELTA_TABLE_FORMAT
                        } else {
                            PARQUET_TABLE_FORMAT
                        })
                        .with_adapter_property(ADAPTER_PROP_USE_UNIFORM, use_uniform.to_string());
                    return Ok(relation);
                }
                Some(catalog_name) => catalog_name,
            }
        }
        AdapterType::Snowflake => match model_catalog_name(model, AdapterType::Snowflake) {
            None => return CatalogRelation::build_without_catalogs_yml(model),
            Some(catalog_name) => catalog_name,
        },
        AdapterType::Bigquery => {
            let model_catalog_name = model_catalog_name(model, AdapterType::Bigquery);
            let wants_iceberg = TableFormat::parse(
                CatalogRelation::get_model_config_value(
                    model,
                    FIELD_TABLE_FORMAT,
                    AdapterType::Bigquery,
                )
                .as_deref(),
            )
            .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))?
            .is_iceberg();

            match model_catalog_name {
                None if !wants_iceberg => {
                    return Ok(CatalogRelation::default_catalog_relation_bigquery());
                }
                None => {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "On Bigquery, table_format=iceberg requires catalogs.yml and a `catalog_name` that selects a v2 catalog.",
                    ));
                }
                Some(catalog_name) => catalog_name,
            }
        }
        // Lake compute behaves as DuckDB-backed for relation-building purposes.
        AdapterType::DuckDB | AdapterType::LakeCompute => {
            match model_catalog_name(model, adapter_type) {
                None => return Ok(CatalogRelation::default_catalog_relation_duckdb()),
                Some(catalog_name) => catalog_name,
            }
        }
        _ => Err(AdapterError::new(
            AdapterErrorKind::Internal,
            format!("build_relation_catalog cannot be invoked by an adapter {adapter_type:?}"),
        ))?,
    };

    let spec = parse_catalogs_view(&catalogs)?;
    let catalog = find_catalog_in_view(&spec, &catalog_name)?;

    if CatalogRelation::get_model_config_value(model, FIELD_CATALOG_TYPE, adapter_type).is_some() {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "catalog_type may only be specified in write integration entries of catalogs.yml",
        ));
    }

    match (adapter_type, catalog.catalog_type) {
        (AdapterType::Databricks, CatalogType::Unity) => {
            CatalogRelation::build_databricks_unity_with_catalogs(model, catalog, &catalog_name)
        }
        (AdapterType::Databricks, CatalogType::HiveMetastore) => {
            CatalogRelation::build_databricks_hive_with_catalogs(model, catalog, &catalog_name)
        }
        (AdapterType::Snowflake, CatalogType::Horizon) => {
            CatalogRelation::build_horizon_with_catalogs(model, catalog, &catalog_name)
        }
        (AdapterType::Snowflake, CatalogType::Glue) => {
            CatalogRelation::build_snowflake_linked_with_catalogs(
                model,
                catalog,
                &catalog_name,
                "glue",
            )
        }
        (AdapterType::Snowflake, CatalogType::IcebergRest) => {
            CatalogRelation::build_snowflake_linked_with_catalogs(
                model,
                catalog,
                &catalog_name,
                "iceberg_rest",
            )
        }
        (AdapterType::Snowflake, CatalogType::Unity) => {
            CatalogRelation::build_snowflake_linked_with_catalogs(
                model,
                catalog,
                &catalog_name,
                "unity",
            )
        }
        (AdapterType::Bigquery, CatalogType::BiglakeMetastore) => {
            CatalogRelation::build_bigquery_biglake_with_catalogs(model, catalog, &catalog_name)
        }
        // Horizon/Unity are Iceberg REST under the hood; with duckdb 1.5.4's
        // write-compat ATTACH options they are writable, so models may target
        // them and they build the same relation as a generic Iceberg REST
        // catalog (this lifts the base PR's read-only model-target gate).
        (
            AdapterType::DuckDB,
            CatalogType::IcebergRest | CatalogType::Horizon | CatalogType::Unity,
        ) => CatalogRelation::build_duckdb_with_catalogs(model, catalog, &catalog_name),
        (AdapterType::DuckDB, CatalogType::DuckLake) => {
            CatalogRelation::build_duckdb_ducklake_with_catalogs(model, catalog, &catalog_name)
        }
        (AdapterType::DuckDB, CatalogType::LocalFilesystem) => {
            CatalogRelation::build_duckdb_local_filesystem_with_catalogs(
                model,
                catalog,
                &catalog_name,
            )
        }
        (AdapterType::DuckDB, other) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' has type '{}'; DuckDB v2 mapping supports only 'iceberg_rest', 'ducklake', and 'local_filesystem'",
                other.as_str()
            ),
        )),
        // Unity is readable through Lake Compute, but model writes remain
        // unsupported.
        (AdapterType::LakeCompute, CatalogType::Unity) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' is a Unity catalog accessed via Lake Compute, which is read-only in this release. Models cannot be materialized against it."
            ),
        )),
        (AdapterType::LakeCompute, CatalogType::Horizon | CatalogType::IcebergRest) => {
            CatalogRelation::build_lake_compute_with_catalogs(catalog, &catalog_name)
        }
        (AdapterType::LakeCompute, other) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' has type '{}'; lake compute v2 mapping supports only 'horizon', 'iceberg_rest', and 'unity'",
                other.as_str()
            ),
        )),
        (AdapterType::Databricks, CatalogType::Horizon) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' is a Horizon catalog accessed via Databricks catalog federation, which is read-only. Models cannot be materialized against it."
            ),
        )),
        (AdapterType::Databricks, other) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' has type '{}'; Databricks v2 mapping supports only 'unity' and 'hive_metastore'",
                other.as_str()
            ),
        )),
        (AdapterType::Snowflake, other) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' has type '{}'; Snowflake v2 mapping supports only 'horizon', 'glue', 'iceberg_rest', and 'unity'",
                other.as_str()
            ),
        )),
        (AdapterType::Bigquery, other) => Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            format!(
                "Catalog '{catalog_name}' has type '{}'; Bigquery v2 mapping supports only 'biglake_metastore'",
                other.as_str()
            ),
        )),
        _ => Err(AdapterError::new(
            AdapterErrorKind::Internal,
            format!("build_relation_catalog cannot be invoked by an adapter {adapter_type:?}"),
        )),
    }
}

fn parse_catalogs_view<'a>(catalogs: &'a DbtCatalogs) -> AdapterResult<DbtCatalogsView<'a>> {
    catalogs
        .view()
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, format!("{e}")))
}

fn parse_model_bool(
    model: &Value,
    key: &str,
    adapter_type: AdapterType,
) -> AdapterResult<Option<bool>> {
    let raw = CatalogRelation::get_model_config_value(model, key, adapter_type);
    try_parse_bool_str(raw.as_deref(), key)
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))
}

fn parse_model_u32(
    model: &Value,
    key: &str,
    adapter_type: AdapterType,
) -> AdapterResult<Option<u32>> {
    CatalogRelation::get_model_config_value(model, key, adapter_type)
        .map(|v| {
            v.parse::<u32>().map_err(|_| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Model field '{key}' must be a non-negative integer"),
                )
            })
        })
        .transpose()
}

fn model_catalog_name(model: &Value, adapter_type: AdapterType) -> Option<String> {
    CatalogRelation::get_model_config_value(model, FIELD_CATALOG_NAME, adapter_type)
        .or_else(|| CatalogRelation::get_model_config_value(model, FIELD_CATALOG, adapter_type))
        .and_then(|s| {
            let t = s.trim();
            if t.eq_ignore_ascii_case(MODEL_NONE_SENTINEL) {
                None
            } else {
                Some(t.to_string())
            }
        })
}

fn get_yaml_str<'a>(map: &'a yml::Mapping, key: &str) -> Option<&'a str> {
    map.get(yml::Value::from(key))
        .and_then(|v| v.as_str())
        .map(str::trim)
}

fn get_yaml_bool(map: &yml::Mapping, key: &str) -> Option<bool> {
    map.get(yml::Value::from(key)).and_then(|v| v.as_bool())
}

fn get_yaml_u32(map: &yml::Mapping, key: &str) -> Option<u32> {
    map.get(yml::Value::from(key)).and_then(|v| {
        v.as_i64()
            .and_then(|i| u32::try_from(i).ok())
            .or_else(|| v.as_u64().and_then(|u| u32::try_from(u).ok()))
    })
}

fn is_valid_databricks_file_format(v: &str) -> bool {
    v.eq_ignore_ascii_case("delta")
        || v.eq_ignore_ascii_case("parquet")
        || v.eq_ignore_ascii_case("hudi")
}

fn find_catalog_in_view<'a>(
    spec: &'a DbtCatalogsView<'a>,
    catalog_name: &str,
) -> AdapterResult<&'a CatalogSpecView<'a>> {
    spec.catalogs
        .iter()
        .find(|catalog| catalog.name == catalog_name)
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' not found in catalogs.yml"),
            )
        })
}

fn require_platform_block<'a>(
    catalog: &'a CatalogSpecView<'a>,
    catalog_name: &str,
    platform: &str,
) -> AdapterResult<&'a yml::Mapping> {
    catalog.config_block(platform).ok_or_else(|| {
        AdapterError::new(
            AdapterErrorKind::Configuration,
            format!("Catalog '{catalog_name}' has no configuration for '{platform}'"),
        )
    })
}

fn reject_unsupported_snowflake_linked_model_fields(
    model: &Value,
    type_name: &str,
) -> AdapterResult<()> {
    if CatalogRelation::get_model_config_value(model, FIELD_TRANSIENT, AdapterType::Snowflake)
        .is_some()
    {
        return Err(AdapterError::new(
            AdapterErrorKind::Configuration,
            "transient may not be specified for ICEBERG catalogs. Snowflake built-in catalog DDL does not support transient ICEBERG tables.",
        ));
    }

    for field in [
        FIELD_EXTERNAL_VOLUME,
        FIELD_BASE_LOCATION_ROOT,
        FIELD_BASE_LOCATION_SUBPATH,
        FIELD_CHANGE_TRACKING,
        FIELD_DATA_RETENTION_TIME_IN_DAYS,
        FIELD_STORAGE_SERIALIZATION_POLICY,
    ] {
        if CatalogRelation::get_model_config_value(model, field, AdapterType::Snowflake).is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Snowflake v2 {type_name} does not support model field '{field}' yet."),
            ));
        }
    }

    Ok(())
}

fn reject_unsupported_databricks_hive_model_fields(model: &Value) -> AdapterResult<()> {
    for field in [FIELD_LOCATION_ROOT, FIELD_USE_UNIFORM] {
        if CatalogRelation::get_model_config_value(model, field, AdapterType::Databricks).is_some()
        {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Databricks v2 hive_metastore does not support model field '{field}'."),
            ));
        }
    }

    Ok(())
}

impl CatalogRelation {
    fn build_databricks_unity_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let databricks = require_platform_block(catalog, catalog_name, "databricks")?;

        let table_format = catalog.table_format;

        let mut file_format =
            Self::get_model_config_value(model, FIELD_FILE_FORMAT, AdapterType::Databricks)
                .or_else(|| get_yaml_str(databricks, FIELD_FILE_FORMAT).map(|s| s.to_string()))
                .unwrap_or_else(|| DELTA_TABLE_FORMAT.to_string());
        file_format.make_ascii_lowercase();

        let location_root =
            Self::get_model_config_value(model, FIELD_LOCATION_ROOT, AdapterType::Databricks)
                .or_else(|| get_yaml_str(databricks, FIELD_LOCATION_ROOT).map(|s| s.to_string()));

        let mut external_volume = None;
        let mut adapter_properties = BTreeMap::new();

        let catalog_database =
            get_yaml_str(databricks, FIELD_CATALOG_DATABASE).map(|s| s.to_string());

        let use_uniform = UniformMode::from_bool(
            parse_model_bool(model, FIELD_USE_UNIFORM, AdapterType::Databricks)?
                .or_else(|| get_yaml_bool(databricks, FIELD_USE_UNIFORM))
                .unwrap_or(false),
        );

        if let Some(location_root) = location_root {
            if location_root.trim().is_empty() {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "Databricks v2 location_root cannot be blank or whitespace",
                ));
            }
            external_volume = Self::dbx_build_external_volume_for_location(model, &location_root);
            adapter_properties.insert(ADAPTER_PROP_LOCATION_ROOT.to_string(), location_root);
        }

        let file_format_enum = FileFormat::parse(&file_format, None)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, format!("{e}")))?;

        match (file_format_enum, use_uniform) {
            (FileFormat::Delta, UniformMode::Enabled)
            | (FileFormat::Parquet, UniformMode::Disabled) => Ok(()),
            (FileFormat::Delta, UniformMode::Disabled) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks v2 unity use_uniform: false (or unset) requires file_format: parquet",
            )),
            (FileFormat::Parquet, UniformMode::Enabled) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks v2 unity use_uniform: true requires file_format: delta",
            )),
            (FileFormat::Hudi, _) => Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks v2 unity does not support file_format 'hudi' (use delta or parquet)",
            )),
        }?;

        adapter_properties.insert(
            ADAPTER_PROP_USE_UNIFORM.to_string(),
            use_uniform.is_enabled().to_string(),
        );
        Ok(CatalogRelation {
            adapter_type: AdapterType::Databricks,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: CatalogType::Unity,
            table_format,
            file_format: Some(file_format),
            external_volume,
            catalog_database,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    fn build_databricks_hive_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        reject_unsupported_databricks_hive_model_fields(model)?;

        let databricks = require_platform_block(catalog, catalog_name, "databricks")?;

        let mut file_format =
            Self::get_model_config_value(model, FIELD_FILE_FORMAT, AdapterType::Databricks)
                .or_else(|| get_yaml_str(databricks, FIELD_FILE_FORMAT).map(|s| s.to_string()))
                .unwrap_or_else(|| DELTA_TABLE_FORMAT.to_string());
        file_format.make_ascii_lowercase();
        if !is_valid_databricks_file_format(&file_format) {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Databricks v2 hive_metastore file_format must be one of (delta|parquet|hudi)",
            ));
        }

        let adapter_properties = BTreeMap::new();

        Ok(CatalogRelation {
            adapter_type: AdapterType::Databricks,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: CatalogType::HiveMetastore,
            table_format: catalog.table_format,
            file_format: Some(file_format),
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    fn build_snowflake_linked_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
        type_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        reject_unsupported_snowflake_linked_model_fields(model, type_name)?;

        let snowflake = require_platform_block(catalog, catalog_name, "snowflake")?;

        let table_format = catalog.table_format;

        let mut adapter_properties = BTreeMap::new();
        let catalog_database = get_yaml_str(snowflake, FIELD_CATALOG_DATABASE)
            .map(|db| db.to_string())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!(
                        "Catalog '{catalog_name}' {type_name}/snowflake requires catalog_database"
                    ),
                )
            })?;

        let auto_refresh = parse_model_bool(model, FIELD_AUTO_REFRESH, AdapterType::Snowflake)?
            .or_else(|| get_yaml_bool(snowflake, FIELD_AUTO_REFRESH));
        if let Some(auto_refresh) = auto_refresh {
            adapter_properties.insert(
                ADAPTER_PROP_AUTO_REFRESH.to_string(),
                auto_refresh.to_string(),
            );
        }

        let max_data_extension_time_in_days = parse_model_u32(
            model,
            FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS,
            AdapterType::Snowflake,
        )?
        .or_else(|| get_yaml_u32(snowflake, FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS));
        if let Some(max_data_extension_time_in_days) = max_data_extension_time_in_days {
            adapter_properties.insert(
                ADAPTER_PROP_MAX_DATA_EXTENSION_TIME_IN_DAYS.to_string(),
                max_data_extension_time_in_days.to_string(),
            );
        }

        let target_file_size =
            Self::get_model_config_value(model, FIELD_TARGET_FILE_SIZE, AdapterType::Snowflake)
                .or_else(|| get_yaml_str(snowflake, FIELD_TARGET_FILE_SIZE).map(|s| s.to_string()));
        if let Some(target_file_size) = target_file_size {
            adapter_properties.insert(ADAPTER_PROP_TARGET_FILE_SIZE.to_string(), target_file_size);
        }

        let iceberg_version =
            Self::get_model_config_value(model, FIELD_ICEBERG_VERSION, AdapterType::Snowflake)
                .or_else(|| get_yaml_str(snowflake, FIELD_ICEBERG_VERSION).map(|s| s.to_string()));
        if let Some(iceberg_version) = iceberg_version {
            adapter_properties.insert(FIELD_ICEBERG_VERSION.to_string(), iceberg_version);
        }

        Ok(CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: CatalogType::IcebergRest,
            table_format,
            external_volume: None,
            catalog_database: Some(catalog_database),
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: Some(false),
            file_format: None,
        })
    }

    fn build_horizon_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        // FIXME(versusfacit): we just swallow transient here for now instead of
        // honoring it. Snowflake actually supports transient iceberg tables when the
        // location is Snowflake managed storage aka SNOWFLAKE_MANAGED. We need to
        // detect that case and stop dropping the value on the floor. See
        // dbt-labs/dbt-core#15427 and
        // https://docs.snowflake.com/en/user-guide/tables-iceberg-internal-storage

        let snowflake = require_platform_block(catalog, catalog_name, "snowflake")?;

        let external_volume =
            Self::get_model_config_value(model, FIELD_EXTERNAL_VOLUME, AdapterType::Snowflake)
                .or_else(|| get_yaml_str(snowflake, FIELD_EXTERNAL_VOLUME).map(|s| s.to_string()));
        // catalogs.yml schema validation requires `external_volume` on every
        // Horizon+Snowflake platform block, so this default never actually
        // triggers for a valid config -- kept only so `external_volume` stays
        // `Option<String>` consistently with the legacy (no catalogs.yml) path.
        let external_volume =
            Some(external_volume.unwrap_or_else(|| SNOWFLAKE_MANAGED_EXTERNAL_VOLUME.to_string()));

        let base_location_root =
            Self::get_model_config_value(model, FIELD_BASE_LOCATION_ROOT, AdapterType::Snowflake)
                .or_else(|| {
                    get_yaml_str(snowflake, FIELD_BASE_LOCATION_ROOT).map(|s| s.to_string())
                });
        let base_location_subpath = Self::get_model_config_value(
            model,
            FIELD_BASE_LOCATION_SUBPATH,
            AdapterType::Snowflake,
        );

        let schema = Self::get_model_config_value(model, "schema", AdapterType::Snowflake);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Snowflake)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Snowflake));
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

        let catalog_database =
            get_yaml_str(snowflake, FIELD_CATALOG_DATABASE).map(|s| s.to_string());

        let change_tracking =
            parse_model_bool(model, FIELD_CHANGE_TRACKING, AdapterType::Snowflake)?
                .or_else(|| get_yaml_bool(snowflake, FIELD_CHANGE_TRACKING));
        if let Some(change_tracking) = change_tracking {
            adapter_properties.insert(
                FIELD_CHANGE_TRACKING.to_string(),
                change_tracking.to_string(),
            );
        }

        let data_retention_time_in_days = parse_model_u32(
            model,
            FIELD_DATA_RETENTION_TIME_IN_DAYS,
            AdapterType::Snowflake,
        )?
        .or_else(|| get_yaml_u32(snowflake, FIELD_DATA_RETENTION_TIME_IN_DAYS));
        if let Some(data_retention_time_in_days) = data_retention_time_in_days {
            adapter_properties.insert(
                FIELD_DATA_RETENTION_TIME_IN_DAYS.to_string(),
                data_retention_time_in_days.to_string(),
            );
        }

        let max_data_extension_time_in_days = parse_model_u32(
            model,
            FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS,
            AdapterType::Snowflake,
        )?
        .or_else(|| get_yaml_u32(snowflake, FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS));
        if let Some(max_data_extension_time_in_days) = max_data_extension_time_in_days {
            adapter_properties.insert(
                FIELD_MAX_DATA_EXTENSION_TIME_IN_DAYS.to_string(),
                max_data_extension_time_in_days.to_string(),
            );
        }

        let storage_serialization_policy = Self::get_model_config_value(
            model,
            FIELD_STORAGE_SERIALIZATION_POLICY,
            AdapterType::Snowflake,
        )
        .or_else(|| {
            get_yaml_str(snowflake, FIELD_STORAGE_SERIALIZATION_POLICY).map(|s| s.to_string())
        });
        if let Some(storage_serialization_policy) = storage_serialization_policy {
            adapter_properties.insert(
                FIELD_STORAGE_SERIALIZATION_POLICY.to_string(),
                storage_serialization_policy,
            );
        }

        let iceberg_version =
            Self::get_model_config_value(model, FIELD_ICEBERG_VERSION, AdapterType::Snowflake)
                .or_else(|| get_yaml_str(snowflake, FIELD_ICEBERG_VERSION).map(|s| s.to_string()));
        if let Some(iceberg_version) = iceberg_version {
            adapter_properties.insert(FIELD_ICEBERG_VERSION.to_string(), iceberg_version);
        }

        Ok(CatalogRelation {
            adapter_type: AdapterType::Snowflake,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: CatalogType::SnowflakeBuiltIn,
            table_format: catalog.table_format,
            external_volume,
            catalog_database,
            lakehouse_catalog: None,
            base_location,
            adapter_properties,
            is_transient: Some(false),
            file_format: None,
        })
    }

    fn build_bigquery_biglake_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let bigquery = require_platform_block(catalog, catalog_name, "bigquery")?;

        let external_volume = Self::get_model_config_value(
            model,
            FIELD_EXTERNAL_VOLUME,
            AdapterType::Bigquery,
        )
        .or_else(|| get_yaml_str(bigquery, FIELD_EXTERNAL_VOLUME).map(|s| s.to_string()))
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Catalog '{catalog_name}' biglake_metastore/bigquery requires external_volume"
                ),
            )
        })?;
        if external_volume.trim().is_empty() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Catalog '{catalog_name}' biglake_metastore/bigquery external_volume cannot be blank"
                ),
            ));
        }
        if !external_volume.starts_with("gs://") {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Catalog '{catalog_name}' biglake_metastore/bigquery external_volume must start with gs://"
                ),
            ));
        }

        let mut file_format = Self::get_model_config_value(
            model,
            FIELD_FILE_FORMAT,
            AdapterType::Bigquery,
        )
        .or_else(|| get_yaml_str(bigquery, FIELD_FILE_FORMAT).map(|s| s.to_string()))
        .ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' biglake_metastore/bigquery requires file_format"),
            )
        })?;
        file_format.make_ascii_lowercase();
        if !file_format.eq_ignore_ascii_case("parquet") {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "Catalog '{catalog_name}' biglake_metastore/bigquery file_format must be parquet"
                ),
            ));
        }

        let base_location_root =
            Self::get_model_config_value(model, FIELD_BASE_LOCATION_ROOT, AdapterType::Bigquery)
                .or_else(|| {
                    get_yaml_str(bigquery, FIELD_BASE_LOCATION_ROOT).map(|s| s.to_string())
                });
        if let Some(base_location_root) = base_location_root.as_deref()
            && base_location_root.trim().is_empty()
        {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Bigquery v2 base_location_root cannot be blank or whitespace",
            ));
        }

        let base_location_subpath =
            Self::get_model_config_value(model, FIELD_BASE_LOCATION_SUBPATH, AdapterType::Bigquery);
        let schema = Self::get_model_config_value(model, "schema", AdapterType::Bigquery);
        let identifier = Self::get_model_config_value(model, "alias", AdapterType::Bigquery)
            .or_else(|| Self::get_model_config_value(model, "identifier", AdapterType::Bigquery));
        let base_location = Self::build_base_location(
            &base_location_root,
            &base_location_subpath,
            &schema,
            &identifier,
        );

        let connection_id =
            Self::get_model_config_value(model, FIELD_CONNECTION_ID, AdapterType::Bigquery)
                .or_else(|| get_yaml_str(bigquery, FIELD_CONNECTION_ID).map(|s| s.to_string()));

        let storage_uri =
            Self::get_model_config_value(model, FIELD_STORAGE_URI, AdapterType::Bigquery)
                .unwrap_or_else(|| format!("{external_volume}/{base_location}"));

        let catalog_database =
            get_yaml_str(bigquery, FIELD_CATALOG_DATABASE).map(|s| s.to_string());
        let lakehouse_catalog =
            get_yaml_str(bigquery, FIELD_LAKEHOUSE_CATALOG).map(|s| s.to_string());

        let mut adapter_properties = BTreeMap::new();

        if let Some(connection_id) = connection_id {
            adapter_properties.insert(FIELD_CONNECTION_ID.to_string(), connection_id);
        }
        adapter_properties.insert(FIELD_STORAGE_URI.to_string(), storage_uri);

        Ok(CatalogRelation {
            adapter_type: AdapterType::Bigquery,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: CatalogType::BiglakeMetastore,
            table_format: catalog.table_format,
            adapter_properties,
            is_transient: None,
            external_volume: None,
            catalog_database,
            lakehouse_catalog,
            base_location: None,
            file_format: Some(file_format),
        })
    }

    /// Lake compute equivalent of `build_horizon_with_catalogs` /
    /// `build_snowflake_linked_with_catalogs`, pared down to the single
    /// field Lake Compute uses for Horizon and Iceberg REST:
    /// `catalog_database` from `config.snowflake`.
    fn build_lake_compute_with_catalogs(
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let snowflake = require_platform_block(catalog, catalog_name, "snowflake")?;

        let catalog_database = get_yaml_str(snowflake, FIELD_CATALOG_DATABASE)
            .map(|s| s.to_string())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!(
                        "Catalog '{catalog_name}' requires config.snowflake.catalog_database for the lake compute adapter"
                    ),
                )
            })?;

        Ok(CatalogRelation {
            adapter_type: AdapterType::LakeCompute,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: catalog.catalog_type,
            table_format: catalog.table_format,
            file_format: None,
            external_volume: None,
            catalog_database: Some(catalog_database),
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties: BTreeMap::new(),
            is_transient: None,
        })
    }

    fn build_duckdb_with_catalogs(
        _model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let duckdb = require_platform_block(catalog, catalog_name, "duckdb")?;

        let table_format = catalog.table_format;

        let endpoint = get_yaml_str(duckdb, "endpoint").map(|s| s.to_string());
        let warehouse = get_yaml_str(duckdb, "warehouse").map(|s| s.to_string());
        let secret = get_yaml_str(duckdb, "secret").map(|s| s.to_string());
        let alias = catalog.resolved_attach_alias().unwrap_or_default();

        let Some(endpoint) = endpoint else {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!("Catalog '{catalog_name}' duckdb config requires 'endpoint'"),
            ));
        };

        let mut adapter_properties = BTreeMap::new();
        adapter_properties.insert("endpoint".to_string(), endpoint);
        if let Some(warehouse) = warehouse {
            adapter_properties.insert("warehouse".to_string(), warehouse);
        }
        if let Some(ref secret) = secret {
            adapter_properties.insert("secret".to_string(), secret.clone());
        }
        adapter_properties.insert("attached_database".to_string(), alias);
        // stage_create_tables steers the write strategy (CTAS opt-in), so it
        // rides on the relation. Same string-bool coercion as the ATTACH
        // composer so a YAML `"true"` cannot diverge between the two readers.
        if let Some(stage_create_tables) =
            dbt_common::serde_utils::try_get_bool(duckdb, "stage_create_tables")
                .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, format!("{e}")))?
        {
            adapter_properties.insert(
                "stage_create_tables".to_string(),
                stage_create_tables.to_string(),
            );
        }

        Ok(CatalogRelation {
            adapter_type: AdapterType::DuckDB,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: catalog.catalog_type,
            table_format,
            file_format: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    fn build_duckdb_ducklake_with_catalogs(
        _model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let duckdb = require_platform_block(catalog, catalog_name, "duckdb")?;
        let table_format = catalog.table_format;

        let metadata_path = get_yaml_str(duckdb, "metadata_path")
            .map(|s| s.to_string())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' duckdb config requires 'metadata_path'"),
                )
            })?;

        let alias = catalog.resolved_attach_alias().unwrap_or_default();

        let mut adapter_properties = BTreeMap::new();
        adapter_properties.insert("metadata_path".to_string(), metadata_path);
        if let Some(dp) = get_yaml_str(duckdb, "data_path") {
            adapter_properties.insert("data_path".to_string(), dp.to_string());
        }
        // Route models to the DuckLake ATTACH alias (the attached database),
        // mirroring the Iceberg REST path. `duckdb__generate_database_name`
        // reads `attached_database` to override `target.database`; without it,
        // `+catalog_name: <ducklake>` models silently land in the built-in
        // profile database instead of the DuckLake catalog.
        adapter_properties.insert("attached_database".to_string(), alias);

        Ok(CatalogRelation {
            adapter_type: AdapterType::DuckDB,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: catalog.catalog_type,
            table_format,
            file_format: None,
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }

    fn build_duckdb_local_filesystem_with_catalogs(
        model: &Value,
        catalog: &CatalogSpecView<'_>,
        catalog_name: &str,
    ) -> AdapterResult<CatalogRelation> {
        let duckdb = require_platform_block(catalog, catalog_name, "duckdb")?;
        let table_format = catalog.table_format;

        let root_path = get_yaml_str(duckdb, "root_path")
            .map(|s| s.to_string())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Catalog '{catalog_name}' duckdb config requires 'root_path'"),
                )
            })?;

        let mut file_format =
            Self::get_model_config_value(model, FIELD_FILE_FORMAT, AdapterType::DuckDB)
                .or_else(|| get_yaml_str(duckdb, FIELD_FILE_FORMAT).map(|s| s.to_string()))
                .unwrap_or_else(|| "parquet".to_string());
        file_format.make_ascii_lowercase();

        let mut adapter_properties = BTreeMap::new();
        adapter_properties.insert(ADAPTER_PROP_EXTERNAL_ROOT.to_string(), root_path);

        Ok(CatalogRelation {
            adapter_type: AdapterType::DuckDB,
            catalog_name: Some(catalog_name.to_string()),
            integration_name: None,
            catalog_type: catalog.catalog_type,
            table_format,
            file_format: Some(file_format),
            external_volume: None,
            catalog_database: None,
            lakehouse_catalog: None,
            base_location: None,
            adapter_properties,
            is_transient: None,
        })
    }
}

#[cfg(test)]
mod default_relation_tests {
    use super::*;
    use minijinja::Value as JVal;
    use serde_json::json;
    use std::path::Path;

    fn adapter_type_to_attr(adapter_type: AdapterType) -> String {
        match adapter_type {
            AdapterType::Snowflake => "snowflake_attr".to_string(),
            AdapterType::Bigquery => "bigquery_attr".to_string(),
            AdapterType::Databricks => "databricks_attr".to_string(),
            AdapterType::DuckDB | AdapterType::LakeCompute => "duckdb_attr".to_string(),
            _ => panic!("Not yet supported"),
        }
    }

    fn model(adapter_type: AdapterType, config: serde_json::Value) -> JVal {
        let mut model_map = serde_json::Map::new();
        let mut config_map = serde_json::Map::new();
        const TOP_LEVEL_KEYS: [&str; 12] = [
            "catalog_name",
            "schema",
            "identifier",
            "database",
            "alias",
            "file_format",
            "location_root",
            "use_uniform",
            "external_volume",
            "auto_refresh",
            "max_data_extension_time_in_days",
            "target_file_size",
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
        if let serde_json::Value::Object(config) = config {
            model_map.insert(
                "config".to_owned(),
                serde_json::Value::Object(config.clone()),
            );
            for (key, value) in config {
                model_map.insert(key, value);
            }
            JVal::from_serialize(serde_json::Value::Object(model_map))
        } else {
            panic!("Config is not a JSON object");
        }
    }

    fn load_catalogs_yaml(yaml: &str) -> DbtCatalogs {
        use dbt_schemas::schemas::dbt_catalogs::validate_catalogs;
        let parsed: dbt_yaml::Value = dbt_yaml::from_str(yaml).expect("valid YAML");
        let (repr, span) = match parsed {
            dbt_yaml::Value::Mapping(m, s) => (m, s),
            _ => panic!("expected top-level mapping"),
        };
        let catalogs = DbtCatalogs::new(repr, span);
        let view = catalogs.view().expect("valid v2 view");
        validate_catalogs(&view, Path::new("<test>")).expect("valid v2 catalogs");
        catalogs
    }

    #[test]
    fn databricks_unity_catalog_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let conf = json!({ "catalog_name": "UC" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("UC"));
            assert!(r.integration_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
        }
    }

    #[test]
    fn lake_compute_unity_model_target_is_read_only() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: UC_DB
"#,
        );
        let error = from_model_config_and_catalogs_default(
            AdapterType::LakeCompute,
            &model(AdapterType::LakeCompute, json!({ "catalog_name": "UC" })),
            Arc::new(catalogs),
        )
        .unwrap_err();
        assert!(error.to_string().contains("Unity"));
        assert!(error.to_string().contains("read-only"));
    }

    #[test]
    fn databricks_iceberg_without_catalog_name_defaults_to_managed_iceberg() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UNRELATED
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let conf = json!({ "table_format": "iceberg" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert!(r.catalog_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn databricks_iceberg_use_uniform_false_without_catalog_name_succeeds() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UNRELATED
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let conf = json!({ "table_format": "iceberg", "use_uniform": false });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert!(r.catalog_name.is_none());
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("false")
            );
        }
    }

    #[test]
    fn databricks_iceberg_explicit_use_uniform_true_without_catalog_name_succeeds() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UNRELATED
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let conf = json!({ "table_format": "iceberg", "use_uniform": true });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert!(r.catalog_name.is_none());
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("delta"));
            assert_eq!(
                r.adapter_properties.get("use_uniform").map(|s| s.as_str()),
                Some("true")
            );
        }
    }

    #[test]
    fn databricks_unity_catalog_builds_relation_parquet_managed_iceberg() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        catalog_database: "MAIN"
        file_format: parquet
"#,
        );
        let conf = json!({ "catalog_name": "UC" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
        }
    }

    #[test]
    fn databricks_unity_catalog_database() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        catalog_database: "MAIN"
        file_format: parquet
"#,
        );
        let conf = json!({ "catalog_name": "UC" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_database.as_deref(), Some("MAIN"));
        }
    }

    #[test]
    fn snowflake_horizon_catalog_database() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: SF_HORIZON
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_volume
        catalog_database: "PROD_DB"
"#,
        );
        let conf = json!({ "catalog_name": "SF_HORIZON", "schema": "S", "identifier": "I" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_database.as_deref(), Some("PROD_DB"));
        }
    }

    #[test]
    fn databricks_no_catalog_name_default_honors_location_root() {
        let catalogs = load_catalogs_yaml("catalogs: []\n");
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
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert!(r.catalog_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::Unity);
            assert_eq!(r.external_volume.as_deref(), Some("s3://bucket/root/a"));
        }
    }

    #[test]
    fn snowflake_horizon_iceberg_with_external_volume_synthesizes_base_location() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: SF_HORIZON
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_volume
        base_location_root: _root
"#,
        );
        let conf = json!({
            "catalog_name": "SF_HORIZON",
            "schema": "SCH",
            "identifier": "ID",
            "base_location_subpath": "sub",
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name, Some("SF_HORIZON".to_string()));
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            assert_eq!(r.external_volume.as_deref(), Some("my_volume"));
            assert_eq!(r.base_location.as_deref(), Some("_root/SCH/ID/sub"));
        }
    }

    #[test]
    fn snowflake_horizon_iceberg_explicit_snowflake_managed_external_volume_omits_base_location() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: SF_HORIZON
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: SNOWFLAKE_MANAGED
        base_location_root: _root
"#,
        );
        let conf = json!({
            "catalog_name": "SF_HORIZON",
            "schema": "SCH",
            "identifier": "ID",
            "base_location_subpath": "sub",
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.external_volume.as_deref(), Some("SNOWFLAKE_MANAGED"));
            assert!(r.base_location.is_none());
        }
    }

    #[test]
    fn snowflake_horizon_iceberg_without_catalog_omits_external_volume_and_base_location() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UNRELATED
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_volume
"#,
        );
        let conf = json!({ "table_format": "ICEBERG", "schema": "SCH", "identifier": "ID" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert!(r.catalog_name.is_none());
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_type, CatalogType::SnowflakeBuiltIn);
            // No `+catalog_name`, so this dispatches to `build_without_catalogs_yml`
            // (the legacy path) -- no `external_volume` configured there either,
            // so it stays unset rather than templating the literal (bogus)
            // `SNOWFLAKE_MANAGED` string into the DDL.
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
        }
    }

    #[test]
    fn bigquery_biglake_catalog_database() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: BQ
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: gs://bucket
        file_format: parquet
        base_location_root: root
        catalog_database: "analytics-project"
"#,
        );
        let conf = json!({
            "catalog_name": "BQ",
            "schema": "analytics",
            "alias": "events"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Bigquery,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_database.as_deref(), Some("analytics-project"));
        }
    }

    #[test]
    fn bigquery_biglake_lakehouse_catalog() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: BQ
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: gs://bucket
        file_format: parquet
        base_location_root: root
        lakehouse_catalog: "sales_catalog"
"#,
        );
        let conf = json!({
            "catalog_name": "BQ",
            "schema": "analytics",
            "alias": "events"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Bigquery,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.lakehouse_catalog.as_deref(), Some("sales_catalog"));
            assert!(r.catalog_database.is_none());
        }
    }

    #[test]
    fn databricks_unity_model_override_rejects_invalid_combo() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: parquet
"#,
        );
        let conf = json!({
            "catalog_name": "UC",
            "file_format": "parquet",
            "use_uniform": true,
        });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let err = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap_err();

            assert!(
                format!("{err}")
                    .contains("Databricks v2 unity use_uniform: true requires file_format: delta")
            );
        }
    }

    #[test]
    fn databricks_unity_model_override_rejects_delta_without_use_uniform() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let conf = json!({ "catalog_name": "UC", "use_uniform": false });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let err = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap_err();

            assert!(format!("{err}").contains(
                "Databricks v2 unity use_uniform: false (or unset) requires file_format: parquet"
            ));
        }
    }

    #[test]
    fn databricks_hive_metastore_allows_hudi() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: HMS
    type: hive_metastore
    table_format: default
    config:
      databricks:
        file_format: hudi
"#,
        );
        let conf = json!({ "catalog_name": "HMS" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("HMS"));
            assert_eq!(r.catalog_type, CatalogType::HiveMetastore);
            assert_eq!(r.table_format, TableFormat::Default);
            assert_eq!(r.file_format.as_deref(), Some("hudi"));
        }
    }

    #[test]
    fn rejects_model_adapter_properties() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: true
"#,
        );
        let model = JVal::from_serialize(json!({
            "catalog_name": "UC",
            "databricks_attr": {
                "adapter_properties": {
                    "location_root": "s3://bucket/path"
                }
            }
        }));

        let err = from_model_config_and_catalogs_default(
            AdapterType::Databricks,
            &model,
            Arc::new(catalogs),
        )
        .unwrap_err();

        assert!(
            format!("{err}").contains("catalogs.yml v2 supports top-level model overrides only")
        );
    }

    #[test]
    fn bigquery_biglake_catalog_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: BQ
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: gs://bucket
        file_format: parquet
        base_location_root: root
"#,
        );
        let conf = json!({
            "catalog_name": "BQ",
            "schema": "analytics",
            "alias": "events"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Bigquery,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("BQ"));
            assert!(r.integration_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://bucket/root/analytics/events")
            );
        }
    }

    #[test]
    fn bigquery_biglake_model_values_override_yaml_values() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: BQ
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: gs://bucket
        file_format: parquet
        base_location_root: root
"#,
        );
        let conf = json!({
            "catalog_name": "BQ",
            "external_volume": "gs://other-bucket",
            "base_location_root": "override",
            "base_location_subpath": "leaf",
            "schema": "analytics",
            "alias": "events"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Bigquery,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert_eq!(
                r.adapter_properties.get("storage_uri").map(|s| s.as_str()),
                Some("gs://other-bucket/override/analytics/events/leaf")
            );
        }
    }

    #[test]
    fn bigquery_biglake_catalog_connection_id_ok() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: BQ
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: gs://bucket
        file_format: parquet
        connection_id: cool_connection
"#,
        );
        let conf = json!({
            "catalog_name": "BQ",
            "schema": "analytics",
            "alias": "events"
        });
        let ms = [
            model(AdapterType::Bigquery, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Bigquery,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("BQ"));
            assert!(r.integration_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::BiglakeMetastore);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.file_format.as_deref(), Some("parquet"));
            assert!(r.external_volume.is_none());
            assert!(r.base_location.is_none());
            assert_eq!(
                r.adapter_properties
                    .get("connection_id")
                    .map(|s| s.as_str()),
                Some("cool_connection")
            );
        }
    }

    #[test]
    fn snowflake_unity_catalog_builds_cld_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_CLD"
        auto_refresh: true
"#,
        );
        let conf = json!({ "catalog_name": "UC", "schema": "S", "identifier": "I" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_name.as_deref(), Some("UC"));
            assert!(r.integration_name.is_none());
            assert_eq!(r.catalog_type, CatalogType::IcebergRest);
            assert_eq!(r.table_format, TableFormat::Iceberg);
            assert_eq!(r.catalog_database.as_deref(), Some("MY_CLD"));
        }
    }

    #[test]
    fn snowflake_unity_uses_yaml_catalog_database_over_model_database() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "YAML_DB"
        auto_refresh: false
        max_data_extension_time_in_days: 30
        target_file_size: "16MB"
"#,
        );
        let conf = json!({
            "catalog_name": "UC",
            "database": "MODEL_DB",
            "auto_refresh": true,
            "max_data_extension_time_in_days": 7,
            "target_file_size": "32MB"
        });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let r = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap();

            assert_eq!(r.catalog_database.as_deref(), Some("YAML_DB"));
            assert_eq!(
                r.adapter_properties.get("auto_refresh").map(|s| s.as_str()),
                Some("true")
            );
            assert_eq!(
                r.adapter_properties
                    .get("max_data_extension_time_in_days")
                    .map(|s| s.as_str()),
                Some("7")
            );
            assert_eq!(
                r.adapter_properties
                    .get("target_file_size")
                    .map(|s| s.as_str()),
                Some("32MB")
            );
        }
    }

    #[test]
    fn databricks_rejects_horizon_catalog_materialization() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: SF_HORIZON
    type: horizon
    table_format: iceberg
    config:
      databricks:
        catalog_database: "MY_FOREIGN_CATALOG"
"#,
        );
        let conf = json!({ "catalog_name": "SF_HORIZON" });
        let ms = [
            model(AdapterType::Databricks, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let err = from_model_config_and_catalogs_default(
                AdapterType::Databricks,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap_err();
            assert!(
                format!("{err}").contains("read-only"),
                "unexpected error: {err}"
            );
        }
    }

    #[test]
    fn snowflake_unity_rejects_stubbed_model_fields() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: UC
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_CLD"
"#,
        );
        let conf = json!({ "catalog_name": "UC", "external_volume": "EV" });
        let ms = [
            model(AdapterType::Snowflake, conf.clone()),
            model_deprecated_config(conf),
        ];

        for m in ms {
            let err = from_model_config_and_catalogs_default(
                AdapterType::Snowflake,
                &m,
                Arc::new(catalogs.clone()),
            )
            .unwrap_err();
            assert!(format!("{err}").contains(
                "Snowflake v2 unity does not support model field 'external_volume' yet."
            ));
        }
    }

    // ===== DuckDB v2 tests =====

    #[test]
    fn duckdb_no_catalog_returns_default() {
        // DuckDB with no catalog_name should return a default relation
        // We need *some* catalogs.yml for the v2 path, but DuckDB with no
        // catalog_name should early-exit before looking it up.
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_rest
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://rest.example.com"
"#,
        );
        let conf = json!({});
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert!(r.catalog_name.is_none());
        assert!(r.integration_name.is_none());
        assert_eq!(r.catalog_type, CatalogType::DuckdbNative);
        assert_eq!(r.table_format, TableFormat::Default);
        assert!(r.file_format.is_none());
        assert!(r.adapter_properties.is_empty());
    }

    #[test]
    fn duckdb_catalog_name_without_catalogs_yml_errors() {
        // catalog_name specified but no matching catalog in catalogs.yml -> error
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: other_catalog
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://rest.example.com"
"#,
        );
        let conf = json!({ "catalog_name": "nonexistent" });
        let m = model(AdapterType::DuckDB, conf);

        let err =
            from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
                .unwrap_err();

        assert!(format!("{err}").contains("Catalog 'nonexistent' not found in catalogs.yml"));
    }

    #[test]
    fn duckdb_iceberg_rest_catalog_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_rest
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://rest.example.com"
"#,
        );
        let conf = json!({ "catalog_name": "my_rest" });
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert_eq!(r.catalog_name.as_deref(), Some("my_rest"));
        assert_eq!(r.catalog_type, CatalogType::IcebergRest);
        assert_eq!(r.table_format, TableFormat::Iceberg);
        assert_eq!(
            r.adapter_properties.get("endpoint").map(|s| s.as_str()),
            Some("https://rest.example.com")
        );
        assert_eq!(
            r.adapter_properties
                .get("attached_database")
                .map(|s| s.as_str()),
            Some("my_rest")
        );
        // No secret specified
        assert!(!r.adapter_properties.contains_key("secret"));
    }

    #[test]
    fn duckdb_iceberg_rest_missing_duckdb_config_errors() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_rest
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "REST_DB"
"#,
        );
        let conf = json!({ "catalog_name": "my_rest" });
        let m = model(AdapterType::DuckDB, conf);

        let err =
            from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
                .unwrap_err();

        assert!(format!("{err}").contains("Catalog 'my_rest' has no configuration for 'duckdb'"));
    }

    #[test]
    fn duckdb_local_filesystem_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: local_files
    type: local_filesystem
    table_format: default
    config:
      duckdb:
        root_path: "data/local_files"
        file_format: csv
"#,
        );
        let conf = json!({ "catalog_name": "local_files" });
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert_eq!(r.catalog_name.as_deref(), Some("local_files"));
        assert_eq!(r.catalog_type, CatalogType::LocalFilesystem);
        assert_eq!(r.table_format, TableFormat::Default);
        assert_eq!(r.file_format.as_deref(), Some("csv"));
        assert_eq!(
            r.adapter_properties
                .get("external_root")
                .map(|s| s.as_str()),
            Some("data/local_files")
        );
    }

    #[test]
    fn duckdb_catalog_name_none_sentinel_returns_default() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_rest
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://rest.example.com"
"#,
        );
        let conf = json!({ "catalog_name": "none" });
        let m = model(AdapterType::DuckDB, conf);
        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();
        assert!(r.catalog_name.is_none());
        assert_eq!(r.catalog_type, CatalogType::DuckdbNative);
        assert_eq!(r.table_format, TableFormat::Default);
    }

    #[test]
    fn duckdb_catalog_alias_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
"#,
        );
        let conf = json!({ "catalog": "my_lake" });
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert_eq!(r.catalog_name.as_deref(), Some("my_lake"));
        assert_eq!(r.catalog_type, CatalogType::DuckLake);
    }

    #[test]
    fn duckdb_horizon_unity_model_targets_build() {
        // Writes are enabled for Horizon/Unity in this PR (duckdb 1.5.4
        // write-compat), lifting the base PR's read-only model-target gate:
        // a model naming one as its catalog builds a relation like any other
        // Iceberg REST catalog.
        for (cat_type, endpoint) in [
            ("horizon", "https://horizon.example.com/catalog"),
            (
                "unity",
                "https://dbc.example.com/api/2.1/unity-catalog/iceberg",
            ),
        ] {
            let catalogs = load_catalogs_yaml(&format!(
                r#"
catalogs:
  - name: writable_cat
    type: {cat_type}
    table_format: iceberg
    config:
      duckdb:
        endpoint: "{endpoint}"
        warehouse: "wh"
"#
            ));
            let conf = json!({ "catalog_name": "writable_cat" });
            let m = model(AdapterType::DuckDB, conf);

            let r =
                from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
                    .unwrap_or_else(|e| panic!("expected {cat_type} relation to build, got: {e}"));
            assert_eq!(r.catalog_name.as_deref(), Some("writable_cat"));
            assert_eq!(
                r.catalog_type,
                CatalogType::parse_from_str(cat_type, AdapterType::DuckDB)
            );
            assert_eq!(r.table_format, TableFormat::Iceberg);
        }
    }

    #[test]
    fn duckdb_stage_create_tables_steers_write_strategy() {
        // Unset (and explicit false): iceberg catalogs write via the safe empty
        // CREATE + INSERT. Explicit `stage_create_tables: true` opts in to
        // staged creates, so dbt may CTAS the target in place
        // (duckdb-iceberg#1017).
        for (cfg_line, expected, stage_creates) in [
            ("", DuckDbWriteStrategy::DirectCreate, false),
            (
                "        stage_create_tables: false",
                DuckDbWriteStrategy::DirectCreate,
                false,
            ),
            (
                "        stage_create_tables: true",
                DuckDbWriteStrategy::DirectCreateAsSelect,
                true,
            ),
            (
                // YAML string bools coerce like the ATTACH composer does.
                "        stage_create_tables: \"true\"",
                DuckDbWriteStrategy::DirectCreateAsSelect,
                true,
            ),
        ] {
            let catalogs = load_catalogs_yaml(&format!(
                r#"
catalogs:
  - name: writable_cat
    type: horizon
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://horizon.example.com/catalog"
        warehouse: "wh"
{cfg_line}
"#
            ));
            let conf = json!({ "catalog_name": "writable_cat" });
            let m = model(AdapterType::DuckDB, conf);

            let r =
                from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
                    .unwrap_or_else(|e| panic!("expected relation to build for {cfg_line:?}: {e}"));
            assert_eq!(r.duckdb_write_strategy(), expected, "cfg: {cfg_line:?}");
            assert_eq!(
                r.supports_stage_create(),
                stage_creates,
                "cfg: {cfg_line:?}"
            );
        }
    }

    // ===== DuckLake v2 tests =====

    #[test]
    fn duckdb_ducklake_builds_relation() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
"#,
        );
        let conf = json!({ "catalog_name": "my_lake" });
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert_eq!(r.catalog_name.as_deref(), Some("my_lake"));
        assert_eq!(r.catalog_type, CatalogType::DuckLake);
        assert_eq!(r.table_format, TableFormat::Default);
        assert_eq!(
            r.adapter_properties
                .get("metadata_path")
                .map(|s| s.as_str()),
            Some("metadata.ducklake")
        );
        assert_eq!(
            r.adapter_properties
                .get("attached_database")
                .map(|s| s.as_str()),
            Some("my_lake")
        );
        assert!(!r.adapter_properties.contains_key("catalog_linked_database"));
        assert!(!r.adapter_properties.contains_key("data_path"));
    }

    #[test]
    fn duckdb_ducklake_with_data_path() {
        let catalogs = load_catalogs_yaml(
            r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
        data_path: "s3://bucket/data/"
        catalog_database: "lake"
"#,
        );
        let conf = json!({ "catalog_name": "my_lake" });
        let m = model(AdapterType::DuckDB, conf);

        let r = from_model_config_and_catalogs_default(AdapterType::DuckDB, &m, Arc::new(catalogs))
            .unwrap();

        assert_eq!(r.catalog_name.as_deref(), Some("my_lake"));
        assert_eq!(r.catalog_type, CatalogType::DuckLake);
        assert_eq!(
            r.adapter_properties
                .get("metadata_path")
                .map(|s| s.as_str()),
            Some("metadata.ducklake")
        );
        assert_eq!(
            r.adapter_properties.get("data_path").map(|s| s.as_str()),
            Some("s3://bucket/data/")
        );
        assert_eq!(
            r.adapter_properties
                .get("attached_database")
                .map(|s| s.as_str()),
            Some("lake")
        );
        assert!(!r.adapter_properties.contains_key("catalog_linked_database"));
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
    fn deprecated_catalog_linked_database_is_linked() {
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
    fn catalog_database_on_a_linked_catalog_is_linked() {
        let mut relation = snowflake(CatalogType::IcebergRest);
        relation.catalog_database = Some("MY_LINKED_DB".to_string());
        assert!(relation.has_catalog_linked_database());
    }

    #[test]
    fn catalog_database_on_the_managed_catalog_is_not_linked() {
        let mut relation = snowflake(CatalogType::SnowflakeBuiltIn);
        relation.catalog_database = Some("ANALYTICS_ICEBERG".to_string());
        assert!(!relation.has_catalog_linked_database());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_common::serde_utils::convert_yml_to_value_map;
    use dbt_schemas::schemas::project::SnapshotConfig;
    use dbt_schemas::schemas::{DbtSnapshot, InternalDbtNode};
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

    fn snapshot(config: SnapshotConfig) -> JVal {
        let mut snapshot = DbtSnapshot {
            deprecated_config: config,
            ..Default::default()
        };
        snapshot.__snapshot_attr__.catalog_name = snapshot
            .deprecated_config
            .__warehouse_specific_config__
            .catalog_name
            .clone();
        snapshot.__snapshot_attr__.table_format = snapshot.deprecated_config.table_format.clone();

        JVal::from_object(convert_yml_to_value_map(snapshot.serialize()))
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
    fn snapshot_config_selects_snowflake_iceberg_relation() {
        let mut config = SnapshotConfig {
            schema: Some("SCH".to_string()),
            alias: Some("SNAPSHOT".to_string()),
            table_format: Some("iceberg".to_string()),
            ..Default::default()
        };
        config.__warehouse_specific_config__.external_volume = Some("EV".to_string());
        config.__warehouse_specific_config__.iceberg_version = Some(2);
        let snapshot = snapshot(config);

        let relation = CatalogRelation::from_model_config_and_catalogs(
            AdapterType::Snowflake,
            &snapshot,
            None,
        )
        .unwrap();

        assert_eq!(relation.catalog_type, CatalogType::SnowflakeBuiltIn);
        assert_eq!(relation.table_format, TableFormat::Iceberg);
        assert_eq!(relation.external_volume.as_deref(), Some("EV"));
        assert_eq!(
            relation
                .adapter_properties
                .get("iceberg_version")
                .map(String::as_str),
            Some("2")
        );
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let err =
                CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap_err();
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
            let err =
                CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap_err();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
    fn snapshot_config_selects_databricks_iceberg_relation() {
        let cats = catalogs_yaml_one(
            "UC",
            "WIN",
            "unity",
            "ICEBERG",
            &[("file_format", s("delta"))],
        );
        let mut config = SnapshotConfig {
            table_format: Some("iceberg".to_string()),
            ..Default::default()
        };
        config.__warehouse_specific_config__.catalog_name = Some("UC".to_string());
        let snapshot = snapshot(config);

        let relation = CatalogRelation::from_model_config_and_catalogs(
            AdapterType::Databricks,
            &snapshot,
            Some(Arc::new(DbtCatalogs::new(cats, Default::default()))),
        )
        .unwrap();

        assert_eq!(relation.catalog_name.as_deref(), Some("UC"));
        assert_eq!(relation.catalog_type, CatalogType::Unity);
        assert_eq!(relation.table_format, TableFormat::Iceberg);
        assert_eq!(relation.file_format.as_deref(), Some("delta"));
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
            let r = CatalogRelation::deprecated_build_with_catalogs(&m, &cats, "CAT").unwrap();
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
