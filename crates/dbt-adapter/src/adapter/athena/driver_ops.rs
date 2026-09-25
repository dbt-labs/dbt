//! Glue / S3 / Athena / STS calls behind the Athena adapter methods.
//!
//! dbt-athena makes these through boto3 on the connection's session. Here they go
//! through the ADBC driver: a statement with `athena.operation` /
//! `athena.operation.payload` set runs the named AWS API call with the connection's
//! credentials and answers with the API response as one JSON cell (see the driver's
//! `operations.go`). The payload and the response are the AWS API JSON shapes, so
//! the composition below follows `impl.py` field for field.

use super::{TableType, is_s3_tables_database, parse_s3_path};
use crate::adapter::Adapter;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use adbc_core::options::OptionValue;
use arrow::array::{Array, StringArray};
use base64::Engine as _;
use dashmap::DashMap;
use dbt_adbc::athena::{OPERATION, OPERATION_PAYLOAD};
use minijinja::State;
use serde_json::{Map, Value as Json, json};
use std::collections::HashMap;
use std::sync::{LazyLock, OnceLock};

/// `AthenaAdapter.GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH`
const GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH: usize = 2048;
/// `AthenaAdapter.PARTITION_PROCESSING_CHUNK_SIZE`
const PARTITION_PROCESSING_CHUNK_SIZE: usize = 1000;

const DEFAULT_WORK_GROUP: &str = "primary";

/// `TableInput` keys, as `_get_table_input` filters a `Table` down to what
/// `UpdateTable` accepts.
const TABLE_INPUT_KEYS: &[&str] = &[
    "Name",
    "Description",
    "Owner",
    "LastAccessTime",
    "LastAnalyzedTime",
    "Retention",
    "StorageDescriptor",
    "PartitionKeys",
    "ViewOriginalText",
    "ViewExpandedText",
    "TableType",
    "Parameters",
    "TargetTable",
];

/// The relation components the operations need.
#[derive(Debug, Clone)]
pub struct RelationParts {
    pub database: Option<String>,
    pub schema: String,
    pub identifier: String,
    /// `AthenaRelation.s3_path_table_part`, when `Relation.create` received one.
    pub s3_path_table_part: Option<String>,
    /// `relation.render()`, for messages.
    pub rendered: String,
}

/// A Glue table as the adapter methods read it back.
#[derive(Debug, Clone)]
pub struct GlueTable {
    pub table_type: TableType,
    pub location: Option<String>,
}

/// Per-engine facts that dbt-athena caches for the run (`_get_aws_account_id`,
/// the `lru_cache` on `_get_work_group`).
#[derive(Default)]
struct Cached {
    account_id: OnceLock<String>,
    output_location_enforced: OnceLock<bool>,
}

static CACHE: LazyLock<DashMap<u64, Cached>> = LazyLock::new(DashMap::new);

/// The operations, bound to the adapter and the Jinja state of one call.
pub struct AthenaOps<'a, 't, 'e> {
    adapter: &'a Adapter,
    state: &'a State<'t, 'e>,
    engine_key: u64,
    work_group: String,
}

fn unexpected(msg: impl Into<String>) -> AdapterError {
    AdapterError::new(AdapterErrorKind::UnexpectedResult, msg)
}

fn str_of(value: &Json, key: &str) -> Option<String> {
    value.get(key).and_then(Json::as_str).map(str::to_string)
}

impl<'a, 't, 'e> AthenaOps<'a, 't, 'e> {
    pub fn new(
        adapter: &'a Adapter,
        state: &'a State<'t, 'e>,
        engine_key: u64,
        work_group: Option<&str>,
    ) -> Self {
        Self {
            adapter,
            state,
            engine_key,
            work_group: work_group.unwrap_or(DEFAULT_WORK_GROUP).to_string(),
        }
    }

