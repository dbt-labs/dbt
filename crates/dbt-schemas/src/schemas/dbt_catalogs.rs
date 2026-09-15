//! dbt_catalogs.yml schema: typed, borrowed view + validation (no IO).
//!
//! Top-level YAML shape (strict keys):
//!   catalogs:
//!     - name|catalog_name: <string, non-empty>
//!       active_write_integration: <string, non-empty>
//!
//!       ## ==== Snowflake
//!       write_integrations:
//!         - name|integration_name: <string, non-empty>
//!           // TODO: catalog type info schema and DDL
//!           catalog_type: built_in | snowflake | rest | iceberg_rest  // case-insensitive
//!           table_format: iceberg                                     // default; case-insensitive
//!           external_volume: <string>                                 // required & non-empty for built_in; forbidden for REST write-support
//!           adapter_properties:                                       // variant-specific; strict keys
//!             // built_in:
//!             base_location_root: <string>                            // optional; if present, non-empty
//!             base_location_subpath: <string>                         // model config only; optional; if present, non-empty
//!             change_tracking: <bool>
//!             data_retention_time_in_days: <u32 in 0..=90>
//!             max_data_extension_time_in_days: <u32 in 0..=90>
//!             storage_serialization_policy: COMPATIBLE|OPTIMIZED      // case-insensitive
//!             // REST:
//!             auto_refresh: <bool>
//!             catalog_linked_database: <string, if present non-empty> // REST write-support gate
//!             catalog_linked_database_type: <string, if present non-empty> // "glue" for AWS Glue
//!             max_data_extension_time_in_days: <u32 in 0..=90>
//!             target_file_size: 'AUTO'|'16MB'|'32MB'|'64MB'|'128MB'   // case-insensitive
//!
//!       ## ==== Databricks
//!       write_integrations:                                           // used as a stronger answer to DatabricksRelation::is_hive_metastore()
//!         - name: <non-empty>
//!           catalog_type: hive_metastore
//!           table_format: default
//!           file_format: delta|hudi|parquet
//!
//!       write_integrations:
//!         - name: <non-empty>
//!           catalog_type: unity
//!           table_format: iceberg
//!           file_format: delta                                        // required with table_format=iceberg
//!           adapter_properties:
//!             location: <string>                                      // optional; if present, non-empty -- will be external_volume under the hood
//!
//!       ## ==== Bigquery
//!       write_integrations:
//!         - name: <non-empty>
//!           external_volume: <string>                                 // required & non-empty; used to generate storage_uri
//!           table_format: iceberg
//!           file_format: parquet
//!           catalog_type: biglake_metastore
//!           adapter_properties:
//!             base_location_root: <string>                            // optional; if present, non-empty
//!             base_location_subpath: <string>                         // model config only; optional; if present, non-empty
//!             storage_uri: <string>                                   // model config only; optional; if present, non-empty
//!             connection_id: <string>                                 // optional; if present, non-empty
//!

use std::collections::HashSet;
use std::path::Path;
use std::sync::OnceLock;

use super::dbt_catalogs_v2::{
    CatalogType, PhysicalFormatResolver, PhysicalTableFormat, TableFormat,
};
use dbt_adapter_core::AdapterType;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_yaml::{self as yml};

#[derive(Clone, Copy)]
pub enum LoadedCatalogs<'a> {
    None,
    V1(&'a DbtCatalogs),
    V2(&'a DbtCatalogs),
}

impl<'a> LoadedCatalogs<'a> {
    pub fn catalog_names(&self) -> FsResult<Option<&'a [String]>> {
        match self {
            LoadedCatalogs::None => Ok(None),
            LoadedCatalogs::V1(c) => Ok(Some(c.v1_catalog_names()?)),
            LoadedCatalogs::V2(c) => Ok(Some(c.v2_catalog_names()?)),
        }
    }

    pub fn has_catalog_name(&self, name: &str) -> FsResult<bool> {
        Ok(self
            .catalog_names()?
            .is_some_and(|names| names.iter().any(|n| n == name)))
    }
}

const CATALOG_DATABASE_PLATFORMS: &[AdapterType] = &[
    AdapterType::DuckDB,
    AdapterType::Snowflake,
    AdapterType::Databricks,
    AdapterType::Bigquery,
];

#[derive(Debug, Clone)]
pub struct CatalogPlatformEntry<'a> {
    pub adapter_type: AdapterType,
    pub catalog_name: &'a str,
    pub catalog_database: Option<&'a str>,
}

/// A validated catalogs.yml mapping.
/// - Holds the raw YAML Mapping so external consumers can copy as needed.
/// - Provides a borrowed, zero-copy typed view on demand.
#[derive(Debug, Clone)]
pub struct DbtCatalogs {
    pub repr: yml::Mapping,
    pub span: yml::Span,
    v1_catalog_names: OnceLock<Vec<String>>,
    v2_catalog_names: OnceLock<Vec<String>>,
    v2_catalog_databases: OnceLock<Vec<String>>,
}

impl DbtCatalogs {
    pub fn new(repr: yml::Mapping, span: yml::Span) -> Self {
        Self {
            repr,
            span,
            v1_catalog_names: OnceLock::new(),
            v2_catalog_names: OnceLock::new(),
            v2_catalog_databases: OnceLock::new(),
        }
    }

    /// Borrow the validated raw mapping.
    pub fn mapping(&self) -> &yml::Mapping {
        &self.repr
    }

    /// Move out the validated raw mapping (for out-of-crate consumers to copy from).
    pub fn into_mapping(self) -> yml::Mapping {
        self.repr
    }

