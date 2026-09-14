use std::sync::Arc;

use crate::materialize::materialize_seed;
use crate::runnable::cache::cache_materialization_return_value;
use dbt_adapter::AdapterType;
use dbt_adapter::relation::create_relation_from_node;
use dbt_agate::AgateTable;
use dbt_common::stats::NodeStatus;
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_df_providers::seed_io::read_parquet_seed_physical;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_jinja_utils::utils::add_task_context;
use dbt_schema_store::read_cached_schema_from_parquet;
use dbt_schemas::schemas::InternalDbtNode;
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{DbtSeed, InternalDbtNodeAttributes};
use dbt_tasks_core::context::TaskRunnerCtx;

use arrow::array::RecordBatch;
use arrow::compute::concat_batches;

/// Checks whether any `column_types` keys are absent from `physical_schema` field names.
/// Returns `true` if mismatches are found. The pre-registration step already emits the
/// human-readable warning log; because that warn fires before the `NodeEvaluated` span opens
/// the middleware cannot stamp `WithWarnings` on it. Callers use this boolean to return
/// `NodeStatus::SucceededWithWarning` so that `on_task_close` stamps `WithWarnings`
/// authoritatively (and avoids calling `set_node_warning_outcome_no_warnings`).
pub fn check_and_warn_missing_column_types(
    seed: &DbtSeed,
    physical_schema: &arrow_schema::SchemaRef,
) -> bool {
    let Some(column_types) = &seed.__seed_attr__.column_types else {
        return false;
    };
    let field_names: std::collections::HashSet<String> = physical_schema
        .fields()
        .iter()
        .map(|f| f.name().to_lowercase())
        .collect();
    let missing: Vec<&str> = column_types
        .keys()
        .map(|k| k.as_ref().as_str())
        .filter(|k| !field_names.contains(&k.to_lowercase()))
        .collect();
    if missing.is_empty() {
        return false;
    }
    true
}

/// Build a hint message when the existing seed table's columns differ from the new CSV.
///
/// https://github.com/dbt-labs/dbt-fusion/issues/862
async fn maybe_seed_column_change_hint(
    seed: &DbtSeed,
    new_cols_sorted: &[String],
    env: &JinjaEnv,
) -> Option<String> {
    let adapter = env.get_base_adapter()?;
    // TODO (harry): Consider whether this logic should apply to other adapters.
    // Adapters that do not require this handling:
    // - DuckDB: The `seed` command ignores local header changes, but overrides the data in the CSV for an existing seed.
    // - BigQuery: Local header changes in `seed` take effect automatically without requiring `--full-refresh`.
    if adapter.adapter_type() != AdapterType::Snowflake {
        return None;
    }
    let metadata_adapter = adapter.metadata_adapter()?;

    let relation: Arc<dyn BaseRelation> =
        create_relation_from_node(adapter.adapter_type(), seed, None)
            .ok()?
            .into();
    let semantic_fqn = relation.semantic_fqn();
    let relations = [Arc::clone(&relation)];

    let schemas = metadata_adapter
        .list_relations_schemas(
            Some(seed.unique_id()),
            None,
            &relations,
            None,
            adapter.cancellation_token(),
        )
        .await
        .ok()?;
    let schema = schemas.get(&semantic_fqn).and_then(|r| r.as_ref().ok())?;

    let mut existing: Vec<String> = schema
        .fields()
        .iter()
        .map(|f| f.name().to_lowercase())
        .collect();
    if existing.is_empty() {
        return None;
    }
    existing.sort();

    if existing == new_cols_sorted {
        return None;
    }

    Some(format!(
        "Seed '{}' has column schema changes that require a full refresh.\n  Existing columns: {:?}\n  New CSV columns:  {:?}\nRe-run with `--full-refresh` to drop and recreate the table.",
        seed.name(),
        existing,
        new_cols_sorted,
    ))
}

/// Chained error marker parsed by [`maybe_resolve_remote_seed_column_hint`].
/// Kept off [`FsError`]'s main `Display` output (only `next` is walked for multi-error tooling).
const PENDING_SEED_COLUMN_HINT_PREFIX: &str = "__DBT_PENDING_SEED_COLUMN_HINT__:";

fn chain_materialize_seed_error_with_pending_hint(
    error: Box<FsError>,
    new_cols_sorted: Vec<String>,
) -> Box<FsError> {
    let Ok(json) = serde_json::to_string(&new_cols_sorted) else {
        return error;
    };
    let marker = FsError::new(
        ErrorCode::Generic,
        format!("{PENDING_SEED_COLUMN_HINT_PREFIX}{json}"),
    );
    Box::new((*error).with_chained_errors(Box::new(marker)))
}

