use crate::catalog_relation::CatalogRelation;
use crate::column::{BigqueryColumnMode, Column, ColumnBuilder};
use crate::config::AdapterConfig;
use crate::connection::{ConnectionGuard, borrow_tlocal_connection};
use crate::engine::{
    AdapterEngine, AdbcEngine, Options as ExecuteOptions, execute_query_with_retry,
};
use crate::errors::{
    AdapterError, AdapterErrorKind, adbc_error_to_adapter_error, arrow_error_to_adapter_error,
};
use crate::formatter::SqlLiteralFormatter;
use crate::formatter::format_sql_with_bindings;
use crate::macro_exec::{
    convert_macro_result_to_record_batch, execute_macro, execute_macro_with_package,
    execute_macro_wrapper, execute_macro_wrapper_with_package,
};
use crate::metadata::bigquery::nested_projection::render_struct_projection;
use crate::metadata::bigquery::{
    BIGQUERY_PSEUDOCOLUMNS, BigqueryMetadataAdapter, is_bigquery_not_found_error,
    nest_column_data_types,
};
use crate::metadata::clickhouse::ClickHouseMetadataAdapter;
use crate::metadata::databricks::DatabricksMetadataAdapter;
use crate::metadata::databricks::dbr_capabilities;
use crate::metadata::databricks::dbr_capabilities::DbrComputeContext;
use crate::metadata::databricks::version::EngineVersion;
use crate::metadata::duckdb::DuckDBMetadataAdapter;
use crate::metadata::duckdb::{classify_attach_entry, duckdb_table_format_for_database};
use crate::metadata::exasol::ExasolMetadataAdapter;
use crate::metadata::fabric::FabricMetadataAdapter;
use crate::metadata::postgres::PostgresMetadataAdapter;
use crate::metadata::redshift::RedshiftMetadataAdapter;
use crate::metadata::salesforce::SalesforceMetadataAdapter;
use crate::metadata::snowflake::SnowflakeMetadataAdapter;
use crate::metadata::{self, CatalogAndSchema, MetadataAdapter};
use crate::query_ctx::{node_id_from_state, query_ctx_from_state};
use crate::record_batch::{RecordBatchExt, RenamedColumn, StructArrayExt};
use crate::relation::Relation;
use crate::relation::RelationObject;
use crate::relation::config_v2::{ComponentConfigLoader, RelationConfig};
use crate::relation::databricks::config::DatabricksRelationMetadata;
use crate::relation::snowflake::config::{INTERACTIVE_TABLE_COLUMNS, INTERACTIVE_TABLE_KEY};
use crate::render_constraint::{render_column_constraint, warn_constraint_support};
use crate::response::AdapterResponse;
use crate::snapshots::SnapshotStrategy;
use crate::sql_types::TypeOps;
use crate::stmt_splitter::StmtSplitter;
use crate::value::*;
use crate::{AdapterResult, load_catalogs, python};

use adbc_core::options::OptionValue;
use arrow::array::{BooleanArray, RecordBatch, StringArray};
use arrow_array::{Array as _, ArrayRef, Decimal128Array};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};
use dashmap::DashMap;
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::is_keyword_ignore_ascii_case;
use dbt_adbc::bigquery::*;
use dbt_adbc::salesforce::DATA_TRANSFORM_RUN_TIMEOUT;
use dbt_adbc::{Connection, QueryCtx};
use dbt_agate::AgateTable;
use dbt_common::behavior_flags::{Behavior, BehaviorFlag};
use dbt_common::cancellation::CancellationToken;
use dbt_common::tracing::dbt_emit::{emit_info_log_message, emit_warn_log_message};
use dbt_common::{ErrorCode, FsResult, unexpected_fs_err};
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::dbt_types::RelationType;
use dbt_schemas::schemas::common::DbtIncrementalStrategy;
use dbt_schemas::schemas::common::DbtMaterialization;
use dbt_schemas::schemas::common::ResolvedQuoting;
use dbt_schemas::schemas::common::{ClusterConfig, Constraint, ConstraintSupport, PartitionConfig};
use dbt_schemas::schemas::common::{ConstraintType, normalize_quote};
use dbt_schemas::schemas::dbt_catalogs_v2::CatalogType;
use dbt_schemas::schemas::dbt_column::{DbtColumn, DbtColumnRef};
use dbt_schemas::schemas::manifest::BigqueryPartitionConfig;
use dbt_schemas::schemas::profiles::DuckDBPathInfo;
use dbt_schemas::schemas::project::ModelConfig;
use dbt_schemas::schemas::properties::ModelConstraint;
use dbt_schemas::schemas::relations::base::{BaseRelation, ComponentName, Policy};
use dbt_schemas::schemas::serde::{StringOrMap, minijinja_value_to_typed_struct};
use dbt_schemas::schemas::{CommonAttributes, InternalDbtNodeAttributes, InternalDbtNodeWrapper};
use dbt_yaml::Value as YmlValue;
use indexmap::IndexMap;
use minijinja::dispatch_object::DispatchObject;
use minijinja::value::{Object, ValueKind, ValueMap};
use minijinja::{self, invalid_argument, invalid_argument_inner};
use minijinja::{State, Value, args};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::str::FromStr;
use std::sync::{Arc, LazyLock};

use AdapterType::*;
use InnerAdapter::*;

static CREDENTIAL_IN_COPY_INTO_REGEX: Lazy<Regex> = Lazy::new(|| {
    // This is NOT the same as the Python regex used in dbt-databricks. Rust lacks lookaround.
    // This achieves the same result for the proper structure.  See original at time of port:
    // https://github.com/databricks/dbt-databricks/blob/66f513b960c62ee21c4c399264a41a56853f3d82/dbt/adapters/databricks/utils.py#L19
    Regex::new(r"credential\s*(\(\s*'[\w\-]+'\s*=\s*'.*?'\s*(?:,\s*'[\w\-]+'\s*=\s*'.*?'\s*)*\))")
        .expect("CREDENTIALS_IN_COPY_INTO_REGEX invalid")
});

/// Returns true if all non-null values in a Float64 column have zero fractional parts.
/// Equivalent to Python adapter's `convert_number_type` implementation.
///
/// An empty (or all-null) column returns `false`
fn try_to_int_col(col: &arrow_array::Float64Array) -> bool {
    if col.len() == col.null_count() {
        return false;
    }
    col.iter().all(|v| match v {
        None => true,
        Some(f) if f.is_nan() || f.is_infinite() => true,
        Some(f) => f.fract() == 0.0,
    })
}

fn warn_duplicate_columns(node_id: Option<String>) -> impl FnOnce(&[RenamedColumn<'_>]) {
    use std::fmt::Write;

    move |renamed: &[RenamedColumn<'_>]| {
        let mut msg = match &node_id {
            Some(id) => format!(
                "Query for node '{}' returned duplicate column names. \
                 Columns were renamed to ensure uniqueness: ",
                id
            ),
            None => "Query returned duplicate column names. \
                     Columns were renamed to ensure uniqueness: "
                .to_string(),
        };

        for (i, r) in renamed.iter().enumerate() {
            if i > 0 {
                msg.push_str(", ");
            }
            write!(msg, "'{}' -> '{}'", r.original, r.renamed).unwrap();
        }

        emit_warn_log_message(ErrorCode::DuplicateColumns, msg);
    }
}

#[cfg(debug_assertions)]
fn debug_compare_column_types(
    state: &State,
    relation: &dyn BaseRelation,
    adapter_impl: &AdapterImpl,
    mut from_local: Vec<Column>,
) {
    if std::env::var("DEBUG_COMPARE_LOCAL_REMOTE_COLUMNS_TYPES").is_ok() {
        match adapter_impl.get_columns_in_relation_uncached(state, relation) {
            Ok(mut from_remote) => {
                from_remote.sort_by(|a, b| a.name().cmp(b.name()));

                from_local.sort_by(|a, b| a.name().cmp(b.name()));

                println!("local vs remote mismatches");
                if !from_remote.is_empty() {
                    assert_eq!(from_local.len(), from_remote.len());
                    for (local, remote) in from_local.iter().zip(from_remote.iter()) {
                        let mismatch =
                            (local.dtype() != remote.dtype()) || (local.name() != remote.name());
                        if mismatch {
                            println!(
                                "adapter.get_columns_in_relation for {}",
                                relation.semantic_fqn()
                            );
                            println!(
                                "{}:{}  {}:{}",
                                local.name(),
                                local.dtype(),
                                remote.name(),
                                remote.dtype()
                            );
                        }
                    }
                } else {
                    println!("WARNING: from_remote is empty");
                }
            }
            Err(e) => {
                println!("Error getting columns in relation from remote: {e}");
            }
        }
    }
}

/// Read a boolean adapter config, tolerating the casing variants
/// dbt-core users may write in `profiles.yml`. Missing keys default to
/// `false`; unparseable values return a `Configuration` error.
pub(crate) fn get_bool_config(engine: &dyn AdapterEngine, key: &str) -> AdapterResult<bool> {
    dbt_common::string_utils::try_parse_bool_str(engine.config(key).as_deref(), key)
        .map(|o| o.unwrap_or(false))
        .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e.to_string()))
}

pub fn quote_ident(adapter_type: AdapterType, identifier: &str) -> String {
    let q = dbt_adapter_core::quote_char(adapter_type);
    format!("{q}{identifier}{q}")
}

pub fn quote_component(
    adapter_type: AdapterType,
    quoting: &ResolvedQuoting,
    identifier: &str,
    component: ComponentName,
) -> String {
    if quoting.must_quote(component) {
        quote_ident(adapter_type, identifier)
    } else {
        identifier.to_string()
    }
}

/// Returns the FQN for the current node's model.
pub fn database_schema_alias_from_state(state: &State) -> Option<(String, String, String)> {
    let model = state.lookup("model", &[])?;
    let database = model.get_attr("database").ok()?.as_str()?.to_string();
    let schema = model.get_attr("schema").ok()?.as_str()?.to_string();
    let alias = model.get_attr("alias").ok()?.as_str()?.to_string();
    Some((database, schema, alias))
}

/// Read the current model's `config.contract.alias_types` from Jinja state, defaulting
/// to `true` (dbt's default) when unavailable.
pub fn alias_types_from_state(state: &State) -> bool {
    state
        .lookup("model", &[])
        .and_then(|m| m.get_attr("config").ok())
        .and_then(|c| c.get_attr("contract").ok())
        .and_then(|c| c.get_attr("alias_types").ok())
        .and_then(|v| (v.kind() == ValueKind::Bool).then(|| v.is_true()))
        .unwrap_or(true)
}

/// Checks if the given [BaseRelation] matches the node currently being rendered
pub(crate) fn matches_current_relation(state: &State, relation: &dyn BaseRelation) -> bool {
    if let Some((database, schema, alias)) = database_schema_alias_from_state(state) {
        // Lowercase name comparison because relation names from the local project
        // are user specified, whereas the input relation may have been a normalized name
        // from the warehouse
        relation
            .database_as_str()
            .is_ok_and(|s| s.eq_ignore_ascii_case(&database))
            && relation
                .schema_as_str()
                .is_ok_and(|s| s.eq_ignore_ascii_case(&schema))
            && relation
                .identifier_as_str()
                .is_ok_and(|s| s.eq_ignore_ascii_case(&alias))
    } else {
        false
    }
}