    /// Borrowed, zero-copy typed view (validated shape).
    pub fn view(&self) -> FsResult<DbtCatalogsView<'_>> {
        DbtCatalogsView::from_mapping(&self.repr, self.span.clone())
    }

    /// Ergonomic helper: return the `catalogs` sequence (validated presence).
    pub fn catalogs_seq(&self) -> FsResult<&yml::Sequence> {
        self.repr
            .get(yml::Value::from("catalogs"))
            .and_then(|yaml_value| yaml_value.as_sequence())
            .ok_or_else(|| fs_err!(ErrorCode::InvalidConfig, "Missing 'catalogs'"))
    }

    pub fn is_v2_catalog(&self, name: &str) -> FsResult<bool> {
        Ok(self.v2_catalog_names()?.iter().any(|n| n == name))
    }

    pub fn v1_catalog_names(&self) -> FsResult<&[String]> {
        if let Some(names) = self.v1_catalog_names.get() {
            return Ok(names);
        }
        self.populate_v1_cache()?;
        Ok(self.v1_catalog_names.get().unwrap())
    }

    pub fn v2_catalog_names(&self) -> FsResult<&[String]> {
        if let Some(names) = self.v2_catalog_names.get() {
            return Ok(names);
        }
        self.populate_v2_caches()?;
        Ok(self.v2_catalog_names.get().unwrap())
    }

    pub fn platform_entries(&self) -> FsResult<impl Iterator<Item = CatalogPlatformEntry<'_>>> {
        let view = self.view_v2()?;
        Ok(view.catalogs.into_iter().flat_map(|catalog| {
            CATALOG_DATABASE_PLATFORMS
                .iter()
                .filter_map(move |&adapter_type| {
                    let block = catalog.config_block(adapter_type.as_ref())?;
                    let catalog_database = block
                        .get(yml::Value::from("catalog_database"))
                        .and_then(|v| v.as_str())
                        .map(str::trim)
                        .filter(|s| !s.is_empty());
                    Some(CatalogPlatformEntry {
                        adapter_type,
                        catalog_name: catalog.name,
                        catalog_database,
                    })
                })
        }))
    }

    pub fn catalogs_matching_database(
        &self,
        database: &str,
        adapter_type: AdapterType,
    ) -> FsResult<Vec<String>> {
        debug_assert!(!database.trim().is_empty());
        let names: Vec<&str> = self
            .platform_entries()?
            .filter(|entry| entry.adapter_type == adapter_type)
            .filter_map(|entry| {
                let alias = match adapter_type {
                    AdapterType::DuckDB => entry.catalog_database.unwrap_or(entry.catalog_name),
                    _ => entry.catalog_database?,
                };
                alias
                    .eq_ignore_ascii_case(database)
                    .then_some(entry.catalog_name)
            })
            .collect();

        let view = self.view_v2()?;
        Ok(view
            .catalogs
            .iter()
            .filter(|catalog| {
                names.contains(&catalog.name)
                    && catalog.physical_table_format() != PhysicalTableFormat::Default
            })
            .map(|catalog| catalog.name.to_string())
            .collect())
    }

    pub fn is_v2_catalog_database(&self, db: &str) -> FsResult<bool> {
        if let Some(cds) = self.v2_catalog_databases.get() {
            return Ok(cds.iter().any(|d| d == db));
        }
        self.populate_v2_caches()?;
        Ok(self
            .v2_catalog_databases
            .get()
            .unwrap()
            .iter()
            .any(|d| d == db))
    }

    /// The v2 catalog type behind a `catalog_name`, if that name is a v2 catalog.
    ///
    /// Callers use this to ask capability questions about storage — which engines
    /// can read it, and whose credentials reaching it requires. Both are properties
    /// of the catalog *type*, not of anything a project declares.
    pub fn v2_catalog_type(&self, name: &str) -> FsResult<Option<CatalogType>> {
        Ok(self
            .view_v2()?
            .catalogs
            .iter()
            .find(|catalog| catalog.name == name)
            .map(|catalog| catalog.catalog_type))
    }

    /// Returns `(catalog_name, catalog_database)` for every v2 `iceberg_rest`
    /// catalog that declares a Snowflake `catalog_database` (the linked
    /// database write support is gated on).
    pub fn iceberg_rest_catalog_databases(&self) -> FsResult<Vec<(String, String)>> {
        use super::dbt_catalogs_v2::CatalogType;

        let view = self.view_v2()?;
        Ok(view
            .catalogs
            .iter()
            .filter(|c| c.catalog_type == CatalogType::IcebergRest)
            .filter_map(|c| {
                let db = c
                    .config_block("snowflake")?
                    .get(yml::Value::from("catalog_database"))?
                    .as_str()?
                    .trim();
                (!db.is_empty()).then(|| (c.name.to_owned(), db.to_owned()))
            })
            .collect())
    }

    fn populate_v1_cache(&self) -> FsResult<()> {
        let view = self.view()?;
        let names: Vec<String> = view
            .catalogs
            .iter()
            .map(|catalog| catalog.catalog_name.0.to_owned())
            .collect();
        self.v1_catalog_names.get_or_init(|| names);
        Ok(())
    }

    // Both is_v2_catalog and is_v2_catalog_database share a single
    // view_v2() parse so that whichever fires first pays the cost once for
    // both caches. This couples their initialization but avoids a redundant
    // parse for callers that use both (the common case).
    //
    // Pre:  self.repr holds a validated v2 catalogs.yml mapping.
    // Post: v2_catalog_names and v2_catalog_databases are populated.
    fn populate_v2_caches(&self) -> FsResult<()> {
        let view = self.view_v2()?;
        let mut names = Vec::with_capacity(view.catalogs.len());
        let mut cds = Vec::new();
        for catalog in &view.catalogs {
            names.push(catalog.name.to_owned());
            if let Some(snowflake) = catalog.config_block("snowflake") {
                if let Some(db) = snowflake
                    .get(yml::Value::from("catalog_database"))
                    .and_then(|val| val.as_str())
                    .map(|name| name.trim())
                    .filter(|name| !name.is_empty())
                {
                    cds.push(db.to_owned());
                }
            }
        }
        self.v2_catalog_names.get_or_init(|| names);
        self.v2_catalog_databases.get_or_init(|| cds);
        Ok(())
    }
}

const V1_CATALOG_TYPES: [(&str, CatalogType); 7] = [
    ("built_in", CatalogType::SnowflakeBuiltIn),
    ("snowflake", CatalogType::SnowflakeBuiltIn),
    ("rest", CatalogType::IcebergRest),
    ("iceberg_rest", CatalogType::IcebergRest),
    ("hive_metastore", CatalogType::HiveMetastore),
    ("unity", CatalogType::Unity),
    ("biglake_metastore", CatalogType::BiglakeMetastore),
];

const V1_CATALOG_TYPE_OPTS: &str =
    "built_in|snowflake|rest|iceberg_rest|unity|hive_metastore|biglake_metastore";

#[derive(Debug, PartialEq)]
pub enum FileFormat {
    Delta,
    Hudi,
    Parquet,
}

impl FileFormat {
    fn parse_file_format(s: &str, catalog_type: CatalogType) -> FsResult<FileFormat> {
        match catalog_type {
            CatalogType::HiveMetastore | CatalogType::Unity => {
                if s.eq_ignore_ascii_case("delta") {
                    Ok(FileFormat::Delta)
                } else if s.eq_ignore_ascii_case("hudi") {
                    Ok(FileFormat::Hudi)
                } else if s.eq_ignore_ascii_case("parquet") {
                    Ok(FileFormat::Parquet)
                } else {
                    err!(
                        ErrorCode::InvalidConfig,
                        "file_format '{}' invalid. choose one of (delta|hudi|parquet)",
                        s
                    )
                }
            }
            CatalogType::BiglakeMetastore => {
                if s.eq_ignore_ascii_case("parquet") {
                    Ok(FileFormat::Delta)
                } else {
                    err!(
                        ErrorCode::InvalidConfig,
                        "file_format '{}' invalid. choose one of (parquet)",
                        s
                    )
                }
            }
            CatalogType::SnowflakeBuiltIn | CatalogType::IcebergRest => err!(
                ErrorCode::InvalidConfig,
                "'file_format' is not supported for catalog type '{}'",
                catalog_type.as_str()
            ),
            _ => unreachable!("V1_CATALOG_TYPES never maps a raw type string onto this variant"),
        }
    }
}

// Snowflake Properties

#[derive(Debug, Default)]
pub struct SnowflakeBuiltInPropsView<'a> {
    pub base_location_root: Option<&'a str>,
    pub change_tracking: Option<bool>,
    pub data_retention_time_in_days: Option<u32>,
    pub max_data_extension_time_in_days: Option<u32>,
    pub storage_serialization_policy: Option<SerializationPolicy>,
    pub iceberg_version: Option<u32>,
}

#[derive(Debug, Default)]
pub struct SnowflakeRestPropsView<'a> {
    pub auto_refresh: Option<bool>,
    pub catalog_linked_database: Option<&'a str>,
    pub catalog_linked_database_type: Option<&'a str>,
    pub max_data_extension_time_in_days: Option<u32>,
    pub target_file_size: Option<TargetFileSize>,
    pub iceberg_version: Option<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationPolicy {
    COMPATIBLE,
    OPTIMIZED,
}

/// 'AUTO' | '16MB' | '32MB' | '64MB' | '128MB'
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFileSize {
    AUTO,
    _16MB,
    _32MB,
    _64MB,
    _128MB,
}

// Bigquery Properties
#[derive(Debug)]
pub struct BigqueryBuiltInPropsView<'a> {
    pub base_location_root: Option<&'a str>,
    pub connection_id: Option<&'a str>,
}

// Databricks Properties
#[derive(Debug)]
pub struct DatabricksUnityPropsView<'a> {
    pub location_root: Option<&'a str>,
}

#[derive(Debug)]
pub enum AdapterPropsView<'a> {
    SnowflakeBuiltIn(SnowflakeBuiltInPropsView<'a>),
    SnowflakeRest(SnowflakeRestPropsView<'a>),
    BigqueryBuiltIn(BigqueryBuiltInPropsView<'a>),
    DatabricksUnity(DatabricksUnityPropsView<'a>),
    Empty,
}

#[inline]
fn get_str<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<(&'a str, yml::Span)>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::String(s, span) => Ok(Some((s.trim(), span.clone()))),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a string",
                k
            )),
        },
        None => Ok(None),
    }
}

#[inline]
fn get_map<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<(&'a yml::Mapping, yml::Span)>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::Mapping(map, span) => Ok(Some((map, span.clone()))),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a mapping",
                k
            )),
        },
        None => Ok(None),
    }
}

#[inline]
fn get_seq<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<(&'a yml::Sequence, yml::Span)>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::Sequence(seq, span) => Ok(Some((seq, span.clone()))),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a sequence/list",
                k
            )),
        },
        None => Ok(None),
    }
}

#[inline]
fn get_bool(m: &yml::Mapping, k: &str) -> FsResult<Option<(bool, yml::Span)>> {
    m.get(yml::Value::from(k))
        .map(|v| match v {
            yml::Value::Bool(b, span) => Ok((*b, span.clone())),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a boolean",
                k
            )),
        })
        .transpose()
}

#[inline]
fn get_u32(m: &yml::Mapping, k: &str) -> FsResult<Option<(u32, yml::Span)>> {
    m.get(yml::Value::from(k))
        .map(|v| match v {
            yml::Value::Number(n, span) => n
                .as_i64()
                .and_then(|i| u32::try_from(i).ok())
                .map(|u| (u, span.clone()))
                .ok_or_else(|| {
                    fs_err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(span.clone()),
                        "Key '{}' must be a non-negative integer",
                        k
                    )
                }),
            _ => Err(fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(v.span().clone()),
                "Key '{}' must be a non-negative integer",
                k
            )),
        })
        .transpose()
}

