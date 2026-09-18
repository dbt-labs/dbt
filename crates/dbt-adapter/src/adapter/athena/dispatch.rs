//! `adapter.<method>` entry points for the Athena-only methods.
//!
//! Signatures follow `AthenaAdapter` in dbt-athena's `impl.py`; the macro package
//! calls them positionally and by keyword, so every parameter is named. AWS-backed
//! methods return `None` (or the falsy default) in the parse phase, like the other
//! adapters' side-effecting methods.

use super::driver_ops::{AthenaOps, RelationParts};
use super::{
    S3DataNaming, clean_sql_comment, ellipsis_comment, format_one_partition_key,
    format_partition_keys, format_value_for_partition, generate_s3_location,
    is_valid_table_parameter_key, murmur3_hash, parse_s3_path, s3_table_prefix,
    stringify_table_parameter_value,
};
use crate::adapter::Adapter;
use crate::adapter::InnerAdapter::{Parse, Typed};
use crate::cast_util::downcast_value_to_dyn_base_relation;
use crate::errors::{AdapterError, AdapterErrorKind};
use crate::value::none_value;
use dbt_agate::AgateTable;
use dbt_auth::AdapterConfig;
use minijinja::arg_utils::ArgsIter;
use minijinja::value::ValueKind;
use minijinja::{Error as JinjaError, ErrorKind as JinjaErrorKind, State, Value};
use std::collections::HashMap;

fn invalid(msg: impl Into<String>) -> JinjaError {
    JinjaError::new(JinjaErrorKind::InvalidOperation, msg.into())
}

fn relation_parts(value: &Value) -> Result<RelationParts, JinjaError> {
    let relation = downcast_value_to_dyn_base_relation(value)?;
    Ok(RelationParts {
        database: relation.database().map(str::to_string),
        schema: relation.schema_as_str()?,
        identifier: relation.identifier_as_str()?,
        s3_path_table_part: crate::relation::s3_path_table_part(relation.as_ref()),
        rendered: relation.render_self_as_str(),
    })
}

fn optional_str(value: Option<&Value>) -> Option<String> {
    value
        .filter(|v| !v.is_none() && !v.is_undefined())
        .map(|v| v.to_string())
        .filter(|s| !s.is_empty())
}

fn map_entries(value: &Value) -> Result<Vec<(String, Value)>, JinjaError> {
    if value.is_none() || value.is_undefined() {
        return Ok(Vec::new());
    }
    if value.kind() != ValueKind::Map {
        return Err(invalid(format!("expected a mapping, got {}", value.kind())));
    }
    value
        .try_iter()?
        .map(|key| Ok((key.to_string(), value.get_item(&key)?)))
        .collect()
}

/// `int(to_keep)`: the macros pass `versions_to_keep` straight from `config`, which
/// may be a number or a string.
fn to_usize(value: &Value, name: &str) -> Result<usize, JinjaError> {
    if let Some(n) = value.as_i64() {
        return usize::try_from(n).map_err(|_| invalid(format!("{name} must be >= 0, got {n}")));
    }
    if let Some(s) = value.as_str() {
        return s
            .trim()
            .parse::<usize>()
            .map_err(|_| invalid(format!("{name} must be an integer, got '{s}'")));
    }
    Err(invalid(format!(
        "{name} must be an integer, got {}",
        value.kind()
    )))
}

fn config_error(msg: &str) -> JinjaError {
    AdapterError::new(AdapterErrorKind::Configuration, msg).into()
}

impl Adapter {
    fn profile_config(&self) -> &AdapterConfig {
        match &self.inner {
            Typed { adapter, .. } => adapter.engine().get_config(),
            Parse(parse_state) => parse_state.engine.get_config(),
        }
    }

    fn athena_ops<'s, 't, 'e>(&'s self, state: &'s State<'t, 'e>) -> Option<AthenaOps<'s, 't, 'e>> {
        match &self.inner {
            Typed { adapter, .. } => {
                let engine = adapter.engine();
                Some(AthenaOps::new(
                    self,
                    state,
                    engine.fingerprint(),
                    engine.get_config().get_str("work_group"),
                ))
            }
            Parse(_) => None,
        }
    }

    /// `adapter.is_list(value)`
    pub fn athena_is_list(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("is_list", &["value"], args);
        let value = iter.next_arg::<&Value>()?;
        iter.finish()?;
        Ok(Value::from(value.kind() == ValueKind::Seq))
    }