/// After remote seed execution: if the result is a seed error carrying a chained Snowflake
/// column-hint marker ([`chain_materialize_seed_error_with_pending_hint`]), resolve it and strip the marker.
pub async fn maybe_resolve_remote_seed_column_hint(
    res: FsResult<NodeStatus>,
    node: &dyn InternalDbtNodeAttributes,
    ctx: &TaskRunnerCtx,
) -> FsResult<NodeStatus> {
    let Err(mut err) = res else {
        return res;
    };
    let Some(seed) = node.as_any().downcast_ref::<DbtSeed>() else {
        return Err(err);
    };
    let Some(marker) = err.pop_next() else {
        return Err(err);
    };
    if !marker.context.starts_with(PENDING_SEED_COLUMN_HINT_PREFIX) {
        return Err(Box::new((*err).with_chained_errors(marker)));
    }
    let json = &marker.context[PENDING_SEED_COLUMN_HINT_PREFIX.len()..];
    let Ok(cols) = serde_json::from_str::<Vec<String>>(json) else {
        return Err(Box::new((*err).with_chained_errors(marker)));
    };
    enrich_seed_error_with_column_hint(err.as_mut(), seed, &cols, &ctx.env).await;
    Err(err)
}

async fn enrich_seed_error_with_column_hint(
    err: &mut FsError,
    seed: &DbtSeed,
    new_cols_sorted: &[String],
    env: &JinjaEnv,
) {
    if let Some(hint) = maybe_seed_column_change_hint(seed, new_cols_sorted, env).await {
        if !err.context.is_empty() {
            err.context.push_str("\n\n");
        }
        err.context.push_str(&hint);
    }
}

/// Lowercased, sorted column names — cheap enough to compute up-front so we can hand
/// `agate_table` off to `materialize_seed` (which consumes it) and still recover the
/// CSV's columns later if we need to enrich an error.
fn sorted_lowercase_columns(agate_table: &AgateTable) -> Vec<String> {
    let mut cols: Vec<String> = agate_table
        .column_names()
        .into_iter()
        .map(|s| s.to_lowercase())
        .collect();
    cols.sort();
    cols
}

pub fn execute_seed_remote(seed: &DbtSeed, ctx: &TaskRunnerCtx) -> FsResult<NodeStatus> {
    let mut base_context = ctx.inner.base_context.clone();

    add_task_context(&mut base_context, seed.common(), &ctx.thread_id);

    let relation = create_relation_from_node(seed.node_adapter(), seed, None)?;
    let canonical_fqn = relation.get_canonical_fqn()?;

    // Read seed parquet file synchronously via Arrow reader (no DataFusion needed).
    let parquet_path = ctx.data_store.get_path_to_data(&canonical_fqn);
    let (physical_schema_entry, _timestamp) = read_cached_schema_from_parquet(&parquet_path)?;
    let had_warning = check_and_warn_missing_column_types(seed, physical_schema_entry.inner());
    let agate_table = read_parquet_to_agate_table(&parquet_path)?;

    // Under `--empty` we zero out the rows here so the materialization Jinja
    // can gate on row count rather than the CLI flag. The schema is preserved.
    let agate_table = if ctx.inner.arg.empty {
        agate_table.limit(0).map_err(|e| {
            fs_err!(
                ErrorCode::IoError,
                "Failed to build empty agate for seed {}: {}",
                seed.name(),
                e
            )
        })?
    } else {
        agate_table
    };

    let is_full_refresh =
        ctx.inner.arg.full_refresh || seed.deprecated_config.full_refresh.unwrap_or(false);
    // Capture the CSV columns now so we can still reference them after `materialize_seed`
    // takes ownership of `agate_table`, in case we need to enrich a downstream error.
    let new_cols_sorted = sorted_lowercase_columns(&agate_table);

    let (relations_map, main_response) = match materialize_seed(
        seed,
        seed.node_adapter(),
        ctx.runtime_config(),
        &ctx.inner.materialization_resolver,
        ctx.env.clone(),
        &base_context,
        agate_table,
        &ctx.inner.arg.io,
    ) {
        Ok(r) => r,
        Err(e) => {
            return Err(if !is_full_refresh {
                chain_materialize_seed_error_with_pending_hint(e, new_cols_sorted)
            } else {
                e
            });
        }
    };
    if let Some(main_response) = main_response {
        ctx.inner
            .main_adapter_responses
            .insert(seed.__common_attr__.unique_id.clone(), main_response);
    }
    let _ = cache_materialization_return_value(ctx.env.clone(), &relations_map);

    if had_warning {
        Ok(NodeStatus::SucceededWithWarning)
    } else {
        Ok(NodeStatus::Succeeded)
    }
}

/// Read a parquet file into an `AgateTable` synchronously.
pub fn read_parquet_to_agate_table(path: &std::path::Path) -> FsResult<AgateTable> {
    let (schema, batches) = read_parquet_seed_physical(path).map_err(|e| {
        fs_err!(
            ErrorCode::ParquetError,
            "Failed to read parquet file {}: {}",
            path.display(),
            e
        )
    })?;

    let batch = if batches.len() == 1 {
        batches.into_iter().next().unwrap()
    } else if batches.len() > 1 {
        concat_batches(&schema, &batches)?
    } else {
        RecordBatch::new_empty(schema)
    };
    Ok(AgateTable::from_record_batch(Arc::new(batch)))
}