#[inline]
fn key_err(key: &str, err_span: Option<yml::Span>) -> Box<dbt_common::FsError> {
    fs_err!(
        code => ErrorCode::InvalidConfig,
        hacky_yml_loc => err_span,
        "Missing required key '{}' in catalogs.yml",
        key
    )
}

#[inline]
fn is_blank(s: &str) -> bool {
    s.trim().is_empty()
}

fn check_unknown_keys(m: &yml::Mapping, allowed: &[&str], ctx: &str) -> FsResult<()> {
    for k in m.keys() {
        let span = k.span();
        let Some(ks) = k.as_str() else {
            return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(span.clone()), "Non-string key in {}", ctx);
        };
        if !allowed.iter().any(|a| a == &ks) {
            return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(span.clone()), "Unknown key '{}' in {}", ks, ctx);
        }
    }
    Ok(())
}

#[derive(Debug)]
pub struct WriteIntegrationView<'a> {
    pub integration_name: &'a str, // alias: name
    pub catalog_type: CatalogType,
    // defaults to iceberg; core reconciles with extra info_schema
    // concept we do not have
    pub table_format: TableFormat,
    pub external_volume: Option<&'a str>,

    // == Databricks and Bigquery (top-level)
    pub file_format: Option<FileFormat>,

    pub adapter_properties: Option<AdapterPropsView<'a>>,
}

#[derive(Debug)]
pub struct CatalogSpecView<'a> {
    pub span: yml::Span,
    pub catalog_name: (&'a str, yml::Span), // alias: name
    pub active_write_integration: (&'a str, yml::Span),
    pub write_integrations: (Vec<WriteIntegrationView<'a>>, yml::Span),
}

#[derive(Debug)]
pub struct DbtCatalogsView<'a> {
    pub span: yml::Span,
    pub catalogs: Vec<CatalogSpecView<'a>>,
}

impl<'a> DbtCatalogsView<'a> {
    pub fn from_mapping(map: &'a yml::Mapping, span: yml::Span) -> FsResult<Self> {
        check_unknown_keys(map, &["catalogs"], "top-level catalogs.yml")?;
        let catalog_entries = match get_seq(map, "catalogs")? {
            Some(entries) => entries,
            None => return Err(key_err("catalogs", Some(span))),
        };

        let mut catalogs = Vec::with_capacity(catalog_entries.0.len());

        for (idx, item) in catalog_entries.0.iter().enumerate() {
            let item_span = item.span().clone();
            let m = match item.as_mapping() {
                Some(m) => m,
                None => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(item_span),
                        "catalogs[{idx}] must be a mapping"
                    );
                }
            };
            catalogs.push(CatalogSpecView::from_mapping(m, item_span)?);
        }

        Ok(Self { catalogs, span })
    }
}

impl<'a> CatalogSpecView<'a> {
    pub fn from_mapping(map: &'a yml::Mapping, span: yml::Span) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "catalog_name",
                "name",
                "active_write_integration",
                "write_integrations",
            ],
            "catalog specification",
        )?;

        // Favor "catalog_name"
        if let (Some((_, _span1)), Some((_, span2))) =
            (get_str(map, "catalog_name")?, get_str(map, "name")?)
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span2),
                "Both 'catalog_name' and 'name' provided in catalog specification; provide exactly one"
            );
        }

        let catalog_name = match get_str(map, "catalog_name")? {
            Some(val) => val,
            None => get_str(map, "name")?
                .ok_or_else(|| key_err("catalog_name|name", Some(span.clone())))?,
        };

        let active = get_str(map, "active_write_integration")?
            .ok_or_else(|| key_err("active_write_integration", Some(span.clone())))?;

        let write_integration_entries = get_seq(map, "write_integrations")?
            .ok_or_else(|| key_err("write_integrations", Some(span.clone())))?;

        let mut write_integrations = Vec::with_capacity(write_integration_entries.0.len());
        for (idx, item) in write_integration_entries.0.iter().enumerate() {
            let item_span = item.span();
            let write_integration_mapping = item.as_mapping().ok_or_else(|| {
                fs_err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(item_span.clone()),
                    "write_integrations[{idx}] must be a mapping"
                )
            })?;
            write_integrations.push(WriteIntegrationView::from_mapping(
                write_integration_mapping,
            )?);
        }
        Ok(Self {
            span,
            catalog_name,
            active_write_integration: active,
            write_integrations: (write_integrations, write_integration_entries.1),
        })
    }
}

