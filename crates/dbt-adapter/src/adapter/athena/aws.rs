//! Glue / S3 / Athena / STS calls behind the Athena adapter methods.
//!
//! dbt-athena makes these through boto3 on the connection's session. Here one
//! client set per adapter engine is built from the profile with `aws-config`,
//! resolving credentials the way `dbt-auth`'s `AthenaAuth` configures the ADBC
//! driver: static keys, a named profile, or the default chain.

use super::{TableType, is_s3_tables_database, parse_s3_path};
use crate::AdapterEngine;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use aws_sdk_glue::error::{DisplayErrorContext, SdkError};
use aws_sdk_glue::types::{PartitionInput, PartitionValueList, TableInput};
use aws_sdk_s3::primitives::ByteStream;
use aws_sdk_s3::types::{Delete, ObjectIdentifier};
use dashmap::DashMap;
use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, LazyLock};
use tokio::sync::OnceCell;

/// `AthenaAdapter.BATCH_CREATE_PARTITION_API_LIMIT`
const BATCH_CREATE_PARTITION_API_LIMIT: usize = 100;
/// `AthenaAdapter.BATCH_DELETE_PARTITION_API_LIMIT`
const BATCH_DELETE_PARTITION_API_LIMIT: usize = 25;
/// `AthenaAdapter.BATCH_DELETE_S3_OBJECTS_API_LIMIT`
const BATCH_DELETE_S3_OBJECTS_API_LIMIT: usize = 1000;
/// `AthenaAdapter.PARTITION_PROCESSING_CHUNK_SIZE`
const PARTITION_PROCESSING_CHUNK_SIZE: usize = 1000;
/// `AthenaAdapter.GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH`
const GET_PARTITIONS_API_EXPRESSION_MAX_LENGTH: usize = 2048;

const DEFAULT_WORK_GROUP: &str = "primary";

/// The relation components the AWS calls need, owned so they can cross into
/// `'static` futures.
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

static CLIENTS: LazyLock<DashMap<u64, Arc<AthenaAws>>> = LazyLock::new(DashMap::new);

fn aws_error<E: std::error::Error + 'static>(op: &str, err: E) -> AdapterError {
    AdapterError::new(
        AdapterErrorKind::Internal,
        format!("[athena] {op}: {}", DisplayErrorContext(&err)),
    )
}

fn block<F>(future: F) -> F::Output
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    dbt_common::tracing::spawn_traced_block_in_place(future)
}

fn is_entity_not_found<E, R>(err: &SdkError<E, R>, check: impl Fn(&E) -> bool) -> bool {
    err.as_service_error().is_some_and(check)
}

pub struct AthenaAws {
    glue: aws_sdk_glue::Client,
    s3: aws_sdk_s3::Client,
    athena: aws_sdk_athena::Client,
    sts: aws_sdk_sts::Client,
    work_group: String,
    account_id: OnceCell<String>,
    output_location_enforced: OnceCell<bool>,
}

impl std::fmt::Debug for AthenaAws {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AthenaAws")
            .field("work_group", &self.work_group)
            .finish_non_exhaustive()
    }
}

impl AthenaAws {
    /// The client set for this engine's profile, built on first use.
    pub fn for_engine(engine: &dyn AdapterEngine) -> AdapterResult<Arc<Self>> {
        let key = engine.fingerprint();
        if let Some(clients) = CLIENTS.get(&key) {
            return Ok(clients.clone());
        }
        let clients = Arc::new(Self::from_config(engine)?);
        Ok(CLIENTS.entry(key).or_insert(clients).clone())
    }

