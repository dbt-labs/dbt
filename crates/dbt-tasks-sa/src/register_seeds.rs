use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Schema, SchemaRef};
use dbt_adapter::column::ColumnStatic;
use dbt_adapter::relation::create_relation_from_node;
use dbt_adapter::sql_types::TypeOps;
use dbt_adapter_core::AdapterType;
use dbt_common::tracing::{dbt_emit::emit_warn_log_message, spawn_traced};
use dbt_common::{ErrorCode, FsResult, fs_err};
use dbt_csv::{CustomCsvOptions, read_to_arrow_records};
use dbt_df_providers::seed_io::{
    TableFormat, adapt_schema, infer_seed_column_name_strategy, read_json_seed,
    read_parquet_seed_view,
};

use dbt_frontend_common::Dialect;
use dbt_schema_store::{CanonicalFqn, DataStoreTrait, SchemaStoreTrait};
use dbt_schemas::schemas::DbtSeed;
use dbt_schemas::schemas::InternalDbtNodeAttributes;
use dbt_yaml::Spanned;

/// Result of registering a single seed: the canonical FQN, adapted schema, and unique_id.
/// Used by the caller to handle local-mode schema mirroring.
pub struct RegisteredSeed {
    pub canonical_fqn: CanonicalFqn,
    pub schema: SchemaRef,
    pub unique_id: String,
}

/// Shared context for seed registration, avoiding a dependency on TaskRunnerCtx.
struct SeedRegistrationCtx {
    schema_cache: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
    in_dir: PathBuf,
}

pub fn resolve_seed_path(in_dir: &Path, seed: &DbtSeed) -> PathBuf {
    seed.__seed_attr__
        .root_path
        .as_ref()
        .map(|root| root.join(&seed.__common_attr__.path))
        .unwrap_or_else(|| in_dir.join(&seed.__common_attr__.original_file_path))
}

/// Synchronously register a CSV seed.
///
/// Intended to run inside `dbt_runtime::spawn_blocking`. The seed's schema is
/// registered in the schema cache and its data is written to the data store.
fn register_seed_csv(
    seed: Arc<DbtSeed>,
    ctx: Arc<SeedRegistrationCtx>,
    type_ops: Arc<dyn TypeOps>,
) -> FsResult<RegisteredSeed> {
    let adapter_type = seed.node_adapter();
    let relation = create_relation_from_node(adapter_type, seed.as_ref(), None)?;
    let canonical_fqn = relation.get_canonical_fqn()?;
    let delimiter = seed
        .__seed_attr__
        .delimiter
        .as_ref()
        .map(|s| s.chars().next().unwrap_or(','))
        .unwrap_or(',');
    let seed_path = resolve_seed_path(&ctx.in_dir, &seed);
    let infer_column_name_strategy =
        infer_seed_column_name_strategy(seed.__seed_attr__.quote_columns, adapter_type);

    let text_columns: Vec<String> = seed
        .__seed_attr__
        .column_types
        .as_ref()
        .map(|ct| ct.keys().map(|k| k.as_ref().clone()).collect())
        .unwrap_or_default();
    let options = CustomCsvOptions::default()
        .with_delimiter(delimiter as u8)
        .with_text_columns(text_columns);
    let result = read_to_arrow_records(&seed_path, &options)?;
    if !result.unmatched_text_columns.is_empty() {
        emit_warn_log_message(
            ErrorCode::SeedColumnTypeMismatch,
            format!(
                "Columns specified in column_types were not found in seed CSV header: {:?}",
                result.unmatched_text_columns
            ),
        );
    }

    let logical_fields = if let Some(provided_types) = &seed.__seed_attr__.column_types {
        Some(parse_provided_types(provided_types, type_ops.as_ref())?)
    } else {
        None
    };
    let target_schema = adapt_schema(result.schema, infer_column_name_strategy);
    let schema =
        dialect_adapted_schema(&target_schema, logical_fields.as_ref(), type_ops.as_ref())?;
    ctx.schema_cache
        .register_schema(&canonical_fqn, None, schema.clone(), true)?;
    ctx.data_store
        .persist_data(&canonical_fqn, target_schema, result.batches)?;

    Ok(RegisteredSeed {
        canonical_fqn,
        schema,
        unique_id: seed.__common_attr__.unique_id.clone(),
    })
}