impl<'a> WriteIntegrationView<'a> {
    pub fn from_mapping(map: &'a yml::Mapping) -> FsResult<Self> {
        if let (Some(_), Some((_, span2))) =
            (get_str(map, "integration_name")?, get_str(map, "name")?)
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span2),
                "Both 'integration_name' and 'name' provided in write_integration; provide exactly one"
            );
        }

        let integration_name = match get_str(map, "integration_name")? {
            Some(val) => val,
            None => get_str(map, "name")?.ok_or_else(|| key_err("integration_name|name", None))?,
        };

        let catalog_type_raw =
            get_str(map, "catalog_type")?.ok_or_else(|| key_err("catalog_type", None))?;
        let catalog_type = V1_CATALOG_TYPES
            .iter()
            .find_map(|(name, v)| catalog_type_raw.0.eq_ignore_ascii_case(name).then_some(*v))
            .ok_or_else(|| {
                fs_err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(catalog_type_raw.1),
                    "catalog_type '{}' invalid. choose one of ({})",
                    catalog_type_raw.0,
                    V1_CATALOG_TYPE_OPTS
                )
            })?;

        match catalog_type {
            CatalogType::SnowflakeBuiltIn => Self::from_snowflake_built_in(map, integration_name.0),
            CatalogType::IcebergRest => Self::from_snowflake_iceberg_rest(map, integration_name.0),
            CatalogType::Unity => Self::from_databricks_unity(map, integration_name.0),
            CatalogType::HiveMetastore => Self::from_databricks_hms(map, integration_name.0),
            CatalogType::BiglakeMetastore => Self::from_bigquery_built_in(map, integration_name.0),
            _ => unreachable!("V1_CATALOG_TYPES never maps a raw type string onto this variant"),
        }
    }

    // Snowflake BUILT_IN
    fn from_snowflake_built_in(map: &'a yml::Mapping, integration_name: &'a str) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "integration_name",
                "name",
                "catalog_type",
                "table_format",
                "external_volume",
                "adapter_properties",
                // These keys are adapter properties, allowed only to throw an explicit error message
                "base_location_root",
                "base_location_subpath",
            ],
            "write_integration(Snowflake built_in)",
        )?;

        let (table_format, table_format_span) = match get_str(map, "table_format")? {
            Some((s, span)) => (TableFormat::parse_from_yaml(s, &span)?, span),
            None => return Err(key_err("table_format", None)),
        };
        if table_format != TableFormat::Iceberg {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(table_format_span),
                "integration '{}': Snowflake built_in requires table_format=iceberg",
                integration_name
            );
        }

        let external_volume = {
            Some(match get_str(map, "external_volume")? {
                None => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => None::<yml::Span>,
                        "integration '{}': 'external_volume' is required for BUILT_IN catalogs.",
                        integration_name
                    );
                }
                Some(("", span)) => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(span),
                        "integration '{}': 'external_volume' is required for BUILT_IN catalogs and cannot be blank.",
                        integration_name
                    );
                }
                Some((s, _)) => s,
            })
        };

        // Intentional deviation from Core: Explicitly disallow base_location_{root, subpath}
        // TODO(anna): These are good candidates to add to the autofix script.
        // If we do, we should print warnings instead of throwing one error at a time.
        if let Some((_, span)) = get_str(map, "base_location_root")? {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span),
                "integration '{}': 'base_location_root' must be set under 'adapter_properties', not at the top level.",
                integration_name
            );
        }
        if let Some((_, span)) = get_str(map, "base_location_subpath")? {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span),
                "integration '{}': 'base_location_subpath' must be set under 'adapter_properties' in model configs.",
                integration_name
            );
        }

        let adapter_properties = {
            if let Some((props, _span)) = get_map(map, "adapter_properties")? {
                Some(parse_adapter_properties(
                    props,
                    CatalogType::SnowflakeBuiltIn,
                )?)
            } else {
                None
            }
        };

        Ok(Self {
            integration_name,
            catalog_type: CatalogType::SnowflakeBuiltIn,
            table_format,
            external_volume,
            file_format: None,
            adapter_properties,
        })
    }

    fn from_snowflake_iceberg_rest(
        map: &'a yml::Mapping,
        integration_name: &'a str,
    ) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "integration_name",
                "name",
                "catalog_type",
                "table_format",
                "adapter_properties",
            ],
            "write_integration(Snowflake iceberg_rest)",
        )?;

        let (table_format, table_format_span) = match get_str(map, "table_format")? {
            Some((s, span)) => (TableFormat::parse_from_yaml(s, &span)?, span),
            None => return Err(key_err("table_format", None)),
        };
        if table_format != TableFormat::Iceberg {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(table_format_span),
                "integration '{}': Snowflake iceberg_rest requires table_format=iceberg",
                integration_name
            );
        }

        let adapter_properties = {
            if let Some((props, _span)) = get_map(map, "adapter_properties")? {
                Some(parse_adapter_properties(props, CatalogType::IcebergRest)?)
            } else {
                None
            }
        };

        Ok(Self {
            integration_name,
            catalog_type: CatalogType::IcebergRest,
            table_format,
            external_volume: None,
            file_format: None,
            adapter_properties,
        })
    }

    fn from_databricks_unity(map: &'a yml::Mapping, integration_name: &'a str) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "integration_name",
                "name",
                "catalog_type",
                "table_format",
                "file_format",
                "adapter_properties",
            ],
            "write_integration(Databricks unity)",
        )?;

        let (table_format, table_format_span) = {
            let (raw, span) = match get_str(map, "table_format")? {
                Some((s, span)) => (s, span),
                None => return Err(key_err("table_format", None)),
            };
            (TableFormat::parse_from_yaml(raw, &span)?, span)
        };
        if table_format != TableFormat::Iceberg {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(table_format_span),
                "integration '{}': Databricks 'unity' requires table_format=iceberg",
                integration_name
            );
        }

        let file_format = {
            let parsed = match get_str(map, "file_format")? {
                Some((s, span)) => FileFormat::parse_file_format(s, CatalogType::Unity)
                    .map_err(|e| e.with_hacky_yml_location(Some(span.clone())))?,
                None => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => None::<yml::Span>,
                        "integration '{}': Databricks 'unity' requires file_format (must be 'delta')",
                        integration_name
                    );
                }
            };
            match parsed {
                FileFormat::Delta => Some(parsed),
                _ => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(table_format_span),
                        "integration '{}': when table_format=iceberg, file_format must be 'delta'",
                        integration_name
                    );
                }
            }
        };

        let adapter_properties = {
            if let Some((props, _span)) = get_map(map, "adapter_properties")? {
                Some(parse_adapter_properties(props, CatalogType::Unity)?)
            } else {
                None
            }
        };

        Ok(Self {
            integration_name,
            catalog_type: CatalogType::Unity,
            table_format,
            external_volume: None,
            file_format,
            adapter_properties,
        })
    }

    fn from_databricks_hms(map: &'a yml::Mapping, integration_name: &'a str) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "integration_name",
                "name",
                "catalog_type",
                "table_format",
                "file_format",
            ],
            "write_integration(Databricks hive_metastore)",
        )?;

        let (table_format, table_format_span) = match get_str(map, "table_format")? {
            Some((s, span)) => (TableFormat::parse_from_yaml(s, &span)?, span),
            None => return Err(key_err("table_format", None)),
        };
        if table_format != TableFormat::Default {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(table_format_span),
                "integration '{}': Databricks 'hive_metastore' requires table_format=DEFAULT",
                integration_name
            );
        }

        let file_format = match get_str(map, "file_format")? {
            Some((s, span)) => Some(
                FileFormat::parse_file_format(s, CatalogType::HiveMetastore)
                    .map_err(|e| e.with_hacky_yml_location(Some(span)))?,
            ),
            None => {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => None::<yml::Span>,
                    "integration '{}': file_format required for Databricks hive_metastore and must be one of (delta|parquet|hudi)",
                    integration_name
                );
            }
        };

        let adapter_properties = {
            if let Some((props, _span)) = get_map(map, "adapter_properties")? {
                Some(parse_adapter_properties(props, CatalogType::HiveMetastore)?)
            } else {
                None
            }
        };

        Ok(Self {
            integration_name,
            catalog_type: CatalogType::HiveMetastore,
            table_format,
            external_volume: None,
            file_format,
            adapter_properties,
        })
    }

    fn from_bigquery_built_in(map: &'a yml::Mapping, integration_name: &'a str) -> FsResult<Self> {
        check_unknown_keys(
            map,
            &[
                "integration_name",
                "name",
                "catalog_type",
                "table_format",
                "file_format",
                "external_volume",
                "adapter_properties",
                // These keys are adapter properties, allowed only to throw an explicit error message
                "base_location_root",
                "base_location_subpath",
            ],
            "write_integration(Bigquery biglake_metastore)",
        )?;

        let (table_format, table_format_span) = match get_str(map, "table_format")? {
            Some((s, span)) => (TableFormat::parse_from_yaml(s, &span)?, span),
            None => {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => None::<yml::Span>,
                    "integration '{}': table_format required for Bigquery biglake_metastore (must be iceberg)",
                    integration_name
                );
            }
        };
        if table_format != TableFormat::Iceberg {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(table_format_span),
                "integration '{}': Bigquery biglake_metastore requires table_format=iceberg",
                integration_name
            );
        }

        let file_format = match get_str(map, "file_format")? {
            Some((s, span)) => Some(
                FileFormat::parse_file_format(s, CatalogType::BiglakeMetastore)
                    .map_err(|e| e.with_hacky_yml_location(Some(span.clone())))?,
            ),
            None => {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => None::<yml::Span>,
                    "integration '{}': file_format required for Bigquery biglake_metastore (must be parquet)",
                    integration_name
                );
            }
        };

        let external_volume = {
            Some(match get_str(map, "external_volume")? {
                None => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => None::<yml::Span>,
                        "integration '{}': 'external_volume' is required for biglake_metastore catalogs.",
                        integration_name
                    );
                }
                Some(("", span)) => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(span),
                        "integration '{}': 'external_volume' is required for biglake_metastore catalogs and cannot be blank.",
                        integration_name
                    );
                }
                Some((s, span)) => {
                    if !s.starts_with("gs://") {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => Some(span),
                            "integration '{}': 'external_volume' is required for biglake_metastore catalogs \
                             and must be a path to a Cloud Storage bucket (gs://<bucket_name>).",
                            integration_name
                        );
                    }
                    s
                }
            })
        };

        // Intentional deviation from Core: Explicitly disallow base_location_{root, subpath}
        // TODO(anna): These are good candidates to add to the autofix script.
        // If we do, we should print warnings instead of throwing one error at a time.
        if let Some((_, span)) = get_str(map, "base_location_root")? {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span),
                "integration '{}': 'base_location_root' must be set under 'adapter_properties', not at the top level.",
                integration_name
            );
        }
        if let Some((_, span)) = get_str(map, "base_location_subpath")? {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span),
                "integration '{}': 'base_location_subpath' must be set under 'adapter_properties' in model configs.",
                integration_name
            );
        }

        let adapter_properties = {
            if let Some((props, _span)) = get_map(map, "adapter_properties")? {
                Some(parse_adapter_properties(
                    props,
                    CatalogType::BiglakeMetastore,
                )?)
            } else {
                None
            }
        };

        Ok(Self {
            integration_name,
            catalog_type: CatalogType::BiglakeMetastore,
            table_format,
            external_volume,
            file_format,
            adapter_properties,
        })
    }
}