/// Discriminator for the adapter implementation path.
///
/// Used by [AdapterImpl] methods to dispatch between the
/// live-database path and the recorded-trace replay path.
pub enum InnerAdapter<'a> {
    /// The standard implementation for running against live databases.
    Impl(AdapterType, &'a Arc<dyn AdapterEngine>),
    /// Delegates to a replay adapter for recorded trace playback.
    Replay(AdapterType, &'a dyn Replayer),
}

impl AdapterImpl {
    pub fn metadata_adapter(&self) -> Option<Box<dyn MetadataAdapter>> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.metadata_adapter(),
            Impl(_, engine) => {
                // In sidecar mode, schema hydration is handled via db_runner.
                if engine.is_sidecar() {
                    return None;
                }
                // The explicit mock adapter variant has no metadata adapter.
                if self.is_explicit_mock() {
                    return None;
                }
                let engine = Arc::clone(engine);
                let metadata_adapter =
                    match self.adapter_type() {
                        Snowflake => Box::new(SnowflakeMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Bigquery => Box::new(BigqueryMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Databricks | Spark => Box::new(DatabricksMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Redshift => Box::new(RedshiftMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Salesforce => Box::new(SalesforceMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Postgres => Box::new(PostgresMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        DuckDB => {
                            Box::new(DuckDBMetadataAdapter::new(engine)) as Box<dyn MetadataAdapter>
                        }
                        LakeCompute => {
                            Box::new(DuckDBMetadataAdapter::new(engine)) as Box<dyn MetadataAdapter>
                        }
                        Fabric => {
                            Box::new(FabricMetadataAdapter::new(engine)) as Box<dyn MetadataAdapter>
                        }
                        ClickHouse => Box::new(ClickHouseMetadataAdapter::new(engine))
                            as Box<dyn MetadataAdapter>,
                        Exasol => {
                            Box::new(ExasolMetadataAdapter::new(engine)) as Box<dyn MetadataAdapter>
                        }
                        Starburst => todo!("Starburst"),
                        Athena => todo!("Athena"),
                        Trino => todo!("Trino"),
                        Datafusion => todo!("Datafusion"),
                        Dremio => todo!("Dremio"),
                        Oracle => todo!("Oracle"),
                    };
                Some(metadata_adapter)
            }
        }
    }

    /// Execute `use warehouse [name]` statement for Snowflake.
    /// For other warehouses, this is noop.
    /// Returns whether the connection changed and must be restored.
    pub fn use_warehouse(
        &self,
        conn: &'_ mut dyn Connection,
        warehouse: String,
        node_id: &str,
        token: CancellationToken,
    ) -> FsResult<bool> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_use_warehouse(conn, warehouse, node_id),
            Impl(Snowflake, _) => {
                let ctx = QueryCtx::default().with_node_id(node_id);
                let sql = format!("use warehouse {warehouse}");
                self.exec_stmt(&ctx, conn, &sql, false, token)?;
                Ok(true)
            }
            Impl(..) => {
                debug_assert!(false, "use_warehouse is Snowflake-specific");
                Ok(false)
            }
        }
    }

    fn warehouse_restore_name(warehouse: &str) -> Cow<'_, str> {
        // `current_warehouse()` drops quotes. Removing them from a canonical
        // uppercase identifier preserves its identity; other quoted names need
        // their delimiters to preserve identity.
        let (unquoted, quoted) = normalize_quote(false, Snowflake, warehouse);
        // Not `need_quotes`/`must_be_quoted`: those accept Unicode letters and `-`.
        let is_canonical = unquoted
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_uppercase() || *byte == b'_')
            && unquoted.as_bytes().iter().skip(1).all(|byte| {
                byte.is_ascii_uppercase() || byte.is_ascii_digit() || [b'_', b'$'].contains(byte)
            })
            && is_keyword_ignore_ascii_case(&unquoted, Snowflake).is_none();
        if quoted && is_canonical {
            Cow::Owned(unquoted)
        } else if warehouse.contains('"') {
            // Quoted, or unbalanced quotes we must not reinterpret.
            Cow::Borrowed(warehouse)
        } else {
            Cow::Owned(warehouse.to_ascii_uppercase())
        }
    }

    /// Execute `use warehouse [name]` statement for Snowflake.
    /// For other warehouses, this is noop.
    ///
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/8b55a4781229a420836fdac6c933969f31188634/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L249-L273
    pub fn restore_warehouse(
        &self,
        conn: &'_ mut dyn Connection,
        node_id: &str,
        token: CancellationToken,
    ) -> FsResult<()> {
        match self.adapter_type() {
            Snowflake => {
                let warehouse = self.get_db_config("warehouse").ok_or_else(|| {
                    unexpected_fs_err!("'warehouse' not found in Snowflake DB config")
                })?;
                let warehouse = Self::warehouse_restore_name(&warehouse);
                let ctx = QueryCtx::default().with_node_id(node_id);
                let sql = format!("use warehouse {warehouse}");
                self.exec_stmt(&ctx, conn, &sql, false, token)?;
            }
            _ => debug_assert!(
                false,
                "only Snowflake adapter should call restore_warehouse"
            ),
        }
        Ok(())
    }

    /// Execute Redshift `USE <database>` so cross-database models run in the
    /// right scope. Only valid for Redshift.
    ///
    /// Ref: dbt-labs/dbt-adapters#1787
    pub fn use_database(
        &self,
        conn: &'_ mut dyn Connection,
        database: String,
        node_id: &str,
        token: CancellationToken,
    ) -> FsResult<()> {
        match self.adapter_type() {
            Redshift => {
                let ctx = QueryCtx::default().with_node_id(node_id);
                let sql = format!("USE {}", quote_ident(Redshift, &database));
                self.exec_stmt(&ctx, conn, &sql, false, token)?;
                Ok(())
            }
            other => {
                unreachable!("only Redshift adapter should call use_database, got {other:?}")
            }
        }
    }

    /// Execute `RESET USE` for Redshift after [`Self::use_database`] switched the
    /// active database. Only valid for Redshift.
    ///
    /// Ref: dbt-labs/dbt-adapters#1787
    pub fn reset_database(
        &self,
        conn: &'_ mut dyn Connection,
        node_id: &str,
        token: CancellationToken,
    ) -> FsResult<()> {
        match self.adapter_type() {
            Redshift => {
                let ctx = QueryCtx::default().with_node_id(node_id);
                self.exec_stmt(&ctx, conn, "RESET USE", false, token)?;
            }
            other => {
                unreachable!("only Redshift adapter should call reset_database, got {other:?}")
            }
        }
        Ok(())
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L655
    pub fn cache_added(
        &self,
        _state: &State,
        relation: Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        let _ = self
            .engine()
            .relation_cache()
            .insert_relation(relation, None);
        Ok(none_value())
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L666
    pub fn cache_dropped(
        &self,
        _state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        let _ = self
            .engine()
            .relation_cache()
            .drop_relation_cascade(relation.as_ref());
        Ok(none_value())
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L678
    pub fn cache_renamed(
        &self,
        _state: &State,
        from_relation: &Arc<dyn BaseRelation>,
        to_relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        let _ = self
            .engine()
            .relation_cache()
            .rename_relation(from_relation.as_ref(), Arc::clone(to_relation));
        Ok(none_value())
    }

    pub fn get_db_config(&self, key: &str) -> Option<Cow<'_, str>> {
        self.engine().config(key)
    }

    pub fn get_db_config_value(&self, key: &str) -> Option<&YmlValue> {
        let engine = self.engine();
        if engine.get_config().contains_key(key) {
            return engine.get_config().get(key);
        }
        None
    }

    /// ClickHouse `adapter.get_credentials(connection_overrides)` (impl.py):
    /// connection parameters for dictionary SOURCE(CLICKHOUSE(...)) clauses.
    /// Profile values merged with the model's `connection_overrides`; override
    /// keys whose final value is falsy are removed.
    pub fn get_credentials(&self, connection_overrides: &Value) -> Value {
        let mut credentials: Vec<(String, Value)> = vec![
            (
                "user".to_string(),
                Value::from(
                    self.get_db_config("user")
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|| "default".to_string()),
                ),
            ),
            (
                "password".to_string(),
                Value::from(
                    self.get_db_config("password")
                        .map(|v| v.into_owned())
                        .unwrap_or_default(),
                ),
            ),
            (
                "database".to_string(),
                Value::from(
                    self.get_db_config("database")
                        .map(|v| v.into_owned())
                        .unwrap_or_default(),
                ),
            ),
            (
                "host".to_string(),
                Value::from(
                    self.get_db_config("host")
                        .map(|v| v.into_owned())
                        .unwrap_or_else(|| "localhost".to_string()),
                ),
            ),
            (
                "port".to_string(),
                Value::from(
                    self.get_db_config("port")
                        .map(|v| v.into_owned())
                        .unwrap_or_default(),
                ),
            ),
        ];

        let mut override_keys: Vec<String> = Vec::new();
        if let Ok(keys) = connection_overrides.try_iter() {
            for key in keys {
                let Some(name) = key.as_str() else { continue };
                let Ok(value) = connection_overrides.get_item(&key) else {
                    continue;
                };
                override_keys.push(name.to_string());
                if let Some(entry) = credentials.iter_mut().find(|(k, _)| k == name) {
                    entry.1 = value;
                } else {
                    credentials.push((name.to_string(), value));
                }
            }
        }
        // Python: overridden keys with falsy final values are dropped.
        credentials.retain(|(k, v)| !override_keys.contains(k) || v.is_true());

        Value::from(credentials.into_iter().collect::<BTreeMap<String, Value>>())
    }

    /// Returns the table format string for `database` (e.g. `"ducklake"`, `"iceberg"`, `"default"`).
    ///
    /// Mirrors the Python reference implementation in dbt-duckdb:
    /// https://github.com/duckdb/dbt-duckdb/blob/main/dbt/adapters/duckdb/credentials.py
    pub fn table_format_for_database(&self, database: &str) -> &'static str {
        // DIVERGENCE: the lake compute arm has no Python counterpart -- lake compute
        // is Fusion-only, and it shares the DuckDB macros this feeds.
        //
        // Lake compute reads and writes open Iceberg tables, and it attaches the
        // catalogs holding them server-side -- so there is no profile `attach:`
        // entry and, in the Lake Compute + MDLS setup, no catalogs.yml entry for
        // the DuckDB lookups below to find.
        if self.adapter_type() == LakeCompute {
            return duckdb_table_format_for_database(database).unwrap_or("iceberg");
        }
        if self.adapter_type() != DuckDB {
            return "default";
        }

        let path_config = self.get_db_config("path");
        let path_info = DuckDBPathInfo::parse_path(path_config.as_deref());
        let primary_database = self
            .get_db_config("database")
            .map(|value| value.into_owned())
            .unwrap_or_else(|| path_info.database.to_owned());
        let primary_is_ducklake = self
            .get_db_config_value("is_ducklake")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
            || path_info.is_ducklake;
        if primary_is_ducklake && primary_database.eq_ignore_ascii_case(database) {
            return "ducklake";
        }

        // Check v2 catalogs first: if this database matches an attached catalog,
        // return the appropriate table format so that macros can skip CASCADE / ALTER TABLE RENAME.
        if let Some(fmt) = duckdb_table_format_for_database(database) {
            return fmt;
        }

        // Legacy path: check profile-level attach: entries. Each entry resolves
        // independently to an (alias, format_str) or is skipped.
        let Some(attach_val) = self.get_db_config_value("attach") else {
            return "default";
        };
        let YmlValue::Sequence(seq, _) = attach_val else {
            return "default";
        };
        seq.iter()
            .filter_map(classify_attach_entry)
            .find(|(alias, _)| alias.eq_ignore_ascii_case(database))
            .map(|(_, fmt)| fmt)
            .unwrap_or("default")
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1749
    /// PostgresAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-postgres/src/dbt/adapters/postgres/impl.py#L175
    /// RedshiftAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-redshift/src/dbt/adapters/redshift/impl.py#L490
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L502
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L808
    pub fn valid_incremental_strategies(&self) -> &[DbtIncrementalStrategy] {
        use DbtIncrementalStrategy::*;

        match self.adapter_type() {
            Postgres | DuckDB | LakeCompute => &[Append, DeleteInsert, Merge, Microbatch],
            Snowflake => &[Append, DeleteInsert, InsertOverwrite, Merge, Microbatch],
            Bigquery => &[Append],
            Databricks => &[
                Append,
                DeleteInsert,
                Merge,
                InsertOverwrite,
                ReplaceWhere,
                Microbatch,
            ],
            Redshift => &[Append, DeleteInsert, Merge, Microbatch],
            Fabric => &[Append, DeleteInsert, Merge, Microbatch],
            Salesforce => &[Append, Merge],
            ClickHouse => &[Append, DeleteInsert, InsertOverwrite, Microbatch, Legacy],
            Spark => &[Append, Merge, InsertOverwrite, Microbatch],
            Exasol => &[Append, DeleteInsert, Merge, Microbatch],
            Athena | Starburst | Trino | Datafusion | Dremio | Oracle => {
                unimplemented!("valid_incremental_strategies not implemented")
            }
        }
    }

    /// Redact credentials expressions from DDL statements
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L833
    pub fn redact_credentials(&self, sql: &str) -> AdapterResult<String> {
        if self.adapter_type() != Databricks {
            return Err(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "redact_credentials is a Databricks-specific function",
            ));
        }
        let Some(caps) = CREDENTIAL_IN_COPY_INTO_REGEX.captures(sql) else {
            // WARN: Malformed input by user means credentials may leak.
            // However, this _is_ the fallback strategy implemented in Python.
            return Ok(sql.to_string());
        };

        // Capture the full matched credential(...) string, including the surrounding parentheses.
        // Then extract only the inner key-value content
        let full_parens = caps.get(1).unwrap().as_str();
        let inner = &full_parens[1..full_parens.len() - 1];

        let redacted_pairs = inner
            .split(',')
            .map(|pair| {
                let key = pair.split('=').next().unwrap_or("").trim();
                format!("{key} = '[REDACTED]'")
            })
            .collect::<Vec<_>>()
            .join(", ");

        let redacted_sql = sql.replacen(full_parens, &format!("({redacted_pairs})"), 1);

        Ok(redacted_sql)
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L505
    pub fn get_partitions_metadata(
        &self,
        _state: &State,
        _relation: &dyn BaseRelation,
    ) -> Result<Value, minijinja::Error> {
        unimplemented!("get_partitions_metadata")
    }

    /// Borrow the current thread-local connection or create one if it's not set yet.
    ///
    /// A guard is returned. When destroyed, the guard returns the connection to
    /// the thread-local variable. If another connection became the thread-local
    /// in the mean time, that connection is dropped and the return proceeds as
    /// normal.
    pub fn borrow_tlocal_connection(
        &self,
        state: Option<&State>,
        node_id: Option<String>,
    ) -> Result<ConnectionGuard<'_>, AdapterError> {
        borrow_tlocal_connection(self.engine().as_ref(), state, node_id)
    }

    /// Helper method for execute
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub fn execute_inner(
        &self,
        engine: Arc<dyn AdapterEngine>,
        state: Option<&State>,
        conn: &'_ mut dyn Connection,
        ctx: &QueryCtx,
        sql: &str,
        _auto_begin: bool,
        fetch: bool,
        _limit: Option<i64>,
        options: Option<ExecuteOptions>,
        token: CancellationToken,
    ) -> AdapterResult<(AdapterResponse, AgateTable)> {
        let splitter = engine.splitter();
        let adapter_type = self.adapter_type();
        let all_stmts = match adapter_type {
            // BigQuery, DuckDB, and lake compute support multi-statement execution.
            //
            // BigQuery: https://cloud.google.com/bigquery/docs/reference/standard-sql/procedural-language
            //
            // DuckDB: temp tables are connection-scoped; batching CREATE TEMP + DML in one
            // execute() call avoids the need for cross-call connection caching.
            //
            // Lake compute: also supports batching
            Bigquery | DuckDB | LakeCompute => vec![sql],
            _ => splitter.split(sql, adapter_type),
        };
        // Filter out empty and comment-only statements.
        let statements = all_stmts
            .into_iter()
            .filter(|stmt| !splitter.is_empty(stmt, adapter_type))
            .collect::<Vec<_>>();
        if statements.is_empty() {
            return Ok((AdapterResponse::default(), AgateTable::default()));
        }

        let mut options = options.unwrap_or_default();
        if let Some(state) = state {
            options.extend(self.get_adbc_execute_options(state));
        }

        // Configure warehouse specific options
        #[allow(clippy::single_match)]
        match self.adapter_type() {
            Salesforce => {
                if let Some(timeout) = engine.config("data_transform_run_timeout") {
                    let timeout = timeout.parse::<i64>().map_err(|e| {
                        AdapterError::new(
                            AdapterErrorKind::Configuration,
                            format!("data_transform_run_timeout must be an integer string: {e}",),
                        )
                    })?;
                    options.push((
                        DATA_TRANSFORM_RUN_TIMEOUT.to_string(),
                        OptionValue::Int(timeout),
                    ));
                }
            }
            _ => {}
        }

        let mut last_batch = None;
        for sql in statements {
            last_batch = Some(execute_query_with_retry(
                engine.clone(),
                state,
                conn,
                ctx,
                sql,
                1,
                &options,
                fetch,
                token.clone(),
            )?);
        }

        let last_batch = last_batch.expect("last_batch should never be None");

        let response = AdapterResponse::from_record_batch(&last_batch, self.adapter_type())
            .with_connection_info(self.adapter_type(), engine.as_ref());

        // Deduplicate column names to match dbt-core's behavior, which renames
        // duplicate columns to `col_2`, `col_3`, etc.
        // BigQuery is the exception to this deduping
        let last_batch = match self.adapter_type() {
            Bigquery => last_batch,
            _ => {
                let node_id = state.and_then(node_id_from_state);
                last_batch.disambiguate_column_names(Some(warn_duplicate_columns(node_id)))
            }
        };

        // Flatten nested struct fields as JSON-strings (some Core adapters do that)
        let last_batch = match self.adapter_type() {
            Databricks => last_batch.jsonify_nested_columns(),
            _ => last_batch,
        };

        let table = AgateTable::from_record_batch(Arc::new(last_batch));

        Ok((response, table))
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L453
    #[allow(clippy::too_many_arguments)]
    pub fn execute(
        &self,
        state: Option<&State>,
        conn: &'_ mut dyn Connection,
        ctx: Option<&QueryCtx>,
        sql: &str,
        auto_begin: bool,
        fetch: bool,
        limit: Option<i64>,
        options: Option<ExecuteOptions>,
        token: CancellationToken,
    ) -> AdapterResult<(AdapterResponse, AgateTable)> {
        if self.mock_state().is_some() {
            if !self.introspect_enabled() {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    "Introspective queries are disabled (--no-introspect).",
                ));
            }
            let response = AdapterResponse::new()
                .with_message("execute".to_string())
                .with_code(sql.to_string())
                .with_rows_affected(1);

            let schema = Arc::new(Schema::new(vec![Field::new(
                "names",
                DataType::Decimal128(38, 10),
                true,
            )]));
            let decimal_array: ArrayRef = Arc::new(Decimal128Array::from(vec![Some(42)]));
            let batch = RecordBatch::try_new(schema, vec![decimal_array]).unwrap();

            let table = AgateTable::from_record_batch(Arc::new(batch));

            return Ok((response, table));
        }
        let ctx = match ctx.map(Cow::Borrowed) {
            Some(ctx) => ctx,
            None => {
                let ctx = match state {
                    Some(s) => query_ctx_from_state(s)?,
                    None => QueryCtx::default(),
                }
                .with_desc("execute adapter call");
                Cow::Owned(ctx)
            }
        };
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_execute(
                state,
                conn,
                ctx.as_ref(),
                sql,
                auto_begin,
                fetch,
                limit,
                options,
            ),
            Impl(_, engine) => self.execute_inner(
                Arc::clone(engine),
                state,
                conn,
                ctx.as_ref(),
                sql,
                auto_begin,
                fetch,
                limit,
                options,
                token,
            ),
        }
    }

    /// Execute a statement, expect no results.
    pub fn exec_stmt(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        auto_begin: bool,
        token: CancellationToken,
    ) -> AdapterResult<AdapterResponse> {
        // default values are the same as in dispatch_adapter_calls()
        let (response, _) = self.execute(
            None,       // empty state
            conn,       // connection
            Some(ctx),  // context around the SQL string
            sql,        // the SQL string
            auto_begin, // auto_begin
            false,      // fetch
            None,       // limit
            None,       // options
            token,
        )?;
        Ok(response)
    }

    /// Execute a query and get results in an [AgateTable].
    pub fn query(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        limit: Option<i64>,
        token: CancellationToken,
    ) -> AdapterResult<(AdapterResponse, AgateTable)> {
        self.execute(
            None,      // state
            conn,      // connection
            Some(ctx), // context around the SQL string
            sql,       // the SQL string
            false,     // auto_begin
            true,      // fetch
            limit,     // limit
            None,      // options
            token,
        )
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L55
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L573
    #[allow(clippy::too_many_arguments)]
    pub fn add_query(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        sql: &str,
        auto_begin: bool,
        bindings: Option<&Value>,
        abridge_sql_log: bool,
        token: CancellationToken,
    ) -> AdapterResult<()> {
        if self.mock_state().is_some() {
            unimplemented!("query addition to connection in MockAdapter")
        }
        let sql = if let Some(bindings) = bindings {
            Cow::Owned(format_sql_with_bindings(
                self.adapter_type(),
                sql,
                bindings,
            )?)
        } else {
            Cow::Borrowed(sql)
        };
        let ctx = query_ctx_from_state(state)?.with_desc("add_query adapter call");
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_add_query(
                &ctx,
                conn,
                sql.as_ref(),
                auto_begin,
                bindings,
                abridge_sql_log,
            ),
            Impl(Bigquery, _) => {
                // Bigquery does not support add_query
                Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    "bigquery.add_query",
                ))
            }
            Impl(_, engine) => {
                self.execute_inner(
                    Arc::clone(engine),
                    None,
                    conn,
                    &ctx,
                    sql.as_ref(),
                    auto_begin,
                    false,
                    None,
                    None,
                    token,
                )?;
                Ok(())
            }
        }
    }

    /// Submit Python job
    ///
    /// Executes Python code in the warehouse's Python runtime.
    /// Default implementation raises Internal error.
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1727
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L417
    pub fn submit_python_job(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        state: &State,
        model: &Value,
        compiled_code: &str,
        token: CancellationToken,
    ) -> AdapterResult<AdapterResponse> {
        match self.inner_adapter() {
            Impl(Snowflake, engine) => {
                let code = python::snowflake::finalize_python_code(state, model, compiled_code)?;
                let (response, _) = self.execute_inner(
                    Arc::clone(engine),
                    Some(state),
                    conn,
                    ctx,
                    &code,
                    false,
                    false,
                    None,
                    None,
                    token,
                )?;
                Ok(response)
            }
            Replay(Snowflake, replay) => {
                let code = python::snowflake::finalize_python_code(state, model, compiled_code)?;
                // In DBT Replay mode, route through the replay adapter to consume recorded execute calls.
                let (response, _) = replay.replay_execute(
                    Some(state),
                    conn,
                    ctx,
                    &code,
                    false,
                    false,
                    None,
                    None,
                )?;
                Ok(response)
            }
            // https://docs.getdbt.com/docs/core/connect-data-platform/bigquery-setup#running-python-models-on-bigquery-dataframes
            // https://docs.getdbt.com/reference/resource-configs/bigquery-configs#python-model-configuration
            Impl(Bigquery, _) => python::bigquery::submit_python_job(
                self,
                ctx,
                conn,
                state,
                model,
                compiled_code,
                token,
            ),
            // https://docs.getdbt.com/reference/resource-configs/databricks-configs
            Impl(Databricks, _) => {
                python::databricks::submit_python_job(self, ctx, conn, state, model, compiled_code)
            }
            Replay(Bigquery | Databricks, replay) => {
                replay.replay_submit_python_job(ctx, conn, state, model, compiled_code)
            }
            Replay(
                adapter_type @ (Postgres | Redshift | Salesforce | DuckDB | LakeCompute | Spark
                | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
                | Datafusion | Dremio | Oracle),
                _,
            )
            | Impl(
                adapter_type @ (Postgres | Redshift | Salesforce | DuckDB | LakeCompute | Spark
                | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
                | Datafusion | Dremio | Oracle),
                _,
            ) => Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!("Python models are not supported for {adapter_type} adapter",),
            )),
        }
    }

    /// Wrap the identifier in the appropriate quoting character for the adapter.
    ///
    /// Assumes the identifier is not quoted.
    ///
    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L214
    pub fn quote(&self, identifier: &str) -> String {
        quote_ident(self.adapter_type(), identifier)
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L217
    /// AthenaAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-athena/src/dbt/adapters/athena/impl.py#L1154
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L299
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L205
    pub fn list_schemas(&self, state: &State, database: &str) -> AdapterResult<Vec<String>> {
        match self.adapter_type() {
            Bigquery if self.mock_state().is_none() => {
                // BigQuery lists datasets through the ADBC metadata API
                // rather than a SQL query.
                // See: https://github.com/dbt-labs/dbt-core/issues/14631
                self.list_schemas_via_adbc(state, database)
            }
            Snowflake | Databricks | Redshift | Spark | DuckDB | Postgres | Salesforce | Fabric
            | ClickHouse | Exasol | Athena | Starburst | Trino | Datafusion | Dremio | Oracle
            | LakeCompute | Bigquery => {
                use crate::macro_exec::execute_macro_wrapper;
                use minijinja::value::{Kwargs, Value};

                let kwargs = Kwargs::from_iter([("database", Value::from(database))]);
                let result = execute_macro_wrapper(state, &[Value::from(kwargs)], "list_schemas")?;

                self.list_schemas_inner(result)
            }
        }
    }

    fn list_schemas_via_adbc(&self, state: &State, database: &str) -> AdapterResult<Vec<String>> {
        let conn = self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
        let (catalog, _) = normalize_quote(false, self.adapter_type(), database);

        let reader = conn
            .get_objects(
                adbc_core::options::ObjectDepth::Schemas,
                Some(&catalog),
                None,
                None,
                None,
                None,
            )
            .map_err(adbc_error_to_adapter_error)?;

        let schema = reader.schema();
        let batches = reader
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| AdapterError::new(AdapterErrorKind::Driver, e.to_string()))?;
        let batch = arrow::compute::concat_batches(&schema, &batches)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Driver, e.to_string()))?;

        // The schema names are nested in the result of the get object call.
        // The batch has the following shape at the top level:
        // - catalog_name: utf8
        // - catalog_db_schemas: list[struct]
        //
        // Each row of the column `catalog_db_schemas` is a list of elements
        // with the shape:
        //   - db_schema_name: utf8
        //   - db_schema_tables: list
        let catalog_db_schemas = batch
            .column_by_name("catalog_db_schemas")
            .and_then(|c| c.as_any().downcast_ref::<arrow::array::ListArray>())
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    "Missing or invalid 'catalog_db_schemas' column",
                )
            })?;

        let schemas_struct = catalog_db_schemas
            .values()
            .as_any()
            .downcast_ref::<arrow::array::StructArray>()
            .ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    "Missing or invalid 'catalog_db_schemas' values",
                )
            })?;
        let db_schema_names = schemas_struct
            .column_as::<StringArray>("db_schema_name")?
            .iter()
            .flatten();

        // In Core, `list_datasets` is called without the `include_all` parameter,
        // which filters out hidden datasets.
        // Reference: https://github.com/dbt-labs/dbt-adapters/blob/860da89225e2ecf1bf47038f5ac40d4eaa4019a2/dbt-bigquery/src/dbt/adapters/bigquery/connections.py#L619
        let db_schema_names = db_schema_names
            .filter(|s| !s.starts_with('_'))
            .map(|s| s.to_string())
            .collect();

        Ok(db_schema_names)
    }

    pub fn list_schemas_inner(&self, result_set: Arc<RecordBatch>) -> AdapterResult<Vec<String>> {
        if self.mock_state().is_some() {
            return Ok(vec![]);
        }
        let schema_column_values = {
            let col_name = match self.adapter_type() {
                Snowflake | Salesforce => "name",
                Databricks => "databaseName",
                // `show databases` returns a single column whose name varies by Spark
                // build/catalog: OSS Spark (3.0+) emits `namespace`, older/Hive-style
                // builds emit `databaseName`; pick whichever name the result actually
                // has so we stay version-agnostic.
                Spark => {
                    if result_set.column_by_name("databaseName").is_some() {
                        "databaseName"
                    } else {
                        "namespace"
                    }
                }
                Bigquery => "schema_name",
                Redshift => {
                    if get_bool_config(self.engine().as_ref(), "datasharing")? {
                        "schema_name"
                    } else {
                        "nspname"
                    }
                }
                Postgres => "nspname",
                DuckDB => "schema_name",
                LakeCompute => "schema_name",
                Fabric => "schema",
                // https://github.com/ClickHouse/dbt-clickhouse/blob/main/dbt/include/clickhouse/macros/adapters.sql
                ClickHouse => "name",
                Exasol => "name",
                Starburst => todo!("Starburst"),
                Athena => todo!("Athena"),
                Trino => todo!("Trino"),
                Datafusion => todo!("Datafusion"),
                Dremio => todo!("Dremio"),
                Oracle => todo!("Oracle"),
            };
            result_set.column_values::<StringArray>(col_name)?
        };

        let n = result_set.num_rows();
        let mut schemas = Vec::<String>::with_capacity(n);
        for i in 0..n {
            let name: &str = schema_column_values.value(i);
            schemas.push(name.to_string());
        }
        Ok(schemas)
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L166
    pub fn create_schema(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        // Matches upstream: the identifier is stripped here, before the macro runs, so
        // `create_schema`/`drop_schema` macros can render the relation as-is.
        let relation = relation.without_identifier()?;
        let args = [RelationObject::new(relation).into_value()];
        execute_macro(state, &args, "create_schema")?;
        Ok(none_value())
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L177
    pub fn drop_schema(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        self.engine()
            .relation_cache()
            .evict_schema_for_relation(relation.as_ref());
        let relation = relation.without_identifier()?;
        let dropped_schema = relation.schema().map(str::to_string);
        let args = [RelationObject::new(relation).into_value()];
        execute_macro(state, &args, "drop_schema")?;
        // httpclient.py `database_dropped` parity: after dropping the
        // connection's own default database, clear it so follow-up statements
        // don't fail with UNKNOWN_DATABASE (falls back to the user's default
        // database).
        if self.adapter_type() == ClickHouse
            && !self.engine().is_mock()
            && let Some(dropped_schema) = dropped_schema
            && crate::engine::clickhouse::target_schema(self.engine().get_config()).as_deref()
                == Some(dropped_schema.as_str())
        {
            let mut conn = self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
            conn.set_option(
                adbc_core::options::OptionConnection::CurrentSchema,
                OptionValue::String(String::new()),
            )
            .map_err(adbc_error_to_adapter_error)?;
        }
        Ok(none_value())
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L894
    pub fn valid_snapshot_target(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
        column_names: Option<BTreeMap<String, String>>,
    ) -> AdapterResult<()> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_valid_snapshot_target(state, relation, column_names),
            Impl(_, _engine) => {
                let no_strategy = SnapshotStrategy {
                    unique_key: None,
                    updated_at: None,
                    row_changed: None,
                    scd_id: None,
                    hard_deletes: None,
                };

                self.assert_valid_snapshot_target_given_strategy(
                    state,
                    relation,
                    column_names,
                    Arc::new(no_strategy),
                )
            }
        }
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1769
    pub fn get_incremental_strategy_macro(
        &self,
        state: &State,
        strategy: &str,
    ) -> Result<Value, minijinja::Error> {
        if strategy != "default" {
            let strategy_ = DbtIncrementalStrategy::from_str(strategy)
                .map_err(|e| invalid_argument_inner!("Invalid strategy value {}", e))?;
            if !self.valid_incremental_strategies().contains(&strategy_)
                && builtin_incremental_strategies().contains(&strategy_)
            {
                return invalid_argument!(
                    "The incremental strategy '{}' is not valid for this adapter",
                    strategy
                );
            }
        }

        let strategy = strategy.replace("+", "_");
        let macro_name = format!("get_incremental_{strategy}_sql");

        // Return the macro
        Ok(Value::from_object(DispatchObject {
            macro_name,
            package_name: None,
            strict: false,
            auto_execute: false,
            context: Some(state.get_base_context()),
        }))
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1047
    #[allow(clippy::too_many_arguments)]
    pub fn get_relation(
        &self,
        state: &State,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        database: &str,
        schema: &str,
        identifier: &str,
        token: CancellationToken,
    ) -> AdapterResult<Option<Arc<dyn BaseRelation>>> {
        if self.mock_state().is_some() {
            if !self.introspect_enabled() {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    "Introspective queries are disabled (--no-introspect).",
                ));
            }
            let relation = Relation::new(
                Snowflake,
                database.to_string(),
                schema.to_string(),
                identifier.to_string(),
            )
            .with_quoting(self.quoting())
            .validate()?;
            return Ok(Some(Arc::new(relation)));
        }
        match self.inner_adapter() {
            Replay(_, replay) => {
                replay.replay_get_relation(state, ctx, conn, database, schema, identifier)
            }
            Impl(adapter_type, engine) if engine.is_sidecar() => {
                let client = engine.sidecar_client().unwrap();
                let query_schema = schema.to_string();
                let query_identifier = identifier.to_string();
                let relation_type = client.get_relation_type(&query_schema, &query_identifier)?;
                match relation_type {
                    Some(rel_type) => {
                        let relation = crate::relation::do_create_relation(
                            adapter_type,
                            database.to_string(),
                            schema.to_string(),
                            Some(identifier.to_string()),
                            Some(rel_type),
                            self.quoting(),
                        )?;
                        Ok(Some(relation.into()))
                    }
                    None => Ok(None),
                }
            }
            Impl(_, _engine) => {
                let relation_opt = metadata::get_relation::get_relation(
                    self, state, ctx, conn, database, schema, identifier, token,
                )?;
                let relation =
                    relation_opt.map(|relation| -> Arc<dyn BaseRelation> { relation.into() });
                Ok(relation)
            }
        }
    }

    /// Get a catalog relation, which in Core is a serialized type.
    /// In Fusion, we treat it as a Jinja accessible flat container of values
    /// needed for Iceberg ddl generation.
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L350
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1384
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L539
    pub fn build_catalog_relation(&self, model: &Value) -> AdapterResult<CatalogRelation> {
        CatalogRelation::from_model_config_and_catalogs(
            self.adapter_type(),
            model,
            load_catalogs::fetch_catalogs(),
        )
    }

    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L559
    pub fn describe_dynamic_table(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        include_transient: bool,
        token: CancellationToken,
    ) -> Result<Value, minijinja::Error> {
        let adapter_type = self.adapter_type();
        match adapter_type {
            Snowflake => {
                let ctx = query_ctx_from_state(state)?.with_desc("describe_dynamic_table");

                let quoting = relation.quote_policy();

                let schema = if quoting.schema {
                    relation.schema_as_quoted_str()?
                } else {
                    relation.schema_as_str()?
                };

                let database = if quoting.database {
                    relation.database_as_quoted_str()?
                } else {
                    relation.database_as_str()?
                };

                let lit_fmt = SqlLiteralFormatter::new(adapter_type);
                let show_sql = format!(
                    "show dynamic tables like {} in schema {database}.{schema}",
                    lit_fmt.format_str(&relation.identifier_as_str()?)
                );

                let (_, table) = self.query(&ctx, conn, &show_sql, None, token.clone())?;

                let table = table
                    .rename(Some(table.column_names()), None, false, false)?
                    .select(&[
                        "name".to_string(),
                        "schema_name".to_string(),
                        "database_name".to_string(),
                        "text".to_string(),
                        "target_lag".to_string(),
                        "scheduler".to_string(),
                        "warehouse".to_string(),
                        "refresh_mode".to_string(),
                        "initialization_warehouse".to_string(),
                        "immutable_where".to_string(),
                        "cluster_by".to_string(),
                    ]);

                // SHOW DYNAMIC TABLES does not expose transient status, so we need to run SHOW
                // TABLES if we need to check transient
                let table = if include_transient {
                    let show_tables_sql = format!(
                        "show tables like {} in schema {database}.{schema}",
                        lit_fmt.format_str(&relation.identifier_as_str()?)
                    );
                    let (_, tables) = self.query(&ctx, conn, &show_tables_sql, None, token)?;
                    let tables = tables.rename(Some(tables.column_names()), None, false, false)?;
                    let tables_batch = tables.to_record_batch();
                    let is_transient = if tables_batch.num_rows() > 0 {
                        tables_batch
                            .column_values::<StringArray>("kind")
                            .ok()
                            .map(|col| col.value(0).eq_ignore_ascii_case("TRANSIENT"))
                            .unwrap_or(false)
                    } else {
                        false
                    };

                    // Fold the transient column into the SHOW DYNAMIC TABLES result
                    let record_batch = table.to_record_batch();
                    let num_rows = record_batch.num_rows();
                    let transient_col: ArrayRef =
                        Arc::new(BooleanArray::from(vec![Some(is_transient); num_rows]));
                    let mut fields: Vec<Arc<Field>> =
                        record_batch.schema().fields().iter().cloned().collect();
                    fields.push(Arc::new(Field::new("transient", DataType::Boolean, true)));
                    let new_schema = Arc::new(Schema::new(fields));
                    let mut columns = record_batch.columns().to_vec();
                    columns.push(transient_col);
                    let new_batch = RecordBatch::try_new(new_schema, columns).map_err(|e| {
                        minijinja::Error::new(minijinja::ErrorKind::InvalidOperation, e.to_string())
                    })?;
                    AgateTable::from_record_batch(Arc::new(new_batch))
                } else {
                    table
                };

                Ok(Value::from(ValueMap::from([(
                    Value::from("dynamic_table"),
                    Value::from_object(table),
                )])))
            }
            Postgres | Bigquery | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                let err = format!(
                    "describe_dynamic_table is not supported by the {} adapter",
                    adapter_type
                );
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    err,
                ))
            }
        }
    }

    /// `SHOW INTERACTIVE TABLES` has no transient status, so unlike `describe_dynamic_table`
    /// this never runs a second `SHOW TABLES` to fold `transient` in.
    ///
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/2d27c26df1a4b71144cd4585cfdefac39cd311bc/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L739-L775
    pub fn describe_interactive_table(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        token: CancellationToken,
    ) -> Result<Value, minijinja::Error> {
        let adapter_type = self.adapter_type();
        match adapter_type {
            Snowflake => {
                let ctx = query_ctx_from_state(state)?.with_desc("describe_interactive_table");

                let quoting = relation.quote_policy();

                let schema = if quoting.schema {
                    relation.schema_as_quoted_str()?
                } else {
                    relation.schema_as_str()?
                };

                let database = if quoting.database {
                    relation.database_as_quoted_str()?
                } else {
                    relation.database_as_str()?
                };

                let lit_fmt = SqlLiteralFormatter::new(adapter_type);
                let show_sql = format!(
                    "show interactive tables like {} in schema {database}.{schema}",
                    lit_fmt.format_str(&relation.identifier_as_str()?)
                );

                let (_, table) = self.query(&ctx, conn, &show_sql, None, token)?;

                let table = table
                    .rename(Some(table.column_names()), None, false, false)?
                    .select(
                        &INTERACTIVE_TABLE_COLUMNS
                            .iter()
                            .map(|s| s.to_string())
                            .collect::<Vec<_>>(),
                    );

                Ok(Value::from(ValueMap::from([(
                    Value::from(INTERACTIVE_TABLE_KEY),
                    Value::from_object(table),
                )])))
            }
            Postgres | Bigquery | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                let err = format!(
                    "describe_interactive_table is not supported by the {} adapter",
                    adapter_type
                );
                Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    err,
                ))
            }
        }
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L145
    pub fn drop_relation(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value> {
        if self.mock_state().is_some() {
            return Ok(none_value());
        }
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_drop_relation(state, relation),
            Impl(_, _engine) => {
                if relation.relation_type().is_none() {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "relation has no type",
                    ));
                }
                let args = vec![RelationObject::new(Arc::clone(relation)).into_value()];
                execute_macro(state, &args, "drop_relation")?;
                Ok(none_value())
            }
        }
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L222
    pub fn check_schema_exists(
        &self,
        state: &State,
        database: &str,
        schema: &str,
    ) -> Result<Value, minijinja::Error> {
        // TODO: migrate this to ADBC Connection.GetObjects() with schema filter

        // Replay fast-path: consult trace-derived cache if available
        if let Replay(..) = self.inner_adapter() {
            // TODO: move this logic to the [ReplayAdapter]
            if let Some(exists) = self.schema_exists_from_trace(state, database, schema) {
                return Ok(Value::from(exists));
            }
        }

        // Spark (Hive metastore) has no ANSI `information_schema`
        if matches!(self.adapter_type(), Spark) {
            let result =
                execute_macro_wrapper(state, &[Value::from(schema)], "check_schema_exists_like")?;
            return Ok(Value::from(
                self.list_schemas_inner(result)?.iter().any(|s| s == schema),
            ));
        }

        // FIXME:
        // 1. This is used in dbt Core 1.0 as just a "container" for a database/schema,
        // but there is no actual "Relation" since it has no identifier. Using it here
        // is wrong in principle.
        //
        // 2. this thing is hardcoded here but it's all BigQuery-specific, even though
        // other platforms use it too
        let info_schema = Relation::new(
            self.adapter_type(),
            database.to_string(),
            "INFORMATION_SCHEMA".to_string(),
            None::<String>,
        )
        .with_quoting(Policy::falses());

        let (package_name, macro_name) = self.check_schema_exists_macro(state, &[])?;
        let batch = execute_macro_wrapper_with_package(
            state,
            &[
                RelationObject::new(Arc::new(info_schema)).into_value(),
                Value::from(schema),
            ],
            &macro_name,
            &package_name,
        )?;

        match batch.first_value_as_i64() {
            Some(0) => Ok(Value::from(false)),
            Some(1) => Ok(Value::from(true)),
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::ReturnValue,
                "invalid return value",
            )),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_relations_by_pattern(
        &self,
        state: &State,
        schema_pattern: &str,
        table_pattern: &str,
        exclude: Option<&str>,
        database: Option<&str>,
        quote_table: Option<bool>,
        excluded_schemas: Option<Value>,
    ) -> Result<Value, minijinja::Error> {
        // Validate excluded_schemas if provided
        if let Some(ref schemas) = excluded_schemas {
            let _ =
                minijinja_value_to_typed_struct::<Vec<String>>(schemas.clone()).map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        e.to_string(),
                    )
                })?;
        }

        // Get default database from state if not provided
        let database_str = if let Some(db) = database {
            db.to_string()
        } else {
            let target = state.lookup("target", &[]).ok_or_else(|| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    "target is not set in state",
                )
            })?;
            let db_value = target.get_attr("database").unwrap_or_default();
            db_value.as_str().unwrap_or_default().to_string()
        };

        // Build args array for macro call
        // Note: For optional string parameters like 'exclude', we pass empty string instead of None
        // because the macro expects a string and None gets converted to "none" string
        let args = vec![
            Value::from(schema_pattern),
            Value::from(table_pattern),
            exclude.map(Value::from).unwrap_or_else(|| Value::from("")),
            Value::from(database_str.as_str()),
            quote_table
                .map(Value::from)
                .unwrap_or_else(|| Value::from(false)),
            excluded_schemas.unwrap_or_else(|| Value::from_iter::<Vec<String>>(vec![])),
        ];

        let result = execute_macro(state, &args, "get_relations_by_pattern_internal")?;
        Ok(result)
    }

    pub fn check_schema_exists_macro(
        &self,
        _state: &State,
        _args: &[Value],
    ) -> AdapterResult<(String, String)> {
        // Spark is handled natively in `check_schema_exists`, so it never reaches here.
        // Databricks still uses spark__check_schema_exists, whose
        // `information_schema.schemata` query is valid against Unity Catalog.
        if matches!(self.adapter_type(), Databricks) {
            Ok((
                "dbt_spark".to_string(),
                "spark__check_schema_exists".to_string(),
            ))
        } else {
            Ok(("dbt".to_string(), "check_schema_exists".to_string()))
        }
    }

    /// is_cluster_http_path https://github.com/databricks/dbt-databricks/blob/34642904170066825cd4f02a4c5f9b2c3d2d547c/dbt/adapters/databricks/utils.py
    fn is_cluster_http_path(config: &AdapterConfig) -> Option<bool> {
        let http_path = config.get_string("http_path")?;
        let normalized = http_path.trim().to_ascii_lowercase();

        Some(if normalized.contains("/warehouses/") {
            false
        } else {
            normalized.contains("/protocolv1/")
        })
    }

    /// Parse has no connection to read the DBR version from, so only a config
    /// that positively identifies a SQL warehouse can answer true.
    ///
    /// https://github.com/databricks/dbt-databricks/blob/34642904170066825cd4f02a4c5f9b2c3d2d547c/dbt/adapters/databricks/impl.py#L306-L315
    pub(crate) fn parse_has_dbr_capability(config: &AdapterConfig, capability_name: &str) -> bool {
        let Ok(capability) = dbr_capabilities::DbrCapability::from_str(capability_name) else {
            return false;
        };
        match Self::is_cluster_http_path(config) {
            // No http_path to classify, so we can't tell cluster from warehouse.
            None => false,
            Some(true) => dbr_capabilities::has_capability(
                capability,
                DbrComputeContext::Cluster(EngineVersion::Unset),
            ),
            Some(false) => {
                dbr_capabilities::has_capability(capability, DbrComputeContext::SqlWarehouse)
            }
        }
    }

    /// Determine if the current Databricks connection points to a classic
    /// cluster (as opposed to a SQL warehouse).
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L961
    pub fn is_cluster(&self) -> AdapterResult<bool> {
        if self.adapter_type() != Databricks {
            return Err(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "is_cluster is only available for the Databricks adapter",
            ));
        }

        Self::is_cluster_http_path(self.engine().get_config()).ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "http_path is required to determine Databricks compute type",
            )
        })
    }

    pub fn has_feature(
        &self,
        state: &State,
        name: &str,
        token: CancellationToken,
    ) -> AdapterResult<Option<bool>> {
        fn duckdb_is_motherduck(config: &AdapterConfig) -> bool {
            matches!(
                dbt_auth::DuckDbTarget::from_config(config),
                Ok(dbt_auth::DuckDbTarget::MotherDuck { .. })
                    | Ok(dbt_auth::DuckDbTarget::MotherDuckWithToken { .. })
            )
        }

        // All-platform features.
        if name == "transactions" {
            if self.adapter_type() == DuckDB && duckdb_is_motherduck(self.engine().get_config()) {
                return Ok(Some(false));
            }
            return Ok(Some(true));
        }

        // platform-specific features.
        match self.adapter_type() {
            DuckDB => match name {
                "motherduck" => Ok(Some(duckdb_is_motherduck(self.engine().get_config()))),
                _ => {
                    emit_warn_log_message(
                        ErrorCode::InvalidArgument,
                        format!("Unrecognized feature: {name} for {} adapter", DuckDB),
                    );
                    Ok(None)
                }
            },
            Redshift => match name {
                "datasharing" => Ok(Some(get_bool_config(
                    self.engine().as_ref(),
                    "datasharing",
                )?)),
                "drop_without_cascade" => Ok(Some(get_bool_config(
                    self.engine().as_ref(),
                    "drop_without_cascade",
                )?)),
                _ => {
                    emit_warn_log_message(
                        ErrorCode::InvalidArgument,
                        format!("Unrecognized feature: {name} for {} adapter", Redshift),
                    );
                    Ok(None)
                }
            },
            Databricks => {
                let mut conn =
                    self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
                let has_capability = self.has_dbr_capability(state, conn.as_mut(), name, token)?;
                Ok(Some(has_capability))
            }
            adapter_type => {
                emit_warn_log_message(
                    ErrorCode::InvalidArgument,
                    format!("Unrecognized feature: {name} for {adapter_type} adapter"),
                );
                Ok(None)
            }
        }
    }

    /// Returns a dict with database/schema/identifier for temp tables on MotherDuck.
    pub fn get_temp_relation_path(
        &self,
        database: &str,
        identifier: &str,
        batch_id: &str,
    ) -> AdapterResult<BTreeMap<String, Value>> {
        let mut path = BTreeMap::new();
        path.insert("database".to_owned(), Value::from(database));
        path.insert("schema".to_owned(), Value::from("dbt_temp"));
        path.insert(
            "identifier".to_owned(),
            Value::from(format!("{identifier}__{batch_id}")),
        );
        Ok(path)
    }

    /// Rename relation
    ///
    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L155
    pub fn rename_relation(
        &self,
        state: &State,
        from_relation: &Arc<dyn BaseRelation>,
        to_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_rename_relation(state, from_relation, to_relation),
            Impl(_, _engine) => {
                // Execute the macro with the relation objects
                let args = vec![
                    RelationObject::new(Arc::clone(from_relation)).into_value(),
                    RelationObject::new(Arc::clone(to_relation)).into_value(),
                ];

                let _empty_retval = execute_macro(state, &args, "rename_relation")?;
                Ok(none_value())
            }
        }
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L862
    pub fn get_missing_columns(
        &self,
        state: &State,
        source_relation: &Arc<dyn BaseRelation>,
        target_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Vec<Column>> {
        match self.inner_adapter() {
            Replay(_, replay) => {
                replay.replay_get_missing_columns(state, source_relation, target_relation)
            }
            Impl(_, _engine) => {
                // Get columns for both relations
                let source_cols = self.get_columns_in_relation(state, source_relation.as_ref())?;
                let target_cols = self.get_columns_in_relation(state, target_relation.as_ref())?;

                let source_cols_map: BTreeMap<_, _> = source_cols
                    .into_iter()
                    .map(|col| (col.name().to_string(), col))
                    .collect();
                let target_cols_set: HashSet<_> =
                    target_cols.into_iter().map(|col| col.into_name()).collect();

                Ok(source_cols_map
                    .into_iter()
                    .filter_map(|(name, col)| {
                        if target_cols_set.contains(&name) {
                            None
                        } else {
                            Some(col)
                        }
                    })
                    .collect())
            }
        }
    }

    /// BigQuery get_columns_in_relation using `AdbcConnectionGetTableSchema()`
    fn bigquery_get_columns_in_relation_via_adbc(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Vec<Column>> {
        let conn = self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
        let schema = match conn.get_table_schema(
            relation.database(),
            relation.schema(),
            relation.identifier().ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::UnexpectedResult,
                    "relation does not have identifier",
                )
            })?,
        ) {
            Ok(schema) => schema,
            Err(err) => {
                let err = adbc_error_to_adapter_error(err);
                if is_bigquery_not_found_error(&err) {
                    // A missing relation has no columns.
                    return Ok(Vec::new());
                }
                return Err(err);
            }
        };
        // NOTE: it's okay to skip conversion to an SDF-frontend since
        // `schema_to_columns()` will first try to parse the type from the
        // `PLATFORM:type` metadata key returned by the driver.
        self.schema_to_columns(None, &Arc::new(schema))
    }

    /// get_columns_in_relation for adapters whose use a `<adapter>__get_table_schema()`
    /// macro returing a list of `Column`
    fn get_columns_in_relation_via_macro(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Vec<Column>> {
        // Run a Jinja macro to fetch columns
        let macro_result: AdapterResult<Value> = match self.adapter_type() {
            Bigquery => unreachable!(),
            Databricks => {
                // use DESCRIBE TABLE EXTENDED ... AS JSON for full type strings
                // Plain DESCRIBE TABLE truncates long data types server-side
                //
                // https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/impl.py#L439-L452
                //
                // Note: is_hive_metastore() returns false for Unity Catalog temporary tables (matching Python semantics).
                // The `temporary` field only tracks UC temporary tables, not HMS temporary views.
                let use_legacy = relation.is_hive_metastore()
                    || relation.is_materialized_view()
                    || relation.is_streaming_table();

                if !use_legacy {
                    let json_result = execute_macro_with_package(
                        state,
                        &[RelationObject::new(relation.to_owned()).into_value()],
                        "get_columns_comments_as_json",
                        "dbt_databricks",
                    );
                    match json_result {
                        Ok(ref val) => {
                            if let Some(columns) = self.try_columns_from_json_describe(val) {
                                return columns;
                            }
                        }
                        Err(ref e) => {
                            if e.message().contains("[TABLE_OR_VIEW_NOT_FOUND]") {
                                return Ok(Vec::new());
                            }
                            // PARSE_SYNTAX_ERROR / UNSUPPORTED_FEATURE -> DBR < 16.2;
                            // fall through to legacy DESCRIBE TABLE
                        }
                    }
                }

                execute_macro_with_package(
                    state,
                    &[RelationObject::new(relation.to_owned()).into_value()],
                    "get_columns_comments",
                    "dbt_databricks",
                )
            }
            // NOTE: This is the default behavior. If said adapter type does not
            // have a get_columns_in_relation() macro, it will fail with a
            // "macro does not exist" error
            Athena | ClickHouse | Datafusion | Dremio | DuckDB | LakeCompute | Exasol | Fabric
            | Oracle | Postgres | Redshift | Salesforce | Snowflake | Spark | Starburst | Trino => {
                execute_macro(
                    state,
                    &[RelationObject::new(relation.to_owned()).into_value()],
                    "get_columns_in_relation",
                )
            }
        };

        macro_result
            // Ignore certain macro errors
            .or_else(|err| {
                // TODO: switch to checking the vendor error code when available.
                // See https://github.com/dbt-labs/fs/pull/4267#discussion_r2182835729
                let ignored_error = match self.adapter_type() {
                    Snowflake => Some("does not exist or not authorized"),
                    Databricks => Some("[TABLE_OR_VIEW_NOT_FOUND]"),
                    _ => None,
                };

                if let Some(ignored_error) = ignored_error
                    && err.message().contains(ignored_error)
                {
                    Ok(Value::from(Vec::<()>::default()))
                } else {
                    Err(err)
                }
            })
            // Post-process macro results
            .and_then(|macro_columns| {
                let to_adapter_err = |e: minijinja::Error| {
                    AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        e.detail().map(|d| d.to_string()).unwrap_or_else(|| {
                            "Could not convert columns from jinja value".to_string()
                        }),
                    )
                };
                match self.adapter_type() {
                    Databricks => {
                        // Databricks inherits the implementation from the Spark adapter.
                        //
                        // The DESCRIBE TABLE output includes metadata sections (e.g. "# Partition Information",
                        // "# Clustering Information") that must be filtered out. This matches the Python
                        // Spark adapter behavior which filters rows where col_name starts with '#'.
                        //
                        // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-spark/src/dbt/adapters/spark/impl.py#L317-L336
                        // https://github.com/dbt-labs/dbt-fusion/issues/1230
                        let record_batch = convert_macro_result_to_record_batch(&macro_columns)?;
                        let name_string_array =
                            record_batch.column_values::<StringArray>("col_name")?;
                        let dtype_string_array =
                            record_batch.column_values::<StringArray>("data_type")?;
                        let comment_string_array =
                            record_batch.column_values::<StringArray>("comment").ok();

                        // Filter out metadata rows (like "# Partition Information", "# Clustering Information")
                        // These are section headers in DESCRIBE TABLE output, not actual columns.
                        let columns = (0..name_string_array.len())
                            .filter(|&i| !name_string_array.value(i).starts_with('#'))
                            .map(|i| {
                                let comment = comment_string_array.as_ref().and_then(|arr| {
                                    if arr.is_null(i) {
                                        None
                                    } else {
                                        let s = arr.value(i);
                                        if s.is_empty() {
                                            None
                                        } else {
                                            Some(s.to_string())
                                        }
                                    }
                                });

                                Column::new(
                                    Databricks,
                                    name_string_array.value(i).to_string(),
                                    dtype_string_array.value(i).to_string(),
                                    None, // char_size
                                    None, // numeric_precision
                                    None, // numeric_scale
                                )
                                .with_comment(comment)
                            })
                            .collect::<Vec<_>>();
                        Ok(columns)
                    }
                    Spark => Ok(metadata::spark::truncate_at_describe_extended_separator(
                        Column::vec_from_jinja_value(Spark, macro_columns)
                            .map_err(to_adapter_err)?,
                    )),
                    adapter_type => Column::vec_from_jinja_value(adapter_type, macro_columns)
                        .map_err(to_adapter_err),
                }
            })
    }

    /// get_columns_in_relation via the schema cache
    ///
    /// This is totally offline, and returns Ok(None) if there was no cache hit.
    pub(crate) fn get_columns_in_relation_via_cache(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Option<Vec<Column>>> {
        // NOTE: We have to check if the relation being queried is the same as the one currently
        // being rendered and skip local compilation results for the current relation since the
        // compiled sql may represent a schema that the model will have when the run is done,
        // not the current state
        if matches_current_relation(state, relation) {
            return Ok(None);
        };

        let Some(from_cache) = self.get_schema_from_cache(relation) else {
            return Ok(None);
        };

        let cached_columns = self.schema_to_columns(from_cache.original(), from_cache.inner())?;
        #[cfg(debug_assertions)]
        debug_compare_column_types(state, relation, self, cached_columns.clone());
        Ok(Some(cached_columns))
    }

    /// get_columns_in_relation via the remote warehouse, without checking the schema cache
    fn get_columns_in_relation_uncached(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Vec<Column>> {
        match self.adapter_type() {
            // TODO: Should we add the schema that was fetched here to the schema cache
            // to avoid further remote lookups?
            Bigquery => self.bigquery_get_columns_in_relation_via_adbc(state, relation),
            _ => self.get_columns_in_relation_via_macro(state, relation),
        }
    }

    /// SQLAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/sql/impl.py#L161
    /// AthenaAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-athena/src/dbt/adapters/athena/impl.py#L1217
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/fe308ee83cfc200b6ff196f8662b9882d7cec505/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L330
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L216
    /// SparkAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-spark/src/dbt/adapters/spark/impl.py#L318
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/impl.py#L454
    pub fn get_columns_in_relation(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Vec<Column>> {
        // Mock adapter: return fake column without executing jinja macro
        if self.engine().is_mock() {
            return Ok(vec![Column::new(
                self.adapter_type(),
                "one".to_string(),
                "text".to_string(),
                Some(256),
                None,
                None,
            )]);
        }

        // Sidecar adapter: delegate to sidecar client
        if let Some(client) = self.engine().sidecar_client() {
            let database = relation.database_as_str()?;
            let schema = relation.schema_as_str()?;
            let identifier = relation.identifier_as_str()?;
            let relation_name = format!("{}.{}.{}", database, schema, identifier);
            let column_infos = client.get_columns(&relation_name)?;
            let columns = column_infos
                .into_iter()
                .map(|info| {
                    Column::new(
                        self.adapter_type(),
                        info.name,
                        info.data_type,
                        None,
                        None,
                        None,
                    )
                })
                .collect();
            return Ok(columns);
        }

        // Check local schema cache first before reaching out to the warehouse
        let mut columns = if let Some(from_cache) =
            // TODO: should we gracefully fallback to the cold path if there is an error
            // fetching from the schema cache? I didn't do it now because IMO it could swallow bugs
            self.get_columns_in_relation_via_cache(state, relation)?
        {
            from_cache
        } else {
            self.get_columns_in_relation_uncached(state, relation)?
        };

        // Post-process columns (regardless of how they've been fetched), or whether they've
        // been cached or not
        let columns = match self.adapter_type() {
            Bigquery => {
                columns.retain(|c| !BIGQUERY_PSEUDOCOLUMNS.contains(&c.name()));
                columns
            }
            _ => columns,
        };

        Ok(columns)
    }

    /// Try to parse columns from a `DESCRIBE TABLE EXTENDED ... AS JSON` result.
    ///
    /// Returns `Some(Ok(columns))` on success, `Some(Err(...))` on hard failure,
    /// or `None` if the result couldn't be parsed as JSON metadata (caller should
    /// fall back to plain DESCRIBE TABLE).
    fn try_columns_from_json_describe(&self, result: &Value) -> Option<AdapterResult<Vec<Column>>> {
        use crate::metadata::MetadataProcessor as _;
        use crate::metadata::databricks::describe_table::DatabricksTableMetadata;

        let batch = match convert_macro_result_to_record_batch(result) {
            Ok(b) => b,
            Err(_) => return None,
        };
        let metadata = match DatabricksTableMetadata::from_record_batch(batch) {
            Ok(m) => m,
            Err(_) => return None,
        };

        let columns = metadata
            .columns
            .iter()
            .map(|col| {
                let comment = col.comment.clone().filter(|s| !s.is_empty());
                Column::new(
                    Databricks,
                    col.name.clone(),
                    col.type_.sql_type(),
                    None,
                    None,
                    None,
                )
                .with_comment(comment)
            })
            .collect();
        Some(Ok(columns))
    }

    /// Truncate relation
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L745
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L257
    pub fn truncate_relation(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_truncate_relation(state, relation),
            Impl(Bigquery, _) => {
                // BigQuery does not support truncate_relation
                Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    "bigquery.truncate_relation",
                ))
            }
            Impl(
                Snowflake | Databricks | Redshift | Salesforce | Postgres | Spark | DuckDB
                | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
                | Datafusion | Dremio | Oracle,
                _,
            ) => {
                // downcast relation
                let relation = RelationObject::new(Arc::clone(relation)).into_value();
                execute_macro(state, &[relation], "truncate_relation")?;
                Ok(none_value())
            }
        }
    }

    /// Quote as configured
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1103
    pub fn quote_as_configured(
        &self,
        _state: &State,
        identifier: &str,
        quote_key: &ComponentName,
    ) -> AdapterResult<String> {
        if self.quoting().get_part(quote_key) {
            Ok(self.quote(identifier))
        } else {
            Ok(identifier.to_string())
        }
    }

    /// Quote seed column, default to true if not provided
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1124
    /// AthenaAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-athena/src/dbt/adapters/athena/impl.py#L452
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L381
    pub fn quote_seed_column(
        &self,
        state: &State,
        column: &str,
        quote_config: Option<bool>,
    ) -> AdapterResult<String> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_quote_seed_column(state, column, quote_config),
            Impl(Snowflake | Salesforce, _) => {
                // Snowflake is special and defaults quoting to false if config is not provided
                if quote_config.unwrap_or(false) {
                    Ok(self.quote(column))
                } else {
                    Ok(column.to_string())
                }
            }
            Impl(
                Postgres | Bigquery | Databricks | Redshift | Spark | DuckDB | LakeCompute | Fabric
                | ClickHouse | Exasol | Starburst | Athena | Trino | Datafusion | Dremio | Oracle,
                _,
            ) => {
                if quote_config.unwrap_or(true) {
                    Ok(self.quote(column))
                } else {
                    Ok(column.to_string())
                }
            }
        }
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1231
    pub fn convert_type(
        &self,
        state: &State,
        table: Arc<AgateTable>,
        col_idx: i64,
    ) -> AdapterResult<String> {
        if self.mock_state().is_some() {
            unimplemented!("type conversion from table column in MockAdapter")
        }
        let batch = table.original_record_batch();
        let schema = batch.schema();
        let data_type = schema.field(col_idx as usize).data_type();

        let data_type = match data_type {
            dt if dt.is_null() => &DataType::Int32,
            DataType::Float64 => {
                let is_int = batch
                    .column(col_idx as usize)
                    .as_any()
                    .downcast_ref::<arrow_array::Float64Array>()
                    .is_some_and(try_to_int_col);
                if is_int { &DataType::Int64 } else { data_type }
            }
            dt => dt,
        };

        // Almost every SQL dialect adds a "NOT NULL" to types with the type alone
        // meaning a nullable type. ClickHouse it the opposite: nullable types are
        // declared with an explicity Nullable(..) wrapper. We want want to render
        // the clean type here, so we set the nullable flag to get the clean type.
        #[allow(clippy::match_like_matches_macro)]
        let nullable = match self.adapter_type() {
            ClickHouse => false,
            _ => true,
        };

        if let Replay(_, replay) = self.inner_adapter()
            && let Some(recorded) = replay.replay_convert_type(state, data_type)?
        {
            return Ok(recorded);
        }

        if self.adapter_type() == Snowflake
            && matches!(
                data_type,
                DataType::Utf8 | DataType::Utf8View | DataType::LargeUtf8
            )
        {
            return Ok("text".to_string());
        }

        let mut out = String::new();
        self.engine()
            .type_ops()
            .format_arrow_type_as_sql(data_type, nullable, &mut out)?;
        Ok(out)
    }

    /// Expand the to_relation table's column types to match the schema of from_relation
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L951
    pub fn expand_target_column_types(
        &self,
        state: &State,
        from_relation: &Arc<dyn BaseRelation>,
        to_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value> {
        match self.inner_adapter() {
            Replay(_, replay) => {
                replay.replay_expand_target_column_types(state, from_relation, to_relation)
            }
            Impl(Bigquery, _) | Impl(DuckDB, _) | Impl(LakeCompute, _) => {
                // This method is a noop for BigQuery and DuckDB.
                // BigQuery: https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L260-L261
                // DuckDB: type widening (e.g. INT→BIGINT) is handled implicitly;
                // real mismatches surface as SQL errors.
                Ok(none_value())
            }
            Impl(_, _) => {
                let from_columns = self.get_columns_in_relation(state, from_relation.as_ref())?;
                let to_columns = self.get_columns_in_relation(state, to_relation.as_ref())?;

                // Create HashMaps for efficient lookup
                let from_columns_map = from_columns
                    .into_iter()
                    .map(|c| (c.name().to_string(), c))
                    .collect::<BTreeMap<_, _>>();

                let to_columns_map = to_columns
                    .into_iter()
                    .map(|c| (c.name().to_string(), c))
                    .collect::<BTreeMap<_, _>>();

                for (column_name, reference_column) in from_columns_map {
                    let to_relation_cloned = to_relation.clone();
                    if let Some(target_column) = to_columns_map.get(&column_name)
                        && target_column.can_expand_to(&reference_column)?
                    {
                        let col_string_size = reference_column.string_size().map_err(|msg| {
                            AdapterError::new(AdapterErrorKind::UnexpectedResult, msg)
                        })?;
                        let mut new_type = reference_column
                            .as_static()
                            .string_type(Some(col_string_size as usize));

                        // Preserve collation from the target (existing) column
                        if let Some(collation) = target_column.collation() {
                            new_type = format!("{new_type} collate '{collation}'");
                        }

                        // Create args for macro execution
                        execute_macro(
                            state,
                            args!(
                                relation => RelationObject::new(to_relation_cloned).into_value(),
                                column_name => column_name,
                                new_column_type => Value::from(new_type),
                            ),
                            "alter_column_type",
                        )?;
                    }
                }
                Ok(none_value())
            }
        }
    }

    /// This was update_columns method from bigquery-adapter where googleapi is used to
    /// update/merge columns in general
    ///
    /// But since internally this is is only used to update columns descriptions, by
    /// bigquery__alter_column_comment macro and due to limitation of bigquery, we cannot update
    /// nested columns using SQL the implementation here only supports columns descriptions update
    pub fn update_columns_descriptions(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        columns: IndexMap<String, DbtColumn>,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_update_columns(state, relation);
                }
                let database = relation.database_as_str()?;
                let table = relation.identifier_as_str()?;
                let schema = relation.schema_as_str()?;

                let nested_columns = self.do_nest_column_data_types(columns, None)?;

                let column_to_description = nested_columns
                    .iter()
                    .filter_map(|(name, col)| {
                        col.description
                            .as_ref()
                            .map(|desc| (name.to_string(), desc.to_string()))
                    })
                    .collect::<BTreeMap<String, String>>();

                // BigQuery policy tags are taxonomy resource-path strings, so mapping entries (e.g. Snowflake masking-policy config) are dropped here rather than sent to the REST API.
                // If a column's tags are all mapping-valued, omit it entirely rather than
                // sending an empty list, which BigQuery would interpret as clearing any
                // existing policy tags on that column.
                let column_to_policy_tags = nested_columns
                    .iter()
                    .filter_map(|(name, col)| {
                        col.policy_tags.as_ref().and_then(|tags| {
                            let string_tags = tags
                                .iter()
                                .filter_map(|tag| match tag {
                                    StringOrMap::StringValue(s) => Some(s.clone()),
                                    StringOrMap::MapValue(_) => None,
                                })
                                .collect::<Vec<String>>();
                            (!string_tags.is_empty()).then(|| (name.to_string(), string_tags))
                        })
                    })
                    .collect::<BTreeMap<String, Vec<String>>>();

                // Skip the ADBC round-trip (and its ETag-race exposure) when
                // there is nothing to update. Matches dbt-core Python's
                // `if len(columns) == 0: return` guard in dbt-bigquery's
                // `update_columns`.
                if column_to_description.is_empty() && column_to_policy_tags.is_empty() {
                    return Ok(none_value());
                }

                // The heavy lift is delegated to the driver via googleapi Table.update
                // since ALTER TABLE ... ALTER COLUMNS doesn't support updating a view.
                // Descriptions and policy tags are applied in a single REST API call,
                // mirroring dbt Core's update_columns behaviour.
                let mut options = self.get_adbc_execute_options(state);
                options.extend(vec![
                    (
                        QUERY_DESTINATION_TABLE.to_string(),
                        OptionValue::String(format!("{database}.{schema}.{table}")),
                    ),
                    (
                        UPDATE_TABLE_COLUMNS_DESCRIPTION.to_string(),
                        OptionValue::String(
                            serde_json::to_string(&column_to_description)
                                .expect("Failed to serialize column_to_description"),
                        ),
                    ),
                    (
                        UPDATE_TABLE_COLUMNS_POLICY_TAGS.to_string(),
                        OptionValue::String(
                            serde_json::to_string(&column_to_policy_tags)
                                .expect("Failed to serialize column_to_policy_tags"),
                        ),
                    ),
                ]);

                let ctx = query_ctx_from_state(state)?;
                let sql = format!(
                    "-- adapter.update_columns via BigQuery REST API on `{database}.{schema}.{table}`"
                );
                self.engine().execute_with_options(
                    Some(state),
                    &ctx,
                    conn,
                    &sql,
                    options,
                    false,
                    token,
                )?;

                Ok(none_value())
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | Fabric | DuckDB
            | LakeCompute | ClickHouse | Exasol | Starburst | Athena | Trino | Datafusion
            | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// render_raw_columns_constraints
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1848
    pub fn render_raw_columns_constraints(
        &self,
        columns_map: IndexMap<String, DbtColumn>,
    ) -> AdapterResult<Vec<String>> {
        match self.adapter_type() {
            // dbt-clickhouse impl.py override: column constraints warn as
            // unsupported and never render; codec/ttl ride after the type.
            ClickHouse => {
                let mut result = vec![];
                for (_, column) in columns_map {
                    let mut rendered = format!(
                        "`{}` {}",
                        column.name,
                        column.data_type.as_deref().unwrap_or_default()
                    );
                    if let Some(codec) = column.codec.as_deref().filter(|c| !c.is_empty()) {
                        rendered.push_str(&format!(" CODEC({codec})"));
                    }
                    if let Some(ttl) = column.ttl.as_deref().filter(|t| !t.is_empty()) {
                        rendered.push_str(&format!(" TTL {ttl}"));
                    }
                    for constraint in column.constraints {
                        warn_constraint_support(
                            ClickHouse,
                            constraint.type_,
                            ConstraintSupport::NotSupported,
                            constraint.warn_unsupported,
                            constraint.warn_unenforced,
                        );
                    }
                    result.push(rendered);
                }
                Ok(result)
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | Exasol | Starburst | Athena | Trino | Datafusion | Dremio
            | Oracle => {
                let mut result = vec![];
                for (_, column) in columns_map {
                    let col_name = if column.quote.unwrap_or(false) {
                        self.quote(&column.name)
                    } else {
                        column.name.clone()
                    };
                    let mut rendered_column_constraint = vec![format!(
                        "{} {}",
                        col_name,
                        column.data_type.as_deref().unwrap_or_default()
                    )];
                    for constraint in column.constraints {
                        let rendered = self.render_column_constraint(constraint);
                        if let Some(rendered) = rendered {
                            rendered_column_constraint.push(rendered);
                        }
                    }
                    result.push(rendered_column_constraint.join(" ").to_string())
                }
                Ok(result)
            }
            adapter_type @ Bigquery => {
                let mut rendered_constraints: BTreeMap<String, String> = BTreeMap::new();
                for (_, column) in columns_map.iter() {
                    for constraint in &column.constraints {
                        warn_constraint_support(
                            adapter_type,
                            constraint.type_,
                            self.get_constraint_support(constraint.type_),
                            constraint.warn_unsupported,
                            constraint.warn_unenforced,
                        );
                        if let Some(rendered) =
                            render_column_constraint(adapter_type, constraint.clone())
                        {
                            rendered_constraints
                                .entry(column.name.clone())
                                .and_modify(|s| {
                                    s.push(' ');
                                    s.push_str(&rendered);
                                })
                                .or_insert(rendered);
                        }
                    }
                }
                let nested_columns =
                    self.do_nest_column_data_types(columns_map, Some(rendered_constraints))?;
                let result = nested_columns
                    .into_values()
                    .map(|column| {
                        format!(
                            "{} {}",
                            if column.quote.unwrap_or(false) {
                                self.quote(&column.name)
                            } else {
                                column.name.clone()
                            },
                            column.data_type.unwrap_or_default()
                        )
                    })
                    .collect();
                Ok(result)
            }
        }
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1816
    pub fn render_column_constraint(&self, constraint: Constraint) -> Option<String> {
        // Custom constraints bypass the support check — dbt-adapters intentionally
        // short-circuits enforcement for custom and passes the expression verbatim.
        // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L1908-L1909
        if constraint.type_ != ConstraintType::Custom {
            let constraint_support = self.get_constraint_support(constraint.type_);
            warn_constraint_support(
                self.adapter_type(),
                constraint.type_,
                constraint_support,
                constraint.warn_unsupported,
                constraint.warn_unenforced,
            );
            if constraint_support == ConstraintSupport::NotSupported {
                return None;
            }
        }

        let constraint_expression = constraint.expression.unwrap_or_default();

        let rendered = match constraint.type_ {
            ConstraintType::Check if !constraint_expression.is_empty() => {
                Some(format!("check ({constraint_expression})"))
            }
            ConstraintType::NotNull => Some(format!("not null {constraint_expression}")),
            ConstraintType::Unique => Some(format!("unique {constraint_expression}")),
            ConstraintType::PrimaryKey => Some(format!("primary key {constraint_expression}")),
            ConstraintType::ForeignKey => match (constraint.to, constraint.to_columns) {
                (Some(to), Some(to_columns)) if !to_columns.is_empty() => {
                    Some(format!("references {} ({})", to, to_columns.join(", ")))
                }
                _ if !constraint_expression.is_empty() => {
                    Some(format!("references {constraint_expression}"))
                }
                _ => None,
            },
            ConstraintType::Custom if !constraint_expression.is_empty() => {
                Some(constraint_expression)
            }
            _ => None,
        };
        rendered.and_then(|r| match (self.adapter_type(), constraint.type_) {
            (Bigquery, ConstraintType::PrimaryKey | ConstraintType::ForeignKey) => {
                Some(format!("{r} not enforced"))
            }
            (Bigquery, _) => None,
            _ => Some(r.trim().to_string()),
        })
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/5379513bad9c75661b990a5ed5f32ac9c62a0758/dbt-adapters/src/dbt/adapters/base/impl.py#L293
    pub fn get_constraint_support(&self, ct: ConstraintType) -> ConstraintSupport {
        use ConstraintSupport::*;
        use ConstraintType::*;

        match (self.adapter_type(), ct) {
            // Postgres
            (Postgres, NotNull) => Enforced,
            (Postgres, ForeignKey) => Enforced,
            (Postgres, Unique) => NotEnforced,
            (Postgres, PrimaryKey) => NotEnforced,
            (Postgres, Check) => NotSupported,
            (Postgres, Custom) => NotSupported,

            // Snowflake
            // https://github.com/dbt-labs/dbt-adapters/blob/aa1de3d16267a456326a36045701fb48a61a6b6c/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L74
            (Snowflake, NotNull) => Enforced,
            (Snowflake, ForeignKey) => Enforced,
            (Snowflake, Unique) => NotEnforced,
            (Snowflake, PrimaryKey) => NotEnforced,
            (Snowflake, Check) => NotSupported,
            (Snowflake, Custom) => NotSupported,

            // BigQuery
            // https://github.com/dbt-labs/dbt-adapters/blob/4a00354a497214d9043bf4122810fe2d04de17bb/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L132
            (Bigquery, NotNull) => Enforced,
            (Bigquery, Unique) => NotSupported,
            (Bigquery, PrimaryKey) => NotEnforced,
            (Bigquery, ForeignKey) => NotEnforced,
            (Bigquery, Check) => NotSupported,
            (Bigquery, Custom) => NotSupported,

            // Databricks
            // https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/constraints.py#L17
            (Databricks, NotNull) => Enforced,
            (Databricks, Unique) => NotSupported,
            (Databricks, PrimaryKey) => NotEnforced,
            (Databricks, ForeignKey) => NotEnforced,
            (Databricks, Check) => Enforced,
            (Databricks, Custom) => NotSupported,

            // Redshift
            // https://github.com/dbt-labs/dbt-adapters/blob/2a94cc75dba1f98fa5caff1f396f5af7ee444598/dbt-redshift/src/dbt/adapters/redshift/impl.py#L53
            (Redshift, NotNull) => Enforced,
            (Redshift, Unique) => NotEnforced,
            (Redshift, PrimaryKey) => NotEnforced,
            (Redshift, ForeignKey) => NotEnforced,
            (Redshift, Check) => NotSupported,
            (Redshift, Custom) => NotSupported,

            // DuckDB - follows Postgres
            (DuckDB, NotNull) => Enforced,
            (DuckDB, ForeignKey) => Enforced,
            (DuckDB, Unique) => NotEnforced,
            (DuckDB, PrimaryKey) => NotEnforced,
            (DuckDB, Check) => NotSupported,
            (DuckDB, Custom) => NotSupported,

            // Lake compute - follows DuckDB
            (LakeCompute, NotNull) => Enforced,
            (LakeCompute, ForeignKey) => Enforced,
            (LakeCompute, Unique) => NotEnforced,
            (LakeCompute, PrimaryKey) => NotEnforced,
            (LakeCompute, Check) => NotSupported,
            (LakeCompute, Custom) => NotSupported,

            // Fabric
            (Fabric, Check) => NotSupported,
            (Fabric, NotNull) => Enforced,
            (Fabric, Unique) => Enforced,
            (Fabric, PrimaryKey) => Enforced,
            (Fabric, ForeignKey) => Enforced,
            (Fabric, Custom) => NotSupported,

            // Exasol (verified on Exasol 8)
            (Exasol, NotNull) => Enforced,
            (Exasol, PrimaryKey) => Enforced,
            (Exasol, ForeignKey) => Enforced,
            (Exasol, Unique) => NotSupported,
            (Exasol, Check) => NotSupported,
            (Exasol, Custom) => NotSupported,

            // ClickHouse (dbt-clickhouse impl.py CONSTRAINT_SUPPORT)
            (ClickHouse, Check) => Enforced,
            (ClickHouse, NotNull | Unique | PrimaryKey | ForeignKey | Custom) => NotSupported,

            // Salesforce
            (Salesforce | Spark | Starburst | Athena | Trino | Datafusion | Dremio | Oracle, _) => {
                unimplemented!("constraint support not implemented")
            }
        }
    }

    /// Given existing columns and columns from our model
    /// we determine which columns to update and persist docs for
    pub fn do_get_persist_doc_columns(
        &self,
        existing_columns: Vec<Column>,
        model_columns: IndexMap<String, DbtColumnRef>,
    ) -> AdapterResult<IndexMap<String, DbtColumnRef>> {
        if self.adapter_type() != Databricks {
            return Err(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "get_persist_doc_columns is a Databricks adapter operation",
            ));
        }
        // Upstream semantics (dbt-databricks): persist a column doc update if and only if the
        // desired comment (model.description, defaulting to "") differs from the existing warehouse
        // comment (defaulting to "").
        //
        // This intentionally supports "clearing" comments: desired="" + existing="foo" => update.
        //
        // Model columns missing from the relation are already filtered out by the Jinja-side
        // `validate_doc_columns` helper before this is called.
        let mut result = IndexMap::new();

        // Case-insensitive lookup for model columns (matches upstream behavior).
        let mut model_columns_lower: HashMap<String, &DbtColumnRef> = HashMap::new();
        for (name, col) in &model_columns {
            model_columns_lower.insert(name.to_lowercase(), col);
        }

        for existing_col in existing_columns {
            let Some(model_col) = model_columns_lower.get(&existing_col.name().to_lowercase())
            else {
                continue;
            };

            let desired = model_col.description.as_deref().unwrap_or("");
            let existing = existing_col.comment().unwrap_or("");

            if desired != existing {
                result.insert(existing_col.name().to_string(), (*model_col).clone());
            }
        }

        Ok(result)
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L859
    pub fn get_persist_doc_columns(
        &self,
        _state: &State,
        existing_columns: &Value,
        model_columns: &Value,
    ) -> Result<Value, minijinja::Error> {
        let existing_columns = Column::vec_from_jinja_value(Databricks, existing_columns.clone())
            .map_err(|e| {
            minijinja::Error::new(minijinja::ErrorKind::SerdeDeserializeError, e.to_string())
        })?;
        let model_columns = minijinja_value_to_typed_struct::<IndexMap<String, DbtColumnRef>>(
            model_columns.clone(),
        )
        .map_err(|e| {
            minijinja::Error::new(minijinja::ErrorKind::SerdeDeserializeError, e.to_string())
        })?;

        let persist_doc_columns =
            self.do_get_persist_doc_columns(existing_columns, model_columns)?;

        let result = IndexMap::from_iter(
            persist_doc_columns
                .into_iter()
                .map(|(col_name, col)| (col_name, Value::from_serialize(col))),
        );

        Ok(Value::from_object(result))
    }

    /// Translate the result of `show grants` (or equivalent) to match the
    /// grants which a user would configure in their project.
    /// Ideally, the SQL to show grants should also be filtering:
    /// filter OUT any grants TO the current user/role (e.g. OWNERSHIP).
    /// If that's not possible in SQL, it can be done in this method instead.
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L833
    /// SnowflakeAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L400
    /// DatabricksAdapter https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-spark/src/dbt/adapters/spark/impl.py#L500
    /// RedshiftAdapter https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-redshift/src/dbt/adapters/redshift/impl.py#L347
    pub fn standardize_grants_dict(
        &self,
        grants_table: Arc<AgateTable>,
    ) -> AdapterResult<IndexMap<String, Vec<String>>> {
        let record_batch = grants_table.original_record_batch();

        // When show_grants returns 0 rows, agate records column_types as
        // ["Integer",...] rather than the actual string types (no values to
        // infer from). Fusion replays that result verbatim, so column_values
        // would fail the StringArray downcast. An empty table means no grants
        // to process, so return early — matching Python's loop-over-rows behaviour.
        if record_batch.num_rows() == 0 {
            return Ok(IndexMap::new());
        }

        match self.adapter_type() {
            Postgres | Bigquery | DuckDB | LakeCompute | Exasol => {
                let grantee_cols = record_batch.column_values::<StringArray>("grantee")?;
                let privilege_cols = record_batch.column_values::<StringArray>("privilege_type")?;

                let mut result = IndexMap::new();
                for i in 0..record_batch.num_rows() {
                    let privilege = privilege_cols.value(i);
                    let grantee = grantee_cols.value(i);

                    let list = result.entry(privilege.to_string()).or_insert_with(Vec::new);
                    list.push(grantee.to_string());
                }

                Ok(result)
            }
            Redshift => {
                // Redshift grants vary along two axes: datasharing selects
                // SHOW GRANTS vs. catalog views, and redshift_grants_extended
                // selects legacy users vs. identity-prefixed grantees.
                #[derive(Clone, Copy)]
                enum GrantsMode {
                    Legacy,
                    ShowUsers,
                    SvvIdentities,
                    ShowIdentities,
                }

                let datasharing_enabled = get_bool_config(self.engine().as_ref(), "datasharing")?;
                let grants_extended = self
                    .behavior_object()
                    .get_value(&Value::from("redshift_grants_extended"))
                    .is_some_and(|flag| flag.is_true());
                let grants_mode = match (datasharing_enabled, grants_extended) {
                    (false, false) => GrantsMode::Legacy,
                    (true, false) => GrantsMode::ShowUsers,
                    (false, true) => GrantsMode::SvvIdentities,
                    (true, true) => GrantsMode::ShowIdentities,
                };
                let privilege_cols = record_batch.column_values::<StringArray>("privilege_type")?;
                let mut result: IndexMap<String, Vec<String>> = IndexMap::new();

                // The legacy and SVV SQL paths filter out current_user in the macro.
                // SHOW GRANTS does not support adding a WHERE predicate, so filter it here.
                let current_user = match grants_mode {
                    GrantsMode::ShowUsers | GrantsMode::ShowIdentities => {
                        Some(match self.engine().as_ref().config("user") {
                            Some(user) if !user.is_empty() => user.into_owned(),
                            _ => {
                                let mut conn = self.borrow_tlocal_connection(None, None)?;
                                let ctx = QueryCtx::default()
                                    .with_desc("standardize_grants_dict current_user");
                                let batch = self.engine().execute(
                                    None,
                                    conn.as_mut(),
                                    &ctx,
                                    "SELECT current_user AS current_user",
                                    CancellationToken::never_cancels(),
                                )?;
                                let users = batch.column_values::<StringArray>("current_user")?;
                                debug_assert_eq!(
                                    batch.num_rows(),
                                    1,
                                    "SELECT current_user must return exactly one row"
                                );
                                users.value(0).to_string()
                            }
                        })
                    }
                    GrantsMode::Legacy | GrantsMode::SvvIdentities => None,
                };

                match grants_mode {
                    GrantsMode::Legacy => {
                        let grantee_cols = record_batch.column_values::<StringArray>("grantee")?;
                        for row in 0..record_batch.num_rows() {
                            result
                                .entry(privilege_cols.value(row).to_string())
                                .or_default()
                                .push(grantee_cols.value(row).to_string());
                        }
                    }
                    GrantsMode::ShowUsers => {
                        let identity_name_cols =
                            record_batch.column_values::<StringArray>("identity_name")?;
                        let identity_type_cols =
                            record_batch.column_values::<StringArray>("identity_type")?;
                        let current_user = current_user
                            .as_deref()
                            .expect("current user is resolved when show APIs are enabled");

                        for row in 0..record_batch.num_rows() {
                            let identity_name = identity_name_cols.value(row);
                            if identity_type_cols.value(row).eq_ignore_ascii_case("user")
                                && !identity_name.eq_ignore_ascii_case(current_user)
                            {
                                result
                                    .entry(privilege_cols.value(row).to_ascii_lowercase())
                                    .or_default()
                                    .push(identity_name.to_string());
                            }
                        }
                    }
                    GrantsMode::SvvIdentities | GrantsMode::ShowIdentities => {
                        let identity_name_cols =
                            record_batch.column_values::<StringArray>("identity_name")?;
                        let identity_type_cols =
                            record_batch.column_values::<StringArray>("identity_type")?;

                        let grantees = (0..record_batch.num_rows()).filter_map(|row| {
                            let identity_name = identity_name_cols.value(row);
                            let identity_type = identity_type_cols.value(row);
                            let is_current_user = current_user.as_deref().is_some_and(|user| {
                                identity_type.eq_ignore_ascii_case("user")
                                    && identity_name.eq_ignore_ascii_case(user)
                            });

                            // PUBLIC and Redshift-reserved identities cannot be managed by dbt grants.
                            if identity_type.eq_ignore_ascii_case("public")
                                || identity_name.starts_with("ds:")
                                || identity_name.starts_with("sys:")
                                || is_current_user
                            {
                                return None;
                            }

                            let grantee = if matches!(grants_mode, GrantsMode::ShowIdentities)
                                && identity_type.eq_ignore_ascii_case("role")
                            {
                                // SHOW GRANTS reports groups as role identities with a
                                // leading '/', so translate them back to dbt's group: shape.
                                identity_name.strip_prefix('/').map_or_else(
                                    || {
                                        format!(
                                            "{}:{identity_name}",
                                            identity_type.to_ascii_lowercase()
                                        )
                                    },
                                    |group| format!("group:{group}"),
                                )
                            } else {
                                format!("{}:{identity_name}", identity_type.to_ascii_lowercase())
                            };
                            Some((privilege_cols.value(row).to_ascii_lowercase(), grantee))
                        });

                        for (privilege, grantee) in grantees {
                            result.entry(privilege).or_default().push(grantee);
                        }
                    }
                }

                Ok(result)
            }
            Snowflake => {
                let grantee_cols = record_batch.column_values::<StringArray>("grantee_name")?;
                let granted_to_cols = record_batch.column_values::<StringArray>("granted_to")?;
                let privilege_cols = record_batch.column_values::<StringArray>("privilege")?;

                let mut result = IndexMap::new();
                for i in 0..record_batch.num_rows() {
                    let privilege = privilege_cols.value(i);
                    let grantee = grantee_cols.value(i);
                    let granted_to = granted_to_cols.value(i);

                    if privilege != "OWNERSHIP"
                        && granted_to != "SHARE"
                        && granted_to != "DATABASE_ROLE"
                    {
                        let list = result.entry(privilege.to_string()).or_insert_with(Vec::new);
                        list.push(grantee.to_string());
                    }
                }

                Ok(result)
            }
            Databricks => {
                let grantee_cols = record_batch.column_values::<StringArray>("Principal")?;
                let privilege_cols = record_batch.column_values::<StringArray>("ActionType")?;
                let object_type_cols = record_batch.column_values::<StringArray>("ObjectType")?;

                let mut result = IndexMap::new();
                for i in 0..record_batch.num_rows() {
                    let privilege = privilege_cols.value(i);
                    let grantee = grantee_cols.value(i);
                    let object_type = object_type_cols.value(i);

                    if object_type == "TABLE" && privilege != "OWN" {
                        let list = result.entry(privilege.to_string()).or_insert_with(Vec::new);
                        list.push(grantee.to_string());
                    }
                }

                Ok(result)
            }
            Salesforce | Spark | Fabric | ClickHouse | Starburst | Athena | Trino | Datafusion
            | Dremio | Oracle => {
                unimplemented!("grants not implemented")
            }
        }
    }

    /// Join `SHOW TABLES FROM SCHEMA` metadata with `SVV_REDSHIFT_COLUMNS` to build the base
    /// catalog used when Redshift datasharing is enabled. The SVV view is leader-only and
    /// cannot be joined to `SHOW` results in SQL, so the catalog macro fetches both and passes
    /// them here for an in-memory join.
    pub fn build_catalog_from_show_tables_and_svv_columns(
        &self,
        show_tables_results: &[Arc<AgateTable>],
        svv_columns: Arc<AgateTable>,
    ) -> AdapterResult<AgateTable> {
        match self.adapter_type() {
            Redshift => {
                let show_tables_batches: Vec<Arc<RecordBatch>> = show_tables_results
                    .iter()
                    .map(|table| table.original_record_batch())
                    .collect();
                let svv_batch = svv_columns.original_record_batch();
                let catalog = metadata::redshift::join_show_tables_and_svv_columns(
                    &show_tables_batches,
                    svv_batch.as_ref(),
                )?;
                Ok(AgateTable::from_record_batch(Arc::new(catalog)))
            }
            Snowflake | Bigquery | Databricks | Spark | DuckDB | LakeCompute | Postgres
            | Salesforce | Fabric | ClickHouse | Exasol | Athena | Starburst | Trino
            | Datafusion | Dremio | Oracle => Err(AdapterError::new(
                AdapterErrorKind::NotSupported,
                "build_catalog_from_show_tables_and_svv_columns is only supported for Redshift",
            )),
        }
    }

    pub fn do_nest_column_data_types(
        &self,
        columns: IndexMap<String, DbtColumn>,
        constraints: Option<BTreeMap<String, String>>,
    ) -> AdapterResult<IndexMap<String, DbtColumn>> {
        match self.adapter_type() {
            Bigquery => nest_column_data_types(columns, constraints),
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L323
    pub fn nest_column_data_types(&self, columns: &Value) -> Result<Value, minijinja::Error> {
        // TODO: 'constraints' arg are ignored; didn't find an usage example, implement later
        let columns =
            minijinja_value_to_typed_struct::<IndexMap<String, DbtColumn>>(columns.clone())
                .map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        e.to_string(),
                    )
                })?;

        let nested_columns = self.do_nest_column_data_types(columns, None)?;
        let result = IndexMap::<String, Value>::from_iter(
            nested_columns
                .into_iter()
                .map(|(col_name, col)| (col_name, Value::from_serialize(col))),
        );

        Ok(Value::from_object(result))
    }

    /// BigQueryColumn https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L233
    pub fn get_struct_select_expression(
        &self,
        _state: &State,
        col_name: &str,
        data_type: &str,
    ) -> Result<Value, minijinja::Error> {
        match self.adapter_type() {
            Bigquery => Ok(Value::from(render_struct_projection(col_name, data_type))),
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1187
    pub fn get_bq_table(
        &self,
        _state: &State,
        _relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        unimplemented!("get_bq_table")
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1219
    #[allow(clippy::too_many_arguments)]
    pub fn grant_access_to(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        entity: &Arc<dyn BaseRelation>,
        entity_type: &str,
        // _role is not used since this method only supports view
        // and googleapi doesn't require role if the entity is view, it'll be default to READ always
        _role: Option<&str>,
        database: &str,
        schema: &str,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_grant_access_to(
                        state,
                        entity,
                        entity_type,
                        database,
                        schema,
                    );
                }
                // https://github.com/dbt-labs/dbt-adapters/blob/4a00354a497214d9043bf4122810fe2d04de17bb/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L834
                /// but instead of locking the thread, put the lock on the dataset
                static DATASET_LOCK: LazyLock<DashMap<String, bool>> = LazyLock::new(DashMap::new);

                // adapter.grant_access_to when seen in Jinja macros, `entity_type` is always set to view
                // https://github.com/dbt-labs/dbt-adapters/blob/4a00354a497214d9043bf4122810fe2d04de17bb/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L842
                // Besides, there is a deserialization bug in the existing py impl when entity_type is not `view`
                if entity_type != "view" {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "Only views are supported for grant_access_to".to_string(),
                    ));
                }

                #[derive(Serialize, Deserialize)]
                struct Dataset {
                    project: String,
                    dataset: String,
                }
                let mut payload = BTreeMap::new();
                payload.insert(
                    format!(
                        "{}.{}.{}",
                        entity.database_as_str()?,
                        entity.schema_as_str()?,
                        entity.identifier_as_str()?
                    ),
                    vec![Dataset {
                        project: database.to_string(),
                        dataset: schema.to_string(),
                    }],
                );

                let _lock = DATASET_LOCK
                    .entry(format!("{database}.{schema}"))
                    .or_insert_with(|| true);

                let ctx = query_ctx_from_state(state)?;
                let sql = "none"; // empty sql that won't really be executed
                let mut options = self.get_adbc_execute_options(state);
                options.push((
                    UPDATE_DATASET_AUTHORIZE_VIEW_TO_DATASETS.to_string(),
                    OptionValue::String(serde_json::to_string(&payload)?),
                ));
                self.engine().execute_with_options(
                    Some(state),
                    &ctx,
                    conn,
                    sql,
                    options,
                    false,
                    token,
                )?;
                Ok(none_value())
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// Parse the single-row `location` result of the BigQuery SCHEMATA query.
    ///
    /// Returns `None` for an empty result without reading the column. This covers
    /// the zero-column batch that replay returns for the SCHEMATA query, which is
    /// never recorded by Mantle during `get_relation` (see dbt9002).
    fn parse_dataset_location(batch: &RecordBatch) -> AdapterResult<Option<String>> {
        debug_assert!(batch.num_rows() <= 1);
        if batch.num_rows() == 1 {
            let location = batch.column_values::<StringArray>("location")?;
            Ok(Some(location.value(0).to_owned()))
        } else {
            Ok(None)
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1241
    pub fn get_dataset_location(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &dyn BaseRelation,
        token: CancellationToken,
    ) -> AdapterResult<Option<String>> {
        match self.adapter_type() {
            Bigquery => {
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_get_dataset_location(state, relation);
                }
                // https://cloud.google.com/bigquery/docs/information-schema-datasets-schemata
                // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L853-L854
                let sql = format!(
                    "SELECT
                location
            FROM `{}.INFORMATION_SCHEMA.SCHEMATA` WHERE schema_name = '{}'",
                    relation.database_as_str()?,
                    relation.schema_as_str()?
                );

                let ctx =
                    query_ctx_from_state(state)?.with_desc("get_dataset_location adapter call");
                let batch = self
                    .engine()
                    .execute(Some(state), conn, &ctx, &sql, token)?;

                Self::parse_dataset_location(&batch)
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L730
    #[allow(clippy::too_many_arguments)]
    pub fn update_table_description(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        database: &str,
        schema: &str,
        identifier: &str,
        description: &str,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_update_table_description(
                        state,
                        database,
                        schema,
                        identifier,
                        description,
                    );
                }
                // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L686-L696
                // Use BigQuery API via driver option instead of SQL
                // Reuse QUERY_DESTINATION_TABLE for the table reference
                let table_ref = format!("{database}.{schema}.{identifier}");

                let ctx =
                    query_ctx_from_state(state)?.with_desc("update_table_description adapter call");
                self.engine().execute_with_options(
                    Some(state),
                    &ctx,
                    conn,
                    "", // Empty SQL - the driver will handle this via the option
                    vec![
                        (
                            QUERY_DESTINATION_TABLE.to_string(),
                            OptionValue::String(table_ref),
                        ),
                        (
                            UPDATE_TABLE_DESCRIPTION.to_string(),
                            OptionValue::String(description.to_string()),
                        ),
                    ],
                    false,
                    token,
                )?;
                Ok(none_value())
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L930
    #[allow(clippy::too_many_arguments)]
    pub fn load_dataframe(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        database: &str,
        schema: &str,
        table_name: &str,
        agate_table: Arc<AgateTable>,
        file_path: &str,
        column_overrides: IndexMap<String, String>,
        field_delimiter: &str,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                // https://github.com/dbt-labs/dbt-adapters/blob/4b3966efc50b1d013907a88bee4ab8ebd022d17a/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L668
                //
                // TODO: Because we don't support custom materialization yet, we're breaking this
                // one. Later we can document to end users that their old way of using this macro
                // is bugged. The fix will be trivial for any power user relying on this adapter
                // method and we can provide clear guidance for migration.
                let ingest_schema = crate::seed::ingest_schema_with_column_overrides(
                    agate_table.original_record_batch().schema().as_ref(),
                    &column_overrides,
                    self.adapter_type(),
                )?;

                let serialized_ingest_schema: Vec<u8> = {
                    // serialize the Arrow schema as an Arrow IPC byte blob
                    let mut buf = Vec::<u8>::new();
                    let () = StreamWriter::try_new(&mut buf, &ingest_schema)
                        .and_then(|mut w| w.finish())
                        .map_err(arrow_error_to_adapter_error)?;
                    Ok(buf) as AdapterResult<Vec<u8>>
                }?;

                self.engine().execute_with_options(
                    None,
                    ctx,
                    conn,
                    sql,
                    vec![
                        (
                            QUERY_DESTINATION_TABLE.to_string(),
                            OptionValue::String(format!("{database}.{schema}.{table_name}")),
                        ),
                        (
                            INGEST_FILE_DELIMITER.to_string(),
                            OptionValue::String(field_delimiter.to_string()),
                        ),
                        (
                            INGEST_PATH.to_string(),
                            OptionValue::String(file_path.to_string()),
                        ),
                        (
                            INGEST_SCHEMA.to_string(),
                            OptionValue::Bytes(serialized_ingest_schema),
                        ),
                    ],
                    false,
                    token,
                )?;

                Ok(none_value())
            }
            Salesforce => todo!("load_dataframe() for the Salesforce adapter"),
            Postgres | Snowflake | Databricks | Redshift | Spark | DuckDB | LakeCompute
            | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino | Datafusion | Dremio
            | Oracle => {
                unimplemented!("only available with BigQuery or Salesforce adapter")
            }
        }
    }

    /// This only supports non-nested columns additions
    ///
    /// Since internally this is only used by snapshot materialization macro where newly added
    /// columns all have non-nested data types, Read from
    /// [here](https://github.com/sdf-labs/fs/blob/9b87be839f6aa54cab1ab91cde2c77855758c396/crates/dbt-loader/src/dbt_macro_assets/dbt-adapters/macros/materializations/snapshots/snapshot.sql#L32-L33).
    /// This builds sql that creates the snapshot relation, and this relation only adds non-nested
    /// columns to the source relation it is supposed to work well for this use case due to
    /// limitation:
    /// https://cloud.google.com/bigquery/docs/managing-table-schemas#add_a_nested_column_to_a_record_column
    ///
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L742
    pub fn alter_table_add_columns(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        columns: Value,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                let columns = Column::vec_from_jinja_value(Bigquery, columns)?;
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_alter_table_add_columns(state, relation, &columns);
                }
                if columns.is_empty() {
                    return Ok(none_value());
                }

                let add_columns: Vec<String> = columns
                    .iter()
                    .map(|col| format!("ADD COLUMN {} {}", col.name(), &col.dtype()))
                    .collect();

                let sql = format!(
                    "ALTER TABLE {}
            {}",
                    relation.render_self_as_str(),
                    add_columns.join("\n,")
                );
                let ctx =
                    query_ctx_from_state(state)?.with_desc("alter_table_add_columns adapter call");
                self.engine().execute_with_options(
                    Some(state),
                    &ctx,
                    conn,
                    &sql,
                    self.get_adbc_execute_options(state),
                    false,
                    token,
                )?;

                Ok(none_value())
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// Convert an Arrow [Schema] to a [Vec] of [Column]s.
    ///
    /// This is not part of the Jinja adapter API.
    ///
    /// NOTE(jason): This schema might come directly out of the driver and is not
    /// a sdf frontend schema - this function might not format types perfectly yet
    ///
    /// NOTE(felipecrv): we are working on making it easy to not confuse
    /// driver-generated schemas versus canonicalized sdf frontend schemas
    pub fn schema_to_columns(
        &self,
        _original: Option<&Arc<Schema>>,
        schema: &Arc<Schema>,
    ) -> AdapterResult<Vec<Column>> {
        let type_formatter = self.engine().type_ops();
        let builder = ColumnBuilder::new(self.adapter_type());

        let fields = schema.fields();
        let mut columns = Vec::<Column>::with_capacity(fields.len());
        for field in fields {
            let column = builder.build(field, type_formatter.as_ref())?;
            columns.push(column);
        }
        Ok(columns)
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L486
    pub fn get_column_schema_from_query(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        ctx: &QueryCtx,
        sql: &str,
        // ClickHouse only: see metadata::clickhouse::describe_query_columns.
        query_settings: Option<&Value>,
        token: CancellationToken,
    ) -> AdapterResult<Vec<Column>> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_get_column_schema_from_query(state, conn, ctx, sql),
            Impl(Bigquery, engine) => {
                // https://github.com/dbt-labs/dbt-adapters/blob/f4dfd350942cce11ff25e3d22f2bee9e60b12b6d/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L444
                let batch = engine.execute(Some(state), conn, ctx, sql, token)?;
                let schema = batch.schema();

                let type_ops = engine.type_ops().as_ref();
                let builder = ColumnBuilder::new(self.adapter_type());

                let fields = schema.fields();

                let mut columns = Vec::<Column>::with_capacity(fields.len());
                for field in fields {
                    let column = builder.build(field, type_ops)?;
                    columns.push(column);
                }

                let flattened_columns =
                    columns.iter().flat_map(|column| column.flatten()).collect();
                Ok(flattened_columns)
            }
            Impl(ClickHouse, engine) => {
                // Server-typed schema via DESCRIBE; the rationale lives on
                // metadata::clickhouse::describe_query_columns.
                let settings_clause = metadata::clickhouse::query_settings_clause(query_settings);
                let pairs = metadata::clickhouse::describe_query_columns(
                    engine.as_ref(),
                    Some(state),
                    conn,
                    ctx,
                    sql,
                    &settings_clause,
                    token,
                )?;
                Ok(pairs
                    .into_iter()
                    .map(|(name, type_text)| {
                        Column::new(self.adapter_type(), name, type_text, None, None, None)
                    })
                    .collect())
            }
            Impl(_, engine) => {
                let (_, table) = self.execute_inner(
                    Arc::clone(engine),
                    Some(state),
                    conn,
                    ctx,
                    sql,
                    false,
                    true,
                    None,
                    None,
                    token,
                )?;
                let schema = table.original_record_batch().schema();
                self.schema_to_columns(None, &schema)
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L541
    pub fn get_columns_in_select_sql(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        ctx: &QueryCtx,
        sql: &str,
        token: CancellationToken,
    ) -> AdapterResult<Vec<Column>> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_get_columns_in_select_sql(state),
            Impl(Bigquery, _) => {
                self.get_column_schema_from_query(state, conn, ctx, sql, None, token)
            }
            Impl(_, _) => unimplemented!("only available with BigQuery adapter"),
        }
    }

    /// Used by redshift and postgres to check if the database string is consistent with what's in the project `config`
    ///
    /// PostgresAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-postgres/src/dbt/adapters/postgres/impl.py#L118
    pub fn verify_database(&self, database: String) -> AdapterResult<Value> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_verify_database(&database),
            Impl(adapter_type @ (Postgres | DuckDB | LakeCompute | ClickHouse), engine) => {
                if let Some(configured_database) = engine.get_configured_database_name() {
                    if database == configured_database {
                        Ok(Value::from(()))
                    } else {
                        Err(AdapterError::new(
                            AdapterErrorKind::UnexpectedDbReference,
                            format!(
                                "Cross-db references not allowed in the {} adapter ({} vs {})",
                                adapter_type, database, configured_database
                            ),
                        ))
                    }
                } else {
                    Ok(Value::from(()))
                }
            }
            Impl(Redshift, engine) => {
                let ra3_node = get_bool_config(engine.as_ref(), "ra3_node")?;
                let datasharing = get_bool_config(engine.as_ref(), "datasharing")?;

                // We have no guarantees that `database` is unquoted, but we do know that `configured_database` will be unquoted.
                // For the Redshift adapter, we can just trim the `"` character per `self.quote`.
                let database = database.trim_matches('\"');
                let configured_database = engine.config("database");

                if let Some(configured_database) = configured_database {
                    if !database.eq_ignore_ascii_case(&configured_database)
                        && !ra3_node
                        && !datasharing
                    {
                        return Err(AdapterError::new(
                            AdapterErrorKind::UnexpectedDbReference,
                            format!(
                                "Cross-db references allowed only in RA3.* node or with datasharing enabled ({database} vs {configured_database})"
                            ),
                        ));
                    }
                }

                Ok(Value::from(()))
            }
            Impl(
                adapter_type @ (Snowflake | Bigquery | Databricks | Salesforce | Spark | Fabric
                | Exasol | Starburst | Athena | Trino | Datafusion | Dremio
                | Oracle),
                _,
            ) => {
                unimplemented!(
                    "verify_database is not implemented for the {} adapter",
                    adapter_type
                )
            }
        }
    }

    /// Check if a given partition and clustering column spec for a table
    /// can replace an existing relation in the database. BigQuery does not
    /// allow tables to be replaced with another table that has a different
    /// partitioning spec. This method returns True if the given config spec is
    /// identical to that of the existing table.
    ///
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/4a00354a497214d9043bf4122810fe2d04de17bb/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L541
    pub fn is_replaceable(
        &self,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        local_partition_by: Option<BigqueryPartitionConfig>,
        local_cluster_by: Option<ClusterConfig>,
        state: Option<&State>,
    ) -> AdapterResult<bool> {
        use crate::relation::bigquery::config::components::{ClusterByLoader, PartitionByLoader};
        match self.adapter_type() {
            Bigquery => {
                if let (Replay(_, replay), Some(state)) = (self.inner_adapter(), state) {
                    return replay.replay_is_replaceable(state);
                }

                let schema_result = conn
                    .get_table_schema(
                        Some(&relation.database_as_str()?),
                        Some(&relation.schema_as_str()?),
                        &relation.identifier_as_str()?,
                    )
                    .map_err(adbc_error_to_adapter_error);

                match schema_result {
                    Ok(schema) => {
                        let remote_partition_by = PartitionByLoader.from_remote_state(&schema)?;
                        let local_partition_by =
                            PartitionByLoader::new_component_type_erased(local_partition_by);
                        let is_partition_match = local_partition_by
                            .diff_from(Some(remote_partition_by.as_ref()))
                            .is_none();

                        let remote_cluster_by = ClusterByLoader.from_remote_state(&schema)?;
                        let local_cluster_by = ClusterByLoader::new_component_type_erased(
                            local_cluster_by
                                .map(|cb| cb.into_fields())
                                .unwrap_or_default(),
                        );
                        let is_cluster_match = local_cluster_by
                            .diff_from(Some(remote_cluster_by.as_ref()))
                            .is_none();

                        Ok(is_partition_match && is_cluster_match)
                    }
                    Err(e) => {
                        if e.kind() == AdapterErrorKind::NotFound {
                            Ok(true)
                        } else {
                            Err(e)
                        }
                    }
                }
            }
            adapter_type @ (Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark
            | DuckDB | LakeCompute | Fabric | ClickHouse | Exasol | Starburst
            | Athena | Trino | Datafusion | Dremio | Oracle) => {
                unimplemented!(
                    "is_replaceable is only available with BigQuery adapter, not {}",
                    adapter_type
                )
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L956
    pub fn upload_file(&self, _state: &State, _args: &[Value]) -> Result<Value, minijinja::Error> {
        unimplemented!("upload_file")
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L670
    pub fn parse_partition_by(&self, partition_by: Value) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L579-L586
                // Pure config parse; safe for both BigQuery and Replay (when adapter type is BigQuery)
                let raw_partition_by = partition_by;
                if raw_partition_by.is_none() {
                    return Ok(none_value());
                }

                // Lowercase all string values to match dbt-core behavior
                let normalized = if let Ok(partition_by_map) =
                    minijinja_value_to_typed_struct::<IndexMap<String, Value>>(
                        raw_partition_by.clone(),
                    ) {
                    let new_map: IndexMap<String, Value> = partition_by_map
                        .into_iter()
                        .map(|(key, value)| {
                            let normalized_value = if let Some(s) = value.as_str() {
                                Value::from(s.to_lowercase())
                            } else {
                                value
                            };
                            (key, normalized_value)
                        })
                        .collect();
                    Value::from_serialize(&new_map)
                } else {
                    raw_partition_by.clone()
                };

                let partition_by = minijinja_value_to_typed_struct::<PartitionConfig>(normalized)
                    .map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        format!("adapter.parse_partition_by failed on {raw_partition_by:?}: {e}"),
                    )
                })?;

                let validated_config = partition_by.into_bigquery().ok_or_else(|| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidArgument,
                        "Expect a BigqueryPartitionConfigStruct",
                    )
                })?;

                Ok(Value::from_object(validated_config))
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1139
    pub fn get_table_options(
        &self,
        state: &State,
        config: ModelConfig,
        node: &InternalDbtNodeWrapper,
        temporary: bool,
    ) -> AdapterResult<IndexMap<String, Value>> {
        match self.adapter_type() {
            adapter_type @ Bigquery => metadata::bigquery::object_options::get_table_options_value(
                state,
                config,
                node,
                temporary,
                adapter_type,
            ),
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1178
    pub fn get_view_options(
        &self,
        state: &State,
        config: ModelConfig,
        common_attr: &CommonAttributes,
    ) -> AdapterResult<IndexMap<String, Value>> {
        match self.adapter_type() {
            Bigquery => Ok(
                metadata::bigquery::object_options::get_common_table_options_value(
                    state,
                    config,
                    common_attr,
                    false,
                ),
            ),
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L1111
    pub fn get_common_options(
        &self,
        state: &State,
        config: ModelConfig,
        node: &InternalDbtNodeWrapper,
        temporary: bool,
    ) -> Result<Value, minijinja::Error> {
        match self.adapter_type() {
            Bigquery => {
                let node = node.as_internal_node();
                let options = metadata::bigquery::object_options::get_common_table_options_value(
                    state,
                    config,
                    node.common(),
                    temporary,
                );
                Ok(Value::from_serialize(options))
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "get_common_options is only available with BigQuery adapter",
            )),
        }
    }

    /// Add time ingestion partition column to columns list
    ///
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L342
    pub fn add_time_ingestion_partition_column(
        &self,
        columns: Value,
        partition_config: BigqueryPartitionConfig,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Bigquery => {
                let mut result = Column::vec_from_jinja_value(Bigquery, columns.clone())?;

                if result
                    .iter()
                    .any(|c| c.name() == BigqueryPartitionConfig::PARTITION_TIME)
                {
                    return Ok(columns);
                }

                result.push(Column::new_bigquery(
                    partition_config
                        .insertable_time_partitioning_field()?
                        .as_str()
                        .expect("must be a str")
                        .to_owned(),
                    partition_config.data_type,
                    &[],
                    // TODO(serramatutu): proper mode
                    BigqueryColumnMode::Nullable,
                ));

                Ok(Value::from(result))
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L973
    pub fn list_relations(
        &self,
        query_ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        db_schema: &CatalogAndSchema,
        token: CancellationToken,
    ) -> AdapterResult<Vec<Arc<dyn BaseRelation>>> {
        if self.mock_state().is_some() {
            if !self.introspect_enabled() {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    "Introspective queries are disabled (--no-introspect).",
                ));
            }
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                format!(
                    "list_relations_without_caching is not implemented for this adapter: {}",
                    self.adapter_type()
                ),
            ));
        }
        use crate::metadata::*;

        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_list_relations(query_ctx, conn, db_schema),
            Impl(adapter_type, engine) if engine.is_sidecar() => {
                let client = engine.sidecar_client().unwrap();
                let query_schema = db_schema.resolved_schema.clone();
                let relation_infos = client.list_relations(&query_schema)?;
                let mut relations: Vec<Arc<dyn BaseRelation>> =
                    Vec::with_capacity(relation_infos.len());
                for (database, schema, name, rel_type) in relation_infos {
                    let relation = crate::relation::do_create_relation(
                        adapter_type,
                        database,
                        schema,
                        Some(name),
                        Some(rel_type),
                        self.quoting(),
                    )?;
                    relations.push(relation.into());
                }
                Ok(relations)
            }
            Impl(Snowflake, engine) => {
                snowflake::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(Bigquery, engine) => {
                bigquery::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(Databricks | Spark, engine) => {
                databricks::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(Redshift, engine) => {
                redshift::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(DuckDB, engine) => {
                duckdb::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(LakeCompute, engine) => {
                duckdb::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(Fabric, engine) => {
                fabric::list_relations(engine.as_ref(), query_ctx, conn, db_schema, token)
            }
            Impl(
                adapter_type @ (Postgres | Salesforce | ClickHouse | Exasol | Starburst | Athena
                | Trino | Datafusion | Dremio | Oracle),
                _,
            ) => {
                let err = AdapterError::new(
                    AdapterErrorKind::Internal,
                    format!(
                        "list_relations_without_caching is not implemented for this adapter: {adapter_type}",
                    ),
                );
                Err(err)
            }
        }
    }

    /// Per-adapter dependency-graph discovery for the relation cache. Adapters
    /// without a native pg_depend-style query return an empty vec.
    pub fn list_relation_dependency_links(
        &self,
        query_ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        db_schema: &CatalogAndSchema,
        token: CancellationToken,
    ) -> AdapterResult<Vec<metadata::ParentChildPair>> {
        if self.mock_state().is_some() {
            return Ok(Vec::new());
        }
        use crate::metadata::*;
        match self.inner_adapter() {
            Impl(Redshift, engine) => redshift::list_relation_dependencies(
                engine.as_ref(),
                query_ctx,
                conn,
                db_schema,
                token,
            ),
            _ => Ok(Vec::new()),
        }
    }

    pub fn behavior_object(&self) -> &Arc<Behavior> {
        if let Some(mock) = self.mock_state() {
            return &mock.behavior;
        }
        self.engine().behavior()
    }

    /// Check if a DBR capability is available for current compute.
    ///
    /// https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/impl.py#L336-L354
    ///
    /// PRE-CONDITION: adapter_type must be Databricks
    fn has_dbr_capability(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        capability_name: &str,
        token: CancellationToken,
    ) -> AdapterResult<bool> {
        debug_assert!(self.adapter_type() == Databricks);

        if let Replay(_, replay) = self.inner_adapter()
            && let Some(recorded) = replay.replay_has_dbr_capability(state, capability_name)?
        {
            return Ok(recorded);
        }

        let capability = dbr_capabilities::DbrCapability::from_str(capability_name)
            .map_err(|e| AdapterError::new(AdapterErrorKind::Configuration, e))?;

        let query_ctx = query_ctx_from_state(state)?.with_desc("has_dbr_capability adapter call");
        let dbr_version =
            DatabricksMetadataAdapter::get_engine_version(self, &query_ctx, conn, token)?;

        let context = if self.is_cluster()? {
            DbrComputeContext::Cluster(dbr_version)
        } else {
            DbrComputeContext::SqlWarehouse
        };

        Ok(dbr_capabilities::has_capability(capability, context))
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L349
    pub fn compare_dbr_version(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        major: i64,
        minor: i64,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        match self.adapter_type() {
            Databricks => {
                let query_ctx =
                    query_ctx_from_state(state)?.with_desc("compare_dbr_version adapter call");

                let current_version =
                    DatabricksMetadataAdapter::get_engine_version(self, &query_ctx, conn, token)?;
                let expected_version = EngineVersion::Full(major, minor);

                let result = match current_version.cmp(&expected_version) {
                    std::cmp::Ordering::Greater => 1,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Less => -1,
                };

                Ok(Value::from(result))
            }
            Postgres | Snowflake | Bigquery | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with Databricksadapter")
            }
        }
    }

    /// Get the external root directory from engine config, defaulting to `"."`.
    pub fn external_root(&self) -> String {
        self.engine()
            .config("external_root")
            .unwrap_or(Cow::Borrowed("."))
            .into_owned()
    }

    /// Build the write-options string for DuckDB external materializations.
    pub fn external_write_options(&self, write_location: &str, rendered_options: &Value) -> String {
        let mut opts: IndexMap<String, String> = IndexMap::new();
        if let Ok(keys) = rendered_options.try_iter() {
            for key in keys {
                let key_str = key.to_string();
                if let Ok(val) = rendered_options.get_item(&key) {
                    opts.insert(key_str, val.to_string());
                }
            }
        }

        // Infer format from file extension if not provided
        if !opts.contains_key("format") {
            let ext = write_location
                .rsplit('.')
                .next()
                .filter(|e| *e != write_location)
                .unwrap_or("");
            if !ext.is_empty() {
                opts.insert("format".to_string(), ext.to_lowercase());
            } else if opts.contains_key("delimiter") {
                opts.insert("format".to_string(), "csv".to_string());
            } else {
                opts.insert("format".to_string(), "parquet".to_string());
            }
        }

        // Default CSV header
        if opts.get("format").map(|f| f.as_str()) == Some("csv") && !opts.contains_key("header") {
            opts.insert("header".to_string(), "1".to_string());
        }

        // Normalize partition_by parens
        if let Some(v) = opts.get("partition_by").cloned() {
            if v.contains(',') && !v.starts_with('(') {
                opts.insert("partition_by".to_string(), format!("({v})"));
            }
        }

        // Build result: quote special keys
        let ret: Vec<String> = opts
            .iter()
            .map(|(k, v)| {
                let lower = k.to_lowercase();
                if matches!(lower.as_str(), "delimiter" | "quote" | "escape" | "null")
                    && !v.starts_with('\'')
                {
                    format!("{k} '{v}'")
                } else {
                    format!("{k} {v}")
                }
            })
            .collect();
        ret.join(", ")
    }

    /// Build the read location (possibly a glob path) for DuckDB external materializations.
    pub fn external_read_location(&self, write_location: &str, rendered_options: &Value) -> String {
        let partition_by = rendered_options
            .get_item(&Value::from("partition_by"))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_none());
        let per_thread = rendered_options
            .get_item(&Value::from("per_thread_output"))
            .ok()
            .filter(|v| !v.is_undefined() && !v.is_none());

        if partition_by.is_some() || per_thread.is_some() {
            let mut globs = vec![write_location.to_string(), "*".to_string()];
            if let Some(pb) = &partition_by {
                let pb_str = pb.to_string();
                let count = pb_str.split(',').count();
                for _ in 0..count {
                    globs.push("*".to_string());
                }
            }
            let format = rendered_options
                .get_item(&Value::from("format"))
                .ok()
                .filter(|v| !v.is_undefined() && !v.is_none())
                .map(|v| v.to_string())
                .unwrap_or_else(|| "parquet".to_string());
            format!("{}.{}", globs.join("/"), format)
        } else {
            write_location.to_string()
        }
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L307
    pub fn compute_external_path(
        &self,
        config: ModelConfig,
        node: &dyn InternalDbtNodeAttributes,
        is_incremental: bool,
    ) -> AdapterResult<String> {
        match self.adapter_type() {
            Databricks => {
                // TODO: dbt seems to allow optional database and schema
                // https://github.com/databricks/dbt-databricks/blob/main/dbt/adapters/databricks/impl.py#L212-L213
                let location_root = config
                    .__warehouse_specific_config__
                    .location_root
                    .ok_or_else(|| {
                        AdapterError::new(
                            AdapterErrorKind::Configuration,
                            "location_root is required for external tables.",
                        )
                    })?;

                let include_full_name_in_path = config
                    .__warehouse_specific_config__
                    .include_full_name_in_path
                    .unwrap_or_default();

                // Build path using the same logic as posixpath.join.
                let path = if include_full_name_in_path {
                    format!(
                        "{}/{}/{}/{}",
                        location_root.trim_end_matches('/'),
                        node.database().trim_end_matches('/'),
                        node.schema().trim_end_matches('/'),
                        node.alias()
                    )
                } else {
                    format!("{}/{}", location_root.trim_end_matches('/'), node.alias())
                };

                let path = if is_incremental {
                    format!("{path}_tmp")
                } else {
                    path
                };
                Ok(path)
            }

            Postgres | Snowflake | Bigquery | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with Databricks adapter")
            }
        }
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L298
    pub fn update_tblproperties_for_uniform_iceberg(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        config: ModelConfig,
        node: &InternalDbtNodeWrapper,
        tblproperties: &mut IndexMap<String, Value>,
        token: CancellationToken,
    ) -> AdapterResult<()> {
        match self.adapter_type() {
            adapter_type @ Databricks => {
                // TODO(anna): Ideally from_model_config_and_catalogs would just take in an InternalDbtNodeWrapper instead of a Value. This is blocked by a Snowflake hack in `snowflake__drop_table`.
                let node_yml = node.as_internal_node().serialize();
                let catalog_relation = CatalogRelation::from_model_config_and_catalogs(
                    adapter_type,
                    &Value::from_object(dbt_common::serde_utils::convert_yml_to_value_map(
                        node_yml,
                    )),
                    load_catalogs::fetch_catalogs(),
                )?;
                // We only have to update tblproperties if using a UniForm Iceberg table
                if catalog_relation.table_format.is_iceberg() {
                    if self
                        .compare_dbr_version(state, conn, 14, 3, token)?
                        .as_i64()
                        .expect("dbr_version is a number")
                        < 0
                    {
                        return Err(AdapterError::new(
                            AdapterErrorKind::Configuration,
                            "Iceberg support requires Databricks Runtime 14.3 or later.",
                        ));
                    }

                    if catalog_relation.file_format != Some("delta".to_string()) {
                        return Err(AdapterError::new(
                            AdapterErrorKind::Configuration,
                            "When table_format is 'iceberg', file_format must be 'delta'.",
                        ));
                    }

                    let materialized = config.materialized.ok_or_else(|| {
                        AdapterError::new(
                            AdapterErrorKind::Configuration,
                            "materialized is required for iceberg tables.",
                        )
                    })?;

                    // TODO(versusfacit): support snapshot
                    if materialized != DbtMaterialization::Incremental
                        && materialized != DbtMaterialization::Table
                        && materialized != DbtMaterialization::Seed
                    {
                        return Err(AdapterError::new(
                            AdapterErrorKind::Configuration,
                            "When table_format is 'iceberg', materialized must be 'incremental', 'table', or 'seed'.",
                        ));
                    }

                    tblproperties
                        .entry("delta.enableIcebergCompatV2".to_string())
                        .or_insert_with(|| Value::from(true));

                    tblproperties
                        .entry("delta.universalFormat.enabledFormats".to_string())
                        .or_insert_with(|| Value::from("iceberg"));
                }
                Ok(())
            }
            Postgres | Snowflake | Bigquery | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with Databricks adapter")
            }
        }
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L274
    pub fn is_uniform(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        config: ModelConfig,
        node: &InternalDbtNodeWrapper,
        token: CancellationToken,
    ) -> AdapterResult<bool> {
        match self.adapter_type() {
            adapter_type @ Databricks => {
                if let Replay(_, replay) = self.inner_adapter()
                    && let Some(recorded) = replay.replay_is_uniform(state)?
                {
                    return Ok(recorded);
                }

                // TODO(anna): Ideally from_model_config_and_catalogs would just take in an InternalDbtNodeWrapper instead of a Value. This is blocked by a Snowflake hack in `snowflake__drop_table`.
                let node_yml = node.as_internal_node().serialize();
                let catalog_relation = CatalogRelation::from_model_config_and_catalogs(
                    adapter_type,
                    &Value::from_object(dbt_common::serde_utils::convert_yml_to_value_map(
                        node_yml,
                    )),
                    load_catalogs::fetch_catalogs(),
                )?;

                if !catalog_relation.table_format.is_iceberg() {
                    return Ok(false);
                }

                if self
                    .compare_dbr_version(state, conn, 14, 3, token)?
                    .as_i64()
                    .expect("dbr_version is a number")
                    < 0
                {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "Iceberg support requires Databricks Runtime 14.3 or later.",
                    ));
                }

                let materialized = config.materialized.ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "materialized is required for iceberg tables.",
                    )
                })?;

                // TODO(versusfacit): support snapshot
                if materialized != DbtMaterialization::Incremental
                    && materialized != DbtMaterialization::Table
                    && materialized != DbtMaterialization::Seed
                {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "When table_format is 'iceberg', materialized must be 'incremental', 'table', or 'seed'.",
                    ));
                }

                // v2: use_uniform from catalog spec is authoritative
                if load_catalogs::fetch_use_catalogs_v2() {
                    return Ok(catalog_relation
                        .adapter_properties
                        .get("use_uniform")
                        .and_then(|v| v.parse::<bool>().ok())
                        .unwrap_or(false));
                }

                // v1: use_managed_iceberg behavior flag drives the decision
                let use_managed_iceberg = self
                    .behavior_object()
                    .get_value(&Value::from("use_managed_iceberg"))
                    .is_some_and(|flag| flag.is_true());

                if use_managed_iceberg
                    && !matches!(catalog_relation.catalog_type, CatalogType::Unity)
                {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "Managed Iceberg tables are only supported in Unity Catalog. Set 'use_uniform' adapter property to true for Hive Metastore.",
                    ));
                }

                Ok(!use_managed_iceberg)
            }
            Postgres | Snowflake | Bigquery | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with Databricks adapter")
            }
        }
    }

    /// When config omits file_format, falls back to this adapter's default (Databricks
    /// defaults to "delta"). Used by clone materialization.
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L994
    pub fn resolve_file_format(&self, config: ModelConfig) -> AdapterResult<String> {
        match self.adapter_type() {
            Databricks => {
                let file_format = config
                    .__warehouse_specific_config__
                    .file_format
                    .as_deref()
                    .unwrap_or("delta")
                    .to_string();
                Ok(file_format)
            }
            _ => unimplemented!("resolve_file_format is only supported in Databricks"),
        }
    }

    /// Given a relation, fetch its configurations from the remote data warehouse
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/7c282cabb518a5e1173222e7901896d31de8401f/dbt/adapters/databricks/impl.py#L1088
    pub fn get_relation_config(
        &self,
        state: &State,
        conn: &mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        model_config: Option<&RelationConfig>,
        token: CancellationToken,
    ) -> AdapterResult<RelationConfig> {
        use crate::relation::databricks::config::relation_types;

        if let Replay(_, replay) = self.inner_adapter()
            && let Some(recorded) = replay.replay_get_relation_config(state)?
        {
            let relation_type = relation.relation_type().ok_or_else(|| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "relation_type is required to reconstruct a recorded get_relation_config"
                        .to_string(),
                )
            })?;
            let rebuilt = relation_types::relation_config_from_recorded(
                self.adapter_type(),
                relation_type,
                &recorded,
            );
            return rebuilt;
        }

        let (relation_type, remote_state) = {
            // IMPORTANT: do not bypass replay by constructing an AdapterImpl from the engine.
            // In replay mode, adapter calls must go through the replay adapter so they consume
            // the recording stream.
            let metadata_adapter = DatabricksMetadataAdapter::new_from_adapter(self.clone());
            metadata_adapter.fetch_relation_config_from_remote(
                state,
                conn,
                relation,
                model_config,
                token,
            )?
        };

        let config_loader = match relation_type {
            RelationType::Table => relation_types::incremental_table::new_loader(),
            RelationType::MaterializedView => relation_types::materialized_view::new_loader(),
            RelationType::MetricView => relation_types::metric_view::new_loader(),
            RelationType::StreamingTable => relation_types::streaming_table::new_loader(),
            RelationType::View => relation_types::view::new_loader(),
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!("Unsupported materialization type: {:?}", relation_type),
                ));
            }
        };

        let config = config_loader.from_remote_state(&remote_state)?;

        Ok(config)
    }

    /// Given a model, parse and build its configurations
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/7c282cabb518a5e1173222e7901896d31de8401f/dbt/adapters/databricks/impl.py#L1107
    pub fn get_config_from_model(&self, model: &InternalDbtNodeWrapper) -> AdapterResult<Value> {
        use crate::relation::databricks::config::relation_types;

        let model = model.as_internal_node();

        let config_loader = match model.materialized() {
            DbtMaterialization::Incremental => relation_types::incremental_table::new_loader(),
            DbtMaterialization::MaterializedView => relation_types::materialized_view::new_loader(),
            DbtMaterialization::MetricView => relation_types::metric_view::new_loader(),
            DbtMaterialization::StreamingTable => relation_types::streaming_table::new_loader(),
            DbtMaterialization::View => relation_types::view::new_loader(),
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!(
                        "Unsupported materialization type: {:?}",
                        model.materialized()
                    ),
                ));
            }
        };
        let config = config_loader.from_local_config(model)?;
        Ok(Value::from_object(config))
    }

    /// Parse columns and constraints for table creation (Databricks).
    ///
    /// Returns [enriched_columns, typed_constraints] for use with get_column_and_constraints_sql
    /// and relation.enrich().
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/impl.py#L1038
    pub fn parse_columns_and_constraints(
        &self,
        _state: &State,
        existing_columns: &Value,
        model_columns: &Value,
        model_constraints: &Value,
        contract_enforced: bool,
        model_name: &str,
    ) -> Result<Value, minijinja::Error> {
        use crate::relation::databricks::typed_constraint;
        use std::collections::{BTreeMap, BTreeSet};

        if self.adapter_type() != Databricks && self.adapter_type() != Spark {
            return Err(minijinja::Error::new(
                minijinja::ErrorKind::InvalidOperation,
                "parse_columns_and_constraints is only available for Databricks/Spark adapter",
            ));
        }

        let columns: Vec<Column> = existing_columns
            .try_iter()
            .map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!("existing_columns must be iterable: {e}"),
                )
            })?
            .map(|v| {
                v.downcast_object_ref::<Column>().cloned().ok_or_else(|| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "existing_columns must contain Column objects",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let model_columns_map: BTreeMap<String, DbtColumn> =
            minijinja_value_to_typed_struct(model_columns.clone()).map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::SerdeDeserializeError,
                    format!("model_columns: {e}"),
                )
            })?;

        let model_constraints_vec: Vec<ModelConstraint> =
            minijinja_value_to_typed_struct(model_constraints.clone()).map_err(|e| {
                minijinja::Error::new(
                    minijinja::ErrorKind::SerdeDeserializeError,
                    format!("model_constraints: {e}"),
                )
            })?;

        let column_refs: Vec<DbtColumnRef> = model_columns_map
            .values()
            .map(|c| Arc::new(c.clone()))
            .collect();

        let (not_nulls, typed_constraints) = if contract_enforced {
            typed_constraint::parse_constraints(&column_refs, &model_constraints_vec).map_err(
                |e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        format!("parse_constraints: {e}"),
                    )
                },
            )?
        } else {
            if model_columns_map
                .values()
                .any(|column| !column.constraints.is_empty())
            {
                let model_ref = if model_name.is_empty() {
                    String::new()
                } else {
                    format!(" on '{model_name}'")
                };
                emit_info_log_message(format!(
                    "Skipping column-level constraints{model_ref}: set `contract.enforced: true` \
                     to apply NOT NULL / primary key / foreign key / check constraints."
                ));
            }
            (BTreeSet::new(), Vec::new())
        };

        let model_columns_lower: BTreeMap<String, &DbtColumn> = model_columns_map
            .iter()
            .map(|(k, v)| (k.to_lowercase(), v))
            .collect();
        let not_nulls_lower: BTreeSet<String> =
            not_nulls.iter().map(|name| name.to_lowercase()).collect();

        let enriched_columns: Vec<Column> = columns
            .iter()
            .map(|col| {
                let model_col = model_columns_lower.get(&col.name().to_lowercase()).copied();
                let not_null = not_nulls_lower.contains(&col.name().to_lowercase());
                col.enrich_for_create(model_col, not_null)
            })
            .collect();

        Ok(Value::from(vec![
            Value::from_iter(enriched_columns.into_iter().map(Value::from_object)),
            Value::from_iter(typed_constraints.into_iter().map(Value::from_object)),
        ]))
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L463
    pub fn get_relations_without_caching(
        &self,
        _state: &State,
        _relation: &Arc<dyn BaseRelation>,
    ) -> Result<Value, minijinja::Error> {
        unimplemented!("get_relations_without_caching")
    }

    /// PostgresAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-postgres/src/dbt/adapters/postgres/impl.py#L128
    pub fn parse_index(
        &self,
        _state: &State,
        _raw_index: &Value,
    ) -> Result<Value, minijinja::Error> {
        unimplemented!("parse_index")
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L990
    pub fn get_column_tags_from_model(
        &self,
        model: &dyn InternalDbtNodeAttributes,
    ) -> AdapterResult<Value> {
        use crate::relation::databricks::config::components::ColumnTagsLoader;

        if self.adapter_type() != Databricks {
            return Err(AdapterError::new(
                AdapterErrorKind::Internal,
                "get_column_tags_from_model is a Databricks adapter operation".to_string(),
            ));
        }

        let tags = (&ColumnTagsLoader as &dyn ComponentConfigLoader<DatabricksRelationMetadata>)
            .from_local_config(model)?;
        Ok(tags.to_jinja())
    }

    /// Trims surrounding whitespace and strips a single trailing semicolon.
    ///
    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2f11abb306a400cde32b27891b766bf41a11fb1f/dbt/adapters/databricks/impl.py#L966
    pub fn clean_sql(&self, sql: &str) -> AdapterResult<String> {
        debug_assert!(
            self.adapter_type() == Databricks,
            "clean_sql is a Databricks-specific adapter operation"
        );
        Ok(dbt_adapter_sql::statements::clean_sql(
            sql,
            self.adapter_type(),
        ))
    }

    /// DatabricksAdapter https://github.com/databricks/dbt-databricks/blob/2c3aa9fdddbab30a3c4a660c5e98722e989a592b/dbt/adapters/databricks/impl.py#L1135-L1137
    pub fn yaml_quote_backtick_values(&self, yaml_body: &str) -> AdapterResult<String> {
        debug_assert!(
            self.adapter_type() == Databricks,
            "yaml_quote_backtick_values is a Databricks-specific adapter operation"
        );
        Ok(crate::relation::databricks::metric_view::quote_metric_view_sources(yaml_body))
    }

    /// Drop the trailing statement terminator from a node body so it can be
    /// spliced into a wrapping query.
    pub fn strip_trailing_statement_terminator<'a>(&self, sql: &'a str) -> &'a str {
        self.engine()
            .splitter()
            .strip_trailing_statement_terminator(sql, self.adapter_type())
    }

    /// relation_max_name_length
    pub fn relation_max_name_length(&self) -> AdapterResult<u32> {
        unimplemented!("only available with Postgres and Redshift adapters")
    }

    /// This uses the BigQuery SDK's copy_table API instead of SQL to properly handle partitioned
    /// tables.
    /// Reference: https://cloud.google.com/python/docs/reference/bigquery/latest/google.cloud.bigquery.client.Client.html#google_cloud_bigquery_client_Client_copy_table
    ///
    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L510
    pub fn copy_table(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        source: &Arc<dyn BaseRelation>,
        dest: &Arc<dyn BaseRelation>,
        materialization: String,
        token: CancellationToken,
    ) -> AdapterResult<()> {
        match self.adapter_type() {
            Bigquery => {
                if let Replay(_, replay) = self.inner_adapter() {
                    return replay.replay_copy_table(state, source, dest, &materialization);
                }
                let append = materialization == "incremental";
                let truncate = materialization == "table";
                if !append && !truncate {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "copy_table 'materialization' must be either 'table' or 'incremental'"
                            .to_string(),
                    ));
                }

                let source_fqn = format!(
                    "{}.{}.{}",
                    source.database_as_str()?,
                    source.schema_as_str()?,
                    source.identifier_as_str()?
                );
                let dest_fqn = format!(
                    "{}.{}.{}",
                    dest.database_as_str()?,
                    dest.schema_as_str()?,
                    dest.identifier_as_str()?
                );

                // Determine write disposition based on materialization
                // WRITE_TRUNCATE for table materialization, WRITE_APPEND for incremental
                let write_disposition = if truncate {
                    "WRITE_TRUNCATE"
                } else {
                    "WRITE_APPEND"
                };

                let mut options = self.get_adbc_execute_options(state);
                options.extend(vec![
                    (
                        COPY_TABLE_SOURCE.to_string(),
                        OptionValue::String(source_fqn),
                    ),
                    (
                        COPY_TABLE_DESTINATION.to_string(),
                        OptionValue::String(dest_fqn),
                    ),
                    (
                        COPY_TABLE_WRITE_DISPOSITION.to_string(),
                        OptionValue::String(write_disposition.to_string()),
                    ),
                ]);

                let ctx = query_ctx_from_state(state)?.with_desc("copy_table adapter call");
                self.engine().execute_with_options(
                    Some(state),
                    &ctx,
                    conn,
                    "",
                    options,
                    false,
                    token,
                )?;

                Ok(())
            }
            Postgres | Snowflake | Databricks | Redshift | Salesforce | Spark | DuckDB
            | LakeCompute | Fabric | ClickHouse | Exasol | Starburst | Athena | Trino
            | Datafusion | Dremio | Oracle => {
                unimplemented!("only available with BigQuery adapter")
            }
        }
    }

    /// Return shapes deliberately differ: BigQuery returns a typed `RelationConfig`, Snowflake
    /// returns a raw `SHOW`-query readback.
    pub fn describe_relation(
        &self,
        state: &State,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        include_transient: bool,
        token: CancellationToken,
    ) -> Result<Value, minijinja::Error> {
        if self.adapter_type() == Snowflake {
            match relation.relation_type() {
                Some(RelationType::DynamicTable) => {
                    self.describe_dynamic_table(state, conn, relation, include_transient, token)
                }
                Some(RelationType::InteractiveTable) => {
                    self.describe_interactive_table(state, conn, relation, token)
                }
                other => Err(minijinja::Error::new(
                    minijinja::ErrorKind::InvalidOperation,
                    format!(
                        "describe_relation is not supported for relation type {other:?} on Snowflake"
                    ),
                )),
            }
        } else {
            Ok(self
                .describe_relation_bigquery(conn, relation, Some(state))?
                .map(Value::from_object)
                .unwrap_or_else(none_value))
        }
    }

    /// BigQueryAdapter https://github.com/dbt-labs/dbt-adapters/blob/4a00354a497214d9043bf4122810fe2d04de17bb/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L818
    fn describe_relation_bigquery(
        &self,
        conn: &'_ mut dyn Connection,
        relation: &Arc<dyn BaseRelation>,
        state: Option<&State>,
    ) -> AdapterResult<Option<RelationConfig>> {
        if self.adapter_type() != Bigquery {
            unimplemented!("only available with BigQuery adapter");
        }

        if let (Replay(_, replay), Some(state)) = (self.inner_adapter(), state) {
            let recorded = match replay.replay_describe_relation(state)? {
                // Nothing recorded for this call: report no relation config, the same answer
                // the path below gives for a relation it cannot describe.
                DescribeRelationReplay::NotRecorded => return Ok(None),
                DescribeRelationReplay::Recorded(payload) => payload,
            };
            return crate::relation::bigquery::config::relation_types::materialized_view::relation_config_from_recorded(
                recorded.as_ref(),
            )
            .map(Some);
        }

        let adbc_schema = conn
            .get_table_schema(
                Some(&relation.database_as_str()?),
                Some(&relation.schema_as_str()?),
                &relation.identifier_as_str()?,
            )
            .map_err(adbc_error_to_adapter_error)?;

        let Some(relation_type) = relation.relation_type() else {
            return Ok(None);
        };

        if relation_type != RelationType::MaterializedView {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                format!(
                    "The method `BigQueryAdapter.describe_relation` is not implemented for this relation type: {relation_type}"
                ),
            ));
        }

        crate::relation::bigquery::config::relation_types::materialized_view::new_loader()
            .from_remote_state(&adbc_schema)
            .map(Some)
    }

    /// Ensure that the target relation is valid, by making sure it
    /// has the expected columns.
    ///
    /// Merged (it was not clear if we need to keep the legacy code in
    /// a separate method so we decided not to)
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L927
    pub fn assert_valid_snapshot_target_given_strategy(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
        column_names: Option<BTreeMap<String, String>>,
        strategy: Arc<SnapshotStrategy>,
    ) -> AdapterResult<()> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_assert_valid_snapshot_target_given_strategy(
                state,
                relation,
                column_names,
                strategy,
            ),
            Impl(_, _engine) => {
                let columns = self.get_columns_in_relation(state, relation.as_ref())?;
                let names_in_relation: Vec<String> =
                    columns.iter().map(|c| c.name().to_lowercase()).collect();

                // missing columns
                let mut missing: Vec<String> = Vec::new();

                // Note: we're not checking dbt_updated_at or dbt_is_deleted
                // here because they aren't always present.
                let mut hardcoded_columns = vec!["dbt_scd_id", "dbt_valid_from", "dbt_valid_to"];

                if let Some(ref s) = strategy.hard_deletes
                    && s == "new_record"
                {
                    hardcoded_columns.push("dbt_is_deleted");
                }

                for column in hardcoded_columns {
                    let desired = match column_names {
                        Some(ref tree) => match tree.get(column) {
                            Some(v) => v.to_string(),
                            None => {
                                return Err(AdapterError::new(
                                    AdapterErrorKind::Configuration,
                                    format!("Could not find key {column}"),
                                ));
                            }
                        },
                        None => column.to_string(),
                    };

                    if !names_in_relation.contains(&desired.to_lowercase()) {
                        missing.push(desired);
                    }
                }

                if !missing.is_empty() {
                    return Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        format!("There are missing columns: {missing:?}"),
                    ));
                }

                Ok(())
            }
        }
    }

    /// AthenaAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-athena/src/dbt/adapters/athena/impl.py#L445
    pub fn generate_unique_temporary_table_suffix(
        &self,
        suffix_initial: Option<String>,
    ) -> AdapterResult<String> {
        let suffix_initial = suffix_initial.as_deref().unwrap_or("__dbt_tmp");
        let uuid_str = Uuid::new_v4().to_string().replace('-', "_");
        Ok(format!("{suffix_initial}_{uuid_str}"))
    }

    /// Check the hard_deletes config enum, and the legacy
    /// invalidate_hard_deletes config flag in order to determine
    /// which behavior should be used for deleted records in a
    /// snapshot. The default is to ignore them.
    ///
    /// BaseAdapter https://github.com/dbt-labs/dbt-adapters/blob/0efd8d3d1081e1ab43e38797d5104f7b424a6284/dbt-adapters/src/dbt/adapters/base/impl.py#L1977
    pub fn get_hard_deletes_behavior(
        &self,
        config: BTreeMap<String, Value>,
    ) -> AdapterResult<String> {
        let invalidate_hard_deletes = config.get("invalidate_hard_deletes");
        let hard_deletes = config.get("hard_deletes");

        let invalidate_hard_deletes_is_true = invalidate_hard_deletes.is_some_and(|v| v.is_true());

        if invalidate_hard_deletes_is_true && hard_deletes.is_some() {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "You cannot set both the invalidate_hard_deletes and hard_deletes config properties on the same snapshot.",
            ));
        }

        if invalidate_hard_deletes_is_true {
            return Ok("invalidate".to_string());
        }

        match hard_deletes {
            None => Ok("ignore".to_string()),
            Some(val) => {
                // Treat null values same as missing (None)
                if val.is_none() {
                    return Ok("ignore".to_string());
                }
                match val.as_str() {
                    Some("invalidate") => Ok("invalidate".to_string()),
                    Some("new_record") => Ok("new_record".to_string()),
                    Some("ignore") => Ok("ignore".to_string()),
                    Some(_) => Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "Invalid string value for property hard_deletes.",
                    )),
                    None => Err(AdapterError::new(
                        AdapterErrorKind::Configuration,
                        "Invalid type for property hard_deletes (expected string).",
                    )),
                }
            }
        }
    }

    /// Optional fast-path for replay adapters: return schema existence from the trace
    /// when available.
    ///
    /// Default is None for non-replay adapters.
    pub fn schema_exists_from_trace(
        &self,
        state: &State,
        database: &str,
        schema: &str,
    ) -> Option<bool> {
        match self.inner_adapter() {
            Replay(_, replay) => replay.replay_schema_exists_from_trace(state, database, schema),
            Impl(_, _engine) => None,
        }
    }

    pub fn get_adbc_execute_options(&self, state: &State) -> ExecuteOptions {
        match self.adapter_type() {
            Bigquery => {
                let mut options = vec![(
                    QUERY_LINK_FAILED_JOB.to_string(),
                    OptionValue::String("true".to_string()),
                )];

                let timeout = bigquery_job_timeout_from_state(state).or_else(|| {
                    self.get_db_config("job_execution_timeout_seconds")
                        .and_then(|v| v.parse::<i64>().ok())
                });

                if let Some(t) = timeout {
                    options.push((QUERY_JOB_TIMEOUT.to_string(), OptionValue::Int(t * 1000)));
                }

                options
            }
            _ => Vec::new(),
        }
    }
}