/// Synchronously register a JSON seed.
///
/// Intended to run inside `dbt_runtime::spawn_blocking`.
fn register_seed_json(
    seed: Arc<DbtSeed>,
    ctx: Arc<SeedRegistrationCtx>,
    type_ops: Arc<dyn TypeOps>,
) -> FsResult<RegisteredSeed> {
    let adapter_type = seed.node_adapter();
    let relation = create_relation_from_node(adapter_type, seed.as_ref(), None)?;
    let canonical_fqn = relation.get_canonical_fqn()?;
    let seed_path = resolve_seed_path(&ctx.in_dir, &seed);
    let infer_column_name_strategy =
        infer_seed_column_name_strategy(seed.__seed_attr__.quote_columns, adapter_type);

    let (source_schema, maybe_batches) = read_json_seed(&seed_path, true)?;
    let logical_fields = if let Some(provided_types) = &seed.__seed_attr__.column_types {
        Some(parse_provided_types(provided_types, type_ops.as_ref())?)
    } else {
        None
    };
    let target_schema = adapt_schema(source_schema, infer_column_name_strategy);
    let schema =
        dialect_adapted_schema(&target_schema, logical_fields.as_ref(), type_ops.as_ref())?;
    ctx.schema_cache
        .register_schema(&canonical_fqn, None, schema.clone(), true)?;

    if let Some(batches) = maybe_batches {
        ctx.data_store
            .persist_data(&canonical_fqn, target_schema, batches)?;
    }

    Ok(RegisteredSeed {
        canonical_fqn,
        schema,
        unique_id: seed.__common_attr__.unique_id.clone(),
    })
}

/// Register a parquet seed using async parquet I/O.
/// (Data persistence is sync, but it runs on ablocking pool.)
async fn register_seed_parquet_async(
    seed: Arc<DbtSeed>,
    ctx: Arc<SeedRegistrationCtx>,
    type_ops: Arc<dyn TypeOps>,
) -> FsResult<RegisteredSeed> {
    let adapter_type = seed.node_adapter();
    let relation = create_relation_from_node(adapter_type, seed.as_ref(), None)?;
    let canonical_fqn = relation.get_canonical_fqn()?;
    let seed_path = resolve_seed_path(&ctx.in_dir, &seed);
    let infer_column_name_strategy =
        infer_seed_column_name_strategy(seed.__seed_attr__.quote_columns, adapter_type);

    let (source_schema, maybe_batches) = read_parquet_seed_view(&seed_path, true).await?;
    let logical_fields = if let Some(provided_types) = &seed.__seed_attr__.column_types {
        Some(parse_provided_types(provided_types, type_ops.as_ref())?)
    } else {
        None
    };
    let target_schema = adapt_schema(source_schema, infer_column_name_strategy);
    let schema =
        dialect_adapted_schema(&target_schema, logical_fields.as_ref(), type_ops.as_ref())?;
    ctx.schema_cache
        .register_schema(&canonical_fqn, None, schema.clone(), true)?;

    if let Some(batches) = maybe_batches {
        let cfqn = canonical_fqn.clone();
        let ts = target_schema.clone();
        let dsc = Arc::clone(&ctx);
        match dbt_runtime::spawn_blocking(move || dsc.data_store.persist_data(&cfqn, ts, batches))
            .await
        {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => return Err(e.into()),
            Err(j) => {
                return Err(fs_err!(
                    ErrorCode::Generic,
                    "seed persist task panicked: {j}"
                ));
            }
        }
    }

    Ok(RegisteredSeed {
        canonical_fqn,
        schema,
        unique_id: seed.__common_attr__.unique_id.clone(),
    })
}

/// Pre-register scheduled seeds before the task graph runs.
/// (CSV, JSON are on blocking workers; parquet is async)
///
/// Errors are logged immediately but do not abort the run. Failed seeds will
/// be detected during the render phase when the schema store check fails.
#[allow(clippy::too_many_arguments)]
pub async fn pre_register_seeds(
    sorted_nodes: &[&String],
    seeds: &BTreeMap<String, Arc<DbtSeed>>,
    schema_cache: Arc<dyn SchemaStoreTrait>,
    data_store: Arc<dyn DataStoreTrait>,
    type_ops: Arc<dyn TypeOps>,
    in_dir: &Path,
) -> Vec<RegisteredSeed> {
    let ctx = Arc::new(SeedRegistrationCtx {
        schema_cache,
        data_store,
        in_dir: in_dir.to_path_buf(),
    });

    let mut handles: Vec<Pin<Box<dyn Future<Output = FsResult<RegisteredSeed>> + Send>>> =
        Vec::new();

    for unique_id in sorted_nodes {
        let Some(seed) = seeds.get(*unique_id) else {
            continue;
        };
        let seed = Arc::clone(seed);
        // Extension dispatch only — the actual read uses `resolve_seed_path`
        // inside the per-format function, which honors `seed.__seed_attr__.root_path`.
        let extension = seed
            .__common_attr__
            .original_file_path
            .extension()
            .and_then(|ext| ext.to_str().map(|ext| ext.to_lowercase()));
        let format = match extension.as_deref() {
            Some("csv") => None,
            Some("parquet") => Some(TableFormat::Parquet),
            Some("json") | Some("jsonl") | Some("ndjson") => Some(TableFormat::Json),
            _ => {
                // Silently skip — visit_render will detect missing schema and
                // emit the error through normal task graph propagation.
                continue;
            }
        };

        let ctx = Arc::clone(&ctx);
        match format {
            None => {
                let type_ops = Arc::clone(&type_ops);
                let h = dbt_runtime::spawn_blocking(move || register_seed_csv(seed, ctx, type_ops));
                handles.push(Box::pin(async move {
                    h.await
                        .map_err(|e| fs_err!(ErrorCode::Generic, "seed task panicked: {e}"))?
                }));
            }
            Some(TableFormat::Json) => {
                let type_ops = Arc::clone(&type_ops);
                let h =
                    dbt_runtime::spawn_blocking(move || register_seed_json(seed, ctx, type_ops));
                handles.push(Box::pin(async move {
                    h.await
                        .map_err(|e| fs_err!(ErrorCode::Generic, "seed task panicked: {e}"))?
                }));
            }
            Some(TableFormat::Parquet) => {
                let type_ops = Arc::clone(&type_ops);
                let h = spawn_traced(register_seed_parquet_async(seed, ctx, type_ops));
                handles.push(Box::pin(async move {
                    h.await
                        .map_err(|e| fs_err!(ErrorCode::Generic, "seed task panicked: {e}"))?
                }));
            }
            Some(TableFormat::Csv) => {
                unreachable!("extension dispatch only yields csv as None, not TableFormat::Csv")
            }
        }
    }

    let mut results = Vec::with_capacity(handles.len());
    for handle in handles {
        match handle.await {
            Ok(registered) => results.push(registered),
            Err(e) => tracing::error!("{}", e),
        }
    }
    results
}