    /// Run one driver operation and decode its JSON response.
    pub(super) fn call(&self, operation: &str, payload: Json) -> AdapterResult<Json> {
        let options = vec![
            (
                OPERATION.to_string(),
                OptionValue::String(operation.to_string()),
            ),
            (
                OPERATION_PAYLOAD.to_string(),
                OptionValue::String(payload.to_string()),
            ),
        ];
        let (_, table) =
            self.adapter
                .execute(self.state, None, "none", false, true, None, Some(options))?;
        let batch = table.original_record_batch();
        let column = batch.column_by_name("result").ok_or_else(|| {
            unexpected(format!(
                "[athena] {operation}: the driver returned no result column (columns {:?})",
                batch
                    .schema()
                    .fields()
                    .iter()
                    .map(|f| f.name().clone())
                    .collect::<Vec<_>>()
            ))
        })?;
        // The driver writes utf8; the engine may hand it back as another string type.
        let column = arrow::compute::cast(column, &arrow::datatypes::DataType::Utf8)
            .map_err(|e| unexpected(format!("[athena] {operation}: result column: {e}")))?;
        let cell = column
            .as_any()
            .downcast_ref::<StringArray>()
            .filter(|column| column.len() == 1 && !column.is_null(0))
            .map(|column| column.value(0).to_string())
            .ok_or_else(|| {
                unexpected(format!(
                    "[athena] {operation}: the driver returned no result cell ({} rows)",
                    batch.num_rows()
                ))
            })?;
        serde_json::from_str(&cell).map_err(|e| {
            unexpected(format!(
                "[athena] {operation}: the driver returned invalid JSON: {e}"
            ))
        })
    }

    fn cached<T: Clone>(&self, read: impl Fn(&Cached) -> &OnceLock<T>) -> Option<T> {
        CACHE
            .get(&self.engine_key)
            .and_then(|cached| read(&cached).get().cloned())
    }

    fn remember<T: Clone>(&self, read: impl Fn(&Cached) -> &OnceLock<T>, value: T) -> T {
        let entry = CACHE.entry(self.engine_key).or_default();
        read(&entry).get_or_init(|| value).clone()
    }

    /// `AthenaAdapter._get_aws_account_id`
    fn account_id(&self) -> AdapterResult<String> {
        if let Some(account) = self.cached(|c| &c.account_id) {
            return Ok(account);
        }
        let identity = self.call("sts.get_caller_identity", json!({}))?;
        let account = str_of(&identity, "Account")
            .ok_or_else(|| unexpected("[athena] sts.get_caller_identity returned no account id"))?;
        Ok(self.remember(|c| &c.account_id, account))
    }

    /// `get_catalog_id(self._get_data_catalog(database))`: the Glue `CatalogId` for a
    /// dbt database. `awsdatacatalog` and S3 Tables catalogs are derived from the account
    /// id; anything else is looked up as an Athena data catalog and yields an id only when
    /// Glue-backed.
    fn catalog_id(&self, database: Option<&str>) -> AdapterResult<Option<String>> {
        let Some(database) = database.filter(|d| !d.is_empty()) else {
            return Ok(None);
        };
        if database.eq_ignore_ascii_case("awsdatacatalog") {
            return Ok(Some(self.account_id()?));
        }
        if is_s3_tables_database(Some(database)) {
            return Ok(Some(format!("{}:{database}", self.account_id()?)));
        }
        let response = self.call("athena.get_data_catalog", json!({ "Name": database }))?;
        let catalog = &response["DataCatalog"];
        if catalog.get("Type").and_then(Json::as_str) != Some("GLUE") {
            return Ok(None);
        }
        Ok(catalog
            .get("Parameters")
            .and_then(|p| p.get("catalog-id"))
            .and_then(Json::as_str)
            .map(str::to_string))
    }

    fn table_scope(&self, relation: &RelationParts) -> AdapterResult<Map<String, Json>> {
        let mut scope = Map::new();
        if let Some(catalog_id) = self.catalog_id(relation.database.as_deref())? {
            scope.insert("CatalogId".into(), Json::String(catalog_id));
        }
        scope.insert("DatabaseName".into(), Json::String(relation.schema.clone()));
        Ok(scope)
    }

