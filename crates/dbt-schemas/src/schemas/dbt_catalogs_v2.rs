//! catalogs.yml v2 — table-driven validation and JSON schema generation.
//!
//! ```yaml
//! catalogs:
//!   - name: <string>
//!     type: <catalog type>
//!     table_format: <iceberg|default>
//!     config:
//!       <platform>:
//!         # per-platform fields — see CATALOG_SCHEMAS + FieldSpec arrays below,
//!         # or run `dbt man --schema catalog` for the canonical JSON schema.
//! ```

use std::collections::HashSet;
use std::path::Path;

use super::dbt_catalogs::DbtCatalogs;
use dbt_common::serde_utils::try_get_bool;
use dbt_common::{ErrorCode, FsResult, err, fs_err};
use dbt_yaml::{self as yml};

const ALL_V2_PLATFORMS: &[&str] = &[
    "snowflake",
    "databricks",
    "bigquery",
    "duckdb",
    "lakecompute",
];

const TARGET_FILE_SIZES: &[&str] = &["AUTO", "16MB", "32MB", "64MB", "128MB"];
const STORAGE_SERIALIZATION_POLICIES: &[&str] = &["COMPATIBLE", "OPTIMIZED"];
const DUCKDB_ENDPOINT_TYPES: &[&str] = &["GLUE", "S3_TABLES"];
const DUCKDB_AUTHORIZATION_TYPES: &[&str] = &["OAUTH2", "SIGV4", "NONE"];
const DUCKDB_ACCESS_DELEGATION_MODES: &[&str] = &["VENDED_CREDENTIALS", "NONE"];
const LOCAL_FS_FILE_FORMATS: &[&str] = &["parquet", "csv", "json"];
// unity only supports delta (with UniForm) or parquet; `hudi` is rejected for
// unity by `validate_unity_semantics`, so it is excluded here and from the
// published JSON schema. hive_metastore does accept hudi (separate const).
const UNITY_DATABRICKS_FILE_FORMATS: &[&str] = &["delta", "parquet"];
const HIVE_METASTORE_FILE_FORMATS: &[&str] = &["delta", "parquet", "hudi"];
const BIGLAKE_FILE_FORMATS: &[&str] = &["parquet"];

fn matches_enum_ci(v: &str, allowed: &[&str]) -> bool {
    allowed.iter().any(|a| v.eq_ignore_ascii_case(a))
}

#[derive(Debug, Clone, Copy)]
enum FieldKind {
    Str,
    Bool,
    U32 { max: Option<u32> },
    Enum(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
struct FieldSpec {
    name: &'static str,
    kind: FieldKind,
    required: bool,
    non_empty: bool,
    forbidden: bool,
    doc: &'static str,
}

impl FieldSpec {
    const fn new(name: &'static str, kind: FieldKind) -> Self {
        Self {
            name,
            kind,
            required: false,
            non_empty: false,
            forbidden: false,
            doc: "",
        }
    }
    const fn string(name: &'static str) -> Self {
        Self::new(name, FieldKind::Str)
    }
    const fn boolean(name: &'static str) -> Self {
        Self::new(name, FieldKind::Bool)
    }
    const fn u32_plain(name: &'static str) -> Self {
        Self::new(name, FieldKind::U32 { max: None })
    }
    const fn u32_max(name: &'static str, max: u32) -> Self {
        Self::new(name, FieldKind::U32 { max: Some(max) })
    }
    const fn enumerated(name: &'static str, allowed: &'static [&'static str]) -> Self {
        Self::new(name, FieldKind::Enum(allowed))
    }
    const fn required(mut self) -> Self {
        self.required = true;
        self
    }
    const fn non_empty(mut self) -> Self {
        self.non_empty = true;
        self
    }
    const fn forbidden(mut self) -> Self {
        self.forbidden = true;
        self
    }
    const fn doc(mut self, doc: &'static str) -> Self {
        self.doc = doc;
        self
    }
}

const HORIZON_SNOWFLAKE_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("external_volume")
        .required()
        .non_empty()
        .doc("Non-empty external volume name registered in Snowflake."),
    FieldSpec::string("catalog_database")
        .non_empty()
        .doc("Snowflake database where models using this catalog should land. When set, takes precedence over model database config and target.database."),
    FieldSpec::boolean("change_tracking"),
    FieldSpec::u32_max("data_retention_time_in_days", 90)
        .doc("Days to retain table data after deletion. Range 0–90."),
    FieldSpec::u32_max("max_data_extension_time_in_days", 90)
        .doc("Days beyond the retention period to extend data availability. Range 0–90."),
    FieldSpec::enumerated("storage_serialization_policy", STORAGE_SERIALIZATION_POLICIES),
    FieldSpec::u32_plain("iceberg_version").doc("Iceberg spec version, e.g. 3 for Iceberg V3."),
    FieldSpec::string("base_location_root")
        .non_empty()
        .doc("Catalog-wide storage path prefix for all Iceberg tables."),
    FieldSpec::string("base_location_subpath")
        .forbidden()
        .doc("Catalog '{}' horizon/snowflake base_location_subpath is model-config only and may not be specified in catalogs.yml"),
];

const HORIZON_DATABRICKS_FIELDS: &[FieldSpec] = &[FieldSpec::string("catalog_database")
    .required()
    .non_empty()
    .doc("Name of the Databricks database linked to the external Horizon catalog.")];

const LINKED_SNOWFLAKE_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("catalog_database")
        .required()
        .non_empty()
        .doc("Name of the Snowflake database linked to the external catalog."),
    FieldSpec::boolean("auto_refresh"),
    FieldSpec::u32_max("max_data_extension_time_in_days", 90)
        .doc("Days beyond the retention period to extend data availability. Range 0–90."),
    FieldSpec::enumerated("target_file_size", TARGET_FILE_SIZES),
    FieldSpec::u32_plain("iceberg_version").doc("Iceberg spec version, e.g. 3 for Iceberg V3."),
];

// Direct AWS creds for the lake compute backend, which signs Glue's
// Iceberg REST endpoint itself (SigV4) server-side rather than attaching via
// a local DuckDB secret -- so it needs the raw credential fields, not a
// `secret`/`endpoint_type` reference like DUCKDB_ICEBERG_FIELDS.
const GLUE_LAKE_COMPUTE_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("catalog_id")
        .required()
        .non_empty()
        .doc("Glue catalog identifier passed to ATTACH. Accepted forms: a 12-digit AWS account ID, ':' for the caller's own account, 'catalog1/catalog2', or '<account_id>:catalog1/catalog2'."),
    FieldSpec::string("region")
        .required()
        .non_empty()
        .doc("AWS region of the Glue catalog. Determines the REST endpoint and SigV4 signing region."),
    FieldSpec::string("access_key_id")
        .required()
        .non_empty()
        .doc("AWS access key ID."),
    FieldSpec::string("secret_access_key")
        .required()
        .non_empty()
        .doc("AWS secret access key."),
    FieldSpec::string("session_token")
        .non_empty()
        .doc("AWS session token; only for temporary/STS credentials."),
];

const DUCKDB_ICEBERG_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("endpoint")
        .non_empty()
        .doc("Full REST catalog URL. Mutually exclusive with endpoint_type."),
    FieldSpec::enumerated("endpoint_type", DUCKDB_ENDPOINT_TYPES)
        .doc("Managed AWS endpoint type. DuckDB derives the endpoint URL and SigV4 authorization from it, so it's mutually exclusive with both endpoint and authorization_type."),
    FieldSpec::string("warehouse")
        .non_empty()
        .doc("Warehouse identifier used as the ATTACH source: the S3 Tables bucket ARN (required when endpoint_type is S3_TABLES), or the Glue catalog path (defaults to ':', the current account's default catalog)."),
    FieldSpec::string("secret")
        .non_empty()
        .doc("Name of a DuckDB secret from profiles.yml to use for authentication."),
    FieldSpec::string("catalog_database").non_empty(),
    FieldSpec::string("default_region").non_empty(),
    FieldSpec::string("default_schema").non_empty(),
    FieldSpec::string("max_table_staleness").non_empty(),
    FieldSpec::enumerated("authorization_type", DUCKDB_AUTHORIZATION_TYPES),
    FieldSpec::enumerated("access_delegation_mode", DUCKDB_ACCESS_DELEGATION_MODES),
    FieldSpec::boolean("support_nested_namespaces"),
    FieldSpec::boolean("support_stage_create"),
    FieldSpec::boolean("purge_requested"),
    FieldSpec::boolean("encode_entire_prefix"),
    FieldSpec::boolean("read_only")
        .doc("Attach the catalog read-only. Read-write (read_only: false) on Horizon/Unity requires DuckDB 1.5.4 / duckdb-iceberg#1017."),
    // Write-compat ATTACH options for managed-storage Iceberg REST (Horizon/Unity);
    // these require DuckDB 1.5.4 / duckdb-iceberg#1017.
    FieldSpec::boolean("stage_create_tables")
        .doc("Opt into staged CREATE TABLE AS SELECT writes (direct CTAS into the target catalog)."),
    FieldSpec::boolean("disable_multi_table_commit")
        .doc("Disable multi-table commits (Unity write-compat default)."),
    FieldSpec::boolean("skip_create_table_metadata_updates")
        .doc("Skip metadata updates on CREATE TABLE (write-compat)."),
    FieldSpec::boolean("remove_files_on_delete")
        .doc("Remove underlying data files when a table is dropped (write-compat)."),
];

const UNITY_DATABRICKS_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("catalog_database")
        .non_empty()
        .doc("Name of the Databricks Unity Catalog to use as the database for models. When set, takes precedence over catalog name for database routing."),
    FieldSpec::enumerated("file_format", UNITY_DATABRICKS_FILE_FORMATS).required(),
    FieldSpec::string("location_root").non_empty(),
    FieldSpec::boolean("use_uniform")
        .doc("UniForm mode. true requires file_format: delta; false (default) requires parquet."),
];

const HIVE_METASTORE_DATABRICKS_FIELDS: &[FieldSpec] =
    &[FieldSpec::enumerated("file_format", HIVE_METASTORE_FILE_FORMATS).required()];

const BIGLAKE_BIGQUERY_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("external_volume")
        .required()
        .non_empty()
        .doc("Cloud Storage bucket path (gs://<bucket_name>)."),
    FieldSpec::enumerated("file_format", BIGLAKE_FILE_FORMATS).required(),
    FieldSpec::string("catalog_database")
        .non_empty()
        .doc("GCP project where models using this catalog should land. When set, takes precedence over model database config and target.database."),
    FieldSpec::string("lakehouse_catalog")
        .non_empty()
        .doc("Lakehouse Runtime Catalog (LRC) catalog name. When set, models using this catalog render a 4-part BigQuery FQN (project.lakehouse_catalog.namespace.table) instead of the standard 3-part project.dataset.table."),
    FieldSpec::string("base_location_root").non_empty(),
    FieldSpec::string("connection_id").non_empty(),
];

