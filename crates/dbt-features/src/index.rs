use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use dbt_common::io_args::EvalArgs;
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_common::{ErrorCode, FsResult};
use dbt_docs_server::Providers;
use dbt_docs_server::providers::{
    Backend, DefaultDistInfoProvider, DistInfo, DistInfoProvider, TelemetryHydration,
};
use dbt_index_core::column_impact::UnavailableColumnImpact;
use dbt_index_core::column_lineage::UnavailableColumnLineage;
use dbt_lineage_core::{CllEdge, PlanGrainInfo};
use dbt_schema_store::SchemaStoreTrait;
use dbt_schemas::schemas::manifest::{DbtManifest, DbtNode};
use dbt_schemas::state::ResolverState;
use dbt_tasks_core::RunTaskResults;

#[async_trait]
pub trait IndexHooks: Send + Sync {
    async fn lineage_grain_infos(
        &self,
        _run_task_results: &RunTaskResults,
    ) -> FsResult<HashMap<String, PlanGrainInfo>> {
        Ok(HashMap::new())
    }

    async fn column_lineage(
        &self,
        _resolved_state: &ResolverState,
        _run_task_results: &RunTaskResults,
    ) -> FsResult<Vec<CllEdge>> {
        Ok(Vec::new())
    }

    /// Classifier propagation results recovered from the run results (a downcast
    /// of the proprietary `ClassifierPropagationResults` showable). Returns
    /// `(node_classifiers, column_classifiers)` — propagated labels keyed by
    /// `unique_id` (and lowercased column name). Default: empty (OSS no-op).
    #[allow(clippy::type_complexity)]
    async fn classifier_results(
        &self,
        _run_task_results: &RunTaskResults,
    ) -> FsResult<(
        HashMap<String, Vec<String>>,
        HashMap<String, HashMap<String, Vec<String>>>,
    )> {
        Ok((HashMap::new(), HashMap::new()))
    }

    /// Called after the Parquet index has been written (and `save_artifact_meta`
    /// succeeded). Proprietary implementations ingest the classifier registry
    /// table and run the post-index "checks" gate; returning `Err` aborts the
    /// build. Default: no-op (OSS).
    async fn did_write_index(
        &self,
        _arg: &EvalArgs,
        _index_dir: &std::path::Path,
        _run_task_results: &RunTaskResults,
        _resolved_state: &ResolverState,
    ) -> FsResult<()> {
        Ok(())
    }
}

pub struct NoOpIndexHooks;

#[async_trait]
impl IndexHooks for NoOpIndexHooks {}

pub struct IndexFeature {
    pub hooks: Box<dyn IndexHooks>,
    pub providers_factory: fn(Arc<dyn Backend>) -> Providers,
}

/// Distribution/telemetry provider backed by the process-global [`dbt_env`].
///
/// Reuses [`DefaultDistInfoProvider`] for the distribution name / login state
/// (OSS: `oss` / not logged in), but overrides [`Self::telemetry_hydration`] to
/// read the real dbt version and dbt Cloud IDs from
/// [`dbt_env::env::InternalEnv::global`] so analytics events are hydrated
/// authoritatively server-side.
pub struct EnvDistInfoProvider;

impl DistInfoProvider for EnvDistInfoProvider {
    fn dist_info(&self) -> DistInfo {
        DefaultDistInfoProvider.dist_info()
    }

    fn telemetry_hydration(&self) -> TelemetryHydration {
        let info = self.dist_info();
        let cfg = dbt_env::env::InternalEnv::global().invocation_config();
        TelemetryHydration {
            distribution: info.name,
            dbt_version: cfg.dbt_version.clone(),
            is_logged_in: info.is_logged_in,
            dbt_cloud_account_identifier: cfg.account_identifier.clone(),
            dbt_cloud_project_id: cfg.project_id.clone(),
            dbt_cloud_environment_id: cfg.environment_id.clone(),
        }
    }
}

pub fn default_providers_factory(backend: Arc<dyn Backend>) -> Providers {
    Providers {
        backend: backend.clone(),
        column_lineage: Arc::new(UnavailableColumnLineage::new()),
        column_impact: Arc::new(UnavailableColumnImpact::new()),
        dist_info: Arc::new(EnvDistInfoProvider),
    }
}