    /// `AthenaAdapter.get_glue_table`: the raw Glue `Table`, `None` when it does not exist.
    fn get_glue_table(&self, relation: &RelationParts) -> AdapterResult<Option<Json>> {
        let mut payload = self.table_scope(relation)?;
        payload.insert("Name".into(), Json::String(relation.identifier.clone()));
        let response = self.call("glue.get_table", Json::Object(payload))?;
        match response.get("Table") {
            Some(Json::Null) | None => {
                tracing::debug!("Table {} does not exist - Ignoring", relation.rendered);
                Ok(None)
            }
            Some(table) => Ok(Some(table.clone())),
        }
    }

    fn table_type_of(table: &Json, relation: &RelationParts) -> AdapterResult<TableType> {
        TableType::from_glue(
            table.get("TableType").and_then(Json::as_str),
            table
                .get("Parameters")
                .and_then(|p| p.get("table_type"))
                .and_then(Json::as_str),
            &relation.rendered,
        )
        .map_err(unexpected)
    }

    /// `AthenaAdapter.get_glue_table_type` plus the location, so callers that need
    /// both make one Glue call.
    pub fn glue_table(&self, relation: &RelationParts) -> AdapterResult<Option<GlueTable>> {
        let Some(table) = self.get_glue_table(relation)? else {
            return Ok(None);
        };
        Ok(Some(GlueTable {
            table_type: Self::table_type_of(&table, relation)?,
            location: table
                .get("StorageDescriptor")
                .and_then(|sd| str_of(sd, "Location")),
        }))
    }

    /// `AthenaAdapter.get_glue_table_location`: the S3 location of a physical table,
    /// `None` for views and missing tables.
    pub fn glue_table_location(&self, relation: &RelationParts) -> AdapterResult<Option<String>> {
        let Some(table) = self.glue_table(relation)? else {
            return Ok(None);
        };
        if !table.table_type.is_physical() {
            return Ok(None);
        }
        match table.location {
            Some(location) if !location.is_empty() => Ok(Some(location)),
            _ => Err(unexpected(format!(
                "Relation {} is of type '{}' which requires a location, but no location returned by Glue.",
                relation.rendered,
                table.table_type.value()
            ))),
        }
    }

    /// `AthenaAdapter.delete_from_glue_catalog`
    pub fn delete_from_glue_catalog(&self, relation: &RelationParts) -> AdapterResult<()> {
        let mut payload = self.table_scope(relation)?;
        payload.insert("Name".into(), Json::String(relation.identifier.clone()));
        let response = self.call("glue.delete_table", Json::Object(payload))?;
        if response.get("Deleted").and_then(Json::as_bool) == Some(true) {
            tracing::debug!("Deleted table from glue catalog: {}", relation.rendered);
        } else {
            tracing::debug!(
                "Table {} does not exist and will not be deleted, ignoring",
                relation.rendered
            );
        }
        Ok(())
    }

    /// `AthenaAdapter.drop_glue_database`
    pub fn drop_glue_database(&self, database_name: &str, catalog_name: &str) -> AdapterResult<()> {
        let mut payload = Map::new();
        if let Some(catalog_id) = self.catalog_id(Some(catalog_name))? {
            payload.insert("CatalogId".into(), Json::String(catalog_id));
        }
        payload.insert("Name".into(), Json::String(database_name.to_string()));
        self.call("glue.delete_database", Json::Object(payload))?;
        tracing::debug!("Glue database successfully deleted: {catalog_name}.{database_name}");
        Ok(())
    }