const DUCKLAKE_DUCKDB_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("metadata_path").required().non_empty(),
    FieldSpec::string("data_path").non_empty(),
    FieldSpec::string("catalog_database").non_empty(),
    FieldSpec::string("metadata_schema").non_empty(),
    FieldSpec::string("metadata_catalog").non_empty(),
    FieldSpec::u32_plain("data_inlining_row_limit")
        .doc("Inline row groups smaller than this many rows into the metadata catalog."),
    FieldSpec::boolean("create_if_not_exists"),
    FieldSpec::boolean("read_only"),
    FieldSpec::boolean("encrypted"),
    FieldSpec::boolean("automatic_migration"),
    FieldSpec::boolean("override_data_path"),
];

const LOCAL_FILESYSTEM_DUCKDB_FIELDS: &[FieldSpec] = &[
    FieldSpec::string("root_path").required().non_empty(),
    FieldSpec::enumerated("file_format", LOCAL_FS_FILE_FORMATS)
        .doc("Local file extension / DuckDB COPY format. Defaults to parquet."),
];

#[derive(Debug, Clone, Copy)]
struct PlatformBlock {
    key: &'static str,
    fields: &'static [FieldSpec],
}

impl PlatformBlock {
    const fn new(key: &'static str, fields: &'static [FieldSpec]) -> Self {
        Self { key, fields }
    }
}

#[derive(Debug, Clone, Copy)]
enum ConfigPresence {
    AllRequired,
    AtLeastOne,
}

#[derive(Debug, Clone, Copy)]
struct CatalogTypeSchema {
    catalog_type: CatalogType,
    table_format: &'static str,
    description: &'static str,
    presence: ConfigPresence,
    platforms: &'static [PlatformBlock],
}

const CATALOG_SCHEMAS: &[CatalogTypeSchema] = &[
    CatalogTypeSchema {
        catalog_type: CatalogType::Horizon,
        table_format: "iceberg",
        description: "Snowflake-managed Iceberg catalog (Horizon). Supports snowflake (native), databricks, and/or duckdb (read-only attach) connection blocks.",
        presence: ConfigPresence::AtLeastOne,
        platforms: &[
            PlatformBlock::new("snowflake", HORIZON_SNOWFLAKE_FIELDS),
            PlatformBlock::new("databricks", HORIZON_DATABRICKS_FIELDS),
            PlatformBlock::new("duckdb", DUCKDB_ICEBERG_FIELDS),
            PlatformBlock::new("lakecompute", DUCKDB_ICEBERG_FIELDS),
        ],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::Glue,
        table_format: "iceberg",
        description: "AWS Glue catalog. Supports snowflake and/or duckdb connection blocks.",
        presence: ConfigPresence::AtLeastOne,
        platforms: &[
            PlatformBlock::new("snowflake", LINKED_SNOWFLAKE_FIELDS),
            PlatformBlock::new("duckdb", DUCKDB_ICEBERG_FIELDS),
            PlatformBlock::new("lakecompute", GLUE_LAKE_COMPUTE_FIELDS),
        ],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::IcebergRest,
        table_format: "iceberg",
        description: "Iceberg REST catalog. Supports snowflake and/or duckdb connection blocks.",
        presence: ConfigPresence::AtLeastOne,
        platforms: &[
            PlatformBlock::new("snowflake", LINKED_SNOWFLAKE_FIELDS),
            PlatformBlock::new("duckdb", DUCKDB_ICEBERG_FIELDS),
            PlatformBlock::new("lakecompute", DUCKDB_ICEBERG_FIELDS),
        ],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::Unity,
        table_format: "iceberg",
        description: "Databricks Unity catalog. Supports snowflake, databricks, and/or duckdb (read-only attach) connection blocks.",
        presence: ConfigPresence::AtLeastOne,
        platforms: &[
            PlatformBlock::new("snowflake", LINKED_SNOWFLAKE_FIELDS),
            PlatformBlock::new("databricks", UNITY_DATABRICKS_FIELDS),
            PlatformBlock::new("duckdb", DUCKDB_ICEBERG_FIELDS),
            PlatformBlock::new("lakecompute", DUCKDB_ICEBERG_FIELDS),
        ],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::HiveMetastore,
        table_format: "default",
        description: "Databricks Hive Metastore catalog. Databricks platform only.",
        presence: ConfigPresence::AllRequired,
        platforms: &[PlatformBlock::new(
            "databricks",
            HIVE_METASTORE_DATABRICKS_FIELDS,
        )],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::BiglakeMetastore,
        table_format: "iceberg",
        description: "BigLake Metastore catalog. BigQuery platform only.",
        presence: ConfigPresence::AllRequired,
        platforms: &[PlatformBlock::new("bigquery", BIGLAKE_BIGQUERY_FIELDS)],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::DuckLake,
        table_format: "default",
        description: "DuckLake metadata store catalog. Supports duckdb and/or lakecompute connection blocks.",
        presence: ConfigPresence::AtLeastOne,
        platforms: &[
            PlatformBlock::new("duckdb", DUCKLAKE_DUCKDB_FIELDS),
            PlatformBlock::new("lakecompute", DUCKLAKE_DUCKDB_FIELDS),
        ],
    },
    CatalogTypeSchema {
        catalog_type: CatalogType::LocalFilesystem,
        table_format: "default",
        description: "Local filesystem catalog. DuckDB platform only.",
        presence: ConfigPresence::AllRequired,
        platforms: &[PlatformBlock::new("duckdb", LOCAL_FILESYSTEM_DUCKDB_FIELDS)],
    },
];

// ===== YAML helpers =====

trait StrExt {
    fn is_empty_or_whitespace(&self) -> bool;
}

impl StrExt for str {
    #[inline]
    fn is_empty_or_whitespace(&self) -> bool {
        self.trim().is_empty()
    }
}

fn get_str<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<&'a str>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::String(s, _) => Ok(Some(s.trim())),
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

fn get_map<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<&'a yml::Mapping>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::Mapping(map, _) => Ok(Some(map)),
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

/// Validates the catalog-level `meta:` field.
fn validate_meta_shape(m: &yml::Mapping, k: &str) -> FsResult<()> {
    if let Some(map) = get_map(m, k)?
        && let Some(key) = map.keys().find(|key| key.as_str().is_none())
    {
        return err!(
            code => ErrorCode::InvalidConfig,
            hacky_yml_loc => Some(key.span().clone()),
            "Non-string key in '{}' mapping",
            k
        );
    }
    Ok(())
}

fn get_seq<'a>(m: &'a yml::Mapping, k: &str) -> FsResult<Option<&'a yml::Sequence>> {
    match m.get(yml::Value::from(k)) {
        Some(v) => match v {
            yml::Value::Sequence(seq, _) => Ok(Some(seq)),
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

fn get_u32(m: &yml::Mapping, k: &str) -> FsResult<Option<u32>> {
    m.get(yml::Value::from(k))
        .map(|v| match v {
            yml::Value::Number(n, span) => n
                .as_i64()
                .and_then(|i| u32::try_from(i).ok())
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

fn field_span<'a>(m: &'a yml::Mapping, k: &str) -> Option<&'a yml::Span> {
    m.get(yml::Value::from(k)).map(|v| v.span())
}

fn check_unknown_keys(m: &yml::Mapping, allowed: &[&str], ctx: &str) -> FsResult<()> {
    for k in m.keys() {
        let span = k.span();
        let Some(ks) = k.as_str() else {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span.clone()),
                "Non-string key in {}",
                ctx
            );
        };
        if !allowed.iter().any(|a| a == &ks) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span.clone()),
                "Unknown key '{}' in {}",
                ks,
                ctx
            );
        }
    }
    Ok(())
}

fn key_err(key: &str, err_span: Option<&yml::Span>) -> Box<dbt_common::FsError> {
    fs_err!(
        code => ErrorCode::InvalidConfig,
        hacky_yml_loc => err_span.cloned(),
        "Missing required key '{}' in catalogs.yml",
        key
    )
}

fn require_mapping<'a>(value: &'a yml::Value, ctx: &str) -> FsResult<&'a yml::Mapping> {
    value.as_mapping().ok_or_else(|| {
        fs_err!(
            code => ErrorCode::InvalidConfig,
            hacky_yml_loc => Some(value.span().clone()),
            "{} must be a mapping",
            ctx
        )
    })
}

// ===== Loader Handoff =====

impl DbtCatalogs {
    /// Rebuild a zero-copy typed v2 view over the raw YAML mapping.
    pub fn view_v2(&self) -> FsResult<DbtCatalogsV2View<'_>> {
        DbtCatalogsV2View::from_mapping(&self.repr, &self.span)
    }
}

