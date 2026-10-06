use std::collections::HashSet;

use dbt_common::{
    ErrorCode, FsResult,
    artifact_io::write_artifact_to_file,
    constants::DBT_CATALOG_JSON,
    io_args::{EvalArgs, FsCommand},
    tracing::dbt_emit::{
        emit_error_log_from_fs_error, emit_info_log_message, emit_warn_log_message,
    },
};
use dbt_compilation::traits::{MetadataOutputFamily, MetadataWriteReport};
use dbt_features::index::{IndexHooks, write_metadata_parquet_with_errors};
use dbt_index_core::{
    WriteSource,
    ingest::{
        ingest_state::IngestState,
        metadata_to_parquet::{has_persisted_state, ingest_from_metadata_direct},
    },
    save_artifact_meta, write_info_schema,
};
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::{
    schemas::{legacy_catalog::DbtCatalog, manifest::DbtManifest},
    state::ResolverState,
};
use dbt_tasks_core::RunTaskResults;
use dbt_telemetry::ArtifactType;

use crate::utils::write_catalog_stats_parquet;

/// Writes the metadata artifacts produced after task execution.
pub(crate) async fn write_metadata(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: &ResolverState,
    schema_store: &dyn SchemaStoreTrait,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
) -> FsResult<()> {
    write_metadata_impl(
        arg,
        manifest,
        resolved_state,
        schema_store,
        run_task_results,
        catalog_data,
        index_hooks,
        None,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_metadata_with_report(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: &ResolverState,
    schema_store: &dyn SchemaStoreTrait,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
    report: &mut MetadataWriteReport,
) -> FsResult<()> {
    write_metadata_impl(
        arg,
        manifest,
        resolved_state,
        schema_store,
        run_task_results,
        catalog_data,
        index_hooks,
        Some(report),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn write_metadata_impl(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: &ResolverState,
    schema_store: &dyn SchemaStoreTrait,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
    mut report: Option<&mut MetadataWriteReport>,
) -> FsResult<()> {
    if let Some(catalog) = catalog_data {
        write_catalog_stats_parquet(catalog, arg).await;
        write_catalog_columns_epoch(catalog, arg);
    }

    let grain_infos = index_hooks.lineage_grain_infos(run_task_results).await?;
    let (node_classifiers, column_classifiers) =
        index_hooks.classifier_results(run_task_results).await?;

    let recomputed_targets: HashSet<String> = if arg.command.compiles_project() {
        run_task_results
            .stats
            .compile
            .stats
            .iter()
            .map(|stat| stat.unique_id.clone())
            .collect()
    } else {
        HashSet::new()
    };

    if recomputed_targets.is_empty() {
        let errors = write_metadata_parquet_with_errors(
            arg,
            manifest,
            Some(resolved_state),
            Some(schema_store),
            None,
            &recomputed_targets,
            &grain_infos,
            &node_classifiers,
            &column_classifiers,
        );
        record_metadata_errors(arg, &mut report, errors);
    } else if !arg.write_lineage {
        let errors = write_metadata_parquet_with_errors(
            arg,
            manifest,
            Some(resolved_state),
            Some(schema_store),
            Some(&[]),
            &recomputed_targets,
            &grain_infos,
            &node_classifiers,
            &column_classifiers,
        );
        record_metadata_errors(arg, &mut report, errors);
    } else {
        match index_hooks
            .column_lineage(resolved_state, run_task_results)
            .await
        {
            Ok(column_lineage) => {
                if column_lineage.is_empty() {
                    emit_warn_log_message(
                        ErrorCode::Generic,
                        "column-level lineage requires --static-analysis strict; no column lineage written.",
                    );
                }
                let errors = write_metadata_parquet_with_errors(
                    arg,
                    manifest,
                    Some(resolved_state),
                    Some(schema_store),
                    Some(&column_lineage),
                    &recomputed_targets,
                    &grain_infos,
                    &node_classifiers,
                    &column_classifiers,
                );
                record_metadata_errors(arg, &mut report, errors);
            }
            Err(error) => {
                emit_warn_log_message(
                    ErrorCode::Generic,
                    format!("dbt-index: column_lineage: {error}"),
                );
                let empty_targets: HashSet<String> = HashSet::new();
                let errors = write_metadata_parquet_with_errors(
                    arg,
                    manifest,
                    Some(resolved_state),
                    Some(schema_store),
                    Some(&[]),
                    &empty_targets,
                    &grain_infos,
                    &node_classifiers,
                    &column_classifiers,
                );
                record_metadata_errors(arg, &mut report, errors);
            }
        }
    }

    // Write catalog.json from pre-fetched catalog — no second warehouse query.
    // Epochs already written unconditionally above; this block is catalog.json only.
    // Only for Run/Build: need executed nodes to populate relations.
    if arg.write_catalog
        && matches!(arg.command, FsCommand::Run | FsCommand::Build)
        && let Some(catalog) = catalog_data
    {
        match write_artifact_to_file(
            catalog,
            ArtifactType::Catalog,
            &arg.io.out_dir,
            DBT_CATALOG_JSON,
            &arg.io.in_dir,
        ) {
            Ok(()) => emit_info_log_message("Successfully wrote catalog.json"),
            Err(error) => emit_warn_log_message(ErrorCode::Generic, format!("catalog: {error}")),
        }
    }

    // When --write-index is active, convert metadata epochs → snapshot index parquet.
    if arg.write_index {
        let metadata_dir = arg.metadata_dir();
        let index_dir = arg.index_dir();
        let mut state = IngestState::default();
        match ingest_from_metadata_direct(&metadata_dir, &index_dir, &mut state) {
            Ok(_) => {
                if let Err(error) =
                    save_artifact_meta(&index_dir, &arg.io.out_dir, WriteSource::DirectWrite, None)
                {
                    if let Some(report) = report.as_deref_mut() {
                        report.mark_failed(
                            MetadataOutputFamily::Index,
                            index_dir.clone(),
                            error.to_string(),
                        );
                    }
                    emit_warn_log_message(
                        ErrorCode::IndexWriteFailed,
                        format!("dbt-index: save_artifact_meta: {error}"),
                    );
                }
            }
            Err(error) => {
                if let Some(report) = report.as_deref_mut() {
                    report.mark_failed(
                        MetadataOutputFamily::Index,
                        index_dir.clone(),
                        error.to_string(),
                    );
                }
                emit_warn_log_message(
                    ErrorCode::IndexWriteFailed,
                    format!("dbt-index: write-index: {error}"),
                );
            }
        }

        // Post-index hook: ingest the classifier registry and run the
        // classifier "checks" gate. No-op in OSS.
        //
        // Recorded rather than propagated: a failing index write should not
        // abort a build whose models already succeeded.
        if let Err(error) = index_hooks
            .did_write_index(arg, &index_dir, run_task_results, resolved_state)
            .await
        {
            emit_error_log_from_fs_error(*error);
        }
    }

    // The information schema is written independently of the index:
    // either, neither, or both may be requested. Its intermediate is the
    // flat index at `target/private/index` when one is present — the same ingest
    // builds both, so an index written by the block just above (or by a
    // prior run) is reused via the delta path rather than re-ingested. With
    // no index to reuse — e.g. `--no-write-index` — it stages privately, so
    // requesting the information schema never materialises an index the
    // caller opted out of.
    if arg.generate_info_schema {
        let metadata_dir = arg.metadata_dir();
        let info_schema_dir = arg.info_schema_dir();
        let index_dir = arg.index_dir();
        let staging_dir = if has_persisted_state(&index_dir) {
            index_dir
        } else {
            arg.info_schema_staging_dir()
        };
        if let Err(error) = write_info_schema(&metadata_dir, &info_schema_dir, &staging_dir) {
            if let Some(report) = report {
                report.mark_failed(
                    MetadataOutputFamily::InfoSchema,
                    dbt_index_core::info_schema::versioned_dir(&info_schema_dir),
                    error.to_string(),
                );
            }
            emit_warn_log_message(
                ErrorCode::InfoSchemaWriteFailed,
                format!("dbt: generate-info-schema: {error}"),
            );
        }
    }

    Ok(())
}

fn record_metadata_errors(
    arg: &EvalArgs,
    report: &mut Option<&mut MetadataWriteReport>,
    errors: Vec<String>,
) {
    if !errors.is_empty()
        && let Some(report) = report.as_deref_mut()
    {
        report.mark_failed(
            MetadataOutputFamily::Metadata,
            arg.metadata_dir(),
            errors.join("; "),
        );
    }
}

fn write_catalog_columns_epoch(catalog: &DbtCatalog, arg: &EvalArgs) {
    use chrono::Utc;
    use dbt_metadata_parquet::catalog_columns::CatalogColumnRow;

    let ingested_at = Utc::now().timestamp_micros();
    let mut rows = Vec::new();

    for (unique_id, table) in catalog.nodes.iter().chain(catalog.sources.iter()) {
        for (idx, (_column_name, column)) in table.columns.iter().enumerate() {
            rows.push(CatalogColumnRow {
                unique_id: unique_id.clone(),
                column_name: column.name.clone(),
                column_index: idx as i32,
                catalog_type: Some(column.data_type.clone()),
                catalog_comment: column.comment.clone(),
                ingested_at,
            });
        }
    }

    let dir = arg.metadata_dir().join("catalog").join("columns");
    if let Err(error) =
        dbt_metadata_parquet::catalog_columns::write_catalog_columns(&dir, rows, None, None, None)
    {
        emit_warn_log_message(
            ErrorCode::Generic,
            format!("metadata: catalog_columns: {error}"),
        );
    }
}