    /// `AthenaAdapter.expire_glue_table_versions`: keep the `to_keep` newest table
    /// versions, delete the rest (and their S3 data when `delete_s3`). Failures on a
    /// single version are logged, as in dbt-athena. Returns the deleted version ids.
    pub fn expire_glue_table_versions(
        &self,
        relation: &RelationParts,
        to_keep: usize,
        delete_s3: bool,
    ) -> AdapterResult<Vec<String>> {
        let catalog_id = self.catalog_id(relation.database.as_deref())?;
        // dbt-athena lists the versions without a CatalogId.
        let response = self.call(
            "glue.get_table_versions",
            json!({ "DatabaseName": relation.schema, "TableName": relation.identifier }),
        )?;
        let mut versions: Vec<(i64, String, Option<String>)> = response["TableVersions"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|version| {
                let table = version.get("Table")?;
                let id = str_of(table, "VersionId")?;
                let ordinal = id.parse::<i64>().ok()?;
                let location = table
                    .get("StorageDescriptor")
                    .and_then(|sd| str_of(sd, "Location"));
                Some((ordinal, id, location))
            })
            .collect();
        versions.sort_by_key(|version| std::cmp::Reverse(version.0));

        let mut deleted = Vec::new();
        for (_, version_id, location) in versions.into_iter().skip(to_keep) {
            let mut payload = Map::new();
            if let Some(catalog_id) = &catalog_id {
                payload.insert("CatalogId".into(), Json::String(catalog_id.clone()));
            }
            payload.insert("DatabaseName".into(), Json::String(relation.schema.clone()));
            payload.insert(
                "TableName".into(),
                Json::String(relation.identifier.clone()),
            );
            payload.insert("VersionId".into(), Json::String(version_id.clone()));
            match self.call("glue.delete_table_version", Json::Object(payload)) {
                Ok(_) => {
                    tracing::debug!(
                        "Deleted version {version_id} of table {}",
                        relation.rendered
                    );
                    deleted.push(version_id);
                    if delete_s3 && let Some(location) = location {
                        if let Err(e) = self.delete_from_s3(&location) {
                            tracing::debug!("There was an error when deleting {location}: {e}");
                        }
                    }
                }
                Err(e) => tracing::debug!(
                    "There was an error when expiring table version {version_id} with error: {e}"
                ),
            }
        }
        Ok(deleted)
    }

    fn list_partitions(
        &self,
        scope: &Map<String, Json>,
        relation: &RelationParts,
        expression: Option<&str>,
        exclude_column_schema: bool,
    ) -> AdapterResult<Vec<Json>> {
        let mut payload = scope.clone();
        payload.insert(
            "TableName".into(),
            Json::String(relation.identifier.clone()),
        );
        if let Some(expression) = expression {
            payload.insert("Expression".into(), Json::String(expression.to_string()));
        }
        payload.insert(
            "ExcludeColumnSchema".into(),
            Json::Bool(exclude_column_schema),
        );
        let response = self.call("glue.get_partitions", Json::Object(payload))?;
        Ok(response["Partitions"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Delete the given partitions; the driver chunks to the API limit. Returns the
    /// `Errors` the API reported.
    fn batch_delete_partitions(
        &self,
        scope: &Map<String, Json>,
        relation: &RelationParts,
        partitions: &[Json],
    ) -> AdapterResult<Vec<Json>> {
        let mut payload = scope.clone();
        payload.insert(
            "TableName".into(),
            Json::String(relation.identifier.clone()),
        );
        payload.insert(
            "PartitionsToDelete".into(),
            Json::Array(
                partitions
                    .iter()
                    .map(|p| json!({ "Values": p.get("Values").cloned().unwrap_or(Json::Array(vec![])) }))
                    .collect(),
            ),
        );
        let response = self.call("glue.batch_delete_partition", Json::Object(payload))?;
        Ok(response["Errors"].as_array().cloned().unwrap_or_default())
    }

    /// `AthenaAdapter.swap_table`: point `target` at `src`'s storage, partition keys,
    /// type, parameters and description, then replace its partitions with `src`'s.
    pub fn swap_table(&self, src: &RelationParts, target: &RelationParts) -> AdapterResult<()> {
        let scope = self.table_scope(src)?;
        let src_table = self.get_glue_table(src)?.ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::NotFound,
                format!(
                    "[athena] glue.get_table returned no table for {}",
                    src.rendered
                ),
            )
        })?;
        let src_partitions = self.list_partitions(&scope, src, None, false)?;
        // dbt-athena resolves the target catalog from the source database too.
        let mut target_scope = scope;
        target_scope.insert("DatabaseName".into(), Json::String(target.schema.clone()));
        let target_partitions = self.list_partitions(&target_scope, target, None, false)?;

        let mut table_input = Map::new();
        table_input.insert("Name".into(), Json::String(target.identifier.clone()));
        for key in [
            "StorageDescriptor",
            "PartitionKeys",
            "TableType",
            "Parameters",
        ] {
            if let Some(value) = src_table.get(key) {
                table_input.insert(key.into(), value.clone());
            }
        }
        table_input.insert(
            "Description".into(),
            src_table
                .get("Description")
                .cloned()
                .unwrap_or(Json::String(String::new())),
        );
        let mut update = target_scope.clone();
        update.insert("TableInput".into(), Json::Object(table_input));
        self.call("glue.update_table", Json::Object(update))?;
        tracing::debug!(
            "Table {} swapped with the content of {}",
            target.rendered,
            src.rendered
        );

        if !target_partitions.is_empty() {
            self.batch_delete_partitions(&target_scope, target, &target_partitions)?;
        }
        if !src_partitions.is_empty() {
            let mut create = target_scope;
            create.insert("TableName".into(), Json::String(target.identifier.clone()));
            create.insert(
                "PartitionInputList".into(),
                Json::Array(
                    src_partitions
                        .iter()
                        .map(|p| {
                            json!({
                                "Values": p.get("Values").cloned().unwrap_or(Json::Array(vec![])),
                                "StorageDescriptor": p.get("StorageDescriptor").cloned().unwrap_or(Json::Null),
                                "Parameters": p.get("Parameters").cloned().unwrap_or(Json::Null),
                            })
                        })
                        .collect(),
                ),
            );
            self.call("glue.batch_create_partition", Json::Object(create))?;
        }
        Ok(())
    }