fn unique_key_to_grain(uk: &Option<dbt_schemas::schemas::common::DbtUniqueKey>) -> Vec<String> {
    use dbt_schemas::schemas::common::DbtUniqueKey;
    match uk {
        Some(DbtUniqueKey::Single(s)) => vec![s.clone()],
        Some(DbtUniqueKey::Multiple(v)) => v.clone(),
        None => vec![],
    }
}

/// Write metadata parquet epoch files from in-memory typed structs.
///
/// Writes only `target/private/metadata/` epoch files — no DuckDB, no `target/private/index/`.
/// Independent of `write_json`. Errors are non-fatal — logged as warnings.
#[allow(clippy::cognitive_complexity)]
pub fn write_metadata_parquet(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: Option<&ResolverState>,
    schema_store: Option<&dyn SchemaStoreTrait>,
    column_lineage: Option<&[CllEdge]>,
    recomputed_column_lineage_targets: &HashSet<String>,
    grain_infos: &HashMap<String, PlanGrainInfo>,
    node_classifiers: &HashMap<String, Vec<String>>,
    column_classifiers: &HashMap<String, HashMap<String, Vec<String>>>,
) {
    write_metadata_parquet_impl(
        arg,
        manifest,
        resolved_state,
        schema_store,
        column_lineage,
        recomputed_column_lineage_targets,
        grain_infos,
        node_classifiers,
        column_classifiers,
    );
}

/// Merge declared (YAML) classifier labels with propagated labels into a
/// sorted, deduplicated JSON-array string (matches the `dbt.classifiers`
/// VARCHAR[] column representation). Returns `"[]"` when both are empty.
fn merge_classifiers_json(declared: &[String], propagated: Option<&Vec<String>>) -> String {
    let v = merge_classifiers_vec(declared, propagated);
    serde_json::to_string(&v).unwrap_or_else(|_| "[]".to_string())
}

/// Sorted, deduplicated union of declared + propagated classifier labels.
fn merge_classifiers_vec(declared: &[String], propagated: Option<&Vec<String>>) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = declared.iter().cloned().collect();
    if let Some(p) = propagated {
        set.extend(p.iter().cloned());
    }
    set.into_iter().collect()
}

/// Node-level declared classifiers from the manifest `CommonAttributes`.
fn node_declared_classifiers(node: &DbtNode) -> &[String] {
    match node {
        DbtNode::Model(x) => &x.__common_attr__.classifiers,
        DbtNode::Seed(x) => &x.__common_attr__.classifiers,
        DbtNode::Snapshot(x) => &x.__common_attr__.classifiers,
        DbtNode::Analysis(x) => &x.__common_attr__.classifiers,
        DbtNode::Test(x) => &x.__common_attr__.classifiers,
        DbtNode::Operation(x) => &x.__common_attr__.classifiers,
        DbtNode::Function(x) => &x.__common_attr__.classifiers,
        DbtNode::Check(x) => &x.__common_attr__.classifiers,
    }
}