// ===== Phase 1: Shape Validation =====
// Preconditions:
// - YAML has been loaded and parsed by the caller.
// Postconditions:
// - Document matches the strict v2 envelope: only known top-level and
//   per-entry keys, all required keys present, no duplicate names,
//   config and platform blocks are mappings.
pub fn validate_catalogs_v2_shape(map: &yml::Mapping, span: &yml::Span) -> FsResult<()> {
    if map.get(yml::Value::from("iceberg_catalogs")).is_some() {
        return err!(
            code => ErrorCode::InvalidConfig,
            hacky_yml_loc => Some(span.clone()),
            "catalogs.yml v2 uses the key 'catalogs:', not 'iceberg_catalogs:'"
        );
    }

    check_unknown_keys(map, &["catalogs"], "top-level catalogs.yml(v2)").map_err(|_| {
        fs_err!(
            code => ErrorCode::InvalidConfig,
            hacky_yml_loc => Some(span.clone()),
            "catalogs.yml v2 accepts only a top-level 'catalogs:' key"
        )
    })?;

    let catalogs = get_seq(map, "catalogs")?.ok_or_else(|| fs_err!(
        code => ErrorCode::InvalidConfig,
        hacky_yml_loc => Some(span.clone()),
        "catalogs.yml requires a 'catalogs:' list, e.g.:\n  catalogs:\n    - name: my_catalog\n      type: horizon\n      table_format: iceberg\n      config:\n        snowflake: {{ ... }}"
    ))?;
    let mut seen_catalog_names = HashSet::new();

    for (idx, item) in catalogs.iter().enumerate() {
        let item_span = item.span();
        let catalog = require_mapping(item, &format!("catalogs[{idx}]"))
            .map_err(|_| fs_err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(item_span.clone()),
                "Each entry under 'catalogs:' must be a mapping with name, type, table_format, and config"
            ))?;

        check_unknown_keys(
            catalog,
            &[
                "name",
                "type",
                "table_format",
                "config",
                "description",
                "owner",
                "meta",
            ],
            "catalog entry",
        )?;

        for required in ["name", "type", "table_format", "config"] {
            if !catalog.contains_key(yml::Value::from(required)) {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(item_span.clone()),
                    "Catalog entry is missing '{}'. Each entry requires: name, type, table_format, config",
                    required
                );
            }
        }

        let name = get_str(catalog, "name")?.ok_or_else(|| key_err("name", Some(item_span)))?;
        if name.is_empty_or_whitespace() {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(catalog, "name").cloned(),
                "Catalog name must be a non-empty string"
            );
        }
        if let Some(description) = get_str(catalog, "description")?
            && description.is_empty_or_whitespace()
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(catalog, "description").cloned(),
                "Supply a value for the 'description' field in catalog '{}' or consider removing it",
                name
            );
        }
        if let Some(owner) = get_str(catalog, "owner")?
            && owner.is_empty_or_whitespace()
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(catalog, "owner").cloned(),
                "Supply a value for the 'owner' field in catalog '{}' or consider removing it",
                name
            );
        }
        validate_meta_shape(catalog, "meta")?;
        if !seen_catalog_names.insert(name) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(catalog, "name").cloned(),
                "Duplicate catalog name '{}', each catalog must have a unique name",
                name
            );
        }

        if let Some(value) = catalog.get(yml::Value::from("config")) {
            let config = require_mapping(value, &format!("catalogs[{idx}].config"))
                .map_err(|_| fs_err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(value.span().clone()),
                    "Catalog '{}' config must be a mapping of platform blocks (e.g. snowflake:, duckdb:, databricks:, bigquery:)",
                    name
                ))?;
            for &platform in ALL_V2_PLATFORMS {
                if let Some(platform_value) = config.get(yml::Value::from(platform)) {
                    require_mapping(platform_value, platform)
                        .map_err(|_| fs_err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => Some(platform_value.span().clone()),
                            "Catalog '{}' config.{} must be a mapping of key-value configuration fields",
                            name, platform
                        ))?;
                }
            }
        }
    }

    Ok(())
}

// ===== Phases 2+3: Structural + Semantic Validation =====
// Preconditions:
// - Phase 1 shape validation has passed.
// - Raw YAML envelope is well-formed: required keys present, config/platform
//   blocks are mappings, no duplicate names.
// Phase 2 (structural): table_format matches type, platform keys are known and
//   allowed for this type, platform presence (AllRequired/AtLeastOne), per-field
//   type checking, requiredness, non-empty, enum membership, forbidden fields.
// Phase 3 (semantic): cross-field constraints that span multiple fields within
//   a platform block.
// Postconditions:
// - Every catalog entry is fully valid for its type and platform mix.

// Two planes render CatalogType, in different casings. The legacy Jinja/
// relation-config plane string-compares a few types (Iceberg-on-Snowflake,
// BigQuery/Snowflake INFO_SCHEMA) against exact uppercase literals inherited
// from pre-v2 code. Everywhere else, YAML config, diagnostics, logs, spells
// types lowercase. Diagnostic/log call sites should to_lowercase() on egress
// so they don't leak the Jinja plane's casing into user-facing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CatalogType {
    Horizon,
    Glue,
    IcebergRest,
    HiveMetastore,
    Unity,
    BiglakeMetastore,
    DuckLake,
    LocalFilesystem,

    // Has no explicit v2 catalog.
    SnowflakeBuiltIn, // superceded by Horizon above
    // native == default platform storage
    SnowflakeNative,
    BigqueryNative,
    DuckdbNative,
}

impl CatalogType {
    fn parse(raw: &str, span: &yml::Span) -> FsResult<Self> {
        if raw.eq_ignore_ascii_case("horizon") {
            Ok(Self::Horizon)
        } else if raw.eq_ignore_ascii_case("glue") {
            Ok(Self::Glue)
        } else if raw.eq_ignore_ascii_case("iceberg_rest") {
            Ok(Self::IcebergRest)
        } else if raw.eq_ignore_ascii_case("hive_metastore") {
            Ok(Self::HiveMetastore)
        } else if raw.eq_ignore_ascii_case("unity") {
            Ok(Self::Unity)
        } else if raw.eq_ignore_ascii_case("biglake_metastore") {
            Ok(Self::BiglakeMetastore)
        } else if raw.eq_ignore_ascii_case("ducklake") {
            Ok(Self::DuckLake)
        } else if raw.eq_ignore_ascii_case("local_filesystem") {
            Ok(Self::LocalFilesystem)
        } else {
            err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span.clone()),
                "type '{}' invalid. choose one of (horizon|glue|iceberg_rest|unity|hive_metastore|biglake_metastore|ducklake|local_filesystem)",
                raw
            )
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Horizon => "horizon",
            Self::Glue => "glue",
            Self::IcebergRest => "ICEBERG_REST", // required by legacy dbt core v1 Jinja macros
            Self::HiveMetastore => "hive_metastore",
            Self::Unity => "unity",
            Self::BiglakeMetastore => "biglake_metastore",
            Self::DuckLake => "ducklake",
            Self::LocalFilesystem => "local_filesystem",
            Self::SnowflakeBuiltIn => "BUILT_IN",
            Self::SnowflakeNative => "INFO_SCHEMA",
            Self::BigqueryNative => "INFO_SCHEMA",
            Self::DuckdbNative => "duckdb",
        }
    }

    /// Whether `lakecompute` can read a catalog of this type.
    ///
    /// A capability of `lakecompute`, expressed here in code: it is a property of the
    /// storage, not of anything a project declares. Requiring a catalog to carry a
    /// `lakecompute` connection block would reject readable data over a missing
    /// declaration.
    ///
    /// Matched exhaustively on purpose, so adding a `CatalogType` forces the
    /// question to be answered rather than defaulting either way.
    pub fn lake_compute_can_read(&self) -> bool {
        match self {
            // Open table formats `lakecompute` can attach.
            Self::Horizon | Self::Glue | Self::IcebergRest => true,
            // Snowflake-managed Iceberg under its older spelling; Horizon supersedes it.
            Self::SnowflakeBuiltIn => true,
            // Engine-owned catalogs `lakecompute` does not support today.
            Self::DuckLake
            | Self::LocalFilesystem
            | Self::BiglakeMetastore
            | Self::Unity
            | Self::HiveMetastore => false,
            // Native platform storage is not readable by an external engine at all --
            // this is the warehouse-native case the check exists to catch.
            Self::SnowflakeNative | Self::BigqueryNative | Self::DuckdbNative => false,
        }
    }

    /// Whether this catalog type represents a customer-owned catalog outside the
    /// warehouse's own storage, as opposed to the warehouse's native/managed storage.
    ///
    /// Matched exhaustively on purpose, so adding a `CatalogType` forces the
    /// question to be answered rather than defaulting either way.
    pub fn is_catalog_linked(&self) -> bool {
        match self {
            Self::Glue
            | Self::IcebergRest
            | Self::HiveMetastore
            | Self::Unity
            | Self::BiglakeMetastore
            | Self::DuckLake
            | Self::LocalFilesystem => true,
            // Snowflake-managed Iceberg, current (Horizon) and superseded
            // (SnowflakeBuiltIn) spellings -- see lake_compute_can_read's comment on
            // the same pairing -- plus non-Iceberg native storage.
            Self::Horizon
            | Self::SnowflakeBuiltIn
            | Self::SnowflakeNative
            | Self::BigqueryNative
            | Self::DuckdbNative => false,
        }
    }

    pub fn parse_from_str(raw: &str, adapter_type: dbt_adapter_core::AdapterType) -> Self {
        match raw {
            "horizon" => Self::Horizon,
            "glue" => Self::Glue,
            // "ICEBERG_REST" only exists on the Jinja plane. Model configs should
            // be lowercase to obscure the internal uppercase as much as possible.
            "iceberg_rest" => Self::IcebergRest,
            "hive_metastore" => Self::HiveMetastore,
            "unity" => Self::Unity,
            "biglake_metastore" => Self::BiglakeMetastore,
            "ducklake" => Self::DuckLake,
            "local_filesystem" => Self::LocalFilesystem,
            "BUILT_IN" => Self::SnowflakeBuiltIn,
            "duckdb" => Self::DuckdbNative,
            "INFO_SCHEMA" => match adapter_type {
                dbt_adapter_core::AdapterType::Bigquery => Self::BigqueryNative,
                dbt_adapter_core::AdapterType::Snowflake => Self::SnowflakeNative,
                _ => unreachable!("only Snowflake/Bigquery ever egress INFO_SCHEMA"),
            },
            _ => unreachable!("not a CatalogType::as_str() output"),
        }
    }
}

impl serde::Serialize for CatalogType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TableFormat {
    Default,
    Iceberg,
}

impl TableFormat {
    pub(crate) fn parse_from_yaml(raw: &str, span: &yml::Span) -> FsResult<Self> {
        if raw.eq_ignore_ascii_case("default") {
            Ok(Self::Default)
        } else if raw.eq_ignore_ascii_case("iceberg") {
            Ok(Self::Iceberg)
        } else {
            err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(span.clone()),
                "table_format '{}' invalid. choose one of ({})",
                raw,
                Self::opts_display()
            )
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Iceberg => "iceberg",
        }
    }

    pub fn opts_display() -> String {
        [Self::Default, Self::Iceberg]
            .iter()
            .map(Self::as_str)
            .collect::<Vec<_>>()
            .join("|")
    }

    pub fn is_iceberg(&self) -> bool {
        matches!(self, Self::Iceberg)
    }

    pub fn parse(value: Option<&str>) -> FsResult<Self> {
        match value {
            Some(s) if s.eq_ignore_ascii_case("iceberg") => Ok(Self::Iceberg),
            Some(s) if s.eq_ignore_ascii_case("default") => Ok(Self::Default),
            Some(other) => err!(
                ErrorCode::InvalidConfig,
                "Unsupported table_format '{}'. Must be one of ({})",
                other,
                Self::opts_display()
            ),
            None => Ok(Self::Default),
        }
    }
}

/// The format used to describe the table format at the materialization layer.
///
/// Often corresponds to predicates used in DDL or the storage format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhysicalTableFormat {
    Default,
    Iceberg,
    DuckLake,
}