    /// `AthenaAdapter.clean_up_partitions`: delete the data and the Glue partitions
    /// matching the `where` conditions, batching conditions up to the Glue expression
    /// length limit.
    pub fn clean_up_partitions(
        &self,
        relation: &RelationParts,
        where_conditions: Vec<String>,
    ) -> AdapterResult<()> {
        let mut expressions: Vec<String> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        for condition in where_conditions {
            let bracketed = format!("({condition})");
            if bracketed.len() > GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH {
                return Err(AdapterError::new(
                    AdapterErrorKind::Configuration,
                    format!(
                        "Partition condition exceeds the Glue API expression limit of {} characters: '{}...'",
                        GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH,
                        bracketed.chars().take(100).collect::<String>()
                    ),
                ));
            }
            if !current.is_empty()
                && current.iter().map(String::len).sum::<usize>()
                    + bracketed.len()
                    + " or ".len() * current.len()
                    > GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH
            {
                expressions.push(current.join(" or "));
                current.clear();
            }
            current.push(bracketed);
        }
        if !current.is_empty() {
            expressions.push(current.join(" or "));
        }

        let scope = self.table_scope(relation)?;
        let mut partitions = Vec::new();
        for expression in expressions {
            partitions.extend(self.list_partitions(&scope, relation, Some(&expression), true)?);
        }
        for chunk in partitions.chunks(PARTITION_PROCESSING_CHUNK_SIZE) {
            let locations = chunk
                .iter()
                .filter_map(|p| {
                    p.get("StorageDescriptor")
                        .and_then(|sd| str_of(sd, "Location"))
                })
                .collect::<Vec<_>>();
            self.bulk_delete_from_s3(locations)?;
            let errors = self.batch_delete_partitions(&scope, relation, chunk)?;
            if !errors.is_empty() {
                for err in &errors {
                    tracing::error!(
                        "Failed to delete Glue partition: Values='{}', Code='{}', Message='{}'",
                        err.get("PartitionValues")
                            .map(Json::to_string)
                            .unwrap_or_default(),
                        err.get("ErrorDetail")
                            .and_then(|d| str_of(d, "ErrorCode"))
                            .unwrap_or_default(),
                        err.get("ErrorDetail")
                            .and_then(|d| str_of(d, "ErrorMessage"))
                            .unwrap_or_default(),
                    );
                }
                return Err(unexpected(format!(
                    "Failed to delete {} partition(s) from Glue table '{}.{}'",
                    errors.len(),
                    relation.schema,
                    relation.identifier
                )));
            }
        }
        Ok(())
    }