/// Reads `job_execution_timeout_seconds` from the BigQuery adapter attr of the current
/// model or snapshot in the Jinja state. Returns `None` if the state has no model or the
/// model has no BigQuery timeout configured.
///
/// The `bigquery_attr` field lives at the top level of the model value because
/// `dbt-yaml`'s `flatten_dunder` serialization merges `__adapter_attr__` into the parent.
fn bigquery_job_timeout_from_state(state: &State) -> Option<i64> {
    let model = state.lookup("model", &[])?;
    let bq_attr = model.get_attr("bigquery_attr").ok()?;
    bq_attr
        .get_attr("job_execution_timeout_seconds")
        .ok()?
        .as_i64()
}

/// List of possible builtin strategies for adapters.
/// Microbatch is always included — `require_batched_execution_for_custom_microbatch_strategy`
/// is always True in Fusion (new behavior only, no legacy path).
/// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/impl.py#L1690-L1691
fn builtin_incremental_strategies() -> Vec<DbtIncrementalStrategy> {
    vec![
        DbtIncrementalStrategy::Append,
        DbtIncrementalStrategy::DeleteInsert,
        DbtIncrementalStrategy::Merge,
        DbtIncrementalStrategy::InsertOverwrite,
        DbtIncrementalStrategy::Microbatch,
        DbtIncrementalStrategy::Legacy, // ClickHouse only — intermediate-table + swap
    ]
}