/// Implementors know how to resolve a table format and catalog_type to the physical table format
pub trait PhysicalFormatResolver {
    fn table_format(&self) -> TableFormat;
    fn catalog_type(&self) -> CatalogType;

    fn physical_table_format(&self) -> PhysicalTableFormat {
        match self.table_format() {
            TableFormat::Default => match self.catalog_type() {
                CatalogType::Horizon
                | CatalogType::Glue
                | CatalogType::IcebergRest
                | CatalogType::HiveMetastore
                | CatalogType::Unity
                | CatalogType::BiglakeMetastore
                | CatalogType::LocalFilesystem
                | CatalogType::SnowflakeBuiltIn
                | CatalogType::SnowflakeNative
                | CatalogType::BigqueryNative
                | CatalogType::DuckdbNative => PhysicalTableFormat::Default,
                CatalogType::DuckLake => PhysicalTableFormat::DuckLake,
            },
            TableFormat::Iceberg => match self.catalog_type() {
                CatalogType::Horizon
                | CatalogType::Glue
                | CatalogType::IcebergRest
                | CatalogType::HiveMetastore
                | CatalogType::Unity
                | CatalogType::BiglakeMetastore
                | CatalogType::LocalFilesystem
                | CatalogType::SnowflakeBuiltIn
                | CatalogType::SnowflakeNative
                | CatalogType::BigqueryNative
                | CatalogType::DuckdbNative => PhysicalTableFormat::Iceberg,
                CatalogType::DuckLake => PhysicalTableFormat::DuckLake,
            },
        }
    }
}

impl PhysicalFormatResolver for CatalogSpecV2View<'_> {
    fn table_format(&self) -> TableFormat {
        self.table_format
    }

    fn catalog_type(&self) -> CatalogType {
        self.catalog_type
    }
}

impl PhysicalTableFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Iceberg => "iceberg",
            Self::DuckLake => "ducklake",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V2FileFormat {
    Delta,
    Parquet,
    Hudi,
}

impl V2FileFormat {
    pub fn parse(raw: &str, span: Option<yml::Span>) -> FsResult<Self> {
        if raw.eq_ignore_ascii_case("delta") {
            Ok(Self::Delta)
        } else if raw.eq_ignore_ascii_case("parquet") {
            Ok(Self::Parquet)
        } else if raw.eq_ignore_ascii_case("hudi") {
            Ok(Self::Hudi)
        } else {
            err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => span,
                "file_format '{}' invalid. choose one of (delta|parquet|hudi)",
                raw
            )
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniformMode {
    Enabled,
    Disabled,
}

impl UniformMode {
    pub fn from_bool(b: bool) -> Self {
        if b { Self::Enabled } else { Self::Disabled }
    }

    pub fn is_enabled(self) -> bool {
        matches!(self, Self::Enabled)
    }
}

#[derive(Debug)]
pub struct CatalogSpecV2View<'a> {
    repr: &'a yml::Mapping,
    pub name: &'a str,
    pub catalog_type: CatalogType,
    pub table_format: TableFormat,
    config: &'a yml::Mapping,
}

#[derive(Debug)]
pub struct DbtCatalogsV2View<'a> {
    pub catalogs: Vec<CatalogSpecV2View<'a>>,
}

impl<'a> CatalogSpecV2View<'a> {
    fn from_mapping(map: &'a yml::Mapping, span: &yml::Span) -> FsResult<Self> {
        let name = get_str(map, "name")?.ok_or_else(|| key_err("name", Some(span)))?;
        let raw_type = get_str(map, "type")?.ok_or_else(|| key_err("type", Some(span)))?;
        let raw_table_format =
            get_str(map, "table_format")?.ok_or_else(|| key_err("table_format", Some(span)))?;
        let type_span = field_span(map, "type").ok_or_else(|| key_err("type", Some(span)))?;
        let table_format_span =
            field_span(map, "table_format").ok_or_else(|| key_err("table_format", Some(span)))?;
        let catalog_type = CatalogType::parse(raw_type, type_span)?;
        let table_format = TableFormat::parse_from_yaml(raw_table_format, table_format_span)?;
        let config_map = get_map(map, "config")?.ok_or_else(|| key_err("config", Some(span)))?;

        Ok(Self {
            name,
            repr: map,
            catalog_type,
            table_format,
            config: config_map,
        })
    }

    fn field_span(&self, key: &str) -> Option<&'a yml::Span> {
        field_span(self.repr, key)
    }

    pub fn config_block(&self, platform: &str) -> Option<&'a yml::Mapping> {
        self.config
            .get(yml::Value::from(platform))
            .and_then(|v| v.as_mapping())
    }

    // ===== Post-hoc semantic validators =====
    // Called from CatalogRegistry::validate_semantic after structural validation passes.

    fn validate_duckdb_semantics(&self, duckdb: &yml::Mapping, type_name: &str) -> FsResult<()> {
        match self.catalog_type {
            CatalogType::Glue
            | CatalogType::IcebergRest
            | CatalogType::Horizon
            | CatalogType::Unity => {
                let has_endpoint = get_str(duckdb, "endpoint")?;
                let has_endpoint_type = get_str(duckdb, "endpoint_type")?;

                match (has_endpoint, has_endpoint_type) {
                    (None, None) => {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => self.field_span("type").cloned(),
                            "Catalog '{}' {}/duckdb config requires 'endpoint' or 'endpoint_type'",
                            self.name, type_name
                        );
                    }
                    (Some(ep), Some(_)) if !ep.is_empty_or_whitespace() => {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(duckdb, "endpoint_type").cloned(),
                            "Catalog '{}' {}/duckdb 'endpoint' and 'endpoint_type' are mutually exclusive",
                            self.name, type_name
                        );
                    }
                    (Some(ep), _) if ep.is_empty_or_whitespace() => {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(duckdb, "endpoint").cloned(),
                            "Catalog '{}' {}/duckdb 'endpoint' must be non-empty",
                            self.name, type_name
                        );
                    }
                    (_, Some(et)) => {
                        let val = et.trim();
                        if !matches_enum_ci(val, DUCKDB_ENDPOINT_TYPES) {
                            return err!(
                                code => ErrorCode::InvalidConfig,
                                hacky_yml_loc => field_span(duckdb, "endpoint_type").cloned(),
                                "Catalog '{}' {}/duckdb 'endpoint_type' must be 'GLUE' or 'S3_TABLES'",
                                self.name, type_name
                            );
                        }
                        if val.eq_ignore_ascii_case("S3_TABLES") {
                            let Some(warehouse) = get_str(duckdb, "warehouse")? else {
                                return err!(
                                    code => ErrorCode::InvalidConfig,
                                    hacky_yml_loc => field_span(duckdb, "endpoint_type").cloned(),
                                    "Catalog '{}' {}/duckdb endpoint_type='S3_TABLES' requires 'warehouse'",
                                    self.name, type_name
                                );
                            };
                            if warehouse.is_empty_or_whitespace() {
                                return err!(
                                    code => ErrorCode::InvalidConfig,
                                    hacky_yml_loc => field_span(duckdb, "warehouse").cloned(),
                                    "Catalog '{}' {}/duckdb 'warehouse' must be non-empty",
                                    self.name, type_name
                                );
                            }
                        }
                    }
                    _ => {}
                }

                if let Some(_auth_type) = get_str(duckdb, "authorization_type")? {
                    if has_endpoint_type.is_some() {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(duckdb, "authorization_type").cloned(),
                            "Catalog '{}' {}/duckdb 'authorization_type' cannot be combined with 'endpoint_type'",
                            self.name, type_name
                        );
                    }
                }
            }
            CatalogType::DuckLake => {}
            _ => debug_assert!(
                false,
                "validate_duckdb_semantics called for unsupported catalog type: {:?}",
                self.catalog_type
            ),
        }

        if let Some(catalog_database) = get_str(duckdb, "catalog_database")?
            && !catalog_database
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(duckdb, "catalog_database").cloned(),
                "Catalog '{}' {}/duckdb 'catalog_database' must contain only ASCII letters, digits, and underscores",
                self.name, type_name
            );
        }

        Ok(())
    }

    // FIXME: validation is currently organized per-platform (validate_duckdb_semantics etc.)
    // but some constraints are catalog-type × platform cross products. As these accumulate,
    // consider a validation schema that can express per-(catalog-type, platform) rules
    // without one-off functions. For now, catalog-type-specific validators like this one
    // compose the shared platform validator as a quick unblock.
    fn validate_horizon_duckdb_semantics(&self, duckdb: &yml::Mapping) -> FsResult<()> {
        // Horizon attached via DuckDB needs the Snowflake warehouse name.
        if get_str(duckdb, "warehouse")?.is_none() {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => self.field_span("type").cloned(),
                "Catalog '{}' horizon/duckdb config requires 'warehouse'",
                self.name
            );
        }
        Ok(())
    }

    fn validate_unity_semantics(&self, databricks: &yml::Mapping) -> FsResult<()> {
        let file_format_str = get_str(databricks, "file_format")?
            .expect("structural validation ensures file_format is present");
        let file_format_span = field_span(databricks, "file_format").cloned();
        let file_format = V2FileFormat::parse(file_format_str, file_format_span.clone())?;
        let use_uniform =
            UniformMode::from_bool(try_get_bool(databricks, "use_uniform")?.unwrap_or(false));

        match (file_format, use_uniform) {
            (V2FileFormat::Delta, UniformMode::Enabled)
            | (V2FileFormat::Parquet, UniformMode::Disabled) => Ok(()),
            (V2FileFormat::Delta, UniformMode::Disabled) => err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => file_format_span,
                "Catalog '{}' unity/databricks use_uniform: false (or unset) requires file_format: parquet",
                self.name
            ),
            (V2FileFormat::Parquet, UniformMode::Enabled) => err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => file_format_span,
                "Catalog '{}' unity/databricks use_uniform: true requires file_format: delta",
                self.name
            ),
            (V2FileFormat::Hudi, _) => err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => file_format_span,
                "Catalog '{}' unity/databricks file_format 'hudi' is not valid for unity (use delta or parquet)",
                self.name
            ),
        }
    }

    fn validate_biglake_semantics(&self, bigquery: &yml::Mapping) -> FsResult<()> {
        let external_volume = get_str(bigquery, "external_volume")?
            .expect("structural validation ensures external_volume is present");
        if !external_volume.starts_with("gs://") {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(bigquery, "external_volume").cloned(),
                "Catalog '{}' biglake_metastore/bigquery 'external_volume' must be a GCS path starting with gs://",
                self.name
            );
        }
        Ok(())
    }
}