    /// `AthenaAdapter.persist_docs_to_glue`. `table_description` / `column_descriptions`
    /// are already cleaned and truncated; `table_parameters` / `column_parameters` are the
    /// `meta` entries already stringified. Updates the table only when something changed.
    pub fn persist_docs_to_glue(
        &self,
        relation: &RelationParts,
        table_description: Option<String>,
        table_parameters: Vec<(String, String)>,
        column_descriptions: HashMap<String, String>,
        column_parameters: HashMap<String, Vec<(String, String)>>,
        skip_archive_table_version: bool,
    ) -> AdapterResult<()> {
        let scope = self.table_scope(relation)?;
        let table = self.get_glue_table(relation)?.ok_or_else(|| {
            AdapterError::new(
                AdapterErrorKind::NotFound,
                format!(
                    "[athena] glue.get_table returned no table for {}",
                    relation.rendered
                ),
            )
        })?;

        // `TableInput` is the writable subset of `Table`, as `_get_table_input` does.
        let mut table_input: Map<String, Json> = table
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(key, value)| TABLE_INPUT_KEYS.contains(&key.as_str()) && !value.is_null())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let mut need_update = false;

        if let Some(new_description) = table_description {
            let parameters = table_input
                .entry("Parameters")
                .or_insert_with(|| Json::Object(Map::new()));
            let parameters = parameters
                .as_object_mut()
                .ok_or_else(|| unexpected("[athena] Glue table Parameters is not an object"))?;
            let current_description = str_of(&table, "Description").unwrap_or_default();
            let current_comment = parameters
                .get("comment")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_string();
            if current_description != new_description || current_comment != new_description {
                need_update = true;
            }
            parameters.insert("comment".into(), Json::String(new_description.clone()));
            for (key, value) in table_parameters {
                if parameters.get(&key).and_then(Json::as_str) != Some(value.as_str()) {
                    need_update = true;
                }
                parameters.insert(key, Json::String(value));
            }
            table_input.insert("Description".into(), Json::String(new_description));
        }

        if (!column_descriptions.is_empty() || !column_parameters.is_empty())
            && let Some(columns) = table_input
                .get_mut("StorageDescriptor")
                .and_then(|sd| sd.get_mut("Columns"))
                .and_then(Json::as_array_mut)
        {
            for column in columns.iter_mut().filter_map(Json::as_object_mut) {
                let Some(name) = column
                    .get("Name")
                    .and_then(Json::as_str)
                    .map(str::to_string)
                else {
                    continue;
                };
                if let Some(comment) = column_descriptions.get(&name) {
                    if column
                        .get("Comment")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        != comment
                    {
                        need_update = true;
                    }
                    column.insert("Comment".into(), Json::String(comment.clone()));
                }
                if let Some(params) = column_parameters.get(&name) {
                    let current = column
                        .entry("Parameters")
                        .or_insert_with(|| Json::Object(Map::new()));
                    if let Some(current) = current.as_object_mut() {
                        for (key, value) in params {
                            if current.get(key).and_then(Json::as_str) != Some(value.as_str()) {
                                need_update = true;
                            }
                            current.insert(key.clone(), Json::String(value.clone()));
                        }
                    }
                }
            }
        }