#[allow(clippy::cognitive_complexity)]
fn write_metadata_parquet_impl(
    arg: &EvalArgs,
    manifest: &DbtManifest,
    resolved_state: Option<&ResolverState>,
    schema_store: Option<&dyn SchemaStoreTrait>,
    column_lineage: Option<&[CllEdge]>,
    recomputed_column_lineage_targets: &HashSet<String>,
    grain_infos: &HashMap<String, PlanGrainInfo>,
    node_classifiers: &HashMap<String, Vec<String>>,
    column_classifiers: &HashMap<String, HashMap<String, Vec<String>>>,
) {
    use dbt_common::static_analysis::is_strict_static_analysis;
    use dbt_index_core::hash_str;

    let ingested_at: i64 = resolved_state
        .map(|rs| rs.run_started_at.timestamp_micros())
        .unwrap_or_else(|| Utc::now().timestamp_micros());

    let mut compiled_node_rows: Vec<dbt_metadata_parquet::compiled_node::CompiledNodeRow> =
        Vec::new();
    let mut compile_column_rows: Vec<dbt_metadata_parquet::compile_columns::CompileColumnRow> =
        Vec::new();

    // grain tracking
    let mut grain_tested: HashMap<&str, Vec<String>> = HashMap::new();
    let mut unique_key_grain: HashMap<&str, Vec<String>> = HashMap::new();

    for (uid, node) in &manifest.nodes {
        match node {
            DbtNode::Test(t) => {
                if let Some(tm) = &t.test_metadata {
                    if let Some(attached) = t.attached_node.as_deref() {
                        match tm.name.as_str() {
                            "unique_combination_of_columns" => {
                                if let Some(cols) =
                                    tm.kwargs.get("combination_of_columns").and_then(|v| {
                                        v.as_sequence().map(|seq| {
                                            seq.iter()
                                                .filter_map(|i| i.as_str().map(str::to_string))
                                                .collect::<Vec<_>>()
                                        })
                                    })
                                {
                                    if !cols.is_empty() {
                                        grain_tested.insert(attached, cols);
                                    }
                                }
                            }
                            "unique" => {
                                let col = t.column_name.as_deref().or_else(|| {
                                    tm.kwargs.get("column_name").and_then(|v| v.as_str())
                                });
                                if let Some(col) = col {
                                    grain_tested
                                        .entry(attached)
                                        .or_insert_with(|| vec![col.to_string()]);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            DbtNode::Model(m) => {
                let cols = unique_key_to_grain(&m.config.unique_key);
                if !cols.is_empty() {
                    unique_key_grain.insert(uid, cols);
                }
            }
            _ => {}
        }
    }
    for (uid, cols) in unique_key_grain {
        grain_tested.entry(uid).or_insert(cols);
    }

    for (uid, node) in &manifest.nodes {
        let b = match node {
            DbtNode::Model(x) => &x.__base_attr__,
            DbtNode::Seed(x) => &x.__base_attr__,
            DbtNode::Snapshot(x) => &x.__base_attr__,
            DbtNode::Analysis(x) => &x.__base_attr__,
            DbtNode::Test(x) => &x.__base_attr__,
            DbtNode::Operation(x) => &x.__base_attr__,
            DbtNode::Function(x) => &x.__base_attr__,
            DbtNode::Check(x) => &x.__base_attr__,
        };

        // compile/nodes requires --write-metadata --static-analysis strict (or
        // baseline + --write-lineage, i.e. `--infer-schemas`). Without either,
        // there is no grain_infos from LP and no schema inference; compile/nodes
        // would only duplicate what parse/nodes already contains.
        let write_compile_nodes = arg.write_metadata
            && (arg.static_analysis.is_some_and(is_strict_static_analysis)
                || arg.infer_schemas_and_typeless);
        if write_compile_nodes {
            let grain_declared: Vec<String> = match node {
                DbtNode::Model(m) => m.primary_key.as_ref().cloned().unwrap_or_default(),
                _ => vec![],
            };
            let grain_tested_val = grain_tested.get(uid.as_str()).cloned().unwrap_or_default();
            let grain_inferred = grain_infos
                .get(uid.as_str())
                .map(|info| info.columns.clone())
                .unwrap_or_default();
            let grain = if !grain_declared.is_empty() {
                grain_declared.clone()
            } else if !grain_tested_val.is_empty() {
                grain_tested_val.clone()
            } else {
                grain_inferred.clone()
            };

            compiled_node_rows.push(dbt_metadata_parquet::compiled_node::CompiledNodeRow {
                unique_id: uid.to_string(),
                compiled_code: b.compiled_code.clone(),
                compiled_code_hash: b.compiled_code.as_deref().map(hash_str),
                compiled_path: b.compiled_path.clone(),
                grain: serde_json::to_string(&grain).unwrap_or_default(),
                grain_declared: serde_json::to_string(&grain_declared).unwrap_or_default(),
                grain_tested: serde_json::to_string(&grain_tested_val).unwrap_or_default(),
                classifiers: merge_classifiers_json(
                    node_declared_classifiers(node),
                    node_classifiers.get(uid.as_str()),
                ),
                table_role: None,
                ingested_at,
            });
        } else if arg.write_metadata {
            // Baseline SA (no strict): still carry node-level classifiers
            // (declared ∪ propagated) into dbt.nodes via the compile epoch.
            // Classifier propagation does not require strict SA, so they must
            // reach the index regardless of the grain/inferred-type gate.
            let node_clf = merge_classifiers_vec(
                node_declared_classifiers(node),
                node_classifiers.get(uid.as_str()),
            );
            if !node_clf.is_empty() {
                compiled_node_rows.push(dbt_metadata_parquet::compiled_node::CompiledNodeRow {
                    unique_id: uid.to_string(),
                    compiled_code: None,
                    compiled_code_hash: None,
                    compiled_path: None,
                    grain: "[]".to_string(),
                    grain_declared: "[]".to_string(),
                    grain_tested: "[]".to_string(),
                    classifiers: serde_json::to_string(&node_clf)
                        .unwrap_or_else(|_| "[]".to_string()),
                    table_role: None,
                    ingested_at,
                });
            }
        }

        // compile/columns: inferred types (strict SA) + classifiers (always, when
        // --write-metadata). Classifier propagation does not require strict SA, and
        // for remote (e.g. Snowflake) builds the schema_store may lack a model's
        // inferred schema — so classifier-carrying columns are emitted from the
        // manifest column list ∪ propagated map regardless of schema_store presence.
        if arg.write_metadata {
            let declared_cols: HashMap<String, Vec<String>> = b
                .columns
                .iter()
                .filter_map(|c| {
                    c.classifiers
                        .clone()
                        .map(|cl| (c.name.to_ascii_lowercase(), cl))
                })
                .collect();
            let col_prop = column_classifiers.get(uid.as_str());
            let mut emitted: HashSet<String> = HashSet::new();

            // (1) strict SA: inferred-type rows from the schema store (+ classifiers).
            if write_compile_nodes {
                if let Some(store) = schema_store {
                    if let Some(entry) = store.get_schema_by_unique_id(uid) {
                        for (idx, field) in entry.inner().fields().iter().enumerate() {
                            let name_lower = field.name().to_ascii_lowercase();
                            let declared = declared_cols
                                .get(&name_lower)
                                .map(|v| v.as_slice())
                                .unwrap_or(&[]);
                            emitted.insert(name_lower.clone());
                            compile_column_rows.push(
                                dbt_metadata_parquet::compile_columns::CompileColumnRow {
                                    unique_id: uid.to_string(),
                                    column_name: field.name().clone(),
                                    column_index: idx as i32,
                                    column_type: Some(field.data_type().to_string()),
                                    description: None,
                                    classifiers: merge_classifiers_vec(
                                        declared,
                                        col_prop.and_then(|m| m.get(&name_lower)),
                                    ),
                                    ingested_at,
                                },
                            );
                        }
                    }
                }
            }

            // (2) Classifier-carrying columns not already emitted (handles baseline
            // SA and remote builds where the schema store lacks the model's columns):
            // manifest columns with declared ∪ propagated labels.
            for (idx, col) in b.columns.iter().enumerate() {
                let name_lower = col.name.to_ascii_lowercase();
                if emitted.contains(&name_lower) {
                    continue;
                }
                let declared = declared_cols
                    .get(&name_lower)
                    .map(|v| v.as_slice())
                    .unwrap_or(&[]);
                let clf =
                    merge_classifiers_vec(declared, col_prop.and_then(|m| m.get(&name_lower)));
                if !clf.is_empty() {
                    emitted.insert(name_lower);
                    compile_column_rows.push(
                        dbt_metadata_parquet::compile_columns::CompileColumnRow {
                            unique_id: uid.to_string(),
                            column_name: col.name.clone(),
                            column_index: idx as i32,
                            column_type: None,
                            description: None,
                            classifiers: clf,
                            ingested_at,
                        },
                    );
                }
            }

            // (3) Propagation-only columns not declared in the manifest.
            if let Some(cp) = col_prop {
                for (col_lower, labels) in cp {
                    if !emitted.contains(col_lower) && !labels.is_empty() {
                        compile_column_rows.push(
                            dbt_metadata_parquet::compile_columns::CompileColumnRow {
                                unique_id: uid.to_string(),
                                column_name: col_lower.clone(),
                                column_index: 0,
                                column_type: None,
                                description: None,
                                classifiers: labels.clone(),
                                ingested_at,
                            },
                        );
                    }
                }
            }
        }
    }

    // sources: compile/columns (parse/columns now written by parse_state::save)
    for (uid, src) in &manifest.sources {
        // compile/columns — src.columns is already merged with schema types by update_manifest
        if !src.columns.is_empty() {
            let col_prop = column_classifiers.get(uid.as_str());
            for (idx, col) in src.columns.iter().enumerate() {
                let name_lower = col.name.to_ascii_lowercase();
                compile_column_rows.push(dbt_metadata_parquet::compile_columns::CompileColumnRow {
                    unique_id: uid.to_string(),
                    column_name: col.name.clone(),
                    column_index: idx as i32,
                    column_type: col.data_type.clone(),
                    description: col.description.clone(),
                    classifiers: merge_classifiers_vec(
                        col.classifiers.as_deref().unwrap_or(&[]),
                        col_prop.and_then(|m| m.get(&name_lower)),
                    ),
                    ingested_at,
                });
            }
        }
    }

    // CLL edges arrive pre-deduped from cll_edges_from_lineage_results.
    let cll_ingested_at = ingested_at;
    let cll_edges: &[CllEdge] = column_lineage.unwrap_or(&[]);

    let targets_opt = if recomputed_column_lineage_targets.is_empty() {
        None
    } else {
        Some(recomputed_column_lineage_targets)
    };
    let compiled_nodes_targets = targets_opt;
    let compile_columns_targets = targets_opt;

    let cll_dir = arg.metadata_dir().join("compile").join("column_lineage");
    let compiled_nodes_dir = arg.metadata_dir().join("compile").join("nodes");
    let compile_columns_dir = arg.metadata_dir().join("compile").join("columns");

    let alive_node_count = manifest.nodes.len();

    let t_write = std::time::Instant::now();
    let errors: Vec<String> = std::thread::scope(|s| {
        let t_cll = s.spawn(|| {
            let cll_rows: Vec<dbt_metadata_parquet::cll_epoch::CllRow> = cll_edges
                .iter()
                .map(|e| dbt_metadata_parquet::cll_epoch::CllRow {
                    from_node_unique_id: e.from_node.clone(),
                    from_column_name: e.from_col.clone(),
                    to_node_unique_id: e.to_node.clone(),
                    to_column_name: e.to_col.clone(),
                    lineage_kind: e.op.to_string(),
                    ingested_at: cll_ingested_at,
                })
                .collect();
            dbt_metadata_parquet::cll_epoch::write_cll_epoch(
                &cll_dir,
                cll_rows,
                cll_ingested_at,
                targets_opt,
                Some(alive_node_count),
                None,
            )
            .err()
            .map(|e| format!("metadata: cll_epoch: {e}"))
        });
        let t_compiled = s.spawn(|| {
            dbt_metadata_parquet::compiled_node::write_compiled_nodes(
                &compiled_nodes_dir,
                compiled_node_rows,
                compiled_nodes_targets,
                Some(alive_node_count),
                None,
            )
            .err()
            .map(|e| format!("metadata: compiled_nodes: {e}"))
        });
        let t_compile_cols = s.spawn(|| {
            dbt_metadata_parquet::compile_columns::write_compile_columns(
                &compile_columns_dir,
                compile_column_rows,
                compile_columns_targets,
                Some(alive_node_count),
                None,
            )
            .err()
            .map(|e| format!("metadata: compile_columns: {e}"))
        });
        [
            t_cll.join().unwrap_or(None),
            t_compiled.join().unwrap_or(None),
            t_compile_cols.join().unwrap_or(None),
        ]
        .into_iter()
        .flatten()
        .collect()
    });

    if std::env::var_os("DBT_LINEAGE_TIMING").is_some() {
        eprintln!(
            "[lineage] {:>8.1}ms  thread::scope write parquet (3 parallel)",
            t_write.elapsed().as_secs_f64() * 1000.0
        );
    }

    for e in errors {
        emit_warn_log_message(ErrorCode::Generic, e);
    }
}