impl<'a> DbtCatalogsV2View<'a> {
    /// Runs Phase 1 shape validation, then constructs zero-copy typed views.
    pub fn from_mapping(map: &'a yml::Mapping, span: &yml::Span) -> FsResult<Self> {
        validate_catalogs_v2_shape(map, span)?;

        let catalog_entries =
            get_seq(map, "catalogs")?.ok_or_else(|| key_err("catalogs", Some(span)))?;

        let mut catalogs = Vec::with_capacity(catalog_entries.len());
        for (idx, item) in catalog_entries.iter().enumerate() {
            let item_span = item.span();
            let m = match item.as_mapping() {
                Some(m) => m,
                None => {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => Some(item_span.clone()),
                        "catalogs[{idx}] must be a mapping"
                    );
                }
            };
            catalogs.push(CatalogSpecV2View::from_mapping(m, item_span)?);
        }

        Ok(Self { catalogs })
    }
}

// ===== CatalogRegistry =====

struct CatalogRegistry {
    schemas: &'static [CatalogTypeSchema],
}

impl CatalogRegistry {
    fn new() -> Self {
        Self {
            schemas: CATALOG_SCHEMAS,
        }
    }

    fn type_schema(&self, ct: CatalogType) -> FsResult<&'static CatalogTypeSchema> {
        self.schemas
            .iter()
            .find(|s| s.catalog_type == ct)
            .ok_or_else(|| {
                fs_err!(
                    ErrorCode::InvalidConfig,
                    "Unknown catalog type '{}'",
                    ct.as_str()
                )
            })
    }

    fn validate_structural(&self, catalog: &CatalogSpecV2View<'_>) -> FsResult<()> {
        let schema = self.type_schema(catalog.catalog_type)?;

        if catalog.table_format.as_str() != schema.table_format {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => catalog.field_span("table_format").cloned(),
                "Catalog '{}' type '{}' requires table_format='{}'",
                catalog.name, schema.catalog_type.as_str().to_lowercase(), schema.table_format
            );
        }

        if let Some(k) = catalog
            .config
            .keys()
            .find(|k| k.as_str().is_none_or(|s| !ALL_V2_PLATFORMS.contains(&s)))
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => Some(k.span().clone()),
                "Unknown key '{}' in catalogs[].config",
                k.as_str().unwrap_or("<non-string>")
            );
        }

        if let Some(&platform) = ALL_V2_PLATFORMS.iter().find(|&&p| {
            catalog.config_block(p).is_some() && !schema.platforms.iter().any(|s| s.key == p)
        }) {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => catalog.field_span("type").cloned(),
                "dbt does not support {} on the {} 'type'",
                platform, schema.catalog_type.as_str().to_lowercase()
            );
        }

        match schema.presence {
            ConfigPresence::AllRequired => {
                if let Some(missing) = schema
                    .platforms
                    .iter()
                    .find(|p| catalog.config_block(p.key).is_none())
                {
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => catalog.field_span("type").cloned(),
                        "Catalog '{}' type '{}' requires config.{}",
                        catalog.name, schema.catalog_type.as_str().to_lowercase(), missing.key
                    );
                }
            }
            ConfigPresence::AtLeastOne => {
                if !schema
                    .platforms
                    .iter()
                    .any(|p| catalog.config_block(p.key).is_some())
                {
                    let keys: Vec<&str> = schema.platforms.iter().map(|p| p.key).collect();
                    return err!(
                        code => ErrorCode::InvalidConfig,
                        hacky_yml_loc => catalog.field_span("type").cloned(),
                        "Catalog '{}' of type '{}' requires at least one config block: {}",
                        catalog.name, schema.catalog_type.as_str().to_lowercase(), keys.join(" or ")
                    );
                }
            }
        }

        for platform in schema.platforms {
            if let Some(block) = catalog.config_block(platform.key) {
                Self::validate_fields(
                    block,
                    platform.fields,
                    &format!(
                        "config.{} of catalog '{}' of type '{}'",
                        platform.key,
                        catalog.name,
                        schema.catalog_type.as_str().to_lowercase()
                    ),
                    catalog.name,
                )?;
            }
        }

        Ok(())
    }

    fn validate_semantic(&self, catalog: &CatalogSpecV2View<'_>) -> FsResult<()> {
        match catalog.catalog_type {
            CatalogType::Glue => {
                if let Some(duckdb) = catalog.config_block("duckdb") {
                    catalog.validate_duckdb_semantics(duckdb, "glue")?;
                }
            }
            CatalogType::IcebergRest => {
                if let Some(duckdb) = catalog.config_block("duckdb") {
                    catalog.validate_duckdb_semantics(duckdb, "iceberg_rest")?;
                }
            }
            CatalogType::Horizon => {
                if let Some(duckdb) = catalog.config_block("duckdb") {
                    catalog.validate_duckdb_semantics(duckdb, "horizon")?;
                    catalog.validate_horizon_duckdb_semantics(duckdb)?;
                }
            }
            CatalogType::Unity => {
                if let Some(databricks) = catalog.config_block("databricks") {
                    catalog.validate_unity_semantics(databricks)?;
                }
                if let Some(duckdb) = catalog.config_block("duckdb") {
                    catalog.validate_duckdb_semantics(duckdb, "unity")?;
                }
            }
            CatalogType::BiglakeMetastore => {
                if let Some(bigquery) = catalog.config_block("bigquery") {
                    catalog.validate_biglake_semantics(bigquery)?;
                }
            }
            CatalogType::DuckLake => {
                if let Some(duckdb) = catalog.config_block("duckdb") {
                    catalog.validate_duckdb_semantics(duckdb, "ducklake")?;
                }
            }
            CatalogType::HiveMetastore | CatalogType::LocalFilesystem => {}
            // These are not supported as explicit catalog types in catalogs.yml's `type` field.
            CatalogType::SnowflakeBuiltIn
            | CatalogType::SnowflakeNative
            | CatalogType::BigqueryNative
            | CatalogType::DuckdbNative => {
                unreachable!(
                    "catalogs.yml has no support for an explicit catalog type of {}",
                    catalog.catalog_type.as_str()
                )
            }
        }
        Ok(())
    }

    pub fn json_schema(&self) -> serde_json::Value {
        let entries: Vec<serde_json::Value> = self
            .schemas
            .iter()
            .map(|cts| {
                let config_props: serde_json::Map<String, serde_json::Value> = cts
                    .platforms
                    .iter()
                    .map(|p| (p.key.to_string(), Self::fields_schema(p.fields)))
                    .collect();
                let mut config_required = Vec::new();
                let mut config = serde_json::json!({
                    "type": "object",
                    "properties": config_props,
                    "additionalProperties": false,
                });
                match cts.presence {
                    ConfigPresence::AllRequired => {
                        config_required.extend(cts.platforms.iter().map(|p| p.key));
                        config["required"] = serde_json::json!(config_required);
                    }
                    ConfigPresence::AtLeastOne => {
                        config["minProperties"] = serde_json::json!(1);
                    }
                }
                serde_json::json!({
                    "type": "object",
                    "description": cts.description,
                    "required": ["name", "type", "table_format", "config"],
                    "additionalProperties": false,
                    "properties": {
                        "name": {
                            "type": "string",
                            "minLength": 1,
                            "description": "Unique catalog name within this project.",
                        },
                        // as_str() is uppercase for the legacy Snowflake variants (Jinja egress); lowercase to get the YAML-facing type name.
                        "type": { "const": cts.catalog_type.as_str().to_lowercase() },
                        "table_format": { "const": cts.table_format },
                        "description": {
                            "type": "string",
                            "description": "Optional human-readable description of this catalog's purpose.",
                        },
                        "config": config,
                        "owner": {
                            "type": "string",
                            "description": "Optional dedicated ownership field (team name or email).",
                        },
                        "meta": {
                            "type": "object",
                            "additionalProperties": true,
                            "description": "Optional free-form metadata map, following the dbt sources/models `meta:` convention.",
                        },
                    },
                })
            })
            .collect();

        serde_json::json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "title": "DbtCatalogsFile",
            "description": "Top-level `catalogs.yml` (v2) schema.",
            "type": "object",
            "required": ["catalogs"],
            "additionalProperties": false,
            "properties": {
                "catalogs": {
                    "type": "array",
                    "items": { "oneOf": entries },
                },
            },
        })
    }

    fn fields_schema(fields: &[FieldSpec]) -> serde_json::Value {
        let mut props = serde_json::Map::new();
        let mut required = Vec::new();
        for f in fields.iter().filter(|f| !f.forbidden) {
            let mut schema = match f.kind {
                FieldKind::Str => serde_json::json!({ "type": "string" }),
                FieldKind::Bool => serde_json::json!({ "type": "boolean" }),
                FieldKind::U32 { max } => {
                    let mut num = serde_json::json!({ "type": "integer", "minimum": 0 });
                    if let Some(m) = max {
                        num["maximum"] = serde_json::json!(m);
                    }
                    num
                }
                FieldKind::Enum(allowed) => {
                    serde_json::json!({ "type": "string", "enum": allowed })
                }
            };
            if f.non_empty && matches!(f.kind, FieldKind::Str) {
                schema["minLength"] = serde_json::json!(1);
            }
            if !f.doc.is_empty() {
                schema["description"] = serde_json::json!(f.doc);
            }
            props.insert(f.name.to_string(), schema);
            if f.required {
                required.push(f.name);
            }
        }
        let mut obj = serde_json::json!({
            "type": "object",
            "properties": props,
            "additionalProperties": false,
        });
        if !required.is_empty() {
            obj["required"] = serde_json::json!(required);
        }
        obj
    }

    fn validate_fields(
        map: &yml::Mapping,
        fields: &[FieldSpec],
        ctx: &str,
        catalog_name: &str,
    ) -> FsResult<()> {
        for k in map.keys() {
            let span = k.span();
            let Some(ks) = k.as_str() else {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span.clone()),
                    "Non-string key in {}",
                    ctx
                );
            };
            if !fields.iter().any(|f| f.name == ks) {
                return err!(
                    code => ErrorCode::InvalidConfig,
                    hacky_yml_loc => Some(span.clone()),
                    "Unknown key '{}' in {}",
                    ks, ctx
                );
            }
        }

        if let Some(f) = fields
            .iter()
            .filter(|f| f.forbidden)
            .find(|f| field_span(map, f.name).is_some())
        {
            return err!(
                code => ErrorCode::InvalidConfig,
                hacky_yml_loc => field_span(map, f.name).cloned(),
                "{}",
                f.doc.replace("{}", catalog_name)
            );
        }

        for f in fields.iter().filter(|f| !f.forbidden) {
            match f.kind {
                FieldKind::Str => {
                    let val = get_str(map, f.name)?;
                    if f.required && val.is_none() {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(map, f.name).cloned(),
                            "Catalog '{}' {} requires '{}'",
                            catalog_name, ctx, f.name
                        );
                    }
                    if f.non_empty {
                        if let Some(v) = val {
                            if v.is_empty_or_whitespace() {
                                return err!(
                                    code => ErrorCode::InvalidConfig,
                                    hacky_yml_loc => field_span(map, f.name).cloned(),
                                    "Catalog '{}' {} '{}' must be non-empty",
                                    catalog_name, ctx, f.name
                                );
                            }
                        }
                    }
                }
                FieldKind::Bool => {
                    let val = match try_get_bool(map, f.name) {
                        Ok(v) => v,
                        Err(_) if map.get(yml::Value::from(f.name)).is_some() => {
                            return Err(fs_err!(
                                code => ErrorCode::InvalidConfig,
                                hacky_yml_loc => field_span(map, f.name).cloned(),
                                "Key '{}' must be a boolean",
                                f.name
                            ));
                        }
                        Err(e) => return Err(e),
                    };
                    if f.required && val.is_none() {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(map, f.name).cloned(),
                            "Catalog '{}' {} requires '{}'",
                            catalog_name, ctx, f.name
                        );
                    }
                }
                FieldKind::U32 { max } => {
                    let val = get_u32(map, f.name)?;
                    if f.required && val.is_none() {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(map, f.name).cloned(),
                            "Catalog '{}' {} requires '{}'",
                            catalog_name, ctx, f.name
                        );
                    }
                    if let Some(v) = val {
                        if let Some(m) = max {
                            if v > m {
                                return err!(
                                    code => ErrorCode::InvalidConfig,
                                    hacky_yml_loc => field_span(map, f.name).cloned(),
                                    "Key '{}' must be in 0..={}",
                                    f.name, m
                                );
                            }
                        }
                    }
                }
                FieldKind::Enum(allowed) => {
                    let val = get_str(map, f.name)?;
                    if f.required && val.is_none() {
                        return err!(
                            code => ErrorCode::InvalidConfig,
                            hacky_yml_loc => field_span(map, f.name).cloned(),
                            "Catalog '{}' {} requires '{}'",
                            catalog_name, ctx, f.name
                        );
                    }
                    if let Some(v) = val {
                        if !matches_enum_ci(v, allowed) {
                            let choices = allowed.join("|");
                            return err!(
                                code => ErrorCode::InvalidConfig,
                                hacky_yml_loc => field_span(map, f.name).cloned(),
                                "{} '{}' invalid ({})",
                                f.name, v, choices
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn validate_catalogs_v2(spec: &DbtCatalogsV2View<'_>, _path: &Path) -> FsResult<()> {
    let registry = CatalogRegistry::new();
    for catalog in &spec.catalogs {
        registry.validate_structural(catalog)?;
        registry.validate_semantic(catalog)?;
    }
    Ok(())
}

/// Build the draft-07 JSON schema for `catalogs.yml` (v2), used by
/// `dbt man --schema catalog`. The schema is generated from the same
/// `CATALOG_SCHEMAS` / `FieldSpec` descriptor tables that drive validation, so
/// it cannot drift from the parser. Each catalog `type` becomes a `oneOf`
/// branch discriminated by a `const` `type`, so editors narrow `config` field
/// completions to those valid for the selected catalog type.
pub fn catalogs_v2_json_schema() -> serde_json::Value {
    CatalogRegistry::new().json_schema()
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_yaml as yml;
    use std::path::Path;

    fn parse_and_validate_with<F: FnOnce(&DbtCatalogsV2View<'_>)>(
        yaml: &str,
        inspect: F,
    ) -> FsResult<()> {
        let v: yml::Value = yml::from_str(yaml)?;
        let v_span = v.span();
        let m = v.as_mapping().expect("top-level YAML must be a mapping");
        validate_catalogs_v2_shape(m, v_span)?;
        let view = DbtCatalogsV2View::from_mapping(m, v_span)?;
        inspect(&view);
        validate_catalogs_v2(&view, Path::new("<test>"))?;
        Ok(())
    }

    fn parse_and_validate(yaml: &str) -> FsResult<()> {
        parse_and_validate_with(yaml, |_| {})
    }

    #[test]
    fn catalog_type_as_str_matches_snowflake_jinja_macros() {
        // crates/dbt-loader/src/dbt_macro_assets/dbt-snowflake/macros/relations/table/create.sql
        // string-compares catalog_relation.catalog_type against these exact uppercase literals.
        assert_eq!(CatalogType::SnowflakeNative.as_str(), "INFO_SCHEMA");
        assert_eq!(CatalogType::SnowflakeBuiltIn.as_str(), "BUILT_IN");
        assert_eq!(CatalogType::IcebergRest.as_str(), "ICEBERG_REST");
    }

    #[test]
    fn unity_multiplatform_v2_valid() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_DB"
        auto_refresh: true
      databricks:
        file_format: delta
        location_root: "s3://bucket/path"
        use_uniform: true
"#;
        parse_and_validate(yaml).expect("v2 should validate");
    }

    #[test]
    fn unity_databricks_parquet_managed_iceberg_valid() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: parquet
        use_uniform: false
"#;
        parse_and_validate(yaml).expect("parquet + use_uniform=false should validate");
    }

    #[test]
    fn unity_databricks_parquet_use_uniform_unset_valid() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: parquet
"#;
        parse_and_validate(yaml).expect("parquet with use_uniform unset should validate");
    }

    #[test]
    fn horizon_v2_valid() {
        let yaml = r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_external_volume
        base_location_root: analytics/iceberg/dbt
        storage_serialization_policy: COMPATIBLE
        data_retention_time_in_days: 1
        max_data_extension_time_in_days: 14
        change_tracking: false
"#;
        parse_and_validate(yaml).expect("v2 horizon should validate");
    }

    #[test]
    fn glue_v2_valid() {
        let yaml = r#"
catalogs:
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_CLD"
        auto_refresh: true
        target_file_size: AUTO
"#;
        parse_and_validate(yaml).expect("v2 glue should validate");
    }

    #[test]
    fn iceberg_rest_v2_valid() {
        let yaml = r#"
catalogs:
  - name: rest_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_REST_CLD"
        auto_refresh: true
        max_data_extension_time_in_days: 1
        target_file_size: AUTO
"#;
        parse_and_validate(yaml).expect("v2 iceberg_rest should validate");
    }

    #[test]
    fn iceberg_rest_rejects_databricks_block() {
        let yaml = r#"
catalogs:
  - name: rest_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_REST_CLD"
      databricks:
        file_format: delta
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("does not support databricks on the iceberg_rest"),
            "unexpected error: {res:?}"
        );
    }

    #[test]
    fn hive_metastore_v2_valid() {
        let yaml = r#"
catalogs:
  - name: hive
    type: hive_metastore
    table_format: default
    config:
      databricks:
        file_format: hudi
"#;
        parse_and_validate(yaml).expect("v2 hive_metastore should validate");
    }

    #[test]
    fn biglake_metastore_v2_valid() {
        let yaml = r#"
catalogs:
  - name: cat1
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
        base_location_root: "root1"
"#;
        parse_and_validate(yaml).expect("v2 bigquery should validate");
    }

    #[test]
    fn biglake_accepts_connection_id() {
        let yaml = r#"
catalogs:
  - name: cat1
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
        base_location_root: "root1"
        connection_id: "cool_connection"
"#;
        parse_and_validate(yaml).expect("v2 bigquery should validate");
    }

    #[test]
    fn biglake_accepts_lakehouse_catalog() {
        let yaml = r#"
catalogs:
  - name: cat1
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
        lakehouse_catalog: "sales_catalog"
"#;
        parse_and_validate_with(yaml, |view| {
            let bigquery_config = view.catalogs[0]
                .config_block("bigquery")
                .expect("bigquery config block");
            assert_eq!(
                get_str(bigquery_config, "external_volume").unwrap(),
                Some("gs://bucket")
            );
            assert_eq!(
                get_str(bigquery_config, "file_format").unwrap(),
                Some("parquet")
            );
            assert_eq!(
                get_str(bigquery_config, "lakehouse_catalog").unwrap(),
                Some("sales_catalog")
            );
        })
        .expect("v2 bigquery LRC catalog should validate");
    }

    #[test]
    fn biglake_rejects_blank_lakehouse_catalog() {
        let yaml = r#"
catalogs:
  - name: cat1
    type: biglake_metastore
    table_format: iceberg
    config:
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
        lakehouse_catalog: ""
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error but got Ok");
    }

    #[test]
    fn v2_rejects_legacy_iceberg_catalogs_key() {
        let yaml = r#"
iceberg_catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config: {}
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("not 'iceberg_catalogs:'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unity_rejects_bigquery_block_in_config() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_DB"
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("does not support bigquery on the unity"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_rejects_bigquery_platform_block() {
        let yaml = r#"
catalogs:
  - name: my_catalog
    type: horizon
    table_format: iceberg
    config:
      bigquery:
        external_volume: "gs://bucket"
        file_format: parquet
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("does not support bigquery on the horizon"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_rejects_unity_only_snowflake_fields() {
        let yaml = r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_external_volume
        auto_refresh: true
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Unknown key 'auto_refresh'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_rejects_catalog_base_location_subpath() {
        let yaml = r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_external_volume
        base_location_subpath: model_only
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("base_location_subpath is model-config only"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn glue_rejects_horizon_only_snowflake_fields() {
        let yaml = r#"
catalogs:
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_CLD"
        external_volume: should_not_be_here
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Unknown key 'external_volume'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unity_rejects_horizon_only_snowflake_fields() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_DB"
        external_volume: should_not_be_here
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Unknown key 'external_volume'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn iceberg_rest_snowflake_only_still_valid() {
        let yaml = r#"
catalogs:
  - name: rest_sf
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_DB"
        auto_refresh: true
"#;
        parse_and_validate(yaml).expect("iceberg_rest + snowflake should validate");
    }

    #[test]
    fn unity_databricks_parquet_with_use_uniform_is_rejected() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        use_uniform: true
        file_format: parquet
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("use_uniform: true requires file_format: delta"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unity_databricks_delta_without_use_uniform_is_rejected() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("use_uniform: false (or unset) requires file_format: parquet"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unity_databricks_delta_with_use_uniform_false_is_rejected() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: delta
        use_uniform: false
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("use_uniform: false (or unset) requires file_format: parquet"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn unity_databricks_unknown_file_format_is_rejected() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    config:
      databricks:
        file_format: iceberg
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("file_format") && msg.contains("invalid") && msg.contains("delta"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn top_level_platform_specific_keys_are_rejected() {
        let yaml = r#"
catalogs:
  - name: linked_catalog
    type: unity
    table_format: iceberg
    file_format: parquet
    config:
      databricks: {}
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Unknown key 'file_format' in catalog entry"),
            "unexpected error: {msg}"
        );
    }

    // ===== DuckDB + IcebergRest tests =====

    #[test]
    fn glue_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: glue_duck
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://glue.us-east-1.amazonaws.com"
"#;
        parse_and_validate(yaml).expect("glue + duckdb should validate");
    }

    #[test]
    fn iceberg_rest_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://my-iceberg-rest.example.com"
        secret: "my_secret"
        catalog_database: "my_catalog"
"#;
        parse_and_validate(yaml).expect("iceberg_rest + duckdb should validate");
    }

    #[test]
    fn iceberg_rest_duckdb_and_snowflake_v2_valid() {
        let yaml = r#"
catalogs:
  - name: rest_mixed
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_DB"
      duckdb:
        endpoint: "https://my-iceberg-rest.example.com"
"#;
        parse_and_validate(yaml).expect("iceberg_rest + snowflake + duckdb should validate");
    }

    #[test]
    fn glue_snowflake_only_still_valid() {
        let yaml = r#"
catalogs:
  - name: glue_sf
    type: glue
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "MY_CLD"
        auto_refresh: true
"#;
        parse_and_validate(yaml).expect("glue + snowflake should still validate");
    }

    #[test]
    fn iceberg_rest_duckdb_missing_endpoint() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        secret: "my_secret"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(msg.contains("'endpoint'"), "unexpected error: {msg}");
    }

    #[test]
    fn iceberg_rest_duckdb_blank_endpoint() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "   "
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("'endpoint' must be non-empty"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn iceberg_rest_duckdb_blank_secret() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://my-rest.example.com"
        secret: ""
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("'secret' must be non-empty"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn iceberg_rest_duckdb_blank_catalog_database() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://my-rest.example.com"
        catalog_database: ""
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("'catalog_database' must be non-empty"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn snowflake_catalog_database_dashes_allowed() {
        // The ascii-identifier check only applies to duckdb's `catalog_database`
        // (which becomes a sanitized ATTACH alias); Snowflake's is never
        // sanitized, so dashes are fine there.
        let yaml = r#"
catalogs:
  - name: rest_sf
    type: iceberg_rest
    table_format: iceberg
    config:
      snowflake:
        catalog_database: "my-linked-db"
        auto_refresh: true
"#;
        parse_and_validate(yaml).expect("dashes in snowflake catalog_database should validate");
    }

    #[test]
    fn ducklake_duckdb_non_ascii_identifier_catalog_database_rejected() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
        catalog_database: "my-lake"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("must contain only ASCII letters, digits, and underscores"),
            "unexpected error: {msg}"
        );
    }

    // -----------------------------------------------------------------------
    // DuckDB config: endpoint_type
    // -----------------------------------------------------------------------

    #[test]
    fn duckdb_endpoint_type_glue_valid() {
        let yaml = r#"
catalogs:
  - name: glue_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint_type: GLUE
"#;
        parse_and_validate(yaml).expect("endpoint_type=GLUE should validate");
    }

    #[test]
    fn duckdb_endpoint_type_s3_tables_valid() {
        let yaml = r#"
catalogs:
  - name: s3t_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint_type: S3_TABLES
        warehouse: "arn:aws:s3tables:us-east-1:123456789012:bucket/example"
"#;
        parse_and_validate(yaml).expect("endpoint_type=S3_TABLES should validate");
    }

    #[test]
    fn duckdb_endpoint_type_invalid_value() {
        let yaml = r#"
catalogs:
  - name: bad_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint_type: INVALID
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("endpoint_type") && format!("{res:?}").contains("invalid"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_endpoint_and_endpoint_type_mutual_exclusion() {
        let yaml = r#"
catalogs:
  - name: both_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        endpoint_type: GLUE
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("mutually exclusive"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_s3_tables_endpoint_type_requires_warehouse() {
        let yaml = r#"
catalogs:
  - name: s3t_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint_type: S3_TABLES
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("requires 'warehouse'"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_neither_endpoint_nor_endpoint_type() {
        let yaml = r#"
catalogs:
  - name: empty_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        secret: "my_secret"
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("'endpoint' or 'endpoint_type'"),
            "unexpected: {res:?}"
        );
    }

    // -----------------------------------------------------------------------
    // DuckDB config: authorization_type, access_delegation_mode
    // -----------------------------------------------------------------------

    #[test]
    fn duckdb_authorization_type_valid() {
        for auth_type in ["OAUTH2", "SIGV4", "NONE"] {
            let yaml = format!(
                r#"
catalogs:
  - name: auth_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        authorization_type: {auth_type}
"#
            );
            parse_and_validate(&yaml)
                .unwrap_or_else(|e| panic!("authorization_type={auth_type} should validate: {e}"));
        }
    }

    #[test]
    fn duckdb_authorization_type_invalid() {
        let yaml = r#"
catalogs:
  - name: auth_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        authorization_type: BEARER
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("authorization_type")
                && format!("{res:?}").contains("invalid"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_authorization_type_cannot_combine_with_endpoint_type() {
        let yaml = r#"
catalogs:
  - name: auth_cat
    type: glue
    table_format: iceberg
    config:
      duckdb:
        endpoint_type: GLUE
        authorization_type: SIGV4
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("cannot be combined with 'endpoint_type'"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_access_delegation_mode_valid() {
        for mode in ["VENDED_CREDENTIALS", "NONE"] {
            let yaml = format!(
                r#"
catalogs:
  - name: deleg_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        access_delegation_mode: {mode}
"#
            );
            parse_and_validate(&yaml)
                .unwrap_or_else(|e| panic!("access_delegation_mode={mode} should validate: {e}"));
        }
    }

    #[test]
    fn duckdb_access_delegation_mode_invalid() {
        let yaml = r#"
catalogs:
  - name: deleg_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        access_delegation_mode: REMOTE
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("access_delegation_mode")
                && format!("{res:?}").contains("invalid"),
            "unexpected: {res:?}"
        );
    }

    // -----------------------------------------------------------------------
    // DuckDB config: full config with all optional keys
    // -----------------------------------------------------------------------

    #[test]
    fn duckdb_full_config_all_optional_keys() {
        let yaml = r#"
catalogs:
  - name: full_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://my-catalog.example.com"
        warehouse: "warehouse_name"
        secret: "my_secret"
        catalog_database: "my_db"
        default_region: "us-east-1"
        default_schema: "demo"
        max_table_staleness: "10 minutes"
        authorization_type: OAUTH2
        access_delegation_mode: VENDED_CREDENTIALS
        support_nested_namespaces: true
        support_stage_create: false
        purge_requested: true
        encode_entire_prefix: true
"#;
        parse_and_validate(yaml).expect("full config should validate");
    }

    #[test]
    fn duckdb_credential_values_belong_in_profile_secrets() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        client_secret: "actual-secret-value"
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error for credential-bearing key");
        assert!(
            format!("{res:?}").contains("client_secret"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_boolean_attach_options_validate_type() {
        let yaml = r#"
catalogs:
  - name: rest_duck
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        support_stage_create: "yes"
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error for non-boolean attach option");
        assert!(
            format!("{res:?}").contains("support_stage_create"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_unknown_key_rejected() {
        let yaml = r#"
catalogs:
  - name: unk_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        bogus_key: "value"
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error for unknown key");
        assert!(
            format!("{res:?}").contains("bogus_key"),
            "unexpected: {res:?}"
        );
    }

    #[test]
    fn duckdb_blank_warehouse_invalid() {
        let yaml = r#"
catalogs:
  - name: bad_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        warehouse: "   "
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("'warehouse' must be non-empty"),
            "unexpected: {res:?}"
        );
    }

    // ===== DuckLake tests =====

    #[test]
    fn ducklake_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
"#;
        parse_and_validate(yaml).expect("ducklake minimal config should validate");
    }

    #[test]
    fn ducklake_duckdb_v2_all_options() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
        data_path: "data/"
        catalog_database: "lake"
        metadata_schema: "my_schema"
        metadata_catalog: "lake_db"
        data_inlining_row_limit: 100
        create_if_not_exists: true
        read_only: false
        encrypted: false
        automatic_migration: true
        override_data_path: true
"#;
        parse_and_validate(yaml).expect("ducklake full config should validate");
    }

    #[test]
    fn ducklake_missing_metadata_path() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      duckdb:
        data_path: "data/"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(msg.contains("'metadata_path'"), "unexpected error: {msg}");
    }

    #[test]
    fn ducklake_wrong_table_format() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: iceberg
    config:
      duckdb:
        metadata_path: "metadata.ducklake"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("requires table_format='default'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn ducklake_snowflake_block_rejected() {
        let yaml = r#"
catalogs:
  - name: my_lake
    type: ducklake
    table_format: default
    config:
      snowflake:
        external_volume: "EV"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("does not support snowflake on the ducklake"),
            "unexpected error: {msg}"
        );
    }

    // ===== Local filesystem tests =====

    #[test]
    fn local_filesystem_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: local_files
    type: local_filesystem
    table_format: default
    config:
      duckdb:
        root_path: "data/local_files"
        file_format: parquet
"#;
        parse_and_validate(yaml).expect("local filesystem config should validate");
    }

    #[test]
    fn local_filesystem_missing_root_path() {
        let yaml = r#"
catalogs:
  - name: local_files
    type: local_filesystem
    table_format: default
    config:
      duckdb:
        file_format: parquet
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(msg.contains("'root_path'"), "unexpected error: {msg}");
    }

    #[test]
    fn local_filesystem_wrong_table_format() {
        let yaml = r#"
catalogs:
  - name: local_files
    type: local_filesystem
    table_format: iceberg
    config:
      duckdb:
        root_path: "data/local_files"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("requires table_format='default'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn json_schema_round_trips_and_covers_all_types() {
        let schema = CatalogRegistry::new().json_schema();
        let pretty = serde_json::to_string_pretty(&schema).unwrap();
        let reparsed: serde_json::Value = serde_json::from_str(&pretty).unwrap();
        assert_eq!(schema, reparsed, "schema must round-trip through JSON");

        let items = schema["properties"]["catalogs"]["items"]["oneOf"]
            .as_array()
            .expect("oneOf array");
        let type_names: Vec<&str> = items
            .iter()
            .filter_map(|i| i["properties"]["type"]["const"].as_str())
            .collect();
        for cts in CATALOG_SCHEMAS {
            let type_name = cts.catalog_type.as_str().to_lowercase();
            assert!(
                type_names.contains(&type_name.as_str()),
                "missing {type_name}"
            );
        }
    }

    /// Drift guard: the schema's accepted config fields for every
    /// `(type, platform)` block must match the descriptor tables the validators
    /// use, modulo `forbidden` fields (which are model-config only and excluded
    /// from the published schema). Adding or renaming a `FieldSpec` updates both
    /// the accepted-key set and the published schema together.
    #[test]
    fn schema_config_fields_match_descriptor_tables() {
        let schema = catalogs_v2_json_schema();
        let branches = schema["properties"]["catalogs"]["items"]["oneOf"]
            .as_array()
            .expect("oneOf array");
        for cts in CATALOG_SCHEMAS {
            let type_name = cts.catalog_type.as_str().to_lowercase();
            let branch = branches
                .iter()
                .find(|b| b["properties"]["type"]["const"] == serde_json::json!(type_name))
                .unwrap_or_else(|| panic!("missing schema branch for {type_name}"));
            let config_props = branch["properties"]["config"]["properties"]
                .as_object()
                .expect("config properties");

            let mut schema_platforms: Vec<&str> = config_props.keys().map(String::as_str).collect();
            schema_platforms.sort_unstable();
            let mut table_platforms: Vec<&str> = cts.platforms.iter().map(|p| p.key).collect();
            table_platforms.sort_unstable();
            assert_eq!(
                schema_platforms, table_platforms,
                "platform blocks for {type_name} diverge"
            );

            for platform in cts.platforms {
                let mut schema_fields: Vec<&str> = config_props[platform.key]["properties"]
                    .as_object()
                    .expect("platform field properties")
                    .keys()
                    .map(String::as_str)
                    .collect();
                schema_fields.sort_unstable();
                let mut table_fields: Vec<&str> = platform
                    .fields
                    .iter()
                    .filter(|f| !f.forbidden)
                    .map(|f| f.name)
                    .collect();
                table_fields.sort_unstable();
                assert_eq!(
                    schema_fields, table_fields,
                    "schema fields for {type_name}/{} diverge from the descriptor table",
                    platform.key
                );
            }
        }
    }

    // ===== Catalog federation tests =====

    #[test]
    fn horizon_databricks_only_valid() {
        let yaml = r#"
catalogs:
  - name: dbx_horizon
    type: horizon
    table_format: iceberg
    config:
      databricks:
        catalog_database: "MY_FOREIGN_CATALOG"
"#;
        parse_and_validate(yaml).expect("horizon with databricks-only block should validate");
    }

    #[test]
    fn horizon_snowflake_and_databricks_valid() {
        let yaml = r#"
catalogs:
  - name: horizon_both
    type: horizon
    table_format: iceberg
    config:
      snowflake:
        external_volume: my_external_volume
      databricks:
        catalog_database: "MY_FOREIGN_CATALOG"
"#;
        parse_and_validate(yaml).expect("horizon with both platform blocks should validate");
    }

    #[test]
    fn horizon_requires_at_least_one_platform_block() {
        let yaml = r#"
catalogs:
  - name: horizon_empty
    type: horizon
    table_format: iceberg
    config: {}
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("requires at least one config block"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_databricks_requires_catalog_database() {
        let yaml = r#"
catalogs:
  - name: dbx_horizon
    type: horizon
    table_format: iceberg
    config:
      databricks: {}
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("requires 'catalog_database'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_databricks_rejects_empty_catalog_database() {
        let yaml = r#"
catalogs:
  - name: dbx_horizon
    type: horizon
    table_format: iceberg
    config:
      databricks:
        catalog_database: "   "
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("'catalog_database' must be non-empty"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_databricks_rejects_unknown_key() {
        let yaml = r#"
catalogs:
  - name: dbx_horizon
    type: horizon
    table_format: iceberg
    config:
      databricks:
        catalog_database: "MY_CATALOG"
        file_format: delta
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Unknown key 'file_format'"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn horizon_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: horizon_demo
    type: horizon
    table_format: iceberg
    config:
      duckdb:
        warehouse: "horizon_wh"
        endpoint: "https://horizon.example.com/catalog"
        secret: "horizon_secret"
        default_schema: "demo"
"#;
        parse_and_validate(yaml).expect("read-only horizon + duckdb should validate");
    }

    #[test]
    fn unity_duckdb_v2_valid() {
        let yaml = r#"
catalogs:
  - name: unity_demo
    type: unity
    table_format: iceberg
    config:
      duckdb:
        warehouse: "unity_wh"
        endpoint: "https://dbc.example.com/api/2.1/unity-catalog/iceberg"
"#;
        parse_and_validate(yaml).expect("read-only unity + duckdb should validate");
    }

    #[test]
    fn horizon_duckdb_requires_warehouse() {
        let yaml = r#"
catalogs:
  - name: horizon_demo
    type: horizon
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://horizon.example.com/catalog"
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(msg.contains("'warehouse'"), "unexpected error: {msg}");
    }

    #[test]
    fn horizon_duckdb_allows_writes() {
        // read_only: false + the #1017 write-compat options validate now.
        let yaml = r#"
catalogs:
  - name: horizon_demo
    type: horizon
    table_format: iceberg
    config:
      duckdb:
        warehouse: "horizon_wh"
        endpoint: "https://horizon.example.com/catalog"
        read_only: false
        stage_create_tables: false
        disable_multi_table_commit: true
"#;
        parse_and_validate(yaml)
            .expect("read-write horizon + write-compat options should validate");
    }

    #[test]
    fn unity_duckdb_allows_writes() {
        let yaml = r#"
catalogs:
  - name: unity_demo
    type: unity
    table_format: iceberg
    config:
      duckdb:
        warehouse: "unity_wh"
        endpoint: "https://dbc.example.com/api/2.1/unity-catalog/iceberg"
        read_only: false
        disable_multi_table_commit: true
"#;
        parse_and_validate(yaml)
            .expect("read-write unity + disable_multi_table_commit should validate");
    }

    #[test]
    fn duckdb_blank_default_region_invalid() {
        let yaml = r#"
catalogs:
  - name: bad_cat
    type: iceberg_rest
    table_format: iceberg
    config:
      duckdb:
        endpoint: "https://example.com"
        default_region: ""
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error");
        assert!(
            format!("{res:?}").contains("default_region"),
            "unexpected: {res:?}"
        );
    }
    // ===== description field (optional, free-text) =====

    #[test]
    fn catalog_with_description_is_valid() {
        let yaml = r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    description: "Primary Snowflake-managed Iceberg catalog for analytics."
    config:
      snowflake:
        external_volume: my_external_volume
"#;
        parse_and_validate(yaml).expect("description should be accepted");
    }

    #[test]
    fn empty_description_is_rejected() {
        let yaml = r#"
catalogs:
  - name: sf_native
    type: horizon
    table_format: iceberg
    description: "   "
    config:
      snowflake:
        external_volume: my_external_volume
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            format!("{res:?}").contains("Supply a value for the 'description' field"),
            "unexpected error: {res:?}"
        );
    }

    #[test]
    fn description_appears_in_json_schema() {
        let schema = catalogs_v2_json_schema();
        let branches = schema["properties"]["catalogs"]["items"]["oneOf"]
            .as_array()
            .expect("oneOf array");
        for branch in branches {
            assert_eq!(
                branch["properties"]["description"]["type"],
                serde_json::json!("string"),
                "every catalog type branch should publish an optional string `description`"
            );
            // description is optional: it must not appear in the required list.
            let required = branch["required"].as_array().expect("required array");
            assert!(
                !required
                    .iter()
                    .any(|r| r == &serde_json::json!("description")),
                "description must be optional"
            );
        }
    }

    // ===== owner / meta fields (optional) =====

    #[test]
    fn owner_and_meta_appear_in_json_schema() {
        let schema = catalogs_v2_json_schema();
        let branches = schema["properties"]["catalogs"]["items"]["oneOf"]
            .as_array()
            .expect("oneOf array");
        for branch in branches {
            assert_eq!(
                branch["properties"]["owner"]["type"],
                serde_json::json!("string"),
                "every catalog type branch should publish an optional string `owner`"
            );
            assert_eq!(
                branch["properties"]["meta"]["type"],
                serde_json::json!("object"),
                "every catalog type branch should publish an optional object `meta`"
            );
            let required = branch["required"].as_array().expect("required array");
            assert!(
                !required
                    .iter()
                    .any(|r| r == &serde_json::json!("owner") || r == &serde_json::json!("meta")),
                "owner and meta must be optional"
            );
        }
    }

    #[test]
    fn catalog_with_owner_is_valid() {
        let yaml = r#"
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    owner: "data-platform@example.com"
    config:
      duckdb:
        root_path: /tmp/catalog
"#;
        parse_and_validate(yaml).expect("owner should validate");
    }

    #[test]
    fn empty_owner_rejected() {
        let yaml = r#"
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    owner: "  "
    config:
      duckdb:
        root_path: /tmp/catalog
"#;
        let res = parse_and_validate(yaml);
        assert!(res.is_err(), "empty owner should be rejected");
        assert!(format!("{res:?}").contains("owner"));
    }

    #[test]
    fn owner_non_string_rejected() {
        let yaml = r#"
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    owner: 123
    config:
      duckdb:
        root_path: /tmp/catalog
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Key 'owner' must be a string"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn catalog_with_meta_is_valid() {
        let yaml = r##"
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    meta:
      slack: "#data-platform"
      sla_hours: 24
      domain: analytics
      nested:
        pagerduty: data-platform-oncall
    config:
      duckdb:
        root_path: /tmp/catalog
"##;
        parse_and_validate(yaml).expect("meta should validate");
    }

    #[test]
    fn meta_non_string_key_rejected() {
        let yaml = "
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    meta:
      42: not-a-string-key
    config:
      duckdb:
        root_path: /tmp/catalog
";
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Non-string key in 'meta' mapping"),
            "unexpected error: {msg}"
        );
    }

    #[test]
    fn meta_non_mapping_rejected() {
        let yaml = r#"
catalogs:
  - name: local
    type: local_filesystem
    table_format: default
    meta: "not-a-map"
    config:
      duckdb:
        root_path: /tmp/catalog
"#;
        let res = parse_and_validate(yaml);
        let msg = format!("{res:?}");
        assert!(res.is_err(), "expected error but got Ok");
        assert!(
            msg.contains("Key 'meta' must be a mapping"),
            "unexpected error: {msg}"
        );
    }
}