fn parse_provided_types(
    column_descriptions: &BTreeMap<Spanned<String>, String>,
    type_ops: &dyn TypeOps,
) -> FsResult<LogicalTypeMap> {
    // Adapter-specific type aliases in `+column_types` (e.g. BigQuery `float`,
    // `integer`) must be normalized to their canonical names (`FLOAT64`, `INT64`)
    // before parsing, mirroring dbt-adapters' `Column.translate_type`. dbt Core
    // does not statically validate these; it hands the type to the warehouse,
    // which accepts the alias. Without this, a valid seed config fails with
    // `Invalid column description`.
    let column_static = ColumnStatic::new(type_ops.adapter_type());
    let mut logical_types: BTreeMap<String, DataType> = BTreeMap::new();
    for (field_name, descr) in column_descriptions.iter() {
        // this creates a Logical type
        let translated = column_static.translate_type(descr);
        let parsed_field = type_ops
            .parse_column_description(&translated)
            .map_err(|_| {
                fs_err!(
                    code => ErrorCode::InvalidType,
                    loc => field_name.span().clone(),
                    "Invalid column description: {descr}",
                )
            })?;
        let logical_type = parsed_field.data_type().clone();

        logical_types.insert(field_name.as_ref().clone(), logical_type);
    }
    Ok(LogicalTypeMap(logical_types))
}

pub struct LogicalTypeMap(BTreeMap<String, DataType>);

impl LogicalTypeMap {
    pub fn get(&self, k: &str) -> Option<DataType> {
        self.0.get(k).cloned()
    }
}

/// Creates a Dialect adapted table provider
///
/// The provided source should represent the physical schema
/// We abstract on top of it using the provided logical schema
pub fn dialect_adapted_schema(
    source_schema: &SchemaRef,
    logical_types: Option<&LogicalTypeMap>,
    type_ops: &dyn TypeOps,
) -> FsResult<SchemaRef> {
    let adapter_type = type_ops.adapter_type();
    let adapted_fields: Vec<Arc<Field>> = source_schema
        .fields()
        .iter()
        .map(|f| {
            // Try to get logical field type
            let logical_field_type = logical_types.and_then(|types| {
                // For Snowflake, normalize field names for matching
                let lookup_name = if matches!(adapter_type, AdapterType::Snowflake) {
                    Dialect::Snowflake
                        .parse_identifier(f.name())
                        .map(|id| id.to_value())
                        .unwrap_or_else(|_| f.name().to_string())
                } else {
                    f.name().to_string()
                };
                types.get(&lookup_name)
            });

            // Determine final field type
            let final_type = if let Some(logical_type) = logical_field_type {
                logical_type
            } else {
                // Use inferred type, with optional data platform adaptation
                type_ops
                    .adapt_seed_type(f.data_type())
                    .unwrap_or_else(|| f.data_type().clone())
            };

            // Create new field with the determined type
            Arc::new(
                Field::new(f.name().clone(), final_type, f.is_nullable())
                    .with_metadata(f.metadata().clone()),
            )
        })
        .collect();

    let adapted_schema = Arc::new(Schema::new_with_metadata(
        adapted_fields,
        source_schema.metadata().clone(),
    ));
    Ok(adapted_schema)
}