    fn from_config(engine: &dyn AdapterEngine) -> AdapterResult<Self> {
        let config = engine.get_config();
        let region = config.require_str("region_name").map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::Configuration,
                "Athena requires 'region_name' in profile configuration",
            )
        })?;
        let mut loader = aws_config::defaults(aws_config::BehaviorVersion::latest())
            .region(aws_config::Region::new(region.to_string()));
        if let Some(access_key_id) = config.get_str("aws_access_key_id") {
            let secret_access_key = config.require_str("aws_secret_access_key").map_err(|_| {
                AdapterError::new(
                    AdapterErrorKind::Configuration,
                    "Athena auth requires 'aws_secret_access_key' when 'aws_access_key_id' is set",
                )
            })?;
            let session_token = config
                .get_str("aws_session_token")
                .filter(|t| !t.is_empty())
                .map(str::to_string);
            loader = loader.credentials_provider(aws_sdk_glue::config::Credentials::new(
                access_key_id,
                secret_access_key,
                session_token,
                None,
                "dbt-profile",
            ));
        } else if let Some(profile_name) = config.get_str("aws_profile_name") {
            loader = loader.profile_name(profile_name);
        }
        let sdk_config = block(async move { loader.load().await });
        Ok(Self {
            glue: aws_sdk_glue::Client::new(&sdk_config),
            s3: aws_sdk_s3::Client::new(&sdk_config),
            athena: aws_sdk_athena::Client::new(&sdk_config),
            sts: aws_sdk_sts::Client::new(&sdk_config),
            work_group: config
                .get_str("work_group")
                .unwrap_or(DEFAULT_WORK_GROUP)
                .to_string(),
            account_id: OnceCell::new(),
            output_location_enforced: OnceCell::new(),
        })
    }

    /// `AthenaAdapter._get_aws_account_id`
    async fn account_id(&self) -> AdapterResult<&str> {
        self.account_id
            .get_or_try_init(|| async {
                let identity = self
                    .sts
                    .get_caller_identity()
                    .send()
                    .await
                    .map_err(|e| aws_error("sts.get_caller_identity", e))?;
                identity.account().map(str::to_string).ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        "[athena] sts.get_caller_identity returned no account id",
                    )
                })
            })
            .await
            .map(String::as_str)
    }

    /// `get_catalog_id(self._get_data_catalog(database))`: the Glue `CatalogId` for a
    /// dbt database. `awsdatacatalog` and S3 Tables catalogs are derived from the account
    /// id; anything else is looked up as an Athena data catalog and yields an id only when
    /// Glue-backed.
    async fn catalog_id(&self, database: Option<&str>) -> AdapterResult<Option<String>> {
        let Some(database) = database.filter(|d| !d.is_empty()) else {
            return Ok(None);
        };
        if database.eq_ignore_ascii_case("awsdatacatalog") {
            return Ok(Some(self.account_id().await?.to_string()));
        }
        if is_s3_tables_database(Some(database)) {
            return Ok(Some(format!("{}:{database}", self.account_id().await?)));
        }
        let output = self
            .athena
            .get_data_catalog()
            .name(database)
            .send()
            .await
            .map_err(|e| aws_error("athena.get_data_catalog", e))?;
        Ok(output.data_catalog().and_then(|catalog| {
            if catalog.r#type().as_str() != "GLUE" {
                return None;
            }
            catalog
                .parameters()
                .and_then(|p| p.get("catalog-id"))
                .cloned()
        }))
    }

    /// `AthenaAdapter.get_glue_table`; `None` when Glue has no such table.
    async fn get_glue_table(
        &self,
        relation: &RelationParts,
    ) -> AdapterResult<Option<aws_sdk_glue::types::Table>> {
        let catalog_id = self.catalog_id(relation.database.as_deref()).await?;
        match self
            .glue
            .get_table()
            .set_catalog_id(catalog_id)
            .database_name(&relation.schema)
            .name(&relation.identifier)
            .send()
            .await
        {
            Ok(output) => Ok(output.table),
            Err(e) if is_entity_not_found(&e, |e| e.is_entity_not_found_exception()) => {
                tracing::debug!("Table {} does not exist - Ignoring", relation.rendered);
                Ok(None)
            }
            Err(e) => Err(aws_error("glue.get_table", e)),
        }
    }

    fn table_type_of(table: &aws_sdk_glue::types::Table) -> AdapterResult<TableType> {
        let full_name = [
            table.catalog_id(),
            table.database_name(),
            Some(table.name()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(".");
        TableType::from_glue(
            table.table_type(),
            table
                .parameters()
                .and_then(|p| p.get("table_type"))
                .map(String::as_str),
            &full_name,
        )
        .map_err(|msg| AdapterError::new(AdapterErrorKind::UnexpectedResult, msg))
    }

    /// `AthenaAdapter.get_glue_table_type` plus the location, so callers that need
    /// both make one Glue call.
    pub fn glue_table(
        self: &Arc<Self>,
        relation: RelationParts,
    ) -> AdapterResult<Option<GlueTable>> {
        let this = self.clone();
        block(async move {
            let Some(table) = this.get_glue_table(&relation).await? else {
                return Ok(None);
            };
            Ok(Some(GlueTable {
                table_type: Self::table_type_of(&table)?,
                location: table
                    .storage_descriptor()
                    .and_then(|sd| sd.location())
                    .map(str::to_string),
            }))
        })
    }

    /// `AthenaAdapter.get_glue_table_location`: the S3 location of a physical table,
    /// `None` for views and missing tables.
    pub fn glue_table_location(
        self: &Arc<Self>,
        relation: RelationParts,
    ) -> AdapterResult<Option<String>> {
        let Some(table) = self.glue_table(relation.clone())? else {
            return Ok(None);
        };
        if !table.table_type.is_physical() {
            return Ok(None);
        }
        match table.location {
            Some(location) if !location.is_empty() => Ok(Some(location)),
            _ => Err(AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!(
                    "Relation {} is of type '{}' which requires a location, but no location returned by Glue.",
                    relation.rendered,
                    table.table_type.value()
                ),
            )),
        }
    }

    /// `AthenaAdapter.delete_from_glue_catalog`
    pub fn delete_from_glue_catalog(
        self: &Arc<Self>,
        relation: RelationParts,
    ) -> AdapterResult<()> {
        let this = self.clone();
        block(async move {
            let catalog_id = this.catalog_id(relation.database.as_deref()).await?;
            match this
                .glue
                .delete_table()
                .set_catalog_id(catalog_id)
                .database_name(&relation.schema)
                .name(&relation.identifier)
                .send()
                .await
            {
                Ok(_) => {
                    tracing::debug!("Deleted table from glue catalog: {}", relation.rendered);
                    Ok(())
                }
                Err(e) if is_entity_not_found(&e, |e| e.is_entity_not_found_exception()) => {
                    tracing::debug!(
                        "Table {} does not exist and will not be deleted, ignoring",
                        relation.rendered
                    );
                    Ok(())
                }
                Err(e) => Err(aws_error("glue.delete_table", e)),
            }
        })
    }

    /// `AthenaAdapter.drop_glue_database`
    pub fn drop_glue_database(
        self: &Arc<Self>,
        database_name: String,
        catalog_name: String,
    ) -> AdapterResult<()> {
        let this = self.clone();
        block(async move {
            let catalog_id = this.catalog_id(Some(&catalog_name)).await?;
            this.glue
                .delete_database()
                .set_catalog_id(catalog_id)
                .name(&database_name)
                .send()
                .await
                .map_err(|e| aws_error("glue.delete_database", e))?;
            tracing::debug!("Glue database successfully deleted: {catalog_name}.{database_name}");
            Ok(())
        })
    }

    /// `AthenaAdapter.expire_glue_table_versions`: keep the `to_keep` newest table
    /// versions, delete the rest (and their S3 data when `delete_s3`). Failures on a
    /// single version are logged, as in dbt-athena. Returns the deleted version ids.
    pub fn expire_glue_table_versions(
        self: &Arc<Self>,
        relation: RelationParts,
        to_keep: usize,
        delete_s3: bool,
    ) -> AdapterResult<Vec<String>> {
        let this = self.clone();
        block(async move {
            let catalog_id = this.catalog_id(relation.database.as_deref()).await?;
            let mut versions: Vec<(i64, String, Option<String>)> = Vec::new();
            let mut pages = this
                .glue
                .get_table_versions()
                .database_name(&relation.schema)
                .table_name(&relation.identifier)
                .into_paginator()
                .send();
            while let Some(page) = pages.next().await {
                let page = page.map_err(|e| aws_error("glue.get_table_versions", e))?;
                for version in page.table_versions() {
                    let Some(table) = version.table() else {
                        continue;
                    };
                    let Some(id) = table.version_id() else {
                        continue;
                    };
                    let Ok(ordinal) = id.parse::<i64>() else {
                        continue;
                    };
                    let location = table
                        .storage_descriptor()
                        .and_then(|sd| sd.location())
                        .map(str::to_string);
                    versions.push((ordinal, id.to_string(), location));
                }
            }
            versions.sort_by_key(|version| std::cmp::Reverse(version.0));
            let mut deleted = Vec::new();
            for (_, version_id, location) in versions.into_iter().skip(to_keep) {
                let result = this
                    .glue
                    .delete_table_version()
                    .set_catalog_id(catalog_id.clone())
                    .database_name(&relation.schema)
                    .table_name(&relation.identifier)
                    .version_id(&version_id)
                    .send()
                    .await;
                match result {
                    Ok(_) => {
                        tracing::debug!(
                            "Deleted version {version_id} of table {}",
                            relation.rendered
                        );
                        deleted.push(version_id);
                        if delete_s3 && let Some(location) = location {
                            if let Err(e) = this.delete_prefix(&location).await {
                                tracing::debug!("There was an error when deleting {location}: {e}");
                            }
                        }
                    }
                    Err(e) => tracing::debug!(
                        "There was an error when expiring table version {version_id} with error: {}",
                        DisplayErrorContext(&e)
                    ),
                }
            }
            Ok(deleted)
        })
    }

    async fn list_partitions(
        &self,
        catalog_id: Option<String>,
        relation: &RelationParts,
        expression: Option<String>,
        exclude_column_schema: bool,
    ) -> AdapterResult<Vec<aws_sdk_glue::types::Partition>> {
        let mut partitions = Vec::new();
        let mut pages = self
            .glue
            .get_partitions()
            .set_catalog_id(catalog_id)
            .database_name(&relation.schema)
            .table_name(&relation.identifier)
            .set_expression(expression)
            .exclude_column_schema(exclude_column_schema)
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| aws_error("glue.get_partitions", e))?;
            partitions.extend(page.partitions().iter().cloned());
        }
        Ok(partitions)
    }

    async fn batch_delete_partitions(
        &self,
        catalog_id: Option<String>,
        relation: &RelationParts,
        partitions: &[aws_sdk_glue::types::Partition],
    ) -> AdapterResult<Vec<aws_sdk_glue::types::PartitionError>> {
        let mut errors = Vec::new();
        for batch in partitions.chunks(BATCH_DELETE_PARTITION_API_LIMIT) {
            let to_delete = batch
                .iter()
                .map(|p| {
                    PartitionValueList::builder()
                        .set_values(Some(p.values().to_vec()))
                        .build()
                        .map_err(|e| aws_error("glue.batch_delete_partition", e))
                })
                .collect::<AdapterResult<Vec<_>>>()?;
            let output = self
                .glue
                .batch_delete_partition()
                .set_catalog_id(catalog_id.clone())
                .database_name(&relation.schema)
                .table_name(&relation.identifier)
                .set_partitions_to_delete(Some(to_delete))
                .send()
                .await
                .map_err(|e| aws_error("glue.batch_delete_partition", e))?;
            errors.extend(output.errors().iter().cloned());
        }
        Ok(errors)
    }

    /// `AthenaAdapter.swap_table`: point `target` at `src`'s storage, partition keys,
    /// type, parameters and description, then replace its partitions with `src`'s.
    pub fn swap_table(
        self: &Arc<Self>,
        src: RelationParts,
        target: RelationParts,
    ) -> AdapterResult<()> {
        let this = self.clone();
        block(async move {
            let src_catalog_id = this.catalog_id(src.database.as_deref()).await?;
            let src_table = this
                .glue
                .get_table()
                .set_catalog_id(src_catalog_id.clone())
                .database_name(&src.schema)
                .name(&src.identifier)
                .send()
                .await
                .map_err(|e| aws_error("glue.get_table", e))?
                .table
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::NotFound,
                        format!(
                            "[athena] glue.get_table returned no table for {}",
                            src.rendered
                        ),
                    )
                })?;
            let src_partitions = this
                .list_partitions(src_catalog_id.clone(), &src, None, false)
                .await?;
            // dbt-athena resolves the target catalog from the source database too.
            let target_catalog_id = src_catalog_id;
            let target_partitions = this
                .list_partitions(target_catalog_id.clone(), &target, None, false)
                .await?;

            let table_input = TableInput::builder()
                .name(&target.identifier)
                .set_storage_descriptor(src_table.storage_descriptor().cloned())
                .set_partition_keys(Some(src_table.partition_keys().to_vec()))
                .set_table_type(src_table.table_type().map(str::to_string))
                .set_parameters(src_table.parameters().cloned())
                .description(src_table.description().unwrap_or_default())
                .build()
                .map_err(|e| aws_error("glue.update_table", e))?;
            this.glue
                .update_table()
                .set_catalog_id(target_catalog_id.clone())
                .database_name(&target.schema)
                .table_input(table_input)
                .send()
                .await
                .map_err(|e| aws_error("glue.update_table", e))?;
            tracing::debug!(
                "Table {} swapped with the content of {}",
                target.rendered,
                src.rendered
            );

            if !target_partitions.is_empty() {
                this.batch_delete_partitions(
                    target_catalog_id.clone(),
                    &target,
                    &target_partitions,
                )
                .await?;
            }
            for batch in src_partitions.chunks(BATCH_CREATE_PARTITION_API_LIMIT) {
                let inputs = batch
                    .iter()
                    .map(|p| {
                        PartitionInput::builder()
                            .set_values(Some(p.values().to_vec()))
                            .set_storage_descriptor(p.storage_descriptor().cloned())
                            .set_parameters(p.parameters().cloned())
                            .build()
                    })
                    .collect::<Vec<_>>();
                this.glue
                    .batch_create_partition()
                    .set_catalog_id(target_catalog_id.clone())
                    .database_name(&target.schema)
                    .table_name(&target.identifier)
                    .set_partition_input_list(Some(inputs))
                    .send()
                    .await
                    .map_err(|e| aws_error("glue.batch_create_partition", e))?;
            }
            Ok(())
        })
    }

    /// `AthenaAdapter.clean_up_partitions`: delete the data and the Glue partitions
    /// matching the `where` conditions, batching conditions up to the Glue expression
    /// length limit.
    pub fn clean_up_partitions(
        self: &Arc<Self>,
        relation: RelationParts,
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

        let this = self.clone();
        block(async move {
            let catalog_id = this.catalog_id(relation.database.as_deref()).await?;
            let mut partitions = Vec::new();
            for expression in expressions {
                partitions.extend(
                    this.list_partitions(catalog_id.clone(), &relation, Some(expression), true)
                        .await?,
                );
            }
            for chunk in partitions.chunks(PARTITION_PROCESSING_CHUNK_SIZE) {
                let locations = chunk
                    .iter()
                    .filter_map(|p| p.storage_descriptor().and_then(|sd| sd.location()))
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                this.bulk_delete_prefixes(locations).await?;
                let errors = this
                    .batch_delete_partitions(catalog_id.clone(), &relation, chunk)
                    .await?;
                if !errors.is_empty() {
                    for err in &errors {
                        tracing::error!(
                            "Failed to delete Glue partition: Values='{:?}', Code='{}', Message='{}'",
                            err.partition_values(),
                            err.error_detail()
                                .and_then(|d| d.error_code())
                                .unwrap_or_default(),
                            err.error_detail()
                                .and_then(|d| d.error_message())
                                .unwrap_or_default(),
                        );
                    }
                    return Err(AdapterError::new(
                        AdapterErrorKind::UnexpectedResult,
                        format!(
                            "Failed to delete {} partition(s) from Glue table '{}.{}'",
                            errors.len(),
                            relation.schema,
                            relation.identifier
                        ),
                    ));
                }
            }
            Ok(())
        })
    }

    /// `AthenaAdapter.persist_docs_to_glue`. `table_description` / `column_descriptions`
    /// are already cleaned and truncated; `table_parameters` / `column_parameters` are the
    /// `meta` entries already stringified. Updates the table only when something changed.
    #[allow(clippy::too_many_arguments)]
    pub fn persist_docs_to_glue(
        self: &Arc<Self>,
        relation: RelationParts,
        table_description: Option<String>,
        table_parameters: Vec<(String, String)>,
        column_descriptions: HashMap<String, String>,
        column_parameters: HashMap<String, Vec<(String, String)>>,
        skip_archive_table_version: bool,
    ) -> AdapterResult<()> {
        let this = self.clone();
        block(async move {
            let catalog_id = this.catalog_id(relation.database.as_deref()).await?;
            let table = this
                .glue
                .get_table()
                .set_catalog_id(catalog_id.clone())
                .database_name(&relation.schema)
                .name(&relation.identifier)
                .send()
                .await
                .map_err(|e| aws_error("glue.get_table", e))?
                .table
                .ok_or_else(|| {
                    AdapterError::new(
                        AdapterErrorKind::NotFound,
                        format!(
                            "[athena] glue.get_table returned no table for {}",
                            relation.rendered
                        ),
                    )
                })?;

            let mut need_update = false;
            let mut parameters = table.parameters().cloned().unwrap_or_default();
            let mut description = table.description().map(str::to_string);

            if let Some(new_description) = table_description {
                let current_comment = parameters.get("comment").cloned().unwrap_or_default();
                if description.as_deref().unwrap_or_default() != new_description
                    || current_comment != new_description
                {
                    need_update = true;
                }
                parameters.insert("comment".to_string(), new_description.clone());
                description = Some(new_description);
                for (key, value) in table_parameters {
                    if parameters.get(&key) != Some(&value) {
                        need_update = true;
                    }
                    parameters.insert(key, value);
                }
            }

            let mut storage_descriptor = table.storage_descriptor().cloned();
            if (!column_descriptions.is_empty() || !column_parameters.is_empty())
                && let Some(sd) = storage_descriptor.as_mut()
            {
                for column in sd.columns.iter_mut().flatten() {
                    if let Some(comment) = column_descriptions.get(&column.name) {
                        if column.comment.as_deref().unwrap_or_default() != comment {
                            need_update = true;
                        }
                        column.comment = Some(comment.clone());
                    }
                    if let Some(params) = column_parameters.get(&column.name) {
                        let current = column.parameters.get_or_insert_with(HashMap::new);
                        for (key, value) in params {
                            if current.get(key) != Some(value) {
                                need_update = true;
                            }
                            current.insert(key.clone(), value.clone());
                        }
                    }
                }
            }

            if !need_update {
                return Ok(());
            }
            // `TableInput` takes the writable subset of `Table`, as `_get_table_input` does.
            let table_input = TableInput::builder()
                .name(table.name())
                .set_description(description)
                .set_owner(table.owner().map(str::to_string))
                .set_last_access_time(table.last_access_time().cloned())
                .set_last_analyzed_time(table.last_analyzed_time().cloned())
                .set_retention(Some(table.retention()))
                .set_storage_descriptor(storage_descriptor)
                .set_partition_keys(Some(table.partition_keys().to_vec()))
                .set_view_original_text(table.view_original_text().map(str::to_string))
                .set_view_expanded_text(table.view_expanded_text().map(str::to_string))
                .set_table_type(table.table_type().map(str::to_string))
                .set_parameters(Some(parameters))
                .set_target_table(table.target_table().cloned())
                .build()
                .map_err(|e| aws_error("glue.update_table", e))?;
            this.glue
                .update_table()
                .set_catalog_id(catalog_id)
                .database_name(&relation.schema)
                .table_input(table_input)
                .skip_archive(skip_archive_table_version)
                .send()
                .await
                .map_err(|e| aws_error("glue.update_table", e))?;
            Ok(())
        })
    }

    async fn list_object_keys(&self, bucket: &str, prefix: &str) -> AdapterResult<Vec<String>> {
        let mut keys = Vec::new();
        let mut pages = self
            .s3
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .into_paginator()
            .send();
        while let Some(page) = pages.next().await {
            let page = page.map_err(|e| aws_error("s3.list_objects_v2", e))?;
            keys.extend(
                page.contents()
                    .iter()
                    .filter_map(|o| o.key())
                    .map(str::to_string),
            );
        }
        Ok(keys)
    }

    async fn delete_object_keys(&self, bucket: &str, keys: Vec<String>) -> AdapterResult<()> {
        let mut all_successful = true;
        for batch in keys.chunks(BATCH_DELETE_S3_OBJECTS_API_LIMIT) {
            let objects = batch
                .iter()
                .map(|key| {
                    ObjectIdentifier::builder()
                        .key(key)
                        .build()
                        .map_err(|e| aws_error("s3.delete_objects", e))
                })
                .collect::<AdapterResult<Vec<_>>>()?;
            let delete = Delete::builder()
                .set_objects(Some(objects))
                .build()
                .map_err(|e| aws_error("s3.delete_objects", e))?;
            let output = self
                .s3
                .delete_objects()
                .bucket(bucket)
                .delete(delete)
                .send()
                .await
                .map_err(|e| aws_error("s3.delete_objects", e))?;
            for err in output.errors() {
                all_successful = false;
                tracing::error!(
                    "Failed to delete files: Key='{}', Code='{}', Message='{}', s3_bucket='{}'",
                    err.key().unwrap_or_default(),
                    err.code().unwrap_or_default(),
                    err.message().unwrap_or_default(),
                    bucket
                );
            }
        }
        if all_successful {
            Ok(())
        } else {
            Err(AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                "Failed to delete files from S3.",
            ))
        }
    }

    /// `AthenaAdapter.delete_from_s3`
    async fn delete_prefix(&self, s3_path: &str) -> AdapterResult<()> {
        let (bucket, prefix) = parse_s3_path(s3_path);
        let keys = self.list_object_keys(&bucket, &prefix).await?;
        if keys.is_empty() {
            tracing::debug!("S3 path does not exist");
            return Ok(());
        }
        tracing::debug!(
            "Deleting table data: path='{s3_path}', bucket='{bucket}', prefix='{prefix}'"
        );
        self.delete_object_keys(&bucket, keys).await
    }

    /// `AthenaAdapter.bulk_delete_from_s3`
    async fn bulk_delete_prefixes(&self, s3_paths: Vec<String>) -> AdapterResult<()> {
        if s3_paths.is_empty() {
            tracing::debug!("No S3 paths provided for deletion");
            return Ok(());
        }
        let mut keys_by_bucket: HashMap<String, Vec<String>> = HashMap::new();
        for s3_path in &s3_paths {
            let (bucket, prefix) = parse_s3_path(s3_path);
            tracing::debug!("Listing files for deletion: {s3_path}");
            let keys = self.list_object_keys(&bucket, &prefix).await?;
            keys_by_bucket.entry(bucket).or_default().extend(keys);
        }
        for (bucket, keys) in keys_by_bucket {
            if !keys.is_empty() {
                tracing::debug!("Calling delete_objects for {} objects", keys.len());
                self.delete_object_keys(&bucket, keys).await?;
            }
        }
        Ok(())
    }

    pub fn delete_from_s3(self: &Arc<Self>, s3_path: String) -> AdapterResult<()> {
        let this = self.clone();
        block(async move { this.delete_prefix(&s3_path).await })
    }

    /// The upload behind `AthenaAdapter.upload_seed_to_s3`. `upload_args` is the model's
    /// `seed_s3_upload_args` (boto3 `ExtraArgs`); the keys dbt-athena users set are mapped,
    /// anything else is rejected rather than silently dropped.
    pub fn upload_object(
        self: &Arc<Self>,
        bucket: String,
        key: String,
        body: Vec<u8>,
        upload_args: Vec<(String, String)>,
    ) -> AdapterResult<()> {
        let this = self.clone();
        block(async move {
            let mut request = this
                .s3
                .put_object()
                .bucket(&bucket)
                .key(&key)
                .body(ByteStream::from(body));
            for (name, value) in upload_args {
                request = match name.as_str() {
                    "ServerSideEncryption" => request.server_side_encryption(value.as_str().into()),
                    "SSEKMSKeyId" => request.ssekms_key_id(value),
                    "ACL" => request.acl(value.as_str().into()),
                    "StorageClass" => request.storage_class(value.as_str().into()),
                    "ContentType" => request.content_type(value),
                    "BucketKeyEnabled" => request.bucket_key_enabled(value == "true"),
                    other => {
                        return Err(AdapterError::new(
                            AdapterErrorKind::NotSupported,
                            format!(
                                "seed_s3_upload_args key '{other}' is not supported by the dbt Athena backend"
                            ),
                        ));
                    }
                };
            }
            request
                .send()
                .await
                .map_err(|e| aws_error("s3.put_object", e))?;
            Ok(())
        })
    }

    /// `AthenaAdapter.is_work_group_output_location_enforced`, cached per engine like
    /// the Python `lru_cache` on `_get_work_group`.
    pub fn is_work_group_output_location_enforced(self: &Arc<Self>) -> AdapterResult<bool> {
        let this = self.clone();
        block(async move {
            this.output_location_enforced
                .get_or_try_init(|| async {
                    let output = this
                        .athena
                        .get_work_group()
                        .work_group(&this.work_group)
                        .send()
                        .await
                        .map_err(|e| aws_error("athena.get_work_group", e))?;
                    let configuration = output.work_group().and_then(|wg| wg.configuration());
                    let has_output_location = configuration
                        .and_then(|c| c.result_configuration())
                        .and_then(|r| r.output_location())
                        .is_some();
                    let enforced = configuration
                        .and_then(|c| c.enforce_work_group_configuration())
                        .unwrap_or(false);
                    Ok(has_output_location && enforced)
                })
                .await
                .copied()
        })
    }
}