// TODO: I think this should take in the table format - what if some adapter properties are only supported for certain table formats?
fn parse_adapter_properties<'a>(
    properties: &'a yml::Mapping,
    catalog_type: CatalogType,
) -> FsResult<AdapterPropsView<'a>> {
    match catalog_type {
        CatalogType::SnowflakeBuiltIn => {
            check_unknown_keys(
                properties,
                &[
                    "change_tracking",
                    "data_retention_time_in_days",
                    "max_data_extension_time_in_days",
                    "storage_serialization_policy",
                    "iceberg_version",
                    "base_location_root",
                    "base_location_subpath",
                ],
                "adapter_properties(built_in)",
            )?;
            // Throw explicit error for model-config-only key 'base_location_subpath'
            if let Some((_, span)) = get_str(properties, "base_location_subpath")? {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "'base_location_subpath' must be set under 'adapter_properties' in a model config, not in the write integration config."
                );
            }
            if let Some((loc, span)) = get_str(properties, "base_location_root")?
                && loc.trim().is_empty()
            {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "adapter_properties.base_location_root cannot be blank"
                );
            }
            Ok(AdapterPropsView::SnowflakeBuiltIn(
                SnowflakeBuiltInPropsView {
                    base_location_root: get_str(properties, "base_location_root")?
                        .filter(|(s, _)| !s.trim().is_empty())
                        .map(|(s, _)| s),
                    storage_serialization_policy: get_str(
                        properties,
                        "storage_serialization_policy",
                    )?
                    .filter(|(s, _)| !s.is_empty())
                    .map(|(s, span)| {
                        if s.eq_ignore_ascii_case("compatible") {
                            Ok(SerializationPolicy::COMPATIBLE)
                        } else if s.eq_ignore_ascii_case("optimized") {
                            Ok(SerializationPolicy::OPTIMIZED)
                        } else {
                            err!(
                                code => ErrorCode::InvalidConfig,
                                hacky_yml_loc => Some(span),
                                "storage_serialization_policy '{}' invalid (COMPATIBLE|OPTIMIZED)",
                                s
                            )
                        }
                    })
                    .transpose()?,
                    max_data_extension_time_in_days: get_u32(
                        properties,
                        "max_data_extension_time_in_days",
                    )?
                    .map(|(v, _)| v),
                    data_retention_time_in_days: get_u32(
                        properties,
                        "data_retention_time_in_days",
                    )?
                    .map(|(v, _)| v),
                    change_tracking: get_bool(properties, "change_tracking")?.map(|(v, _)| v),
                    iceberg_version: get_u32(properties, "iceberg_version")?.map(|(v, _)| v),
                },
            ))
        }
        CatalogType::IcebergRest => {
            check_unknown_keys(
                properties,
                &[
                    "auto_refresh",
                    "catalog_linked_database",
                    "catalog_linked_database_type",
                    "max_data_extension_time_in_days",
                    "target_file_size",
                    "iceberg_version",
                ],
                "adapter_properties(iceberg_rest)",
            )?;
            Ok(AdapterPropsView::SnowflakeRest(SnowflakeRestPropsView {
                target_file_size: target_file_size(get_str(properties, "target_file_size")?)?,
                auto_refresh: get_bool(properties, "auto_refresh")?.map(|(v, _)| v),
                max_data_extension_time_in_days: get_u32(
                    properties,
                    "max_data_extension_time_in_days",
                )?
                .map(|(v, _)| v),
                catalog_linked_database: get_str(properties, "catalog_linked_database")?
                    .map(|(s, _)| s),
                catalog_linked_database_type: get_str(properties, "catalog_linked_database_type")?
                    .map(|(s, _)| s),
                iceberg_version: get_u32(properties, "iceberg_version")?.map(|(v, _)| v),
            }))
        }
        CatalogType::BiglakeMetastore => {
            check_unknown_keys(
                properties,
                &[
                    "base_location_root",
                    "base_location_subpath",
                    "storage_uri",
                    "connection_id",
                ],
                "adapter_properties(biglake_metastore)",
            )?;
            // Throw explicit errors for model-config-only keys 'base_location_subpath' and 'storage_uri'
            if let Some((_, span)) = get_str(properties, "base_location_subpath")? {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "'base_location_subpath' must be set under 'adapter_properties' in a model config, not in the write integration config."
                );
            }
            if let Some((_, span)) = get_str(properties, "storage_uri")? {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "'storage_uri' must be set under 'adapter_properties' in a model config, not in the write integration config."
                );
            }
            if let Some((loc, span)) = get_str(properties, "base_location_root")?
                && loc.trim().is_empty()
            {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "adapter_properties.base_location_root cannot be blank"
                );
            }
            if let Some((loc, span)) = get_str(properties, "connection_id")?
                && loc.trim().is_empty()
            {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "adapter_properties.connection_id cannot be blank"
                );
            }
            Ok(AdapterPropsView::BigqueryBuiltIn(
                BigqueryBuiltInPropsView {
                    base_location_root: get_str(properties, "base_location_root")?
                        .filter(|(s, _)| !s.trim().is_empty())
                        .map(|(s, _)| s),
                    connection_id: get_str(properties, "connection_id")?
                        .filter(|(s, _)| !s.trim().is_empty())
                        .map(|(s, _)| s),
                },
            ))
        }
        CatalogType::Unity => {
            check_unknown_keys(properties, &["location_root"], "adapter_properties(unity)")?;
            if let Some((loc, span)) = get_str(properties, "location_root")?
                && loc.trim().is_empty()
            {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "adapter_properties.location_root cannot be blank"
                );
            }
            Ok(AdapterPropsView::DatabricksUnity(
                DatabricksUnityPropsView {
                    location_root: get_str(properties, "location_root")?
                        .filter(|(s, _)| !s.trim().is_empty())
                        .map(|(s, _)| s),
                },
            ))
        }
        CatalogType::HiveMetastore => {
            if !properties.is_empty() {
                let first_key_span = properties.keys().next().map(|k| k.span().clone());
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => first_key_span,
                    "adapter_properties not allowed for {}", catalog_type.as_str()
                );
            }
            // unreachable in practice because caller only invokes this if adapter_properties exists
            Ok(AdapterPropsView::Empty)
        }
        _ => unreachable!("V1_CATALOG_TYPES never maps a raw type string onto this variant"),
    }
}

fn target_file_size(
    maybe_target_file_size: Option<(&str, yml::Span)>,
) -> FsResult<Option<TargetFileSize>> {
    maybe_target_file_size
        .map(|(v, span)| (v.trim(), span))
        .filter(|(v, _)| !v.is_empty())
        .map(|(v, span)| {
            if v.eq_ignore_ascii_case("AUTO") {
                Ok(TargetFileSize::AUTO)
            } else if v.eq_ignore_ascii_case("16MB") {
                Ok(TargetFileSize::_16MB)
            } else if v.eq_ignore_ascii_case("32MB") {
                Ok(TargetFileSize::_32MB)
            } else if v.eq_ignore_ascii_case("64MB") {
                Ok(TargetFileSize::_64MB)
            } else if v.eq_ignore_ascii_case("128MB") {
                Ok(TargetFileSize::_128MB)
            } else {
                err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span),
                    "target_file_size '{}' invalid (AUTO|16MB|32MB|64MB|128MB)",
                    v
                )
            }
        })
        .transpose()
}