// https://github.com/dbt-labs/dbt-adapters/blob/3ed165d452a0045887a5032c621e605fd5c57447/dbt-adapters/src/dbt/adapters/base/impl.py#L117
pub(crate) static DEFAULT_BASE_BEHAVIOR_FLAGS: LazyLock<[BehaviorFlag; 4]> = LazyLock::new(|| {
    [
        BehaviorFlag::new(
            "require_batched_execution_for_custom_microbatch_strategy",
            true,
            Some("https://docs.getdbt.com/docs/build/incremental-microbatch"),
            None,
            None,
        ),
        BehaviorFlag::new("enable_truthy_nulls_equals_macro", false, None, None, None),
        BehaviorFlag::new(
            "use_catalogs_v2",
            true,
            None,
            Some(
                "Validate catalogs.yml against the v2 schema. Set to `false` to keep the deprecated v1 schema.",
            ),
            Some("https://github.com/dbt-labs/dbt-core/discussions/12723"),
        ),
        BehaviorFlag::new(
            "require_resource_names_without_plus_prefix",
            false,
            None,
            Some(
                "When enabled, the + prefix in dbt_project.yml always indicates a configuration key. Folder and file names may not start with the + prefix.",
            ),
            None,
        ),
    ]
});

/// Get adapter-specific behavior flags for a given adapter type
/// This is a standalone function to avoid needing to create adapter instances
/// just to get the flags
pub(crate) fn adapter_specific_behavior_flags(adapter_type: AdapterType) -> Vec<BehaviorFlag> {
    match adapter_type {
        Snowflake => {
            // https://github.com/dbt-labs/dbt-adapters/blob/c4c04de76d5a6c56c95965041a93156fdeaf4641/dbt-snowflake/src/dbt/adapters/snowflake/impl.py#L46
            let flag = BehaviorFlag::new(
                "snowflake_default_transient_dynamic_tables",
                false,
                Some(
                    "When enabled, dynamic tables default to transient (matching regular table behavior). This is a breaking change from previous behavior where dynamic tables were non-transient.",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Databricks => {
            let use_user_folder_for_python = BehaviorFlag::new(
                "use_user_folder_for_python",
                true,
                Some(
                    "Use the user's home folder for uploading python notebooks. Shared folder use is deprecated due to governance concerns.",
                ),
                None,
                None,
            );

            let use_materialization_v2 = BehaviorFlag::new(
                "use_materialization_v2",
                false,
                Some(
                    "Use revamped materializations based on separating create and insert. This allows more performant column comments, as well as new column features.",
                ),
                None,
                None,
            );

            let use_replace_on_for_insert_overwrite = BehaviorFlag::new(
                "use_replace_on_for_insert_overwrite",
                true,
                Some(
                    "Use INSERT INTO ... REPLACE ON syntax for insert_overwrite on SQL Warehouses. When enabled, only matching partitions are overwritten; historical partitions are preserved.",
                ),
                None,
                None,
            );

            let use_managed_iceberg = BehaviorFlag::new(
                "use_managed_iceberg",
                false,
                Some(
                    "Use managed Iceberg tables when table_format is iceberg. When this flag is disabled, UniForm is used instead.",
                ),
                None,
                None,
            );

            // https://github.com/databricks/dbt-databricks/blob/3caad339bb3e60b7c795684374c3c8a1d9042279/dbt/adapters/databricks/impl.py#L160
            let use_describe_as_json_for_relation_metadata = BehaviorFlag::new(
                "use_describe_as_json_for_relation_metadata",
                false,
                Some(
                    "Use DESCRIBE TABLE EXTENDED AS JSON when supported to fetch relation metadata like constraints, column masks, row filters, view definition, etc. When disabled, falls back to information_schema queries.",
                ),
                None,
                None,
            );

            vec![
                use_user_folder_for_python,
                use_materialization_v2,
                use_replace_on_for_insert_overwrite,
                use_managed_iceberg,
                use_describe_as_json_for_relation_metadata,
            ]
        }
        Bigquery => {
            // https://github.com/dbt-labs/dbt-adapters/blob/b9ebd240e39882a8c43ed659de423c7504d4642a/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L109-L110
            let flag = BehaviorFlag::new(
                "bigquery_noop_alter_relation_comment",
                false,
                Some(
                    "Make bigquery__alter_relation_comment a no-op. This is useful when relation descriptions are already set in DDL (e.g. via OPTIONS(description=...)) to avoid an unnecessary update.",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Fabric => {
            let flag = BehaviorFlag::new(
                "empty",
                false,
                Some(
                    "When enabled, table and view materializations will be created as empty structures (no data).",
                ),
                None,
                None,
            );
            vec![flag]
        }
        Redshift => {
            // TODO: https://github.com/dbt-labs/fs/issues/11871
            let skip_autocommit_transaction_statements = BehaviorFlag::new(
                "redshift_skip_autocommit_transaction_statements",
                false,
                Some(
                    "When enabled, skip BEGIN/COMMIT wrapping so statements that cannot run in a transaction block (e.g. ALTER COLUMN TYPE for VARCHAR/VARBYTE size changes) can be issued.",
                ),
                None,
                None,
            );
            // https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-redshift/src/dbt/adapters/redshift/impl.py#L56-L66
            let grants_extended = BehaviorFlag::new(
                "redshift_grants_extended",
                false,
                Some(
                    "Enable Redshift grants for groups and roles using 'user:', 'group:', and 'role:' prefixes. Unprefixed grantees remain users for backward compatibility.",
                ),
                None,
                None,
            );
            vec![skip_autocommit_transaction_statements, grants_extended]
        }
        Postgres | Salesforce | Spark | DuckDB | LakeCompute | ClickHouse | Exasol | Starburst
        | Athena | Trino | Datafusion | Dremio | Oracle => vec![],
    }
}

/// The adapter implementation. All adapter methods live here.
#[derive(Clone)]
pub struct AdapterImpl {
    inner: AdapterImplInner,
    schema_store: Option<Arc<dyn SchemaStoreTrait>>,
}

#[derive(Clone)]
struct MockState {
    engine: Arc<dyn AdapterEngine>,
    flags: BTreeMap<String, Value>,
    behavior: Arc<Behavior>,
}

#[derive(Clone)]
enum AdapterImplInner {
    Impl(Arc<dyn AdapterEngine>),
    Replay(Arc<dyn Replayer>),
    Mock(MockState),
}

impl AdapterImpl {
    pub fn new(
        engine: Arc<dyn AdapterEngine>,
        schema_store: Option<Arc<dyn SchemaStoreTrait>>,
    ) -> Self {
        Self {
            inner: AdapterImplInner::Impl(engine),
            schema_store,
        }
    }

    pub fn new_replay(
        replay: Arc<dyn Replayer>,
        schema_store: Option<Arc<dyn SchemaStoreTrait>>,
    ) -> Self {
        Self {
            inner: AdapterImplInner::Replay(replay),
            schema_store,
        }
    }

    pub fn new_mock(
        adapter_type: AdapterType,
        flags: BTreeMap<String, Value>,
        quoting: ResolvedQuoting,
        type_ops: Arc<dyn TypeOps>,
        stmt_splitter: Arc<dyn StmtSplitter>,
    ) -> Self {
        let backend = crate::adapter::adapter_factory::backend_of(adapter_type);
        let auth: Arc<dyn dbt_auth::Auth> = dbt_auth::auth_for_backend(backend).into();
        let engine: Arc<dyn AdapterEngine> = Arc::new(AdbcEngine::new_mock(
            adapter_type,
            auth,
            AdapterConfig::default(),
            quoting,
            type_ops,
            stmt_splitter,
            Arc::new(crate::cache::RelationCache::default()),
            BTreeMap::new(),
        ));
        let is_true = flags.get("is_true").is_none_or(|v| v.is_true());
        let is_false = flags.get("is_false").is_some_and(|v| v.is_true());
        let is_unknown = flags.get("is_unknown").is_none_or(|v| v.is_true());
        let enable_truthy_nulls_equals_macro = flags
            .get("enable_truthy_nulls_equals_macro")
            .is_some_and(|v| v.is_true());
        let behavior = Arc::new(Behavior::new(
            vec![
                BehaviorFlag::new("is_true", is_true, None, None, None),
                BehaviorFlag::new("is_false", is_false, None, None, None),
                BehaviorFlag::new("is_unknown", is_unknown, None, None, None),
                BehaviorFlag::new(
                    "enable_truthy_nulls_equals_macro",
                    enable_truthy_nulls_equals_macro,
                    None,
                    None,
                    None,
                ),
            ],
            &BTreeMap::new(),
        ));
        Self {
            inner: AdapterImplInner::Mock(MockState {
                engine,
                flags,
                behavior,
            }),
            schema_store: None,
        }
    }

    pub fn get_schema_from_cache(
        &self,
        relation: &dyn BaseRelation,
    ) -> Option<dbt_schema_store::SchemaEntry> {
        self.schema_store
            .as_ref()
            .and_then(|ss| ss.get_schema(&relation.get_canonical_fqn().unwrap_or_default()))
    }

    fn mock_state(&self) -> Option<&MockState> {
        match &self.inner {
            AdapterImplInner::Mock(state) => Some(state),
            _ => None,
        }
    }

    fn is_explicit_mock(&self) -> bool {
        matches!(&self.inner, AdapterImplInner::Mock(_))
    }

    fn introspect_enabled(&self) -> bool {
        match self.mock_state() {
            Some(mock) => mock
                .flags
                .get("introspect")
                .map(|value| value.is_true())
                .unwrap_or(true),
            None => true,
        }
    }
}

impl AdapterImpl {
    pub fn inner_adapter(&self) -> InnerAdapter<'_> {
        match &self.inner {
            AdapterImplInner::Impl(engine) => Impl(engine.adapter_type(), engine),
            AdapterImplInner::Replay(replay) => {
                Replay(replay.engine().adapter_type(), replay.as_ref())
            }
            AdapterImplInner::Mock(mock) => Impl(mock.engine.adapter_type(), &mock.engine),
        }
    }

    #[inline]
    pub fn adapter_type(&self) -> AdapterType {
        match self.inner_adapter() {
            Impl(adapter_type, _) | Replay(adapter_type, _) => adapter_type,
        }
    }

    pub fn as_replay(&self) -> Option<&dyn Replayer> {
        match self.inner_adapter() {
            Replay(_, replay) => Some(replay),
            Impl(..) => None,
        }
    }

    /// ClickHouse `adapter.is_before_version(version)` — see
    /// [`metadata::clickhouse::ClickHouseCapabilities::is_before`].
    pub fn is_before_version(
        &self,
        state: &State,
        version: &str,
        token: CancellationToken,
    ) -> AdapterResult<bool> {
        metadata::clickhouse::server_capabilities(self, state, token).is_before(version)
    }

    /// ClickHouse `adapter.is_at_or_after_version(version)` — see
    /// [`metadata::clickhouse::ClickHouseCapabilities::is_at_or_after`].
    pub fn is_at_or_after_version(
        &self,
        state: &State,
        version: &str,
        token: CancellationToken,
    ) -> AdapterResult<bool> {
        metadata::clickhouse::server_capabilities(self, state, token).is_at_or_after(version)
    }

    /// ClickHouse `adapter.calculate_incremental_strategy(strategy)` — see
    /// [`metadata::clickhouse::calculate_incremental_strategy`].
    pub fn calculate_incremental_strategy(
        &self,
        state: &State,
        strategy: Option<&str>,
        token: CancellationToken,
    ) -> String {
        metadata::clickhouse::calculate_incremental_strategy(
            strategy,
            metadata::clickhouse::server_capabilities(self, state, token).use_lw_deletes,
        )
    }

    /// ClickHouse `adapter.validate_incremental_strategy(strategy, predicates,
    /// unique_key, partition_by)` — see
    /// [`metadata::clickhouse::validate_incremental_strategy`].
    pub fn validate_incremental_strategy(
        &self,
        state: &State,
        strategy: &str,
        has_predicates: bool,
        has_unique_key: bool,
        has_partition_by: bool,
        token: CancellationToken,
    ) -> AdapterResult<()> {
        metadata::clickhouse::validate_incremental_strategy(
            strategy,
            has_predicates,
            has_unique_key,
            has_partition_by,
            metadata::clickhouse::server_capabilities(self, state, token).has_lw_deletes,
        )
    }

    /// ClickHouse `adapter.check_incremental_schema_changes(...)` — see
    /// [`metadata::clickhouse::column_changes_value`]. Returns none when the
    /// relation does not exist yet.
    #[allow(clippy::too_many_arguments)]
    pub fn check_incremental_schema_changes(
        &self,
        state: &State,
        on_schema_change: &str,
        existing: Option<Arc<dyn BaseRelation>>,
        target_sql: &str,
        materialization: &str,
        // dbt-clickhouse #689: applied to the DESCRIBE probe so introspected
        // types match runtime (e.g. join_use_nulls).
        query_settings: Option<&Value>,
        token: CancellationToken,
    ) -> AdapterResult<Value> {
        if !matches!(
            on_schema_change,
            "fail" | "ignore" | "append_new_columns" | "sync_all_columns"
        ) {
            return Err(AdapterError::new(
                AdapterErrorKind::Configuration,
                "Only `fail`, `ignore`, `append_new_columns`, and `sync_all_columns` supported for `on_schema_change`.",
            ));
        }
        let Some(existing) = existing else {
            return Ok(none_value());
        };

        let source = self.get_columns_in_relation(state, existing.as_ref())?;
        let target = {
            let ctx = query_ctx_from_state(state)?.with_desc("check_incremental_schema_changes");
            let mut conn = self.borrow_tlocal_connection(Some(state), node_id_from_state(state))?;
            self.get_column_schema_from_query(
                state,
                conn.as_mut(),
                &ctx,
                target_sql,
                query_settings,
                token,
            )?
        };

        metadata::clickhouse::column_changes_value(
            on_schema_change,
            source,
            target,
            materialization,
        )
    }

    /// ClickHouse: render the model's `settings` config as a table-level `SETTINGS ...`
    /// block for CREATE TABLE DDL. The leading `-- end_of_sql` marker is what
    /// `clickhouse__place_limit` (utils/utils.sql) splits on to inject LIMIT ahead of
    /// the SETTINGS clause. Mirrors impl.py `get_model_settings`, including dbclient.py's
    /// `replicated_deduplication_window='0'` default (profile flag
    /// `allow_automatic_deduplication` opts out; a user-provided value wins).
    pub fn get_model_settings(&self, model: &Value, engine: &str) -> String {
        // dbclient.py DEDUP_WINDOW_SETTING_SUPPORTED_MATERIALIZATION
        const DEDUP_SUPPORTED_MATERIALIZATIONS: &[&str] =
            &["table", "incremental", "ephemeral", "materialized_view"];
        const DEDUP_WINDOW_SETTING: &str = "replicated_deduplication_window";

        let mut settings = metadata::clickhouse::model_config_map(model, "settings");

        let materialized = model
            .get_attr("config")
            .ok()
            .and_then(|config| config.get_attr("materialized").ok())
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        let allow_automatic_deduplication = self
            .get_db_config_value("allow_automatic_deduplication")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        if DEDUP_SUPPORTED_MATERIALIZATIONS.contains(&materialized.as_str())
            && !allow_automatic_deduplication
            && !settings.iter().any(|(key, _)| key == DEDUP_WINDOW_SETTING)
        {
            // Upstream sets the string '0', which _build_settings_str single-quotes.
            settings.push((DEDUP_WINDOW_SETTING.to_string(), Value::from("0")));
        }

        // filter_settings_by_engine: replicated_deduplication_window is MergeTree-only.
        settings.retain(|(key, _)| engine.contains("MergeTree") || key != DEDUP_WINDOW_SETTING);

        let settings_str = metadata::clickhouse::build_settings_str(&settings);
        format!("\n-- end_of_sql\n{settings_str}\n")
    }

    /// ClickHouse: render the model's `query_settings` config as a query-level
    /// `SETTINGS ...` clause appended to the SELECT; empty string when unset.
    pub fn get_model_query_settings(&self, model: &Value) -> String {
        let settings = metadata::clickhouse::model_config_map(model, "query_settings");
        let settings_str = metadata::clickhouse::build_settings_str(&settings);
        if settings_str.is_empty() {
            String::new()
        } else {
            format!("\n-- settings_section\n{settings_str}\n")
        }
    }

    /// ClickHouse: see [metadata::clickhouse::s3source_clause].
    #[allow(clippy::too_many_arguments)]
    pub fn s3source_clause(
        &self,
        vars_config: Option<&Value>,
        model_config: Option<&Value>,
        structure: &Value,
        bucket: &str,
        path: &str,
        fmt: &str,
        aws_access_key_id: &str,
        aws_secret_access_key: &str,
        role_arn: &str,
        compression: &str,
        external_id: &str,
    ) -> AdapterResult<String> {
        let s3config = metadata::clickhouse::merge_s3_config(vars_config, model_config);
        metadata::clickhouse::s3source_clause(
            &s3config,
            structure,
            bucket,
            path,
            fmt,
            aws_access_key_id,
            aws_secret_access_key,
            role_arn,
            compression,
            external_id,
        )
    }

    pub fn engine(&self) -> &Arc<dyn AdapterEngine> {
        match self.inner_adapter() {
            Impl(_, engine) => engine,
            Replay(_, replay) => replay.engine(),
        }
    }

    pub fn quoting(&self) -> ResolvedQuoting {
        match self.inner_adapter() {
            Impl(_, engine) => engine.quoting(),
            Replay(_, replay) => replay.engine().quoting(),
        }
    }
}

impl fmt::Debug for AdapterImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.adapter_type())
    }
}

/// Outcome of asking the recording for a `describe_relation` call.
///
/// Distinct from a plain `Option` so a recording with no record for this call (fall back to no
/// relation config) can't be confused with a recording that has one whose payload is null (an
/// error case `relation_config_from_recorded` already handles).
#[derive(Debug, Clone)]
pub enum DescribeRelationReplay {
    /// No `describe_relation` record was at the replay cursor.
    NotRecorded,
    /// A record was present; its payload may itself be null.
    Recorded(Option<serde_json::Value>),
}

/// Abstract interface for the functions that the adapter can call to perform replays
/// consuming recorded runs instead of making real calls to the data warehouse.
pub trait Replayer: fmt::Debug + Send + Sync {
    fn engine(&self) -> &Arc<dyn AdapterEngine>;

    fn adapter_type(&self) -> AdapterType {
        self.engine().adapter_type()
    }

    fn metadata_adapter(&self) -> Option<Box<dyn MetadataAdapter>>;

    /// Seed a mapping from a truncated/hashed generic test name back to the original pre-hash
    /// full test name. Default implementation is a no-op.
    fn record_test_name_truncation(&self, _truncated_name: &str, _full_name: &str) {}

    /// Seed a mapping from a model's unique_id to its resolved alias, so an ephemeral model's
    /// own node_id can be traced back to the CTE name dbt-core actually compiled it under. A
    /// versioned model's unique_id ends in `.v<N>`, which is not its alias (e.g. `stg_thing_v2`
    /// vs. unique_id `model.pkg.stg_thing.v2`), so this cannot be reconstructed from the
    /// unique_id alone. Default implementation is a no-op.
    fn record_node_alias(&self, _unique_id: &str, _alias: &str) {}

    fn replay_use_warehouse(
        &self,
        conn: &'_ mut dyn Connection,
        warehouse: String,
        node_id: &str,
    ) -> FsResult<bool>;

    fn replay_verify_database(&self, database: &str) -> AdapterResult<Value>;

    /// Non-consuming peek: return true if the next per-node replay record is a BigQuery
    /// `is_replaceable` record.
    ///
    /// This exists for cross-implementation replay compatibility: Mantle recorder may emit an
    /// `is_replaceable(relation=None, ...)` record even when the adapter implementation would
    /// trivially return `true` without consulting the warehouse.
    ///
    /// Default is `false` to preserve behavior for replay adapters that don't support peeking.
    fn replay_peek_is_replaceable_next(&self, _state: &State) -> AdapterResult<bool> {
        Ok(false)
    }

    /// Non-consuming peek: return true if the next per-node replay record is an execute record.
    fn replay_peek_execute_next(&self, _state: &State) -> AdapterResult<bool> {
        Ok(false)
    }

    #[allow(clippy::too_many_arguments)]
    fn replay_execute(
        &self,
        state: Option<&State>,
        conn: &'_ mut dyn Connection,
        ctx: &QueryCtx,
        sql: &str,
        auto_begin: bool,
        fetch: bool,
        limit: Option<i64>,
        options: Option<ExecuteOptions>,
    ) -> AdapterResult<(AdapterResponse, AgateTable)>;

    fn replay_add_query(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        sql: &str,
        auto_begin: bool,
        bindings: Option<&Value>,
        abridge_sql_log: bool,
    ) -> AdapterResult<()>;

    fn replay_get_relation(
        &self,
        state: &State,
        query_ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        database: &str,
        schema: &str,
        identifier: &str,
    ) -> AdapterResult<Option<Arc<dyn BaseRelation>>>;

    fn replay_truncate_relation(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value>;

    fn replay_quote(&self, state: &State, identifier: &str) -> AdapterResult<String>;

    fn replay_quote_seed_column(
        &self,
        state: &State,
        column: &str,
        quote_config: Option<bool>,
    ) -> AdapterResult<String>;

    fn replay_convert_type(
        &self,
        state: &State,
        data_type: &DataType,
    ) -> AdapterResult<Option<String>>;

    fn replay_list_relations(
        &self,
        query_ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        db_schema: &CatalogAndSchema,
    ) -> AdapterResult<Vec<Arc<dyn BaseRelation>>>;

    fn replay_rename_relation(
        &self,
        state: &State,
        from_relation: &Arc<dyn BaseRelation>,
        to_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value>;

    fn replay_get_column_schema_from_query(
        &self,
        state: &State,
        _conn: &mut dyn Connection,
        _query_ctx: &QueryCtx,
        sql: &str,
    ) -> AdapterResult<Vec<Column>>;

    fn replay_get_columns_in_select_sql(&self, state: &State) -> AdapterResult<Vec<Column>>;

    fn replay_get_columns_in_relation(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
        cache_result: Option<Vec<Column>>,
    ) -> Result<Value, minijinja::Error>;

    fn replay_submit_python_job(
        &self,
        ctx: &QueryCtx,
        conn: &'_ mut dyn Connection,
        state: &State,
        model: &Value,
        compiled_code: &str,
    ) -> AdapterResult<AdapterResponse>;

    fn replay_render_raw_columns_constraints(
        &self,
        _state: &State,
        _columns_map: IndexMap<String, DbtColumn>,
    ) -> AdapterResult<Vec<String>>;

    fn replay_render_raw_model_constraints(
        &self,
        _state: &State,
        _raw_constraints: &[ModelConstraint],
    ) -> Result<Value, minijinja::Error>;

    fn replay_expand_target_column_types(
        &self,
        state: &State,
        _from_relation: &Arc<dyn BaseRelation>,
        _to_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value>;

    fn replay_is_replaceable(&self, state: &State) -> AdapterResult<bool>;

    fn replay_update_table_description(
        &self,
        state: &State,
        database: &str,
        schema: &str,
        identifier: &str,
        description: &str,
    ) -> AdapterResult<Value>;

    fn replay_update_columns(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value>;

    fn replay_load_dataframe(
        &self,
        state: &State,
        database: &str,
        schema: &str,
        table_name: &str,
    ) -> AdapterResult<Value>;

    fn replay_copy_table(
        &self,
        state: &State,
        source: &Arc<dyn BaseRelation>,
        destination: &Arc<dyn BaseRelation>,
        materialization: &str,
    ) -> AdapterResult<()>;

    fn replay_get_dataset_location(
        &self,
        state: &State,
        relation: &dyn BaseRelation,
    ) -> AdapterResult<Option<String>>;

    fn replay_alter_table_add_columns(
        &self,
        state: &State,
        relation: &Arc<dyn BaseRelation>,
        columns: &[Column],
    ) -> AdapterResult<Value>;

    fn replay_grant_access_to(
        &self,
        state: &State,
        entity: &Arc<dyn BaseRelation>,
        entity_type: &str,
        database: &str,
        schema: &str,
    ) -> AdapterResult<Value>;

    fn replay_describe_relation(&self, state: &State) -> AdapterResult<DescribeRelationReplay>;

    fn replay_schema_exists_from_trace(
        &self,
        state: &State,
        database: &str,
        schema: &str,
    ) -> Option<bool>;

    fn replay_get_missing_columns(
        &self,
        state: &State,
        _source_relation: &Arc<dyn BaseRelation>,
        _target_relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Vec<Column>>;

    fn replay_drop_relation(
        &self,
        state: &State,
        _relation: &Arc<dyn BaseRelation>,
    ) -> AdapterResult<Value>;

    fn replay_valid_snapshot_target(
        &self,
        state: &State,
        _relation: &Arc<dyn BaseRelation>,
        _column_names: Option<BTreeMap<String, String>>,
    ) -> AdapterResult<()>;

    fn replay_assert_valid_snapshot_target_given_strategy(
        &self,
        state: &State,
        _relation: &Arc<dyn BaseRelation>,
        _column_names: Option<BTreeMap<String, String>>,
        _strategy: Arc<SnapshotStrategy>,
    ) -> AdapterResult<()>;

    /// The following four calls are only instrumented by recorders that know about them, so each
    /// returns `None` when the recording holds no dedicated record for it. Callers must then
    /// compute the value themselves: consuming an unrelated record instead would shift every
    /// following record for the node.
    fn replay_is_uniform(&self, state: &State) -> AdapterResult<Option<bool>>;

    fn replay_has_dbr_capability(
        &self,
        state: &State,
        capability_name: &str,
    ) -> AdapterResult<Option<bool>>;

    fn replay_is_cluster(&self, state: &State) -> AdapterResult<Option<bool>>;

    /// Returns the recorded `get_relation_config` payload (`{"config": {...}}`).
    /// The engine method reconstructs a `RelationConfig` from it.
    fn replay_get_relation_config(&self, state: &State)
    -> AdapterResult<Option<serde_json::Value>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::adapter_factory::backend_of;
    use crate::cache::RelationCache;
    use crate::column::Column;
    use crate::config::AdapterConfig;
    use crate::engine::AdbcEngine;

    use crate::engine::query_comment::QueryCommentConfig;
    use crate::sql_types::DefaultTypeOps;
    use crate::stmt_splitter::DefaultStmtSplitter;

    use dbt_adapter_core::AdapterType;
    use dbt_auth::auth_for_backend;
    use dbt_common::AdapterResult;
    use dbt_schemas::schemas::dbt_column::{DbtColumn, DbtColumnRef};
    use dbt_schemas::schemas::relations::base::ComponentName;
    use dbt_schemas::schemas::relations::{DEFAULT_RESOLVED_QUOTING, SNOWFLAKE_RESOLVED_QUOTING};
    use dbt_yaml::Mapping;

    use minijinja::{Environment, State, Value};

    #[test]
    fn test_databricks_describe_as_json_behavior_flag_registered_and_off_by_default() {
        let flags = adapter_specific_behavior_flags(Databricks);
        let flag = flags
            .iter()
            .find(|f| f.name == "use_describe_as_json_for_relation_metadata")
            .expect(
                "use_describe_as_json_for_relation_metadata should be registered for Databricks",
            );

        assert!(
            !Arc::new(flag.clone()).is_true(),
            "use_describe_as_json_for_relation_metadata should default to off"
        );
    }

    fn engine(adapter_type: AdapterType) -> Arc<dyn AdapterEngine> {
        let config = match adapter_type {
            Snowflake => Mapping::from_iter([
                ("user".into(), "U".into()),
                ("password".into(), "P".into()),
                ("account".into(), "A".into()),
                ("database".into(), "D".into()),
                ("schema".into(), "S".into()),
                ("role".into(), "role".into()),
                ("warehouse".into(), "warehouse".into()),
            ]),
            DuckDB => {
                let attach = YmlValue::Sequence(
                    vec![YmlValue::Mapping(
                        Mapping::from_iter([
                            ("path".into(), "md:some_db".into()),
                            ("is_ducklake".into(), true.into()),
                        ]),
                        Default::default(),
                    )],
                    Default::default(),
                );
                Mapping::from_iter([
                    ("path".into(), "md:my_db".into()),
                    ("is_ducklake".into(), true.into()),
                    ("attach".into(), attach),
                ])
            }
            Bigquery | Redshift | Spark | Databricks => Mapping::new(),
            _ => unimplemented!("mock config for adapter type {:?}", adapter_type),
        };
        build_engine(adapter_type, config)
    }

    fn build_engine(adapter_type: AdapterType, config: Mapping) -> Arc<dyn AdapterEngine> {
        build_engine_with_behavior(adapter_type, config, BTreeMap::new())
    }

    fn build_engine_with_behavior(
        adapter_type: AdapterType,
        config: Mapping,
        behavior_flag_overrides: BTreeMap<String, bool>,
    ) -> Arc<dyn AdapterEngine> {
        let auth = auth_for_backend(backend_of(adapter_type));
        let resolved_quoting = match adapter_type {
            Snowflake => SNOWFLAKE_RESOLVED_QUOTING,
            _ => DEFAULT_RESOLVED_QUOTING,
        };
        Arc::new(AdbcEngine::new(
            adapter_type,
            auth.into(),
            AdapterConfig::new(config),
            resolved_quoting,
            QueryCommentConfig::from_query_comment(None, adapter_type, false, None),
            Arc::new(DefaultTypeOps::new(adapter_type)), // XXX: NaiveTypeOpsImpl
            Arc::new(DefaultStmtSplitter), // XXX: may cause bugs if these tests run SQL
            Arc::new(RelationCache::default()),
            behavior_flag_overrides,
            None,
            None,
        ))
    }

    #[test]
    fn test_adapter_type() {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        assert_eq!(adapter.adapter_type(), Snowflake);
    }

    fn databricks_external_path(include_full_name_in_path: bool, is_incremental: bool) -> String {
        use crate::relation::databricks::config::test_helpers::{
            TestModelConfig, create_mock_dbt_model,
        };
        use dbt_schemas::schemas::project::WarehouseSpecificNodeConfig;

        let adapter = AdapterImpl::new(engine(Databricks), None);
        let node = create_mock_dbt_model(TestModelConfig::default());
        let config = ModelConfig {
            __warehouse_specific_config__: WarehouseSpecificNodeConfig {
                location_root: Some("s3://bucket/root".to_string()),
                include_full_name_in_path: Some(include_full_name_in_path),
                ..Default::default()
            },
            ..Default::default()
        };

        adapter
            .compute_external_path(config, &node, is_incremental)
            .expect("location_root is set")
    }

    #[test]
    fn test_databricks_compute_external_path_uses_alias_without_full_name() {
        assert_eq!(
            databricks_external_path(false, false),
            "s3://bucket/root/test_table"
        );
    }

    #[test]
    fn test_databricks_compute_external_path_with_full_name() {
        assert_eq!(
            databricks_external_path(true, false),
            "s3://bucket/root/test_db/test_schema/test_table"
        );
    }

    #[test]
    fn test_databricks_compute_external_path_incremental_suffix() {
        assert_eq!(
            databricks_external_path(false, true),
            "s3://bucket/root/test_table_tmp"
        );
    }

    #[test]
    fn test_restore_warehouse_matches_snowflake_current_warehouse_text() {
        assert_eq!(
            AdapterImpl::warehouse_restore_name("analytics_transform"),
            "ANALYTICS_TRANSFORM"
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"DBT_TRANSFORMATION\""),
            "DBT_TRANSFORMATION"
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"CaseSensitive\""),
            "\"CaseSensitive\""
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"CASE SENSITIVE\""),
            "\"CASE SENSITIVE\""
        );
        assert_eq!(AdapterImpl::warehouse_restore_name("\"A-B\""), "\"A-B\"");
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"3RD_WH\""),
            "\"3RD_WH\""
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"QUOTE\"\"HERE\""),
            "\"QUOTE\"\"HERE\""
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"SELECT\""),
            "\"SELECT\""
        );
        assert_eq!(
            AdapterImpl::warehouse_restore_name("\"Unbalanced"),
            "\"Unbalanced"
        );
        assert_eq!(AdapterImpl::warehouse_restore_name("\"WH$1\""), "WH$1");
        assert_eq!(AdapterImpl::warehouse_restore_name("\"_WH\""), "_WH");
        assert_eq!(AdapterImpl::warehouse_restore_name("\"\""), "\"\"");
    }

    #[test]
    fn databricks_accepts_delete_insert_incremental_strategy() {
        let adapter = AdapterImpl::new(engine(Databricks), None);

        assert!(
            adapter
                .valid_incremental_strategies()
                .contains(&DbtIncrementalStrategy::DeleteInsert)
        );
    }

    #[test]
    fn databricks_resolves_delete_insert_incremental_macro() {
        let adapter = AdapterImpl::new(engine(Databricks), None);
        let env = Environment::new();
        let state = State::new_for_env(&env);

        let macro_value = adapter
            .get_incremental_strategy_macro(&state, "delete+insert")
            .expect("delete+insert must resolve for Databricks");

        assert_eq!(
            macro_value
                .get_attr("macro_name")
                .expect("dispatch object must expose its macro name")
                .as_str(),
            Some("get_incremental_delete_insert_sql")
        );
    }

    #[test]
    fn test_quote_for_snowflake() {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        assert_eq!(adapter.quote("abc"), "\"abc\"");
    }

    #[test]
    fn test_quote_for_bigquery() {
        let adapter = AdapterImpl::new(engine(Bigquery), None);
        assert_eq!(adapter.quote("abc"), "`abc`");
    }

    fn clickhouse_adapter(config: Mapping) -> AdapterImpl {
        AdapterImpl::new(build_engine(ClickHouse, config), None)
    }

    #[test]
    fn test_get_credentials_profile_values_and_defaults() {
        // user/host fall back to impl.py defaults when absent from the profile;
        // password/database/port default to empty strings.
        let adapter = clickhouse_adapter(Mapping::from_iter([
            ("password".into(), "secret".into()),
            ("database".into(), "analytics".into()),
        ]));
        let creds = adapter.get_credentials(&Value::UNDEFINED);

        assert_eq!(creds.get_attr("user").unwrap().as_str(), Some("default"));
        assert_eq!(creds.get_attr("host").unwrap().as_str(), Some("localhost"));
        assert_eq!(creds.get_attr("password").unwrap().as_str(), Some("secret"));
        assert_eq!(
            creds.get_attr("database").unwrap().as_str(),
            Some("analytics")
        );
        assert_eq!(creds.get_attr("port").unwrap().as_str(), Some(""));
    }

    #[test]
    fn test_get_credentials_overrides_replace_and_add_keys() {
        let adapter = clickhouse_adapter(Mapping::from_iter([
            ("user".into(), "profile_user".into()),
            ("password".into(), "profile_pass".into()),
            ("host".into(), "ch.example.com".into()),
            ("port".into(), 8123.into()),
        ]));
        let overrides = Value::from(BTreeMap::from([
            ("user".to_string(), Value::from("override_user")),
            ("cluster".to_string(), Value::from("test_shard")),
        ]));
        let creds = adapter.get_credentials(&overrides);

        // Overridden key replaced; unknown override key added.
        assert_eq!(
            creds.get_attr("user").unwrap().as_str(),
            Some("override_user")
        );
        assert_eq!(
            creds.get_attr("cluster").unwrap().as_str(),
            Some("test_shard")
        );
        // Non-overridden profile values untouched (port stringified from the profile int).
        assert_eq!(
            creds.get_attr("password").unwrap().as_str(),
            Some("profile_pass")
        );
        assert_eq!(
            creds.get_attr("host").unwrap().as_str(),
            Some("ch.example.com")
        );
        assert_eq!(creds.get_attr("port").unwrap().as_str(), Some("8123"));
    }

    #[test]
    fn test_get_credentials_drops_falsy_overridden_keys() {
        // impl.py parity: keys explicitly overridden to a falsy value are removed,
        // while non-overridden keys keep their (possibly empty) profile values.
        let adapter = clickhouse_adapter(Mapping::from_iter([
            ("user".into(), "profile_user".into()),
            ("password".into(), "profile_pass".into()),
        ]));
        let overrides = Value::from(BTreeMap::from([("password".to_string(), Value::from(""))]));
        let creds = adapter.get_credentials(&overrides);

        assert!(creds.get_attr("password").unwrap().is_undefined());
        assert_eq!(
            creds.get_attr("user").unwrap().as_str(),
            Some("profile_user")
        );
        // database was never configured nor overridden: present as empty string.
        assert_eq!(creds.get_attr("database").unwrap().as_str(), Some(""));
    }

    #[test]
    fn test_quote_seed_column_for_snowflake() -> AdapterResult<()> {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let quoted = adapter
            .quote_seed_column(&state, "my_column", None)
            .unwrap();
        assert_eq!(quoted, "my_column");
        let quoted = adapter
            .quote_seed_column(&state, "my_column", Some(false))
            .unwrap();
        assert_eq!(quoted, "my_column");
        let quoted = adapter
            .quote_seed_column(&state, "my_column", Some(true))
            .unwrap();
        assert_eq!(quoted, "\"my_column\"");
        Ok(())
    }

    #[test]
    fn test_quote_as_configured_for_snowflake() -> AdapterResult<()> {
        let adapter = AdapterImpl::new(engine(Snowflake), None);

        let env = Environment::new();
        let state = State::new_for_env(&env);
        let quoted = adapter
            .quote_as_configured(&state, "my_schema", &ComponentName::Schema)
            .unwrap();
        assert_eq!(quoted, "my_schema");

        let quoted = adapter
            .quote_as_configured(&state, "my_database", &ComponentName::Database)
            .unwrap();
        assert_eq!(quoted, "my_database");

        let quoted = adapter
            .quote_as_configured(&state, "my_table", &ComponentName::Identifier)
            .unwrap();
        assert_eq!(quoted, "my_table");
        Ok(())
    }

    #[test]
    fn test_redshift_quote() {
        let adapter = AdapterImpl::new(engine(Redshift), None);
        assert_eq!(adapter.quote("abc"), "\"abc\"");
    }

    #[test]
    fn test_table_format_primary_motherduck_ducklake() {
        let adapter = AdapterImpl::new(engine(DuckDB), None);

        assert_eq!(adapter.table_format_for_database("my_db"), "ducklake");
        assert_eq!(adapter.table_format_for_database("other"), "default");
    }

    /// Lake compute attaches its catalogs server-side, so no `attach:` entry or
    /// catalogs.yml entry names them -- but every relation it can see is still an
    /// Iceberg table, and the DuckDB macros branch on this to avoid
    /// `information_schema.columns` (which reports Iceberg REST columns as `__`).
    #[test]
    fn test_table_format_lake_compute_is_iceberg_without_any_attach_entry() {
        let adapter = AdapterImpl::new(build_engine(LakeCompute, Mapping::new()), None);

        assert_eq!(
            adapter.table_format_for_database("wrapped_refutation"),
            "iceberg"
        );
        assert_eq!(
            adapter.table_format_for_database("anything_else"),
            "iceberg"
        );
    }

    #[test]
    fn test_table_format_unaliased_motherduck_attachment() {
        let adapter = AdapterImpl::new(engine(DuckDB), None);

        assert_eq!(adapter.table_format_for_database("some_db"), "ducklake");
        assert_eq!(adapter.table_format_for_database("main"), "default");
    }

    #[test]
    fn use_database_skips_redshift_without_datasharing_or_database_change() {
        use crate::adapter::Adapter;
        let wrap = |inner: AdapterImpl| {
            Adapter::new(Arc::new(inner), None, CancellationToken::never_cancels())
        };

        // Redshift without `datasharing` never switches.
        let no_ds = wrap(AdapterImpl::new(
            build_engine(
                Redshift,
                Mapping::from_iter([("database".into(), "dev".into())]),
            ),
            None,
        ));
        assert_eq!(no_ds.use_database("analytics", "n").unwrap(), None);

        // Redshift + datasharing: target equal to the default (case-insensitive) or empty
        // → no switch.
        let ds = wrap(AdapterImpl::new(
            build_engine(
                Redshift,
                Mapping::from_iter([
                    ("database".into(), "dev".into()),
                    ("datasharing".into(), true.into()),
                ]),
            ),
            None,
        ));
        assert_eq!(ds.use_database("dev", "n").unwrap(), None);
        assert_eq!(ds.use_database("DEV", "n").unwrap(), None);
        assert_eq!(ds.use_database("", "n").unwrap(), None);
    }

    #[test]
    fn test_table_format_profile_level_iceberg_attachment() {
        let attach = YmlValue::Sequence(
            vec![YmlValue::Mapping(
                Mapping::from_iter([
                    ("path".into(), "demo".into()),
                    ("alias".into(), "iceberg_demo".into()),
                    ("type".into(), "iceberg".into()),
                ]),
                Default::default(),
            )],
            Default::default(),
        );
        let config = Mapping::from_iter([
            ("path".into(), "demo.duckdb".into()),
            ("attach".into(), attach),
        ]);
        let adapter = AdapterImpl::new(build_engine(DuckDB, config), None);

        assert_eq!(adapter.table_format_for_database("iceberg_demo"), "iceberg");
        assert_eq!(adapter.table_format_for_database("demo"), "default");
    }

    // Checks that get_persist_doc_columns generates an explicit empty comment update only when the existing
    // warehouse comment is non-empty.
    #[test]
    fn test_get_persist_doc_columns_clear_comment_only_when_needed() {
        let adapter = AdapterImpl::new_mock(
            Databricks,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(Databricks)),
            Arc::new(DefaultStmtSplitter),
        );

        let env = Environment::new();
        let state = State::new_for_env(&env);

        // Model column has *no* description, which round-trips through Jinja as `""` (empty string).
        let model_col = Arc::new(DbtColumn {
            name: "sales_channel_name".to_string(),
            description: None,
            ..Default::default()
        });
        let mut model_columns_map: IndexMap<String, DbtColumnRef> = IndexMap::new();
        model_columns_map.insert("sales_channel_name".to_string(), model_col);
        let model_columns = Value::from_serialize(model_columns_map);

        let existing_non_empty = Value::from(vec![Value::from_object(
            Column::new(
                Databricks,
                "sales_channel_name".to_string(),
                "string".to_string(),
                None,
                None,
                None,
            )
            .with_comment(Some("Name of the sales channel".to_string())),
        )]);
        let selected = adapter
            .get_persist_doc_columns(&state, &existing_non_empty, &model_columns)
            .expect("get_persist_doc_columns should succeed");
        let v = selected
            .get_item(&Value::from("sales_channel_name"))
            .expect("get_item should succeed");
        assert!(
            !v.is_undefined(),
            "Expected column to be selected to clear existing non-empty comment, got: {selected:?}"
        );

        let existing_empty = Value::from(vec![Value::from_object(
            Column::new(
                Databricks,
                "sales_channel_name".to_string(),
                "string".to_string(),
                None,
                None,
                None,
            )
            .with_comment(Some("".to_string())),
        )]);
        let selected = adapter
            .get_persist_doc_columns(&state, &existing_empty, &model_columns)
            .expect("get_persist_doc_columns should succeed");
        let v = selected
            .get_item(&Value::from("sales_channel_name"))
            .expect("get_item should succeed");
        assert!(
            v.is_undefined(),
            "Expected column NOT to be selected when existing comment is already empty, got: {selected:?}"
        );
    }

    #[test]
    fn test_get_persist_doc_columns_skips_missing_column() {
        let adapter = AdapterImpl::new_mock(
            Databricks,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(Databricks)),
            Arc::new(DefaultStmtSplitter),
        );

        let mut model_columns: IndexMap<String, DbtColumnRef> = IndexMap::new();
        model_columns.insert(
            "col1".to_string(),
            Arc::new(DbtColumn {
                name: "col1".to_string(),
                description: Some("new comment".to_string()),
                ..Default::default()
            }),
        );
        model_columns.insert(
            "col2".to_string(),
            Arc::new(DbtColumn {
                name: "col2".to_string(),
                description: Some("comment for missing column".to_string()),
                ..Default::default()
            }),
        );

        let existing_columns = vec![
            Column::new(
                Databricks,
                "col1".to_string(),
                "string".to_string(),
                None,
                None,
                None,
            )
            .with_comment(Some("old comment".to_string())),
        ];

        let result = adapter
            .do_get_persist_doc_columns(existing_columns, model_columns)
            .expect("do_get_persist_doc_columns should succeed");

        assert!(
            result.contains_key("col1"),
            "expected col1 to be persisted, got: {result:?}"
        );
        assert!(
            !result.contains_key("col2"),
            "expected missing col2 to be skipped, got: {result:?}"
        );
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_parse_columns_and_constraints_matches_not_null_case_insensitively() {
        let adapter = AdapterImpl::new_mock(
            Databricks,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(Databricks)),
            Arc::new(DefaultStmtSplitter),
        );
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let existing_columns = Value::from(vec![Value::from_object(Column::new(
            Databricks,
            "id".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        ))]);
        let model_columns = Value::from_serialize(BTreeMap::from([(
            "ID".to_string(),
            DbtColumn {
                name: "ID".to_string(),
                data_type: Some("int".to_string()),
                constraints: vec![Constraint {
                    type_: ConstraintType::NotNull,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )]));
        let model_constraints = Value::from_serialize(Vec::<ModelConstraint>::new());

        let result = adapter
            .parse_columns_and_constraints(
                &state,
                &existing_columns,
                &model_columns,
                &model_constraints,
                true,
                "case_mismatch_model",
            )
            .expect("constraint parsing should succeed");
        let mut result_parts = result.try_iter().expect("result should be iterable");
        let enriched_columns = result_parts.next().expect("enriched columns should exist");
        let enriched_column = enriched_columns
            .try_iter()
            .expect("enriched columns should be iterable")
            .next()
            .expect("one enriched column should exist");
        let rendered = enriched_column
            .downcast_object_ref::<Column>()
            .expect("enriched value should be a Column")
            .render_for_create();

        assert!(
            rendered.contains("NOT NULL"),
            "expected NOT NULL when YAML column ID matches warehouse id, got: {rendered}"
        );
    }

    #[test]
    fn test_parse_columns_and_constraints_defaults_to_unenforced() {
        use crate::adapter::Adapter;

        let adapter = Adapter::new(
            Arc::new(AdapterImpl::new_mock(
                Databricks,
                BTreeMap::new(),
                DEFAULT_RESOLVED_QUOTING,
                Arc::new(DefaultTypeOps::new(Databricks)),
                Arc::new(DefaultStmtSplitter),
            )),
            None,
            dbt_common::cancellation::never_cancels(),
        );
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let existing_columns = Value::from(vec![Value::from_object(Column::new(
            Databricks,
            "id".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        ))]);
        let model_columns = Value::from_serialize(BTreeMap::from([(
            "id".to_string(),
            DbtColumn {
                name: "id".to_string(),
                data_type: Some("int".to_string()),
                constraints: vec![Constraint {
                    type_: ConstraintType::NotNull,
                    ..Default::default()
                }],
                ..Default::default()
            },
        )]));
        let model_constraints = Value::from_serialize(vec![ModelConstraint {
            type_: ConstraintType::PrimaryKey,
            name: Some("pk_contract_model".to_string()),
            columns: Some(vec!["id".to_string()]),
            ..Default::default()
        }]);

        let result = adapter
            .parse_columns_and_constraints(
                &state,
                &[existing_columns, model_columns, model_constraints],
            )
            .expect("constraint parsing should succeed");
        let mut result_parts = result.try_iter().expect("result should be iterable");
        let enriched_columns = result_parts.next().expect("enriched columns should exist");
        let parsed_constraints = result_parts.next().expect("constraints should exist");
        let enriched_column = enriched_columns
            .try_iter()
            .expect("enriched columns should be iterable")
            .next()
            .expect("one enriched column should exist");
        let rendered = enriched_column
            .downcast_object_ref::<Column>()
            .expect("enriched value should be a Column")
            .render_for_create();

        assert!(!rendered.contains("NOT NULL"));
        assert_eq!(
            parsed_constraints
                .try_iter()
                .expect("parsed constraints should be iterable")
                .count(),
            0
        );
    }

    /// Test that verifies the logic for determining when to use legacy DESCRIBE TABLE
    /// vs. DESCRIBE EXTENDED ... AS JSON for Databricks relations.
    ///
    /// This test documents the expected behavior, matching Python dbt-databricks semantics:
    /// - The `temporary` field only tracks Unity Catalog temporary tables, not HMS temporary views
    /// - is_hive_metastore() returns false for UC temporary tables (even if database is "hive_metastore")
    /// - Hive Metastore tables (both regular and temporary views): use legacy
    /// - Unity Catalog regular tables: use JSON (NOT legacy)
    /// - Unity Catalog temporary tables: use JSON (NOT legacy)
    /// - Materialized views: use legacy
    /// - Streaming tables: use legacy
    #[test]
    fn test_databricks_get_columns_use_legacy_logic() {
        use crate::relation::Relation;
        use crate::relation::databricks::DEFAULT_DATABRICKS_DATABASE;
        use dbt_schemas::schemas::relations::DEFAULT_RESOLVED_QUOTING;

        // Test 1: Non-temporary Hive Metastore table -> should use legacy
        let hive_table = Relation::new(
            Databricks,
            DEFAULT_DATABRICKS_DATABASE.to_string(),
            "schema1".to_string(),
            "table1".to_string(),
        )
        .with_relation_type(RelationType::Table)
        .with_quoting(DEFAULT_RESOLVED_QUOTING);
        assert!(
            hive_table.is_hive_metastore(),
            "Expected is_hive_metastore() to return true for non-temporary table in hive_metastore"
        );
        assert!(!hive_table.is_temporary(), "Expected non-temporary table");
        // use_legacy = is_hive_metastore || is_materialized_view || is_streaming_table
        // use_legacy = true || false || false = true
        let use_legacy_hive_table = hive_table.is_hive_metastore()
            || hive_table.is_materialized_view()
            || hive_table.is_streaming_table();
        assert!(
            use_legacy_hive_table,
            "Expected non-temporary Hive Metastore table to use legacy DESCRIBE"
        );

        // Test 2: Unity Catalog temporary table (with hive_metastore database name) -> should NOT use legacy
        // Key test: is_hive_metastore() should return FALSE for UC temporary tables
        // Note: In practice, UC temp tables wouldn't have database="hive_metastore", but this tests the logic
        let uc_temp_table = Relation::new(
            Databricks,
            DEFAULT_DATABRICKS_DATABASE.to_string(),
            "schema1".to_string(),
            "temp_table".to_string(),
        )
        .with_relation_type(RelationType::Table)
        .with_quoting(DEFAULT_RESOLVED_QUOTING)
        .with_temporary(true);
        assert!(
            !uc_temp_table.is_hive_metastore(),
            "Expected is_hive_metastore() to return FALSE for UC temporary table (matching Python semantics)"
        );
        assert!(uc_temp_table.is_temporary(), "Expected temporary table");
        // use_legacy = is_hive_metastore || is_materialized_view || is_streaming_table
        // use_legacy = false || false || false = false
        let use_legacy_uc_temp = uc_temp_table.is_hive_metastore()
            || uc_temp_table.is_materialized_view()
            || uc_temp_table.is_streaming_table();
        assert!(
            !use_legacy_uc_temp,
            "Expected UC temporary table to use JSON DESCRIBE (not legacy)"
        );

        // Test 3: Unity Catalog table (non-temporary) -> should NOT use legacy
        let unity_table = Relation::new(
            Databricks,
            "unity_catalog".to_string(),
            "schema1".to_string(),
            "table1".to_string(),
        )
        .with_relation_type(RelationType::Table)
        .with_quoting(DEFAULT_RESOLVED_QUOTING);
        assert!(
            !unity_table.is_hive_metastore(),
            "Expected Unity Catalog table (not Hive Metastore)"
        );
        // use_legacy = is_hive_metastore || is_materialized_view || is_streaming_table
        // use_legacy = false || false || false = false
        let use_legacy_unity = unity_table.is_hive_metastore()
            || unity_table.is_materialized_view()
            || unity_table.is_streaming_table();
        assert!(
            !use_legacy_unity,
            "Expected Unity Catalog table to use JSON DESCRIBE (not legacy)"
        );

        // Test 4: Materialized view -> should use legacy
        let mv = Relation::new(
            Databricks,
            "unity_catalog".to_string(),
            "schema1".to_string(),
            "mv1".to_string(),
        )
        .with_relation_type(RelationType::MaterializedView)
        .with_quoting(DEFAULT_RESOLVED_QUOTING);
        assert!(mv.is_materialized_view(), "Expected materialized view");
        // use_legacy = is_hive_metastore || is_materialized_view || is_streaming_table
        // use_legacy = false || true || false = true
        let use_legacy_mv =
            mv.is_hive_metastore() || mv.is_materialized_view() || mv.is_streaming_table();
        assert!(
            use_legacy_mv,
            "Expected materialized view to use legacy DESCRIBE"
        );
    }

    #[test]
    fn test_try_to_int_col() {
        use arrow_array::Float64Array;

        // whole numbers → true
        assert!(try_to_int_col(&Float64Array::from(vec![1.0, 2.0, 100.0])));
        // fractional values → false
        assert!(!try_to_int_col(&Float64Array::from(vec![1.0, 2.5, 3.0])));
        // nulls are ignored → true
        assert!(try_to_int_col(&Float64Array::from(vec![
            Some(1.0),
            None,
            Some(3.0)
        ])));
        // NaN/Inf are ignored → true
        assert!(try_to_int_col(&Float64Array::from(vec![
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            1.0
        ])));
        assert!(!try_to_int_col(&Float64Array::from(Vec::<f64>::new())));
        assert!(!try_to_int_col(&Float64Array::from(vec![
            None::<f64>,
            None,
            None
        ])));
    }

    #[test]
    fn test_convert_type_clickhouse_never_nullable() {
        use arrow_array::Int64Array;

        // Seed columns must never render as Nullable(...), regardless of the field
        // flag (seed schemas always declare nullable) or the actual data: this
        // matches the Python adapter's agate-based typing. Missing values are
        // inserted as literal NULLs and become column defaults server-side via
        // input_format_null_as_default.
        let schema = Arc::new(Schema::new(vec![
            Field::new("with_nulls", DataType::Int64, true),
            Field::new("no_nulls", DataType::Int64, true),
        ]));
        let with_nulls = Arc::new(Int64Array::from(vec![Some(1), None, Some(3)])) as ArrayRef;
        let no_nulls = Arc::new(Int64Array::from(vec![Some(1), Some(2), Some(3)])) as ArrayRef;
        let batch = RecordBatch::try_new(schema, vec![with_nulls, no_nulls]).unwrap();
        let table = Arc::new(AgateTable::from_record_batch(Arc::new(batch)));

        let adapter = clickhouse_adapter(Mapping::new());
        let env = Environment::new();
        let state = State::new_for_env(&env);

        assert_eq!(
            adapter.convert_type(&state, Arc::clone(&table), 0).unwrap(),
            "Int64"
        );
        assert_eq!(adapter.convert_type(&state, table, 1).unwrap(), "Int64");
    }

    #[test]
    fn test_verify_database_redshift_cross_db_blocked_without_flags() {
        let config = Mapping::from_iter([("database".into(), "mydb".into())]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        let result = adapter.verify_database("otherdb".to_string());
        assert!(
            result.is_err(),
            "cross-db ref should be blocked without ra3_node or datasharing"
        );
    }

    #[test]
    fn test_verify_database_redshift_same_db_always_allowed() {
        let config = Mapping::from_iter([("database".into(), "mydb".into())]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        assert!(adapter.verify_database("mydb".to_string()).is_ok());
    }

    #[test]
    fn test_verify_database_redshift_cross_db_allowed_with_ra3_node() {
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("ra3_node".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        assert!(adapter.verify_database("otherdb".to_string()).is_ok());
    }

    #[test]
    fn test_verify_database_redshift_cross_db_allowed_with_datasharing() {
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        assert!(adapter.verify_database("otherdb".to_string()).is_ok());
    }

    #[test]
    fn test_verify_database_redshift_accepts_mixed_case_string_flags() {
        // dbt-core accepts booleans as strings in any casing (e.g. "True" from YAML).
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), "True".into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        assert!(adapter.verify_database("otherdb".to_string()).is_ok());
    }

    #[test]
    fn test_has_feature_datasharing_false_by_default() {
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let adapter = AdapterImpl::new(engine(Redshift), None);
        let result = adapter
            .has_feature(&state, "datasharing", CancellationToken::never_cancels())
            .unwrap();
        assert_eq!(result, Some(false));
    }

    #[test]
    fn test_has_feature_datasharing_true_when_set() {
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        let result = adapter
            .has_feature(&state, "datasharing", CancellationToken::never_cancels())
            .unwrap();
        assert_eq!(result, Some(true));
    }

    #[test]
    fn test_has_feature_drop_without_cascade_false_by_default() {
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let adapter = AdapterImpl::new(engine(Redshift), None);
        let result = adapter
            .has_feature(
                &state,
                "drop_without_cascade",
                CancellationToken::never_cancels(),
            )
            .unwrap();
        assert_eq!(result, Some(false));
    }

    #[test]
    fn test_has_feature_drop_without_cascade_true_when_set() {
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("drop_without_cascade".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        let result = adapter
            .has_feature(
                &state,
                "drop_without_cascade",
                CancellationToken::never_cancels(),
            )
            .unwrap();
        assert_eq!(result, Some(true));
    }

    fn databricks_config(pairs: &[(&str, &str)]) -> AdapterConfig {
        AdapterConfig::new(Mapping::from_iter(
            pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())),
        ))
    }

    #[test]
    fn test_is_cluster_http_path() {
        let is_cluster = |http_path: &str| {
            AdapterImpl::is_cluster_http_path(&databricks_config(&[("http_path", http_path)]))
        };

        assert_eq!(is_cluster("/sql/1.0/warehouses/abc"), Some(false));
        assert_eq!(is_cluster("sql/protocolv1/o/1/0101-abc"), Some(true));
        assert_eq!(is_cluster("/custom"), Some(false));
        assert_eq!(is_cluster("  /SQL/1.0/WAREHOUSES/abc  "), Some(false));

        assert_eq!(
            AdapterImpl::is_cluster_http_path(&databricks_config(&[])),
            None
        );
    }

    #[test]
    fn test_parse_has_dbr_capability() {
        let warehouse = databricks_config(&[("http_path", "/sql/1.0/warehouses/abc")]);
        let cluster = databricks_config(&[("http_path", "sql/protocolv1/o/1/0101-abc")]);

        assert!(AdapterImpl::parse_has_dbr_capability(
            &warehouse,
            "timestampdiff"
        ));
        assert!(!AdapterImpl::parse_has_dbr_capability(
            &cluster,
            "timestampdiff"
        ));

        // `streaming_table_json_metadata` is the one capability warehouses do
        // not support, so this fails if the capability is never consulted.
        assert!(!AdapterImpl::parse_has_dbr_capability(
            &warehouse,
            "streaming_table_json_metadata"
        ));

        assert!(!AdapterImpl::parse_has_dbr_capability(
            &warehouse,
            "not_a_capability"
        ));
        assert!(!AdapterImpl::parse_has_dbr_capability(
            &databricks_config(&[]),
            "timestampdiff"
        ));
    }

    fn record_batch_with_string_column(name: &str, values: Vec<&str>) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(name, DataType::Utf8, false)]));
        let array = Arc::new(StringArray::from(values)) as ArrayRef;
        Arc::new(RecordBatch::try_new(schema, vec![array]).unwrap())
    }

    #[test]
    fn test_redshift_list_schemas_uses_nspname_by_default() {
        let adapter = AdapterImpl::new(engine(Redshift), None);
        let batch = record_batch_with_string_column("nspname", vec!["public", "analytics"]);
        let schemas = adapter.list_schemas_inner(batch).unwrap();
        assert_eq!(schemas, vec!["public".to_string(), "analytics".to_string()]);
    }

    #[test]
    fn test_redshift_list_schemas_uses_schema_name_with_datasharing() {
        // SHOW SCHEMAS FROM DATABASE returns a `schema_name` column instead of `nspname`.
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        let batch = record_batch_with_string_column("schema_name", vec!["public", "shared_a"]);
        let schemas = adapter.list_schemas_inner(batch).unwrap();
        assert_eq!(schemas, vec!["public".to_string(), "shared_a".to_string()]);
    }

    #[test]
    fn test_redshift_list_schemas_datasharing_rejects_nspname_column() {
        // Sanity: with datasharing on, `nspname` is no longer the expected column,
        // so a batch shaped for the postgres path should error rather than silently match.
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), true.into()),
        ]);
        let adapter = AdapterImpl::new(build_engine(Redshift, config), None);
        let batch = record_batch_with_string_column("nspname", vec!["public"]);
        assert!(adapter.list_schemas_inner(batch).is_err());
    }

    // -- Redshift standardize_grants_dict tests -------------------------------

    /// Mirrors the shared columns returned by Redshift's grants APIs:
    /// https://docs.aws.amazon.com/redshift/latest/dg/r_SHOW_GRANTS.html
    /// https://docs.aws.amazon.com/redshift/latest/dg/r_SVV_RELATION_PRIVILEGES.html
    fn identity_grants_table(
        identity_names: Vec<&str>,
        identity_types: Vec<&str>,
        privilege_types: Vec<&str>,
    ) -> Arc<AgateTable> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("identity_name", DataType::Utf8, false),
            Field::new("identity_type", DataType::Utf8, false),
            Field::new("privilege_type", DataType::Utf8, false),
        ]));
        let arrs: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(identity_names)),
            Arc::new(StringArray::from(identity_types)),
            Arc::new(StringArray::from(privilege_types)),
        ];
        Arc::new(AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(schema, arrs).unwrap(),
        )))
    }

    /// Mirrors the `(grantee, privilege_type)` shape projected by the
    /// pre-datasharing Redshift macro (and dbt's other Postgres-derived
    /// adapters), modeled on `information_schema.role_table_grants`:
    /// https://www.postgresql.org/docs/current/infoschema-role-table-grants.html
    fn legacy_grants_table(grantees: Vec<&str>, privileges: Vec<&str>) -> Arc<AgateTable> {
        let schema = Arc::new(Schema::new(vec![
            Field::new("grantee", DataType::Utf8, false),
            Field::new("privilege_type", DataType::Utf8, false),
        ]));
        let arrs: Vec<ArrayRef> = vec![
            Arc::new(StringArray::from(grantees)),
            Arc::new(StringArray::from(privileges)),
        ];
        Arc::new(AgateTable::from_record_batch(Arc::new(
            RecordBatch::try_new(schema, arrs).unwrap(),
        )))
    }

    fn redshift_adapter_with_datasharing() -> AdapterImpl {
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            ("datasharing".into(), true.into()),
            ("user".into(), "dbt_runner".into()),
        ]);
        AdapterImpl::new(build_engine(Redshift, config), None)
    }

    enum GrantsSource {
        Svv,
        Show,
    }

    fn redshift_adapter_with_extended_grants(source: GrantsSource) -> AdapterImpl {
        let config = Mapping::from_iter([
            ("database".into(), "mydb".into()),
            (
                "datasharing".into(),
                matches!(source, GrantsSource::Show).into(),
            ),
            ("user".into(), "dbt_runner".into()),
        ]);
        let behavior_flag_overrides =
            BTreeMap::from([("redshift_grants_extended".to_string(), true)]);
        AdapterImpl::new(
            build_engine_with_behavior(Redshift, config, behavior_flag_overrides),
            None,
        )
    }

    #[test]
    fn test_redshift_standardize_grants_dict_legacy() {
        let adapter = AdapterImpl::new(engine(Redshift), None);
        let table = legacy_grants_table(
            vec!["alice", "bob", "alice"],
            vec!["select", "select", "insert"],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(
            result["select"],
            vec!["alice".to_string(), "bob".to_string()]
        );
        assert_eq!(result["insert"], vec!["alice".to_string()]);
    }

    #[test]
    fn test_redshift_standardize_grants_dict_datasharing() {
        let adapter = redshift_adapter_with_datasharing();
        let table = identity_grants_table(
            vec!["alice", "bob", "alice"],
            vec!["user", "user", "user"],
            vec!["SELECT", "SELECT", "INSERT"],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(
            result["select"],
            vec!["alice".to_string(), "bob".to_string()]
        );
        assert_eq!(result["insert"], vec!["alice".to_string()]);
    }

    #[test]
    fn test_redshift_standardize_grants_dict_datasharing_filters_non_user_identities() {
        let adapter = redshift_adapter_with_datasharing();
        let table = identity_grants_table(
            vec!["alice", "analyst_role", "everyone", "ds:foo"],
            vec!["user", "role", "public", "role"],
            vec!["SELECT", "SELECT", "SELECT", "SELECT"],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(result["select"], vec!["alice".to_string()]);
    }

    #[test]
    fn test_redshift_standardize_grants_dict_datasharing_filters_current_user() {
        let adapter = redshift_adapter_with_datasharing();
        let table = identity_grants_table(
            vec!["dbt_runner", "DBT_RUNNER", "alice"],
            vec!["user", "user", "user"],
            vec!["SELECT", "INSERT", "SELECT"],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(result["select"], vec!["alice".to_string()]);
        assert!(!result.contains_key("insert"));
    }

    #[test]
    fn test_redshift_standardize_grants_dict_extended_svv() {
        let adapter = redshift_adapter_with_extended_grants(GrantsSource::Svv);
        let table = identity_grants_table(
            vec![
                "alice",
                "readonly_group",
                "readonly_role",
                "PUBLIC",
                "ds:named_datashare",
                "sys:dba",
            ],
            vec!["user", "group", "role", "public", "role", "role"],
            vec!["SELECT", "SELECT", "SELECT", "SELECT", "SELECT", "SELECT"],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(
            result["select"],
            vec![
                "user:alice".to_string(),
                "group:readonly_group".to_string(),
                "role:readonly_role".to_string(),
            ]
        );
    }

    #[test]
    fn test_redshift_standardize_grants_dict_extended_datasharing() {
        let adapter = redshift_adapter_with_extended_grants(GrantsSource::Show);
        let table = identity_grants_table(
            vec![
                "alice",
                "/readonly_group",
                "readonly_role",
                "PUBLIC",
                "ds:named_datashare",
                "sys:dba",
                "DBT_RUNNER",
            ],
            vec!["user", "role", "role", "public", "role", "role", "user"],
            vec![
                "SELECT", "SELECT", "SELECT", "SELECT", "SELECT", "SELECT", "SELECT",
            ],
        );
        let result = adapter.standardize_grants_dict(table).unwrap();
        assert_eq!(
            result["select"],
            vec![
                "user:alice".to_string(),
                "group:readonly_group".to_string(),
                "role:readonly_role".to_string(),
            ]
        );
    }

    #[test]
    fn test_redshift_standardize_grants_dict_datasharing_rejects_legacy_columns() {
        let adapter = redshift_adapter_with_datasharing();
        let table = legacy_grants_table(vec!["alice"], vec!["select"]);
        assert!(adapter.standardize_grants_dict(table).is_err());
    }

    // -- standardize_grants_dict tests ----------------------------------------

    #[test]
    fn test_standardize_grants_dict_redshift_empty_integer_typed_columns() {
        // Regression: when show_grants returns 0 rows on Redshift, agate records
        // column_types as ["Integer","Integer"] rather than ["Text","Text"] because
        // it has no values to infer types from. Fusion replays that execute result
        // and passes the resulting Int64-typed RecordBatch to standardize_grants_dict,
        // which must return an empty map rather than erroring on the type mismatch.
        use arrow_array::Int64Array;

        let schema = Arc::new(Schema::new(vec![
            Field::new("grantee", DataType::Int64, true),
            Field::new("privilege_type", DataType::Int64, true),
        ]));
        let grantee = Arc::new(Int64Array::from(vec![] as Vec<i64>)) as ArrayRef;
        let privilege = Arc::new(Int64Array::from(vec![] as Vec<i64>)) as ArrayRef;
        let batch = RecordBatch::try_new(schema, vec![grantee, privilege]).unwrap();
        let table = Arc::new(AgateTable::from_record_batch(Arc::new(batch)));

        let adapter = AdapterImpl::new(engine(Redshift), None);
        let result = adapter.standardize_grants_dict(table);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_standardize_grants_dict_snowflake_empty_integer_typed_columns() {
        // Same root cause as the Redshift case: agate records column_types as
        // ["Integer",...] for 0-row results, so Fusion replays a RecordBatch
        // with Int64-typed columns instead of the expected StringArrays.
        use arrow_array::Int64Array;

        let schema = Arc::new(Schema::new(vec![
            Field::new("grantee_name", DataType::Int64, true),
            Field::new("granted_to", DataType::Int64, true),
            Field::new("privilege", DataType::Int64, true),
        ]));
        let col = || Arc::new(Int64Array::from(vec![] as Vec<i64>)) as ArrayRef;
        let batch = RecordBatch::try_new(schema, vec![col(), col(), col()]).unwrap();
        let table = Arc::new(AgateTable::from_record_batch(Arc::new(batch)));

        let adapter = AdapterImpl::new(engine(Snowflake), None);
        let result = adapter.standardize_grants_dict(table);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_standardize_grants_dict_databricks_empty_integer_typed_columns() {
        // Same root cause as the Redshift case: agate records column_types as
        // ["Integer",...] for 0-row results, so Fusion replays a RecordBatch
        // with Int64-typed columns instead of the expected StringArrays.
        use arrow_array::Int64Array;

        let schema = Arc::new(Schema::new(vec![
            Field::new("Principal", DataType::Int64, true),
            Field::new("ActionType", DataType::Int64, true),
            Field::new("ObjectType", DataType::Int64, true),
        ]));
        let col = || Arc::new(Int64Array::from(vec![] as Vec<i64>)) as ArrayRef;
        let batch = RecordBatch::try_new(schema, vec![col(), col(), col()]).unwrap();
        let table = Arc::new(AgateTable::from_record_batch(Arc::new(batch)));

        let adapter = AdapterImpl::new(build_engine(Databricks, Mapping::new()), None);
        let result = adapter.standardize_grants_dict(table);
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    // -- get_dataset_location parsing tests -----------------------------------

    #[test]
    fn test_parse_dataset_location_empty_batch_returns_none() {
        // Regression: `AdapterGetRelationRecord` is skipped in replay, so Fusion
        // re-executes its BigQuery `get_relation`, which calls `get_dataset_location`.
        // That SCHEMATA query is never recorded by Mantle, so replay returns a
        // zero-column batch. Parsing must return None instead of erroring with
        // `expected column location not found, available are: []`.
        let batch = RecordBatch::new_empty(Arc::new(Schema::empty()));
        let result = AdapterImpl::parse_dataset_location(&batch);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn test_parse_dataset_location_single_row_returns_location() {
        let schema = Arc::new(Schema::new(vec![Field::new(
            "location",
            DataType::Utf8,
            true,
        )]));
        let location = Arc::new(StringArray::from(vec!["US"])) as ArrayRef;
        let batch = RecordBatch::try_new(schema, vec![location]).unwrap();
        let result = AdapterImpl::parse_dataset_location(&batch).unwrap();
        assert_eq!(result, Some("US".to_string()));
    }

    // -- BigQuery job_execution_timeout_seconds tests -------------------------

    fn make_bigquery_model_with_timeout(timeout_seconds: u64) -> Value {
        // Production models have bigquery_attr at the top level because dbt-yaml's
        // flatten_dunder serialization merges __adapter_attr__ fields into the parent map.
        use std::collections::BTreeMap;
        let bq_attr = BTreeMap::from([("job_execution_timeout_seconds", timeout_seconds as i64)]);
        let model = BTreeMap::from([("bigquery_attr", bq_attr)]);
        Value::from_serialize(&model)
    }

    fn make_bigquery_snapshot_with_timeout(timeout_seconds: u64) -> Value {
        // Same structure as model: bigquery_attr at top level.
        make_bigquery_model_with_timeout(timeout_seconds)
    }

    fn find_job_timeout(options: &[(String, OptionValue)]) -> Option<i64> {
        options.iter().find_map(|(k, v)| {
            if k == QUERY_JOB_TIMEOUT {
                if let OptionValue::Int(t) = v {
                    Some(*t)
                } else {
                    None
                }
            } else {
                None
            }
        })
    }

    #[test]
    fn test_bigquery_adbc_options_no_timeout_when_not_configured() {
        let adapter = AdapterImpl::new(engine(Bigquery), None);
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert!(find_job_timeout(&options).is_none());
    }

    #[test]
    fn test_bigquery_adbc_options_model_level_timeout() {
        let adapter = AdapterImpl::new(engine(Bigquery), None);
        let mut env = Environment::new();
        env.add_global("model", make_bigquery_model_with_timeout(300));
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert_eq!(find_job_timeout(&options), Some(300 * 1000));
    }

    #[test]
    fn test_bigquery_adbc_options_snapshot_model_level_timeout() {
        let adapter = AdapterImpl::new(engine(Bigquery), None);
        let mut env = Environment::new();
        env.add_global("model", make_bigquery_snapshot_with_timeout(600));
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert_eq!(find_job_timeout(&options), Some(600 * 1000));
    }

    #[test]
    fn test_bigquery_adbc_options_connection_level_timeout_fallback() {
        let config = Mapping::from_iter([("job_execution_timeout_seconds".into(), 120_i64.into())]);
        let adapter = AdapterImpl::new(build_engine(Bigquery, config), None);
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert_eq!(find_job_timeout(&options), Some(120 * 1000));
    }

    #[test]
    fn test_bigquery_adbc_options_model_level_overrides_connection_level() {
        let config = Mapping::from_iter([("job_execution_timeout_seconds".into(), 120_i64.into())]);
        let adapter = AdapterImpl::new(build_engine(Bigquery, config), None);
        let mut env = Environment::new();
        env.add_global("model", make_bigquery_model_with_timeout(900));
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert_eq!(find_job_timeout(&options), Some(900 * 1000));
    }

    #[test]
    fn test_non_bigquery_adapter_has_no_timeout_option() {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        let env = Environment::new();
        let state = State::new_for_env(&env);
        let options = adapter.get_adbc_execute_options(&state);
        assert!(find_job_timeout(&options).is_none());
    }

    // Regression test for https://github.com/dbt-labs/dbt-fusion/issues/1733:
    // type:custom column constraints were silently dropped because get_constraint_support
    // returns NotSupported for Custom on all adapters, and the NotSupported guard ran
    // before the Custom arm in the match — bypassing the expression entirely.
    #[test]
    fn test_render_column_constraint_custom_snowflake() {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        let constraint = Constraint {
            type_: ConstraintType::Custom,
            expression: Some("with tag (governance.masking.pii_type = 'SSN')".to_string()),
            name: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };
        let rendered = adapter.render_column_constraint(constraint);
        assert_eq!(
            rendered,
            Some("with tag (governance.masking.pii_type = 'SSN')".to_string())
        );
    }

    #[test]
    fn test_render_column_constraint_custom_empty_expression_returns_none() {
        let adapter = AdapterImpl::new(engine(Snowflake), None);
        let constraint = Constraint {
            type_: ConstraintType::Custom,
            expression: None,
            name: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };
        assert!(adapter.render_column_constraint(constraint).is_none());
    }

    fn clickhouse_mock_adapter() -> AdapterImpl {
        AdapterImpl::new_mock(
            ClickHouse,
            BTreeMap::new(),
            DEFAULT_RESOLVED_QUOTING,
            Arc::new(DefaultTypeOps::new(ClickHouse)),
            Arc::new(DefaultStmtSplitter),
        )
    }

    /// impl.py CONSTRAINT_SUPPORT: only CHECK is enforced.
    #[test]
    fn test_constraint_support_clickhouse() {
        let adapter = clickhouse_mock_adapter();
        assert_eq!(
            adapter.get_constraint_support(ConstraintType::Check),
            ConstraintSupport::Enforced
        );
        for ct in [
            ConstraintType::NotNull,
            ConstraintType::Unique,
            ConstraintType::PrimaryKey,
            ConstraintType::ForeignKey,
            ConstraintType::Custom,
        ] {
            assert_eq!(
                adapter.get_constraint_support(ct),
                ConstraintSupport::NotSupported
            );
        }
    }

    /// Column constraints never render (warn only); codec/ttl ride after the
    /// type, codec first, like impl.py.
    #[test]
    fn test_render_raw_columns_constraints_clickhouse() {
        let adapter = clickhouse_mock_adapter();
        let mut columns: IndexMap<String, DbtColumn> = IndexMap::new();
        columns.insert(
            "id".to_string(),
            DbtColumn {
                name: "id".to_string(),
                data_type: Some("Int32".to_string()),
                constraints: vec![Constraint {
                    type_: ConstraintType::NotNull,
                    expression: None,
                    name: None,
                    to: None,
                    to_columns: None,
                    warn_unsupported: None,
                    warn_unenforced: None,
                }],
                ..Default::default()
            },
        );
        columns.insert(
            "payload".to_string(),
            DbtColumn {
                name: "payload".to_string(),
                data_type: Some("String".to_string()),
                codec: Some("ZSTD".to_string()),
                ttl: Some("created_at + INTERVAL 1 DAY".to_string()),
                ..Default::default()
            },
        );
        let rendered = adapter.render_raw_columns_constraints(columns).unwrap();
        assert_eq!(
            rendered,
            vec![
                "`id` Int32",
                "`payload` String CODEC(ZSTD) TTL created_at + INTERVAL 1 DAY",
            ]
        );
    }

    /// BigQuery constraints on dotted columns must render inside their STRUCT types.
    #[test]
    fn test_render_raw_columns_constraints_bigquery_nested() {
        let adapter = AdapterImpl::new(engine(Bigquery), None);
        let not_null = Constraint {
            type_: ConstraintType::NotNull,
            expression: None,
            name: None,
            to: None,
            to_columns: None,
            warn_unsupported: None,
            warn_unenforced: None,
        };
        let column = |name: &str, data_type: &str, constraints| {
            (
                name.to_string(),
                DbtColumn {
                    name: name.to_string(),
                    data_type: Some(data_type.to_string()),
                    constraints,
                    ..Default::default()
                },
            )
        };
        let columns = IndexMap::from([
            column("id", "int64", vec![]),
            column("my_array", "array", vec![]),
            column("my_array.sub_id", "string", vec![not_null.clone()]),
            column("my_array.my_struct", "struct", vec![]),
            column("my_array.my_struct.first_field", "string", vec![not_null]),
            column("my_array.my_struct.second_field", "string", vec![]),
        ]);

        assert_eq!(
            adapter.render_raw_columns_constraints(columns).unwrap(),
            vec![
                "id int64",
                "my_array array<struct<sub_id string not null, my_struct struct<first_field string not null, second_field string>>>",
            ]
        );
    }

    /// Build a single-column `show databases` style result with the given column
    /// name and schema values.
    fn schemas_batch(col_name: &str, values: &[&str]) -> Arc<RecordBatch> {
        let schema = Arc::new(Schema::new(vec![Field::new(
            col_name,
            DataType::Utf8,
            false,
        )]));
        let col: ArrayRef = Arc::new(StringArray::from(values.to_vec()));
        Arc::new(RecordBatch::try_new(schema, vec![col]).unwrap())
    }

    // OSS Spark's `show databases` returns the schema in a column named
    // `namespace`. list_schemas_inner must resolve it (regression for the
    // hardcoded `databaseName` lookup that errored on OSS Spark).
    #[test]
    fn test_list_schemas_inner_spark_namespace() -> AdapterResult<()> {
        let adapter = AdapterImpl::new(engine(Spark), None);
        let batch = schemas_batch("namespace", &["default", "dt_demo"]);
        assert_eq!(
            adapter.list_schemas_inner(batch)?,
            vec!["default", "dt_demo"]
        );
        Ok(())
    }

    // Some Spark/Hive-style builds emit `databaseName` instead; the Spark arm is
    // version-agnostic and must resolve that too.
    #[test]
    fn test_list_schemas_inner_spark_database_name() -> AdapterResult<()> {
        let adapter = AdapterImpl::new(engine(Spark), None);
        let batch = schemas_batch("databaseName", &["default", "dt_demo"]);
        assert_eq!(
            adapter.list_schemas_inner(batch)?,
            vec!["default", "dt_demo"]
        );
        Ok(())
    }

    // Databricks is unchanged: it always reads `databaseName`.
    #[test]
    fn test_list_schemas_inner_databricks_database_name() -> AdapterResult<()> {
        let adapter = AdapterImpl::new(engine(Databricks), None);
        let batch = schemas_batch("databaseName", &["main", "analytics"]);
        assert_eq!(
            adapter.list_schemas_inner(batch)?,
            vec!["main", "analytics"]
        );
        Ok(())
    }

    // Spark must NOT dispatch to spark__check_schema_exists (which queries the
    // nonexistent information_schema.schemata); it is handled natively in
    // check_schema_exists. Databricks keeps using that macro (valid on Unity Catalog).
    #[test]
    fn test_check_schema_exists_macro_dispatch() -> AdapterResult<()> {
        let env = Environment::new();
        let state = State::new_for_env(&env);

        let databricks = AdapterImpl::new(engine(Databricks), None);
        assert_eq!(
            databricks.check_schema_exists_macro(&state, &[])?,
            (
                "dbt_spark".to_string(),
                "spark__check_schema_exists".to_string()
            )
        );

        let spark = AdapterImpl::new(engine(Spark), None);
        assert_eq!(
            spark.check_schema_exists_macro(&state, &[])?,
            ("dbt".to_string(), "check_schema_exists".to_string())
        );
        Ok(())
    }
}
