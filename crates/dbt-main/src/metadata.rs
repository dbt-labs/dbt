use std::collections::HashSet;

use dbt_adapter::sql_types::TypeOpsFactory;
use dbt_common::io_args::project_flag_lookup;
use dbt_common::{
    ErrorCode, FsResult,
    artifact_io::{ArtifactSerialization, write_artifact_to_file},
    constants::{DBT_CATALOG_JSON, DBT_PROJECT_YML},
    io_args::{EvalArgs, FsCommand},
    tracing::dbt_emit::{
        emit_error_log_from_fs_error, emit_info_log_message, emit_warn_log_from_fs_error,
        emit_warn_log_message,
    },
};
use dbt_compilation::traits::{MetadataOutputFamily, MetadataWriteReport};
use dbt_features::feature_stack::FeatureStack;
use dbt_features::index::{IndexHooks, write_metadata_parquet_with_errors};
use dbt_flags::{NoProjectFlags, ProjectFlags};
use dbt_index_core::info_schema::injected::{self, ResolvedInjectedTable};
use dbt_index_core::{
    WriteSource,
    ingest::{
        ingest_state::IngestState,
        metadata_to_parquet::{has_persisted_state, ingest_from_metadata_direct},
    },
    save_artifact_meta, write_info_schema_with_injected,
};
use dbt_loader::{args::LoadArgs, load_simplified_project_only};
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::{
    schemas::{legacy_catalog::DbtCatalog, manifest::DbtManifest},
    state::ResolverState,
};
use dbt_tasks_core::RunTaskResults;
use dbt_telemetry::ArtifactType;

use crate::utils::write_catalog_stats_parquet;

/// Writes the metadata artifacts produced after task execution.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn write_metadata(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: &ResolverState,
    schema_store: &dyn SchemaStoreTrait,
    type_ops_factory: &dyn TypeOpsFactory,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
    injected_tables: &[ResolvedInjectedTable<'_>],
) -> FsResult<()> {
    write_metadata_impl(
        arg,
        manifest,
        resolved_state,
        schema_store,
        type_ops_factory,
        run_task_results,
        catalog_data,
        index_hooks,
        injected_tables,
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
    type_ops_factory: &dyn TypeOpsFactory,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
    injected_tables: &[ResolvedInjectedTable<'_>],
    report: &mut MetadataWriteReport,
) -> FsResult<()> {
    write_metadata_impl(
        arg,
        manifest,
        resolved_state,
        schema_store,
        type_ops_factory,
        run_task_results,
        catalog_data,
        index_hooks,
        injected_tables,
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
    type_ops_factory: &dyn TypeOpsFactory,
    run_task_results: &RunTaskResults,
    catalog_data: Option<&DbtCatalog>,
    index_hooks: &dyn IndexHooks,
    injected_tables: &[ResolvedInjectedTable<'_>],
    mut report: Option<&mut MetadataWriteReport>,
) -> FsResult<()> {
    if let Some(catalog) = catalog_data {
        write_catalog_epochs(catalog, arg).await;
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
            Some(type_ops_factory),
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
            Some(type_ops_factory),
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
                    Some(type_ops_factory),
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
                    Some(type_ops_factory),
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
            ArtifactSerialization::WithYamlFlattening(catalog),
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
        if let Err(error) = write_info_schema_with_injected(
            &metadata_dir,
            &info_schema_dir,
            &staging_dir,
            injected_tables,
        ) {
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

/// Writes the `catalog/columns` and `run/catalog_stats` epochs from a fetched catalog.
pub(crate) async fn write_catalog_epochs(catalog: &DbtCatalog, arg: &EvalArgs) {
    write_catalog_stats_parquet(catalog, arg).await;
    write_catalog_columns_epoch(catalog, arg);
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

/// The tables that the build injects into the information schema, each with whether its flag
/// is on.
///
/// `root_project` is the `flags:` of the root project that the command loaded. A flag is
/// resolved with them, as the flag of a kind of node is.
pub(crate) fn resolved_injected_tables(
    feature_stack: &FeatureStack,
    root_project: &dyn ProjectFlags,
) -> Vec<ResolvedInjectedTable<'static>> {
    injected::resolve(
        feature_stack.injected_kinds.kinds.injected_tables(),
        root_project,
    )
}

/// [`resolved_injected_tables`], for a command that reads what an earlier command wrote and
/// loads no project: `dbt show --info`, `dbt docs`.
///
/// The root project's `flags:` come from the loader, which loads `dbt_project.yml` alone, as
/// it does for `dbt deps`. Nothing is loaded for a build that injects no table. Where there
/// is no `dbt_project.yml`, or it does not load (with a warning), a flag is as the invocation
/// sets it.
pub(crate) fn resolved_injected_tables_without_project(
    feature_stack: &FeatureStack,
    arg: &EvalArgs,
) -> Vec<ResolvedInjectedTable<'static>> {
    let declared = feature_stack.injected_kinds.kinds.injected_tables();
    if declared.is_empty() || !arg.io.in_dir.join(DBT_PROJECT_YML).exists() {
        return resolved_injected_tables(feature_stack, &NoProjectFlags);
    }
    match load_simplified_project_only(&LoadArgs::from_eval_args(arg)) {
        Ok(project) => {
            resolved_injected_tables(feature_stack, &project_flag_lookup(project.flags.as_ref()))
        }
        Err(error) => {
            emit_warn_log_from_fs_error(*error);
            resolved_injected_tables(feature_stack, &NoProjectFlags)
        }
    }
}

/// Writes anew the tables that the build adds to the information schema, in an information
/// schema that exists, after a command that did not write the information schema: one of its
/// tasks may have added rows. Nothing is written where there is no information schema.
///
/// A failure warns, as a failed write of the information schema does.
pub(crate) fn refresh_injected_tables(arg: &EvalArgs, tables: &[ResolvedInjectedTable<'_>]) {
    if tables.is_empty() {
        return;
    }
    let dir = dbt_index_core::info_schema::versioned_dir(&arg.info_schema_dir());
    if let Err(error) = injected::publish(tables, &arg.metadata_dir(), &dir) {
        emit_warn_log_message(
            ErrorCode::InfoSchemaWriteFailed,
            format!("dbt: generate-info-schema: {error}"),
        );
    }
}