// Structurally recursive down the struct of a catalog entry and validate
pub fn validate_catalogs(spec: &DbtCatalogsView<'_>, _path: &Path) -> FsResult<()> {
    // === 1. Ensure no duplicate-name catalogs, ensure no blanks. One pass.
    let mut seen_catalogs = HashSet::new();
    for catalog in &spec.catalogs {
        if is_blank(catalog.catalog_name.0) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(catalog.catalog_name.1.clone()),
                "Catalog field 'name' or 'catalog_name' must be non-empty"
            );
        }
        if !seen_catalogs.insert(catalog.catalog_name.0) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(catalog.catalog_name.1.clone()),
                "Duplicate catalog name '{}'", catalog.catalog_name.0
            );
        }
    }

    for catalog in &spec.catalogs {
        // === 2. Every catalog requires an active write integration
        if is_blank(catalog.active_write_integration.0) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(catalog.active_write_integration.1.clone()),
                "In catalog '{}', 'active_write_integration' must be non-empty",
                catalog.catalog_name.0
            );
        }

        // === 3. Avoid duplicate write integrations
        let mut seen = HashSet::new();
        catalog
            .write_integrations
            .0
            .iter()
            .try_for_each(|write_integration| {
                seen.insert(write_integration.integration_name)
                    .then_some(())
                    .ok_or_else(|| {
                        fs_err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                            "Duplicate write_integration name '{}' in catalog '{}'",
                            write_integration.integration_name, catalog.catalog_name.0
                        )
                    })
            })?;

        // === 4. Ensure named write integration actually exists
        if catalog
            .write_integrations
            .0
            .iter()
            .all(|w| w.integration_name != catalog.active_write_integration.0)
        {
            let choices = if catalog.write_integrations.0.is_empty() {
                "<none>".to_string()
            } else {
                catalog
                    .write_integrations
                    .0
                    .iter()
                    .map(|w| w.integration_name)
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(catalog.active_write_integration.1.clone()),
                "In catalog '{}', active_write_integration '{}' not found. Available: {}",
                catalog.catalog_name.0, catalog.active_write_integration.0, choices
            );
        }

        // === 5. Validate write integration entries
        for write_integration in &catalog.write_integrations.0 {
            // must have a name
            if is_blank(write_integration.integration_name) {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                    "Catalog '{}' integration has no field 'name' or 'integration_name'",
                    catalog.catalog_name.0
                );
            }

            // === 6. Validate other write integration fields modulo catalog type
            match write_integration.catalog_type {
                // === 6a. built in catalog type
                CatalogType::SnowflakeBuiltIn => { /* no non-structural constraints */ }

                // === 6b. iceberg rest catalog type
                CatalogType::IcebergRest => match &write_integration.adapter_properties {
                    Some(AdapterPropsView::SnowflakeRest(rest)) => {
                        match rest.catalog_linked_database {
                            Some(name) if !name.is_empty() => {}
                            _ => {
                                return err!(
                                    code => ErrorCode::InvalidConfig,
                                    hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                                    "integration '{}': 'catalog_linked_database' is required and cannot be blank for Iceberg REST",
                                    write_integration.integration_name
                                );
                            }
                        }
                    }
                    _ => {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                            "integration '{}': only REST adapter_properties are valid for Iceberg REST",
                            write_integration.integration_name
                        );
                    }
                },
                CatalogType::Unity => {
                    if let Some(AdapterPropsView::DatabricksUnity(properties)) =
                        &write_integration.adapter_properties
                    {
                        if let Some(loc) = properties.location_root
                            && loc.trim().is_empty()
                        {
                            return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                                    "integration '{}': adapter_properties.location_root cannot be blank",
                                    write_integration.integration_name);
                        }
                    } else if write_integration.adapter_properties.is_some() {
                        return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                            "integration '{}': only adapter_properties.location_root is allowed for unity",
                            write_integration.integration_name);
                    }

                    if let Some(file_format) = &write_integration.file_format
                        && *file_format != FileFormat::Delta
                    {
                        return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                                "integration '{}': when table_format=iceberg, file_format must be 'delta'",
                                write_integration.integration_name);
                    }
                }

                CatalogType::HiveMetastore => {
                    if write_integration.adapter_properties.is_some() {
                        return err!(code => ErrorCode::InvalidConfig, hacky_yml_loc => Some(catalog.write_integrations.1.clone()),
                            "integration '{}': adapter_properties not allowed for hive_metastore",
                            write_integration.integration_name);
                    }
                }

                CatalogType::BiglakeMetastore => { /* no non-structural constraints */ }
                _ => {
                    unreachable!("V1_CATALOG_TYPES never maps a raw type string onto this variant")
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_yaml as yml;
    use std::path::Path;

    fn parse_view_and_validate(yaml: &str) -> FsResult<()> {
        let v: yml::Value = yml::from_str(yaml).unwrap();
        let v_span = v.span();
        let m = v.as_mapping().expect("top-level YAML must be a mapping");
        let view = DbtCatalogsView::from_mapping(m, v_span.clone())?;
        validate_catalogs(&view, Path::new("<test>"))
    }

    fn assert_err_contains(yaml: &str, needle: &str) {
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok(())");
        assert!(
            msg.contains(needle),
            "error did not contain '{needle}', got: {msg}"
        );
    }

    #[test]
    fn master_full_catalog_valid() {
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        table_format: iceberg
        external_volume: dbt_external_volume
        adapter_properties:
          base_location_root: analytics/iceberg/dbt
          storage_serialization_policy: COMPATIBLE
          data_retention_time_in_days: 1
          max_data_extension_time_in_days: 14
          change_tracking: false
  - name: polaris
    active_write_integration: polaris_int
    write_integrations:
      - name: polaris_int
        catalog_type: iceberg_rest
        table_format: iceberg
        adapter_properties:
          target_file_size: AUTO
          auto_refresh: true
          max_data_extension_time_in_days: 14
          catalog_linked_database: "CAT_NS/SCHEMA_NS"
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "expected master full catalog to validate, got: {:?}",
            res.err()
        );
    }

    #[test]
    fn happy_built_in_minimal() {
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        external_volume: dbt_external_volume
        table_format: iceberg
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "built_in minimal should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn happy_rest_minimal_without_ext_vol() {
        let yaml = r#"
catalogs:
  - name: polaris
    active_write_integration: polaris_int
    write_integrations:
      - name: polaris_int
        catalog_type: iceberg_rest
        table_format: iceberg
        adapter_properties:
          catalog_linked_database: another_db
          auto_refresh: true
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "rest minimal (no external_volume) should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn accepts_name_alias_for_catalog() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        table_format: iceberg
        adapter_properties:
            catalog_linked_database: cld
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_ok(),
            "alias `name` for catalog should parse: {:?}",
            v.err()
        );
        let v = v.unwrap();
        assert_eq!(v.catalogs.len(), 1);
        assert_eq!(v.catalogs[0].catalog_name.0, "c1");
        let res = validate_catalogs(&v, Path::new("<test>"));
        assert!(res.is_ok(), "alias case should validate: {:?}", res.err());
    }

    #[test]
    fn write_integrations_must_be_mapping() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations: [42]
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "non-mapping write_integration element must fail"
        );
    }

    #[test]
    fn missing_catalogs_key() {
        let yaml = r#"
not_catalogs: []
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "missing top-level 'catalogs' must fail");
    }

    #[test]
    fn missing_active_write_integration() {
        let yaml = r#"
    catalogs:
      - name: c1
        write_integrations:
          - name: i1
            catalog_type: built_in
            external_volume: ev
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "missing required key should fail parse");
    }

    #[test]
    fn invalid_catalog_type() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: mango
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "invalid catalog_type must fail parse");
    }

    #[test]
    fn invalid_table_format() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: built_in
        table_format: delta
        external_volume: ev
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "invalid table_format must fail parse");
    }

    #[test]
    fn invalid_storage_serialization_policy() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: built_in
        external_volume: ev
        adapter_properties:
          storage_serialization_policy: FASTEST
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "invalid storage_serialization_policy must fail parse"
        );
    }

    #[test]
    fn invalid_target_file_size_value() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        adapter_properties:
          target_file_size: "1MB"
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "invalid target_file_size must fail parse");
    }

    #[test]
    fn built_in_requires_external_volume() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: built_in
        # external_volume: missing
        table_format: iceberg
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let res = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            res.is_err(),
            "built_in without external_volume must fail validation"
        );
    }

    #[test]
    fn rest_forbids_base_location_fields() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        base_location_root: some/root
        table_format: iceberg
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "Unknown key 'base_location_root' in write_integration(Snowflake iceberg_rest)"
        );
    }

    #[test]
    fn built_in_rejects_base_location_keys_at_top_level() {
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        external_volume: dbt_external_volume
        table_format: iceberg
        base_location_root: invalid
"#;
        assert_err_contains(yaml, "must be set under 'adapter_properties'");
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        external_volume: dbt_external_volume
        table_format: iceberg
        base_location_subpath: invalid
"#;
        assert_err_contains(yaml, "must be set under 'adapter_properties'");
    }

    #[test]
    fn built_in_rejects_base_location_subpath_adapter_property() {
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        external_volume: dbt_external_volume
        table_format: iceberg
        adapter_properties:
          base_location_root: analytics/iceberg/dbt
          base_location_subpath: invalid
"#;
        assert_err_contains(
            yaml,
            "must be set under 'adapter_properties' in a model config",
        );
    }

    #[test]
    fn active_write_integration_must_exist() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: does_not_exist
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        table_format: iceberg
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span).unwrap();
        let res = validate_catalogs(&v, Path::new("<test>"));
        assert!(
            res.is_err(),
            "active write integration must exist among write_integrations"
        );
    }

    #[test]
    fn case_insensitive_enums_parse() {
        let yaml = r#"
catalogs:
  - name: C
    active_write_integration: I
    write_integrations:
      - name: I
        catalog_type: BUILT_IN
        table_format: ICEBERG
        external_volume: EV
        adapter_properties:
          storage_serialization_policy: compatible
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "case-insensitive enums should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn blank_catalog_name_fails() {
        let yaml = r#"
catalogs:
  - catalog_name: "   "
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
"#;
        let res = parse_view_and_validate(yaml);
        assert!(res.is_err(), "blank catalog_name must fail validation");
    }

    #[test]
    fn blank_integration_name_or_active_fails() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: " "
    write_integrations:
      - name: "   "
        catalog_type: iceberg_rest
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_err(),
            "blank integration name / active_write_integration must fail"
        );
    }

    #[test]
    fn blank_base_location_fields_fail() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: built_in
        external_volume: ev
        base_location_root: " "
        base_location_subpath: "   "
"#;
        let res = parse_view_and_validate(yaml);
        assert!(res.is_err(), "blank base_location_* must fail for built_in");
    }

    #[test]
    fn blank_catalog_linked_database_fails() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        adapter_properties:
          catalog_linked_database: "  "