    /// `adapter.format_value_for_partition(value, column_type)` -> `(value, operator)`
    pub fn athena_format_value_for_partition(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "format_value_for_partition",
            &["value", "column_type"],
            args,
        );
        let value = iter.next_arg::<&Value>()?;
        let column_type = iter.next_arg::<&str>()?;
        iter.finish()?;
        let (rendered, operator) =
            format_value_for_partition(value, column_type).map_err(invalid)?;
        Ok(Value::from(vec![
            Value::from(rendered),
            Value::from(operator),
        ]))
    }

    /// `adapter.format_one_partition_key(partition_key)`
    pub fn athena_format_one_partition_key(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("format_one_partition_key", &["partition_key"], args);
        let partition_key = iter.next_arg::<&str>()?;
        iter.finish()?;
        Ok(Value::from(format_one_partition_key(partition_key)))
    }

    /// `adapter.format_partition_keys(partition_keys)`
    pub fn athena_format_partition_keys(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("format_partition_keys", &["partition_keys"], args);
        let keys = iter.next_arg::<&Value>()?;
        iter.finish()?;
        let keys = keys.try_iter()?.map(|k| k.to_string()).collect::<Vec<_>>();
        Ok(Value::from(format_partition_keys(
            keys.iter().map(String::as_str),
        )))
    }

    /// `adapter.murmur3_hash(value, num_buckets)`
    pub fn athena_murmur3_hash(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("murmur3_hash", &["value", "num_buckets"], args);
        let value = iter.next_arg::<&Value>()?;
        let num_buckets = iter.next_arg::<i64>()?;
        iter.finish()?;
        Ok(Value::from(
            murmur3_hash(value, num_buckets).map_err(invalid)?,
        ))
    }

    fn s3_location(
        &self,
        relation: &RelationParts,
        s3_data_dir: Option<String>,
        s3_data_naming: Option<String>,
        s3_tmp_table_dir: Option<String>,
        external_location: Option<String>,
        is_temporary_table: bool,
    ) -> Result<String, JinjaError> {
        let config = self.profile_config();
        let s3_staging_dir = config.get_str("s3_staging_dir").ok_or_else(|| {
            config_error("Athena requires 's3_staging_dir' in profile configuration")
        })?;
        let naming = match s3_data_naming
            .as_deref()
            .or_else(|| config.get_str("s3_data_naming"))
        {
            Some(name) => S3DataNaming::parse(name)
                .ok_or_else(|| invalid(format!("'{name}' is not a valid s3_data_naming")))?,
            None => S3DataNaming::TableUnique,
        };
        let prefix = s3_table_prefix(
            s3_staging_dir,
            config.get_str("s3_tmp_table_dir"),
            s3_data_dir.as_deref(),
            s3_tmp_table_dir.as_deref(),
            is_temporary_table,
        );
        Ok(generate_s3_location(
            &relation.schema,
            relation
                .s3_path_table_part
                .as_deref()
                .unwrap_or(&relation.identifier),
            &prefix,
            naming,
            external_location.as_deref(),
            is_temporary_table,
            &uuid::Uuid::new_v4().to_string(),
        ))
    }

    /// `adapter.generate_s3_location(relation, s3_data_dir, s3_data_naming, s3_tmp_table_dir, external_location, is_temporary_table)`
    pub fn athena_generate_s3_location(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "generate_s3_location",
            &[
                "relation",
                "s3_data_dir",
                "s3_data_naming",
                "s3_tmp_table_dir",
                "external_location",
                "is_temporary_table",
            ],
            args,
        );
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let s3_data_dir = optional_str(iter.next_arg::<Option<&Value>>()?);
        let s3_data_naming = optional_str(iter.next_arg::<Option<&Value>>()?);
        let s3_tmp_table_dir = optional_str(iter.next_arg::<Option<&Value>>()?);
        let external_location = optional_str(iter.next_arg::<Option<&Value>>()?);
        let is_temporary_table = iter
            .next_arg::<Option<&Value>>()?
            .is_some_and(Value::is_true);
        iter.finish()?;
        Ok(Value::from(self.s3_location(
            &relation,
            s3_data_dir,
            s3_data_naming,
            s3_tmp_table_dir,
            external_location,
            is_temporary_table,
        )?))
    }

    /// `adapter.get_glue_table_type(relation)` -> `TableType | None`
    pub fn athena_get_glue_table_type(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("get_glue_table_type", &["relation"], args);
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        iter.finish()?;
        let Some(aws) = self.athena_ops(state) else {
            return Ok(none_value());
        };
        Ok(aws
            .glue_table(&relation)?
            .map(|table| table.table_type.to_jinja())
            .unwrap_or_else(none_value))
    }

    /// `adapter.clean_up_table(relation)`: delete the table's S3 data, if it has any.
    pub fn athena_clean_up_table(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("clean_up_table", &["relation"], args);
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        iter.finish()?;
        if let Some(aws) = self.athena_ops(state)
            && let Some(location) = aws.glue_table_location(&relation)?
        {
            aws.delete_from_s3(&location)?;
        }
        Ok(none_value())
    }

    /// `adapter.delete_from_glue_catalog(relation)`
    pub fn athena_delete_from_glue_catalog(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("delete_from_glue_catalog", &["relation"], args);
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        iter.finish()?;
        if let Some(aws) = self.athena_ops(state) {
            aws.delete_from_glue_catalog(&relation)?;
        }
        Ok(none_value())
    }

    /// `adapter.drop_glue_database(database_name, catalog_name='awsdatacatalog')`
    pub fn athena_drop_glue_database(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "drop_glue_database",
            &["database_name", "catalog_name"],
            args,
        );
        let database_name = iter.next_arg::<&str>()?.to_string();
        let catalog_name = iter
            .next_arg::<Option<&str>>()?
            .unwrap_or("awsdatacatalog")
            .to_string();
        iter.finish()?;
        if let Some(aws) = self.athena_ops(state) {
            aws.drop_glue_database(&database_name, &catalog_name)?;
        }
        Ok(none_value())
    }

    /// `adapter.expire_glue_table_versions(relation, to_keep, delete_s3)` -> deleted version ids
    pub fn athena_expire_glue_table_versions(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "expire_glue_table_versions",
            &["relation", "to_keep", "delete_s3"],
            args,
        );
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let to_keep = to_usize(iter.next_arg::<&Value>()?, "to_keep")?;
        let delete_s3 = iter.next_arg::<bool>()?;
        iter.finish()?;
        let Some(aws) = self.athena_ops(state) else {
            return Ok(Value::from(Vec::<Value>::new()));
        };
        let deleted = aws.expire_glue_table_versions(&relation, to_keep, delete_s3)?;
        Ok(Value::from(deleted))
    }

    /// `adapter.swap_table(src_relation, target_relation)`
    pub fn athena_swap_table(&self, state: &State, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("swap_table", &["src_relation", "target_relation"], args);
        let src = relation_parts(iter.next_arg::<&Value>()?)?;
        let target = relation_parts(iter.next_arg::<&Value>()?)?;
        iter.finish()?;
        if let Some(aws) = self.athena_ops(state) {
            aws.swap_table(&src, &target)?;
        }
        Ok(none_value())
    }

    /// `adapter.clean_up_partitions(relation, where_condition: str | list[str])`
    pub fn athena_clean_up_partitions(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "clean_up_partitions",
            &["relation", "where_condition"],
            args,
        );
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let where_condition = iter.next_arg::<&Value>()?;
        iter.finish()?;
        let conditions = if let Some(single) = where_condition.as_str() {
            vec![single.to_string()]
        } else {
            where_condition.try_iter()?.map(|c| c.to_string()).collect()
        };
        if let Some(aws) = self.athena_ops(state) {
            aws.clean_up_partitions(&relation, conditions)?;
        }
        Ok(none_value())
    }

    /// `adapter.delete_from_s3(s3_path)`
    pub fn athena_delete_from_s3(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("delete_from_s3", &["s3_path"], args);
        let s3_path = iter.next_arg::<&str>()?.to_string();
        iter.finish()?;
        if let Some(aws) = self.athena_ops(state) {
            aws.delete_from_s3(&s3_path)?;
        }
        Ok(none_value())
    }

    /// `adapter.is_work_group_output_location_enforced()`
    pub fn athena_is_work_group_output_location_enforced(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        ArgsIter::nullary("is_work_group_output_location_enforced", args).finish()?;
        match self.athena_ops(state) {
            Some(aws) => Ok(Value::from(aws.is_work_group_output_location_enforced()?)),
            None => Ok(Value::from(false)),
        }
    }

    /// `adapter.upload_seed_to_s3(relation, table, s3_data_dir, s3_data_naming, external_location, seed_s3_upload_args)`
    /// -> the S3 prefix the CSV was written under. The CSV carries a header row; fields
    /// are quoted where the CSV grammar needs it, which OpenCSVSerde reads the same as
    /// agate's `QUOTE_NONNUMERIC` output.
    pub fn athena_upload_seed_to_s3(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "upload_seed_to_s3",
            &[
                "relation",
                "table",
                "s3_data_dir",
                "s3_data_naming",
                "external_location",
                "seed_s3_upload_args",
            ],
            args,
        );
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let table = iter
            .next_arg::<&Value>()?
            .downcast_object::<AgateTable>()
            .ok_or_else(|| invalid("upload_seed_to_s3: table must be an agate.Table"))?;
        let s3_data_dir = optional_str(iter.next_arg::<Option<&Value>>()?);
        let s3_data_naming = optional_str(iter.next_arg::<Option<&Value>>()?);
        let external_location = optional_str(iter.next_arg::<Option<&Value>>()?);
        let upload_args = iter
            .next_arg::<Option<&Value>>()?
            .map(map_entries)
            .transpose()?
            .unwrap_or_default()
            .into_iter()
            .map(|(k, v)| (k, v.to_string()))
            .collect::<Vec<_>>();
        iter.finish()?;

        let Some(aws) = self.athena_ops(state) else {
            return Ok(none_value());
        };
        let location = self.s3_location(
            &relation,
            s3_data_dir,
            s3_data_naming,
            None,
            external_location,
            false,
        )?;
        let (bucket, prefix) = parse_s3_path(&location);
        let key = format!("{prefix}{}.csv", relation.identifier);

        let batch = table.original_record_batch();
        let mut body: Vec<u8> = Vec::new();
        let mut writer = arrow::csv::WriterBuilder::new()
            .with_header(true)
            .with_date_format("%Y-%m-%d".to_string())
            .with_timestamp_format("%Y-%m-%d %H:%M:%S%.f".to_string())
            .build(&mut body);
        writer
            .write(&batch)
            .map_err(|e| invalid(format!("upload_seed_to_s3: failed to format CSV: {e}")))?;
        drop(writer);

        aws.upload_object(&bucket, &key, &body, upload_args)?;
        Ok(Value::from(location))
    }

    /// `adapter.run_query_with_partitions_limit_catching(sql)`: `'TOO_MANY_OPEN_PARTITIONS'`
    /// when Athena rejects the statement for that reason, otherwise a JSON string with
    /// the row count and bytes scanned.
    pub fn athena_run_query_with_partitions_limit_catching(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("run_query_with_partitions_limit_catching", &["sql"], args);
        let sql = iter.next_arg::<&str>()?;
        iter.finish()?;
        if matches!(&self.inner, Parse(_)) {
            return Ok(none_value());
        }
        match self.execute(state, None, sql, false, false, None, None) {
            Ok((response, _)) => Ok(Value::from(format!(
                r#"{{"rowcount":{},"data_scanned_in_bytes":{}}}"#,
                response.rows_affected_i64(),
                response.bytes_processed().unwrap_or(0)
            ))),
            Err(e) if e.message().contains("TOO_MANY_OPEN_PARTITIONS") => {
                Ok(Value::from("TOO_MANY_OPEN_PARTITIONS"))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// `adapter.run_operation_with_potential_multiple_runs(query, op)`: rerun an Iceberg
    /// OPTIMIZE / VACUUM while Athena answers `ICEBERG_<OP>_MORE_RUNS_NEEDED`.
    pub fn athena_run_operation_with_potential_multiple_runs(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "run_operation_with_potential_multiple_runs",
            &["query", "op"],
            args,
        );
        let query = iter.next_arg::<&str>()?;
        let op = iter.next_arg::<&str>()?;
        iter.finish()?;
        if matches!(&self.inner, Parse(_)) {
            return Ok(none_value());
        }
        let more_runs_needed = format!("ICEBERG_{}_MORE_RUNS_NEEDED", op.to_ascii_uppercase());
        loop {
            match self.execute(state, None, query, false, false, None, None) {
                Ok(_) => return Ok(none_value()),
                Err(e) if e.message().contains(&more_runs_needed) => continue,
                Err(e) => return Err(e.into()),
            }
        }
    }

    /// `adapter.persist_docs_to_glue(relation, model, persist_relation_docs, persist_column_docs, skip_archive_table_version)`
    pub fn athena_persist_docs_to_glue(
        &self,
        state: &State,
        args: &[Value],
    ) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new(
            "persist_docs_to_glue",
            &[
                "relation",
                "model",
                "persist_relation_docs",
                "persist_column_docs",
                "skip_archive_table_version",
            ],
            args,
        );
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let model = iter.next_arg::<&Value>()?;
        let persist_relation_docs = iter.next_arg::<Option<bool>>()?.unwrap_or(false);
        let persist_column_docs = iter.next_arg::<Option<bool>>()?.unwrap_or(false);
        let skip_archive_table_version = iter.next_arg::<Option<bool>>()?.unwrap_or(false);
        iter.finish()?;

        let Some(aws) = self.athena_ops(state) else {
            return Ok(none_value());
        };

        let model_config = model.get_attr("config").unwrap_or(Value::UNDEFINED);
        let mut table_description = None;
        let mut table_parameters = Vec::new();
        if persist_relation_docs {
            let description = model.get_attr("description").unwrap_or(Value::UNDEFINED);
            table_description = Some(ellipsis_comment(
                &clean_sql_comment(&optional_str(Some(&description)).unwrap_or_default()),
                2048,
            ));
            let mut meta = map_entries(&model_config.get_attr("meta").unwrap_or(Value::UNDEFINED))?;
            meta.push((
                "unique_id".to_string(),
                model.get_attr("unique_id").unwrap_or(Value::UNDEFINED),
            ));
            meta.push((
                "materialized".to_string(),
                model_config
                    .get_attr("materialized")
                    .unwrap_or(Value::UNDEFINED),
            ));
            if let Some(project_name) = state.lookup("project_name", &[]) {
                meta.push(("dbt_project_name".to_string(), project_name));
            }
            for (key, value) in meta {
                if !is_valid_table_parameter_key(&key) {
                    tracing::warn!("Meta key '{key}' is not supported and will be ignored");
                    continue;
                }
                match stringify_table_parameter_value(&value) {
                    Some(value) => table_parameters.push((key, value)),
                    None => tracing::warn!(
                        "Meta value for key '{key}' is not supported and will be ignored"
                    ),
                }
            }
        }

        let mut column_descriptions = HashMap::new();
        let mut column_parameters: HashMap<String, Vec<(String, String)>> = HashMap::new();
        if persist_column_docs {
            let columns = model.get_attr("columns").unwrap_or(Value::UNDEFINED);
            for (name, column) in map_entries(&columns)? {
                let description = column.get_attr("description").unwrap_or(Value::UNDEFINED);
                column_descriptions.insert(
                    name.clone(),
                    ellipsis_comment(
                        &clean_sql_comment(&optional_str(Some(&description)).unwrap_or_default()),
                        255,
                    ),
                );
                for (key, value) in
                    map_entries(&column.get_attr("meta").unwrap_or(Value::UNDEFINED))?
                {
                    if !is_valid_table_parameter_key(&key) {
                        tracing::warn!(
                            "Column meta key '{key}' is not supported and will be ignored"
                        );
                        continue;
                    }
                    match stringify_table_parameter_value(&value) {
                        Some(value) => column_parameters
                            .entry(name.clone())
                            .or_default()
                            .push((key, value)),
                        None => tracing::warn!(
                            "Column meta value for key '{key}' is not supported and will be ignored"
                        ),
                    }
                }
            }
        }

        aws.persist_docs_to_glue(
            &relation,
            table_description,
            table_parameters,
            column_descriptions,
            column_parameters,
            skip_archive_table_version,
        )?;
        Ok(none_value())
    }

    /// `adapter.add_lf_tags(relation, lf_tags_config)`. Lake Formation tagging is not
    /// ported yet: a disabled config is a no-op as in dbt-athena, an enabled one is an error.
    pub fn athena_add_lf_tags(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("add_lf_tags", &["relation", "lf_tags_config"], args);
        let relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let config = iter.next_arg::<&Value>()?;
        iter.finish()?;
        let enabled = config.get_attr("enabled").is_ok_and(|v| v.is_true());
        if !enabled {
            tracing::debug!("Lakeformation is disabled for {}", relation.rendered);
            return Ok(none_value());
        }
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "lf_tags_config is not yet supported by the dbt Athena backend",
        )
        .into())
    }

    /// `adapter.apply_lf_grants(relation, lf_grants_config)`; see [`Self::athena_add_lf_tags`].
    pub fn athena_apply_lf_grants(&self, args: &[Value]) -> Result<Value, JinjaError> {
        let iter = ArgsIter::new("apply_lf_grants", &["relation", "lf_grants_config"], args);
        let _relation = relation_parts(iter.next_arg::<&Value>()?)?;
        let config = iter.next_arg::<&Value>()?;
        iter.finish()?;
        let enabled = config
            .get_attr("data_cell_filters")
            .and_then(|f| f.get_attr("enabled"))
            .is_ok_and(|v| v.is_true());
        if !enabled {
            return Ok(none_value());
        }
        Err(AdapterError::new(
            AdapterErrorKind::NotSupported,
            "lf_grants is not yet supported by the dbt Athena backend",
        )
        .into())
    }
}
