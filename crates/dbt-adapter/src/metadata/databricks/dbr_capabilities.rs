//! DBR (Databricks Runtime) capability management system.
//!
//! Provides a centralized way to check for DBR version-gated features,
//! replacing scattered version comparisons with named capabilities.
//!
//! Reference: https://github.com/databricks/dbt-databricks/blob/25caa2a14ed0535f08f6fd92e29b39df1f453e4d/dbt/adapters/databricks/dbr_capabilities.py

use std::str::FromStr;

use crate::metadata::databricks::version::EngineVersion;

/// Named capabilities that depend on DBR version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbrCapability {
    Timestampdiff,
    Iceberg,
    CommentOnColumn,
    /// Reference: https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/dbr_capabilities.py#L17
    DescribeTableExtendedAsJson,
    JsonColumnMetadata,
    StreamingTableJsonMetadata,
    InsertByName,
    InsertByNameReplaceWhere,
    ReplaceOn,
}

impl DbrCapability {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Timestampdiff => "timestampdiff",
            Self::Iceberg => "iceberg",
            Self::CommentOnColumn => "comment_on_column",
            Self::DescribeTableExtendedAsJson => "describe_table_extended_as_json",
            Self::JsonColumnMetadata => "json_column_metadata",
            Self::StreamingTableJsonMetadata => "streaming_table_json_metadata",
            Self::InsertByName => "insert_by_name",
            Self::InsertByNameReplaceWhere => "insert_by_name_replace_where",
            Self::ReplaceOn => "replace_on",
        }
    }

    pub fn valid_names() -> &'static [&'static str] {
        &[
            "timestampdiff",
            "iceberg",
            "comment_on_column",
            "describe_table_extended_as_json",
            "json_column_metadata",
            "streaming_table_json_metadata",
            "insert_by_name",
            "insert_by_name_replace_where",
            "replace_on",
        ]
    }
}

impl FromStr for DbrCapability {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "timestampdiff" => Ok(Self::Timestampdiff),
            "iceberg" => Ok(Self::Iceberg),
            "comment_on_column" => Ok(Self::CommentOnColumn),
            "describe_table_extended_as_json" => Ok(Self::DescribeTableExtendedAsJson),
            "json_column_metadata" => Ok(Self::JsonColumnMetadata),
            "streaming_table_json_metadata" => Ok(Self::StreamingTableJsonMetadata),
            "insert_by_name" => Ok(Self::InsertByName),
            "insert_by_name_replace_where" => Ok(Self::InsertByNameReplaceWhere),
            "replace_on" => Ok(Self::ReplaceOn),
            _ => Err(format!(
                "Unknown DBR capability: '{}'. Valid capabilities are: {}",
                s,
                Self::valid_names().join(", ")
            )),
        }
    }
}

/// A capability's version-gating constraint, expressed as a minimum DBR
/// (major, minor) version, inclusive, plus whether SQL warehouses satisfy it.
#[derive(Clone, Copy, Debug)]
pub enum CapabilitySpec {
    /// Satisfied on SQL warehouses (always treated as running the latest
    /// DBR), and on classic clusters running DBR >= (major, minor).
    WarehouseSupported(i64, i64),
    /// Never satisfied on SQL warehouses, regardless of version. Satisfied
    /// only on classic clusters running DBR >= (major, minor).
    ClusterOnly(i64, i64),
}

impl CapabilitySpec {
    fn minimum_version(&self) -> (i64, i64) {
        match self {
            Self::WarehouseSupported(major, minor) | Self::ClusterOnly(major, minor) => {
                (*major, *minor)
            }
        }
    }
}

fn capability_spec(capability: DbrCapability) -> CapabilitySpec {
    match capability {
        DbrCapability::Timestampdiff => CapabilitySpec::WarehouseSupported(10, 4),
        DbrCapability::Iceberg => CapabilitySpec::WarehouseSupported(14, 3),
        DbrCapability::CommentOnColumn => CapabilitySpec::WarehouseSupported(16, 1),
        DbrCapability::DescribeTableExtendedAsJson => CapabilitySpec::WarehouseSupported(17, 3),
        DbrCapability::JsonColumnMetadata => CapabilitySpec::WarehouseSupported(16, 2),
        DbrCapability::StreamingTableJsonMetadata => CapabilitySpec::ClusterOnly(17, 1),
        DbrCapability::InsertByName => CapabilitySpec::WarehouseSupported(12, 2),
        // `BY NAME REPLACE WHERE` was added in DBR 18.0 (SPARK-54803); plain
        // `BY NAME` retains its DBR 12.2 floor. v1 reference:
        // https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/dbr_capabilities.py#L63-L68
        DbrCapability::InsertByNameReplaceWhere => CapabilitySpec::WarehouseSupported(18, 0),
        DbrCapability::ReplaceOn => CapabilitySpec::WarehouseSupported(17, 1),
    }
}

/// The Databricks compute a capability is being evaluated against.
///
/// SQL warehouses are always treated as running the latest DBR, so whether a
/// capability is available there is a fixed property of the capability
/// itself (`CapabilitySpec::WarehouseSupported` vs `ClusterOnly`). Classic
/// clusters report an actual DBR version, which may be unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DbrComputeContext {
    SqlWarehouse,
    Cluster(EngineVersion),
}

pub fn has_capability(capability: DbrCapability, context: DbrComputeContext) -> bool {
    match context {
        DbrComputeContext::SqlWarehouse => {
            let spec = capability_spec(capability);
            matches!(spec, CapabilitySpec::WarehouseSupported(..))
        }
        DbrComputeContext::Cluster(EngineVersion::Unset) => false,
        DbrComputeContext::Cluster(dbr_version) => {
            let spec = capability_spec(capability);
            let (major, minor) = spec.minimum_version();
            dbr_version >= EngineVersion::Full(major, minor)
        }
    }
}