"#;
        let res = parse_view_and_validate(yaml);
        assert!(res.is_err(), "blank catalog_linked_database must fail");
    }

    #[test]
    fn rest_linked_db_requires_external_volume() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        catalog_linked_database: my_db
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_err(),
            "linked DB true without external_volume must fail"
        );
    }

    #[test]
    fn rest_write_prop_requires_external_volume_ext_volen_without_linked_db() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        adapter_properties:
          max_data_extension_time_in_days: 1
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_err(),
            "write-support prop without external_volume must fail"
        );
    }

    #[test]
    fn rest_linked_db_false_conflicts_with_write_prop() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        catalog_linked_database: false
        adapter_properties:
          max_data_extension_time_in_days: 1
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_err(),
            "write-support properties not allowed when no linked database"
        );
    }

    #[test]
    fn rest_linked_db_write_ok_with_external_volume() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i1
    write_integrations:
      - name: i1
        catalog_type: iceberg_rest
        table_format: iceberg
        adapter_properties:
          catalog_linked_database: more_more_databases
          max_data_extension_time_in_days: 1
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "linked DB write-support should pass when external_volume present"
        );
    }

    #[test]
    fn strict_unknown_key_built_in_fails() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: built_in
        external_volume: ev
        adapter_properties:
          storage_serialisation_policy: COMPATIBLE   # typo (s/z)
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "unknown key in built_in adapter_properties must fail parse/validation"
        );
    }

    #[test]
    fn strict_unknown_key_rest_fails() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: iceberg_rest
        adapter_properties:
          foo: 1
"#;
        let v = parse_view_and_validate(yaml);
        assert!(
            v.is_err(),
            "unknown key in rest adapter_properties must fail"
        );
    }

    #[test]
    fn non_string_key_in_adapter_properities_fails() {
        let yaml = r#"
catalogs:
  - name: c1
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: built_in
        external_volume: ev
        adapter_properties: { 123: "bad" }
"#;
        let v = parse_view_and_validate(yaml);
        assert!(v.is_err(), "non-string key in adapter_properties must fail");
    }

    #[test]
    fn top_level_unknown_key_fails() {
        let yaml = r#"
    catalogs:
      - name: c1
        active_write_integration: i1
        write_integrations:
          - name: i1
            catalog_type: iceberg_rest
    extra_top_level: true
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "unknown key at top-level must fail parse");
    }

    #[test]
    fn catalog_spec_unknown_key_fails() {
        let yaml = r#"
    catalogs:
      - name: c1
        active_write_integration: i1
        write_integrations: []
        unexpected: 42
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "unknown key in catalog spec must fail parse");
    }

    #[test]
    fn write_integration_unknown_key_fails() {
        let yaml = r#"
    catalogs:
      - name: c1
        active_write_integration: i1
        write_integrations:
          - name: i1
            catalog_type: iceberg_rest
            oops: true
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "unknown key in write_integration must fail parse"
        );
    }

    #[test]
    fn catalog_alias_collision_fails() {
        let yaml = r#"
    catalogs:
      - catalog_name: c1
        name: also_c1
        active_write_integration: i1
        write_integrations:
          - name: i1
            catalog_type: iceberg_rest
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "providing both catalog_name and name must fail parse"
        );
    }

    #[test]
    fn integration_alias_collision_fails() {
        let yaml = r#"
    catalogs:
      - name: c1
        active_write_integration: i1
        write_integrations:
          - integration_name: i1
            name: also_i1
            catalog_type: iceberg_rest
    "#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            v.is_err(),
            "providing both integration_name and name must fail parse"
        );
    }

    #[test]
    fn built_in_blank_external_volume_fails() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: built_in
        external_volume: "  "
        table_format: iceberg
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let res = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(
            res.is_err(),
            "'external_volume' is required for BUILT_IN catalogs"
        );
    }

    #[test]
    fn rest_forbids_base_location_subpath() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: iceberg_rest
        base_location_subpath: foo/bar
        table_format: iceberg
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err());
    }

    #[test]
    fn negative_u32_fails_when_present() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: built_in
        external_volume: ev
        adapter_properties:
          data_retention_time_in_days: -1
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "negative u32 must fail parse");
    }

    #[test]
    fn non_bool_change_tracking_fails() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: built_in
        external_volume: ev
        adapter_properties:
          change_tracking: "yes"
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "non-bool change_tracking must fail");
    }

    #[test]
    fn adapter_properties_wrong_type_fails() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: iceberg_rest
        adapter_properties: []
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span);
        assert!(v.is_err(), "adapter_properties must be a mapping");
    }

    #[test]
    fn rest_target_file_size_requires_ext_vol() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: iceberg_rest
        adapter_properties:
          target_file_size: "64MB"
"#;
        let res = parse_view_and_validate(yaml);
        assert!(res.is_err(), "REST writer prop requires external_volume");
    }

    #[test]
    fn rest_linked_db_false_conflicts_with_target_file_size() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: iceberg_rest
        catalog_linked_database: false
        external_volume: ev
        adapter_properties:
          target_file_size: AUTO
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_err(),
            "writer properties not allowed when linked_db=false"
        );
    }

    #[test]
    fn duplicate_integration_names_fail() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i1
    write_integrations:
      - name: i1
        table_format: iceberg
        catalog_type: iceberg_rest
        adapter_properties:
            catalog_linked_database: cld
      - name: i1
        catalog_type: built_in
        table_format: iceberg
        external_volume: ev
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span).unwrap();
        let res = validate_catalogs(&v, Path::new("<test>"));
        assert!(res.is_err(), "duplicate integration names must fail");
    }

    #[test]
    fn duplicate_catalog_names_fail() {
        let yaml = r#"
catalogs:
  - name: c
    active_write_integration: i1
    write_integrations: [{ name: i1, catalog_type: iceberg_rest, table_format: iceberg}]
  - catalog_name: c
    active_write_integration: j1
    write_integrations: [{ name: j1, catalog_type: built_in, external_volume: ev, table_format: iceberg}]
"#;
        let yml_val: yml::Value = yml::from_str(yaml).unwrap();
        let yml_span = yml_val.span().clone();
        let m = yml_val
            .as_mapping()
            .expect("top-level YAML must be a mapping");
        let v = DbtCatalogsView::from_mapping(m, yml_span).unwrap();
        let res = validate_catalogs(&v, Path::new("<test>"));
        assert!(res.is_err(), "duplicate catalog names must fail");
    }

    // === Databricks: UNITY

    #[test]
    fn databricks_unity_min_valid_with_location_root() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: delta
        adapter_properties:
          location_root: /Volumes/org/area/lake
"#;
        let res = parse_view_and_validate(yaml);
        assert!(res.is_ok(), "unity(min) should validate: {:?}", res.err());
    }

    #[test]
    fn databricks_unity_min_valid_without_location_root() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: delta
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "unity(no adapter_props) should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn databricks_unity_rejects_non_delta_in_iceberg_mode() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: parquet
"#;
        assert_err_contains(
            yaml,
            "when table_format=iceberg, file_format must be 'delta'",
        );
    }

    #[test]
    fn databricks_unity_rejects_external_volume() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: delta
        external_volume: some_volume
"#;
        assert_err_contains(
            yaml,
            "Unknown key 'external_volume' in write_integration(Databricks unity)",
        );
    }

    #[test]
    fn databricks_unity_rejects_unknown_adapter_property_keys() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: delta
        adapter_properties:
          location_root: /Volumes/x
          extra_key: nope
"#;
        assert_err_contains(yaml, "Unknown key 'extra_key' in adapter_properties(unity)");
    }

    #[test]
    fn databricks_unity_rejects_blank_location_root() {
        let yaml = r#"
catalogs:
  - name: uc
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: unity
        table_format: iceberg
        file_format: delta
        adapter_properties:
          location_root: "   "
"#;
        assert_err_contains(yaml, "adapter_properties.location_root cannot be blank");
    }

    // === Databricks: HIVE_METASTORE

    #[test]
    fn databricks_hms_hudi_format() {
        let yaml = r#"
catalogs:
  - name: hms
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: hive_metastore
        table_format: default
        file_format: hudi
"#;
        assert!(parse_view_and_validate(yaml).is_ok());
    }

    #[test]
    fn databricks_hms_rejects_iceberg_format() {
        let yaml = r#"
catalogs:
  - name: hms
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: hive_metastore
        table_format: iceberg
        file_format: hudi
"#;
        assert_err_contains(
            yaml,
            "Databricks 'hive_metastore' requires table_format=DEFAULT",
        );
    }

    #[test]
    fn databricks_hms_rejects_adapter_properties_entirely() {
        let yaml = r#"
catalogs:
  - name: hms
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: hive_metastore
        table_format: iceberg
        file_format: delta
        adapter_properties:
          location_root: /mnt/should_not_be_here
"#;
        assert_err_contains(
            yaml,
            "Unknown key 'adapter_properties' in write_integration",
        );
    }

    #[test]
    fn databricks_hms_rejects_external_volume() {
        let yaml = r#"
catalogs:
  - name: hms
    active_write_integration: i
    write_integrations:
      - name: i
        catalog_type: hive_metastore
        table_format: iceberg
        file_format: delta
        external_volume: anything
"#;
        // External volume error may trigger first depending on parse order; check for either.
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains(
                "Unknown key 'external_volume' in write_integration(Databricks hive_metastore)"
            ),
            "unexpected error: {msg}"
        );
    }

    // === Bigquery: BIGLAKE_METASTORE

    #[test]
    fn bigquery_builtin_min_valid() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "biglake_metastore(min) should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn bigquery_builtin_with_base_location_root_adapter_property() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
        adapter_properties:
            base_location_root: _not_dbt
