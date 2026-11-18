pub mod attach;
pub mod seed;

use dbt_adapter_core::AdapterType;
use dbt_schemas::schemas::dbt_catalogs::{CatalogSpecView, CatalogType};

/// An external Iceberg REST catalog that DuckDB reaches through an Iceberg REST
/// `ATTACH`. Returned instead of a bare bool so callers see *which* catalog
/// matched (its sanitized attach alias) rather than only whether one did.
pub struct ExternalIcebergAttach {
    pub attach_alias: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckDbAttachKind {
    /// Horizon, Unity, and Glue are Iceberg REST services under the hood.
    IcebergRest,
    DuckLake,
}

pub fn duckdb_attaches_via(catalog_type: CatalogType) -> Option<DuckDbAttachKind> {
    match catalog_type {
        CatalogType::IcebergRest
        | CatalogType::Horizon
        | CatalogType::Unity
        | CatalogType::Glue => Some(DuckDbAttachKind::IcebergRest),
        CatalogType::DuckLake => Some(DuckDbAttachKind::DuckLake),
        CatalogType::LocalFilesystem
        | CatalogType::HiveMetastore
        | CatalogType::BiglakeMetastore
        | CatalogType::SnowflakeBuiltIn
        | CatalogType::SnowflakeNative
        | CatalogType::BigqueryNative
        | CatalogType::DuckdbNative
        | CatalogType::SparkNative => None,
    }
}

/// DuckDB-specific classification of a single catalog spec.
pub trait CatalogSpecDuckDbExt {
    /// `Some(..)` iff this catalog is an external Iceberg REST-attached catalog
    /// (IcebergRest/Horizon/Unity/Glue with `table_format: iceberg` and a `duckdb`
    /// config block), carrying its sanitized `ATTACH` alias.
    fn external_iceberg_attach(&self) -> Option<ExternalIcebergAttach>;

    fn resolved_attach_alias(&self) -> Option<String>;
}

impl CatalogSpecDuckDbExt for CatalogSpecView<'_> {
    fn external_iceberg_attach(&self) -> Option<ExternalIcebergAttach> {
        // DuckDB exposes these catalogs through Iceberg REST-style attachments.
        // Their information_schema coverage is incomplete, so schema-wide listing
        // is avoided while targeted DESCRIBE-based get_relation remains available.
        let is_external_iceberg = duckdb_attaches_via(self.catalog_type)
            == Some(DuckDbAttachKind::IcebergRest)
            && self.table_format.is_iceberg();
        if !is_external_iceberg {
            return None;
        }

        Some(ExternalIcebergAttach {
            attach_alias: self.resolved_attach_alias()?,
        })
    }

    /// Single source of truth for alias resolution, so routing and attachment
    /// can never drift apart.
    fn resolved_attach_alias(&self) -> Option<String> {
        let duckdb_block = self
            .config_block(AdapterType::DuckDB.as_ref())
            .or_else(|| self.config_block(AdapterType::LakeCompute.as_ref()))?;
        let alias = duckdb_block
            .get(dbt_yaml::Value::from("catalog_database"))
            .and_then(|value| value.as_str())
            .unwrap_or(self.name);
        Some(dbt_adapter_sql::ident::sanitize_identifier(
            alias,
            AdapterType::DuckDB,
        ))
    }
}