        if !need_update {
            return Ok(());
        }
        let mut update = scope;
        update.insert("TableInput".into(), Json::Object(table_input));
        update.insert("SkipArchive".into(), Json::Bool(skip_archive_table_version));
        self.call("glue.update_table", Json::Object(update))?;
        Ok(())
    }

    fn list_object_keys(&self, bucket: &str, prefix: &str) -> AdapterResult<Vec<String>> {
        let response = self.call(
            "s3.list_objects",
            json!({ "Bucket": bucket, "Prefix": prefix }),
        )?;
        Ok(response["Keys"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(str::to_string)
            .collect())
    }

    fn delete_object_keys(&self, bucket: &str, keys: Vec<String>) -> AdapterResult<()> {
        let response = self.call(
            "s3.delete_objects",
            json!({ "Bucket": bucket, "Keys": keys }),
        )?;
        let errors = response["Errors"].as_array().cloned().unwrap_or_default();
        for err in &errors {
            tracing::error!(
                "Failed to delete files: Key='{}', Code='{}', Message='{}', s3_bucket='{}'",
                str_of(err, "Key").unwrap_or_default(),
                str_of(err, "Code").unwrap_or_default(),
                str_of(err, "Message").unwrap_or_default(),
                bucket
            );
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(unexpected("Failed to delete files from S3."))
        }
    }

    /// `AthenaAdapter.delete_from_s3`
    pub fn delete_from_s3(&self, s3_path: &str) -> AdapterResult<()> {
        let (bucket, prefix) = parse_s3_path(s3_path);
        let keys = self.list_object_keys(&bucket, &prefix)?;
        if keys.is_empty() {
            tracing::debug!("S3 path does not exist");
            return Ok(());
        }
        tracing::debug!(
            "Deleting table data: path='{s3_path}', bucket='{bucket}', prefix='{prefix}'"
        );
        self.delete_object_keys(&bucket, keys)
    }

    /// `AthenaAdapter.bulk_delete_from_s3`
    fn bulk_delete_from_s3(&self, s3_paths: Vec<String>) -> AdapterResult<()> {
        if s3_paths.is_empty() {
            tracing::debug!("No S3 paths provided for deletion");
            return Ok(());
        }
        let mut keys_by_bucket: HashMap<String, Vec<String>> = HashMap::new();
        for s3_path in &s3_paths {
            let (bucket, prefix) = parse_s3_path(s3_path);
            tracing::debug!("Listing files for deletion: {s3_path}");
            let keys = self.list_object_keys(&bucket, &prefix)?;
            keys_by_bucket.entry(bucket).or_default().extend(keys);
        }
        for (bucket, keys) in keys_by_bucket {
            if !keys.is_empty() {
                tracing::debug!("Calling delete_objects for {} objects", keys.len());
                self.delete_object_keys(&bucket, keys)?;
            }
        }
        Ok(())
    }

    /// The upload behind `AthenaAdapter.upload_seed_to_s3`. `upload_args` is the model's
    /// `seed_s3_upload_args` (boto3 `ExtraArgs`); the keys dbt-athena users set are
    /// forwarded, anything else is rejected rather than silently dropped.
    pub fn upload_object(
        &self,
        bucket: &str,
        key: &str,
        body: &[u8],
        upload_args: Vec<(String, String)>,
    ) -> AdapterResult<()> {
        let mut payload = Map::new();
        payload.insert("Bucket".into(), Json::String(bucket.to_string()));
        payload.insert("Key".into(), Json::String(key.to_string()));
        payload.insert(
            "Body".into(),
            Json::String(base64::engine::general_purpose::STANDARD.encode(body)),
        );
        for (name, value) in upload_args {
            match name.as_str() {
                "ServerSideEncryption" | "SSEKMSKeyId" | "ACL" | "StorageClass" | "ContentType" => {
                    payload.insert(name, Json::String(value));
                }
                "BucketKeyEnabled" => {
                    payload.insert(name, Json::Bool(value.eq_ignore_ascii_case("true")));
                }
                other => {
                    return Err(AdapterError::new(
                        AdapterErrorKind::NotSupported,
                        format!(
                            "seed_s3_upload_args key '{other}' is not supported by the dbt Athena backend"
                        ),
                    ));
                }
            }
        }
        self.call("s3.put_object", Json::Object(payload))?;
        Ok(())
    }

    /// `AthenaAdapter.is_work_group_output_location_enforced`, cached per engine like
    /// the Python `lru_cache` on `_get_work_group`.
    pub fn is_work_group_output_location_enforced(&self) -> AdapterResult<bool> {
        if let Some(enforced) = self.cached(|c| &c.output_location_enforced) {
            return Ok(enforced);
        }
        let response = self.call(
            "athena.get_work_group",
            json!({ "WorkGroup": self.work_group }),
        )?;
        let configuration = &response["WorkGroup"]["Configuration"];
        let has_output_location =
            configuration["ResultConfiguration"]["OutputLocation"].is_string();
        let enforced = configuration["EnforceWorkGroupConfiguration"]
            .as_bool()
            .unwrap_or(false);
        Ok(self.remember(
            |c| &c.output_location_enforced,
            has_output_location && enforced,
        ))
    }
}