"#;
        let res = parse_view_and_validate(yaml);
        assert!(
            res.is_ok(),
            "biglake_metastore(min) should validate: {:?}",
            res.err()
        );
    }

    #[test]
    fn bigquery_builtin_rejects_blank_base_location_root() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
        adapter_properties:
            base_location_root: ""
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(msg.contains("cannot be blank"), "unexpected error: {msg}");
    }

    #[test]
    fn bigquery_builtin_rejects_base_location_root_at_top_level() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        base_location_root: _not_dbt
        catalog_type: biglake_metastore
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains(
                "'base_location_root' must be set under 'adapter_properties', not at the top level."
            ),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_base_location_subpath_at_top_level() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        base_location_subpath: sub/path
        catalog_type: biglake_metastore
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains("'base_location_subpath' must be set under 'adapter_properties'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_base_location_subpath_adapter_property() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
        adapter_properties:
            base_location_root: _not_dbt
            base_location_subpath: sub/path
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains(
                "'base_location_subpath' must be set under 'adapter_properties' in a model config"
            ),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_storage_uri_adapter_property() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
        adapter_properties:
            storage_uri: "gs://bucket/cool/path"
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains("'storage_uri' must be set under 'adapter_properties' in a model config"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_blank_external_volume() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: ""
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains(
                "'external_volume' is required for biglake_metastore catalogs and cannot be blank."
            ),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_invalid_gs_path() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
"#;
        let res = parse_view_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got OK");
        assert!(
            msg.contains("must be a path to a Cloud Storage bucket"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn bigquery_builtin_rejects_unknown_adapter_property_keys() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: parquet
        catalog_type: biglake_metastore
        adapter_properties:
          auto_refresh: true
"#;
        assert_err_contains(
            yaml,
            "Unknown key 'auto_refresh' in adapter_properties(biglake_metastore)",
        );
    }

    #[test]
    fn bigquery_builtin_missing_file_format_fails() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        catalog_type: biglake_metastore
"#;
        assert_err_contains(
            yaml,
            "file_format required for Bigquery biglake_metastore (must be parquet)",
        );
    }

    #[test]
    fn bigquery_builtin_unsupported_file_format_fails() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: iceberg
        file_format: delta
        catalog_type: biglake_metastore
"#;
        assert_err_contains(yaml, "file_format 'delta' invalid. choose one of (parquet)");
    }

    #[test]
    fn bigquery_builtin_missing_table_format_fails() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        file_format: parquet
        catalog_type: biglake_metastore
"#;
        assert_err_contains(
            yaml,
            "table_format required for Bigquery biglake_metastore (must be iceberg)",
        );
    }

    #[test]
    fn bigquery_builtin_invalid_table_format_fails() {
        let yaml = r#"
catalogs:
  - name: bq
    active_write_integration: biglake
    write_integrations:
      - name: biglake
        external_volume: "gs://bucket"
        table_format: default
        file_format: parquet
        catalog_type: biglake_metastore
"#;
        assert_err_contains(
            yaml,
            "Bigquery biglake_metastore requires table_format=iceberg",
        );
    }

    // === v2 cache methods ===

    fn make_v2_catalogs(yaml: &str) -> DbtCatalogs {
        let v: yml::Value = yml::from_str(yaml).unwrap();
        let (repr, span) = match v {
            yml::Value::Mapping(m, s) => (m, s),
            _ => panic!("expected mapping"),
        };
        DbtCatalogs::new(repr, span)
    }

    #[test]
    fn is_v2_catalog_finds_all_names() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "GLUE_DB"
  - name: horizon_cat
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: ev
"#,
        );
        assert!(c.is_v2_catalog("glue_cat").unwrap());
        assert!(c.is_v2_catalog("horizon_cat").unwrap());
        assert!(!c.is_v2_catalog("nonexistent").unwrap());
    }

    #[test]
    fn v1_catalog_names_finds_all_names() {
        let yaml = r#"
catalogs:
  - catalog_name: sf_native
    active_write_integration: sf_native_int
    write_integrations:
      - name: sf_native_int
        catalog_type: built_in
        table_format: iceberg
        external_volume: dbt_external_volume
  - name: polaris
    active_write_integration: polaris_int
    write_integrations:
      - name: polaris_int
        catalog_type: iceberg_rest
        table_format: iceberg
"#;
        let v: yml::Value = yml::from_str(yaml).unwrap();
        let (repr, span) = match v {
            yml::Value::Mapping(m, s) => (m, s),
            _ => panic!("expected mapping"),
        };
        let c = DbtCatalogs::new(repr, span);
        assert_eq!(
            c.v1_catalog_names().unwrap(),
            &["sf_native".to_string(), "polaris".to_string()]
        );
    }

    #[test]
    fn loaded_catalogs_none_has_no_catalog_names() {
        let state = LoadedCatalogs::None;
        assert_eq!(state.catalog_names().unwrap(), None);
        assert!(!state.has_catalog_name("anything").unwrap());
    }

    #[test]
    fn is_v2_catalog_database_collects_snowflake_catalog_databases() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "GLUE_DB"
  - name: unity_cat
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "UNITY_DB"
      databricks:
        file_format: delta
  - name: horizon_cat
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: ev
"#,
        );
        assert!(c.is_v2_catalog_database("GLUE_DB").unwrap());
        assert!(c.is_v2_catalog_database("UNITY_DB").unwrap());
        assert!(!c.is_v2_catalog_database("horizon_cat").unwrap());
    }

    #[test]
    fn is_v2_catalog_database_false_when_no_snowflake_blocks() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: hive
    type: hive_metastore
    table_format: default
    config:
      databricks:
        file_format: delta
"#,
        );
        assert!(!c.is_v2_catalog_database("anything").unwrap());
    }

    #[test]
    fn is_v2_catalog_database_false_for_horizon() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: ev
"#,
        );
        assert!(!c.is_v2_catalog_database("sf_native").unwrap());
    }

    #[test]
    fn v2_caches_populated_together_on_first_call() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: uc
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "CD"
"#,
        );
        assert!(c.is_v2_catalog("uc").unwrap());
        assert!(c.is_v2_catalog_database("CD").unwrap());
    }

    #[test]
    fn iceberg_rest_catalog_databases_collects_only_iceberg_rest() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: mdls_horizon
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "DBT_COMPUTE_STAGING"
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "GLUE_DB"
"#,
        );
        let dbs = c.iceberg_rest_catalog_databases().unwrap();
        assert_eq!(
            dbs,
            vec![(
                "mdls_horizon".to_string(),
                "DBT_COMPUTE_STAGING".to_string()
            )]
        );
    }

    #[test]
    fn iceberg_rest_catalog_databases_empty_when_no_snowflake_block() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: mdls_horizon
    type: iceberg_rest
    table_format: iceberg
    config: {}
"#,
        );
        assert!(c.iceberg_rest_catalog_databases().unwrap().is_empty());
    }

    #[test]
    fn iceberg_rest_catalog_databases_empty_when_no_rest_catalogs() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: ev
"#,
        );
        assert!(c.iceberg_rest_catalog_databases().unwrap().is_empty());
    }

    #[test]
    fn catalogs_matching_database_default_table_format_is_excluded() {
        let c = make_v2_catalogs(
            r#"
catalogs:
  - name: horizon_plain
    type: horizon
    table_format: default
    config:
      duckdb:
        endpoint: "https://horizon.example.com"
        catalog_database: "horizon_db"
"#,
        );
        let matches = c
            .catalogs_matching_database("horizon_db", AdapterType::DuckDB)
            .unwrap();
        assert!(matches.is_empty(), "matches was: {matches:?}");
    }
}
