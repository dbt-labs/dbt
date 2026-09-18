//! Publication of audited working tables. The manifest always names the public relation.

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use arrow::array::{Array, RecordBatch, StringArray};
use dbt_adapter::Adapter;
use dbt_adapter::catalog_relation::CatalogRelation;
use dbt_adapter::connection::drop_thread_local_connection;
use dbt_adapter::record_batch::RecordBatchExt;
use dbt_adapter::relation::RelationObject;
use dbt_adapter::response::AdapterResponse;
use dbt_adapter_core::AdapterType;
use dbt_agate::AgateTable;
use dbt_common::stats::{NodeStatus, Stat};
use dbt_common::status_reporter::report_completed;
use dbt_common::tracing::dbt_emit::{
    emit_error_log_from_fs_error, emit_info_log_message, emit_warn_log_message,
};
use dbt_common::{ErrorCode, FsError, FsResult, fs_err};
use dbt_jinja_utils::phases::run::build_run_node_context;
use dbt_jinja_utils::utils::add_task_context;
use dbt_schemas::schemas::dbt_catalogs_v2::{CatalogType, TableFormat};
use dbt_schemas::schemas::relations::base::BaseRelation;
use dbt_schemas::schemas::{DbtModel, InternalDbtNode, InternalDbtNodeAttributes, NodePathKind};
use dbt_tasks_core::context::TaskRunnerCtx;
use dbt_tasks_core::run_cache::run_cache_service::evict_node_metadata_for_untracked_rebuild;
use dbt_tasks_core::task::{TP, Task, TaskOp};
use dbt_tasks_core::wap::WapModel;
use dbt_telemetry::{ExecutionPhase, NodeType};
use minijinja::Value;

use crate::materialize::{
    apply_node_overrides, materialize_latest_version_pointer, reset_node_overrides,
    should_create_latest_version_pointer,
};
use crate::runnable::cache::cache_materialization_return_value;
use crate::runnable::runnable::{RunExecutionPath, emit_run_usage_stats};

/// The model's terminal task: no public success is recorded by the preceding stage task.
pub struct PublishTask {
    model: Arc<DbtModel>,
}

impl PublishTask {
    pub fn new(model: Arc<DbtModel>) -> Self {
        Self { model }
    }
}

impl Task for PublishTask {
    fn run_task<'a>(
        &'a self,
        ctx: &'a mut TaskRunnerCtx,
    ) -> Pin<Box<dyn Future<Output = FsResult<NodeStatus>> + Send + 'a>> {
        Box::pin(async move {
            let unique_id = &self.model.common().unique_id;
            let model = Arc::clone(&self.model);
            let publish_ctx = ctx.clone();
            let result = TaskOp::Blocking(Box::new(move || publish_model(&model, &publish_ctx)))
                .run()
                .await
                .and_then(|result| result);

            let mut stat = ctx
                .inner
                .wap_stage_stats
                .remove(unique_id)
                .map(|(_, stat)| stat)
                .unwrap_or_else(|| {
                    Stat::new(
                        unique_id.clone(),
                        SystemTime::now(),
                        None,
                        NodeStatus::Errored,
                        None,
                        ctx.thread_id,
                    )
                });
            stat.end_time = SystemTime::now();
            match result {
                Ok(()) => {
                    stat.status = NodeStatus::Succeeded;
                    stat.message = Some("WAP: all audits passed; table published".to_string());
                }
                Err(error) => {
                    stat.status = NodeStatus::Errored;
                    stat.message = Some(error.to_string());
                    emit_error_log_from_fs_error(*error);
                    report_retained_candidate(ctx, unique_id);
                }
            }
            let status = stat.status.clone();
            ctx.inner.run_stats.insert(unique_id.clone(), stat);
            if ctx.inner.arg.io.send_anonymous_usage_stats {
                emit_run_usage_stats(self.model.as_ref(), ctx, RunExecutionPath::Remote);
            }
            report_completed(
                &status,
                self.model.defined_at().cloned(),
                &self
                    .model
                    .get_node_path(
                        NodePathKind::Definition,
                        &ctx.inner.arg.io.in_dir,
                        &ctx.inner.arg.io.out_dir,
                    )
                    .display()
                    .to_string(),
                false,
                ctx.inner.arg.io.status_reporter.as_ref(),
            );
            Ok(status)
        })
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Model
    }

    fn task_type(&self) -> &str {
        "wap_publish_run"
    }

    fn work_node_id(&self) -> &str {
        &self.model.common().unique_id
    }

    fn dbt_nodes(&self) -> Vec<Arc<dyn InternalDbtNodeAttributes>> {
        vec![self.model.clone()]
    }

    fn task_phase(&self) -> Option<TP> {
        Some(TP::Run)
    }
}

pub(crate) fn require_passing_audits<'a>(
    audits: impl IntoIterator<Item = (&'a str, Option<NodeStatus>)>,
) -> FsResult<()> {
    let mut count = 0;
    for (id, status) in audits {
        count += 1;
        if status != Some(NodeStatus::TestPassed) {
            return Err(fs_err!(
                ErrorCode::ExecutionError,
                "WAP: table not published because audit '{id}' did not PASS (result: {}). \
                 Warnings, missing results, and skipped audits also prevent publication",
                status.map_or_else(|| "missing".to_string(), |s| s.default_message())
            ));
        }
    }
    if count == 0 {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP requires at least one passing audit before publication"
        ));
    }
    Ok(())
}

fn model_context(model: &DbtModel, ctx: &TaskRunnerCtx) -> BTreeMap<String, Value> {
    let mut base_context = ctx.inner.base_context.clone();
    add_task_context(&mut base_context, model.common(), &ctx.thread_id);
    build_run_node_context(
        model,
        &model.deprecated_config,
        model.node_adapter(),
        None,
        &base_context,
        &ctx.inner.arg.io,
        ExecutionPhase::Run,
        None,
        ctx.runtime_config().dependencies.keys().cloned().collect(),
    )
    .0
}

fn eval(
    ctx: &TaskRunnerCtx,
    context: &BTreeMap<String, Value>,
    expression: &str,
) -> FsResult<Value> {
    ctx.env.compile_expression(expression)?.eval(context, &[])
}

fn relations(wap: &WapModel) -> FsResult<(Arc<dyn BaseRelation>, Arc<dyn BaseRelation>)> {
    Ok((
        Arc::from(wap.public_relation()?),
        Arc::from(wap.candidate_relation()?),
    ))
}

/// Rendering can set a header after the parsed model configuration was validated.
pub(crate) fn validate_runtime_sql_header(sql_header: Option<&Value>) -> FsResult<()> {
    if sql_header.is_some_and(|header| {
        !header.is_none()
            && !header.is_undefined()
            && !header.as_str().is_some_and(|text| text.trim().is_empty())
    }) {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP does not support sql_header, including headers added while rendering"
        ));
    }
    Ok(())
}

/// Check the live warehouse, not the adapter cache, before touching a working table.
pub(crate) fn preflight_stage(wap: &WapModel, ctx: &TaskRunnerCtx) -> FsResult<()> {
    if ctx
        .inner
        .materialization_resolver
        .is_custom_materialization("table", wap.model.node_adapter())
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP requires the built-in Snowflake table materialization: {}",
            wap.model.common().unique_id
        ));
    }
    let adapter = ctx
        .env
        .get_base_adapter()
        .ok_or_else(|| fs_err!(ErrorCode::Unexpected, "Missing adapter for WAP staging"))?;
    wap.validate_materialization_quoting(adapter.engine().quoting())?;
    let (target, candidate) = relations(wap)?;
    let mut context = model_context(&wap.model, ctx);
    resolved_transient(ctx, &context)?;
    inspect_relations(ctx, &mut context, &target, &candidate, false)?;
    emit_info_log_message(format!(
        "WAP: building {} in working table {}",
        target.render_self_as_str(),
        candidate.render_self_as_str()
    ));
    Ok(())
}

fn inspect_relations(
    ctx: &TaskRunnerCtx,
    context: &mut BTreeMap<String, Value>,
    target: &Arc<dyn BaseRelation>,
    candidate: &Arc<dyn BaseRelation>,
    candidate_must_exist: bool,
) -> FsResult<bool> {
    let mut target_exists = false;
    for (relation, expectation) in [
        (target, RelationExpectation::OptionalTable),
        (
            candidate,
            if candidate_must_exist {
                RelationExpectation::ExistingTable
            } else {
                RelationExpectation::Absent
            },
        ),
    ] {
        let sql = match relation.adapter_type() {
            AdapterType::Snowflake => {
                dbt_adapter::metadata::snowflake::relation_lookup_sql(relation.as_ref())?
            }
            _ => return Err(fs_err!(ErrorCode::InvalidConfig, "WAP requires Snowflake")),
        };
        let objects = fetch_relation_metadata(ctx, context, sql)?;
        let name = relation
            .identifier_as_resolved_str()
            .map_err(|e| FsError::from_jinja_err(e, "WAP relation identifier"))?;
        let exists = inspect_relation_rows(&objects, &name, expectation)?;
        if exists {
            let sql = match relation.adapter_type() {
                AdapterType::Snowflake => {
                    dbt_adapter::metadata::snowflake::table_lookup_sql(relation.as_ref())?
                }
                _ => return Err(fs_err!(ErrorCode::InvalidConfig, "WAP requires Snowflake")),
            };
            let tables = fetch_relation_metadata(ctx, context, sql)?;
            inspect_table_rows(&tables, &name)?;
        }
        if matches!(expectation, RelationExpectation::OptionalTable) {
            target_exists = exists;
        }
    }
    Ok(target_exists)
}

fn fetch_relation_metadata(
    ctx: &TaskRunnerCtx,
    context: &mut BTreeMap<String, Value>,
    sql: String,
) -> FsResult<Arc<RecordBatch>> {
    context.insert("__wap_sql".to_string(), Value::from(sql));
    let result = eval(
        ctx,
        context,
        "adapter.execute(__wap_sql, auto_begin=false, fetch=true)",
    )?;
    let value = result
        .get_item_by_index(1)
        .map_err(|e| FsError::from_jinja_err(e, "WAP relation lookup"))?;
    let table = value.downcast_object::<AgateTable>().ok_or_else(|| {
        fs_err!(
            ErrorCode::Unexpected,
            "WAP relation inspection did not return a table"
        )
    })?;
    Ok(table.to_record_batch())
}

#[derive(Clone, Copy)]
enum RelationExpectation {
    Absent,
    OptionalTable,
    ExistingTable,
}

fn inspect_relation_rows(
    batch: &RecordBatch,
    identifier: &str,
    expectation: RelationExpectation,
) -> FsResult<bool> {
    let row = exact_relation_row(batch, identifier)?;
    if let Some(row) = row {
        if matches!(expectation, RelationExpectation::Absent) {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "WAP working relation '{identifier}' already exists; it will not be overwritten"
            ));
        }
        require_relation_kind(batch, row, identifier, &["TABLE"])?;
        require_disabled_flag(batch, row, identifier, "is_dynamic")?;
        // The adapter also tolerates this column being absent on accounts without
        // interactive tables. A present but unknown value cannot establish safety.
        if batch.column_by_name("is_interactive").is_some() {
            require_disabled_flag(batch, row, identifier, "is_interactive")?;
        }
    }
    if matches!(expectation, RelationExpectation::ExistingTable) && row.is_none() {
        return Err(fs_err!(
            ErrorCode::ExecutionError,
            "WAP audited working table '{identifier}' is missing; table not published"
        ));
    }
    Ok(row.is_some())
}

fn inspect_table_rows(batch: &RecordBatch, identifier: &str) -> FsResult<()> {
    let row = exact_relation_row(batch, identifier)?.ok_or_else(|| {
        fs_err!(
            ErrorCode::ExecutionError,
            "WAP table '{identifier}' disappeared during inspection; table not published"
        )
    })?;
    require_relation_kind(batch, row, identifier, &["TABLE", "TRANSIENT"])?;
    for flag in ["is_external", "is_event", "is_hybrid", "is_iceberg"] {
        require_disabled_flag(batch, row, identifier, flag)?;
    }
    if batch.column_by_name("is_immutable").is_some() {
        require_disabled_flag(batch, row, identifier, "is_immutable")?;
    }
    Ok(())
}

fn exact_relation_row(batch: &RecordBatch, identifier: &str) -> FsResult<Option<usize>> {
    let names = batch.column_values::<StringArray>("name")?;
    let mut matched = None;
    for row in 0..batch.num_rows() {
        if names.is_null(row) {
            return Err(fs_err!(
                ErrorCode::Unexpected,
                "WAP cannot inspect '{identifier}': SHOW returned a NULL relation name"
            ));
        }
        if names.value(row) == identifier && matched.replace(row).is_some() {
            return Err(fs_err!(
                ErrorCode::InvalidConfig,
                "WAP found shadowing relations for '{identifier}'; remove temporary name collisions first"
            ));
        }
    }
    Ok(matched)
}

fn require_relation_kind(
    batch: &RecordBatch,
    row: usize,
    identifier: &str,
    allowed: &[&str],
) -> FsResult<()> {
    let kinds = batch.column_values::<StringArray>("kind")?;
    if kinds.is_null(row)
        || !allowed
            .iter()
            .any(|kind| kinds.value(row).eq_ignore_ascii_case(kind))
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP requires an ordinary native table; '{identifier}' has unsupported or unknown kind: {}",
            if kinds.is_null(row) {
                "NULL"
            } else {
                kinds.value(row)
            }
        ));
    }
    Ok(())
}

fn require_disabled_flag(
    batch: &RecordBatch,
    row: usize,
    identifier: &str,
    flag: &str,
) -> FsResult<()> {
    let values = batch.column_values::<StringArray>(flag)?;
    if values.is_null(row)
        || (!values.value(row).eq_ignore_ascii_case("n")
            && !values.value(row).eq_ignore_ascii_case("false"))
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP does not support '{identifier}' with {flag}={}",
            if values.is_null(row) {
                "NULL"
            } else {
                values.value(row)
            }
        ));
    }
    Ok(())
}

fn resolved_transient(ctx: &TaskRunnerCtx, context: &BTreeMap<String, Value>) -> FsResult<bool> {
    let value = eval(ctx, context, "adapter.build_catalog_relation(config.model)")?;
    let catalog = value.downcast_object::<CatalogRelation>().ok_or_else(|| {
        fs_err!(
            ErrorCode::Unexpected,
            "WAP could not resolve the model catalog"
        )
    })?;
    if catalog.catalog_type != CatalogType::SnowflakeNative
        || catalog.table_format != TableFormat::Default
    {
        return Err(fs_err!(
            ErrorCode::InvalidConfig,
            "WAP requires a native Snowflake catalog and table format"
        ));
    }
    catalog.is_transient.ok_or_else(|| {
        fs_err!(
            ErrorCode::InvalidConfig,
            "WAP could not resolve the table lifecycle"
        )
    })
}

fn execute_sql(
    ctx: &TaskRunnerCtx,
    context: &mut BTreeMap<String, Value>,
    sql: String,
) -> FsResult<AdapterResponse> {
    context.insert("__wap_sql".to_string(), Value::from(sql));
    let result = eval(
        ctx,
        context,
        "adapter.execute(__wap_sql, auto_begin=false, fetch=false)",
    )?;
    let response = result
        .get_item_by_index(0)
        .map_err(|e| FsError::from_jinja_err(e, "WAP statement response"))?;
    Ok(AdapterResponse::try_from(response)
        .map_err(|e| FsError::from_jinja_err(e, "WAP adapter response"))?)
}

/// Only these operations may surround the public clone. Keeping the ordering in one
/// place lets failure tests exercise the same cleanup decisions as warehouse runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PublicationStep {
    Prepare,
    SetQueryTag,
    Clone,
    Finalize,
    ResetQueryTag,
    ResetOverrides,
    DropCandidate,
}

fn run_publication_steps<'a>(
    audits: impl IntoIterator<Item = (&'a str, Option<NodeStatus>)>,
    target_name: &str,
    candidate_name: &str,
    mut execute: impl FnMut(PublicationStep) -> FsResult<()>,
) -> FsResult<()> {
    use PublicationStep::*;

    require_passing_audits(audits)?;
    let mut published = false;
    let result = (|| {
        execute(Prepare)?;
        execute(SetQueryTag)?;
        let result = execute(Clone).and_then(|()| {
            published = true;
            execute(Finalize)
        });
        // Reset even when cloning or finalization failed; preserve the original error.
        let reset_tag = execute(ResetQueryTag);
        result.and(reset_tag)
    })();
    let reset_overrides = execute(ResetOverrides);
    result.and(reset_overrides).map_err(|error| {
        if published {
            let message = format!(
                "WAP: {target_name} was published; finalization failed: {error}. \
                 Working table retained: {candidate_name}"
            );
            Box::new(error.with_context(message))
        } else {
            error
        }
    })?;

    // A cleanup error cannot undo a successful publication.
    if let Err(error) = execute(DropCandidate) {
        emit_warn_log_message(
            ErrorCode::ExecutionError,
            format!(
                "WAP published {target_name}; could not remove working table {candidate_name}: {error}"
            ),
        );
    }
    Ok(())
}

fn publication_model(model: &DbtModel, transient: bool, target_exists: bool) -> DbtModel {
    let mut publication = model.clone();
    let config = &mut publication.deprecated_config.__warehouse_specific_config__;
    // CTAS resolves the catalog lifecycle, whereas the clone macro reads config.transient.
    config.transient = Some(transient);
    config.copy_grants = Some(target_exists && config.copy_grants == Some(true));
    publication
}

fn publication_context(
    model: &DbtModel,
    ctx: &TaskRunnerCtx,
    target: &Arc<dyn BaseRelation>,
    candidate: &Arc<dyn BaseRelation>,
) -> FsResult<BTreeMap<String, Value>> {
    let mut context = model_context(model, ctx);
    let target_exists = inspect_relations(ctx, &mut context, target, candidate, true)?;
    let transient = resolved_transient(ctx, &context)?;
    let publication = publication_model(model, transient, target_exists);
    context = model_context(&publication, ctx);
    context.insert(
        "__wap_candidate".to_string(),
        RelationObject::new(Arc::clone(candidate)).into_value(),
    );
    context.insert(
        "__wap_target_exists".to_string(),
        Value::from(target_exists),
    );
    Ok(context)
}

fn finalize_publication(
    model: &DbtModel,
    ctx: &TaskRunnerCtx,
    adapter: &Adapter,
    context: &mut BTreeMap<String, Value>,
    target: &Arc<dyn BaseRelation>,
) -> FsResult<()> {
    if ctx.inner.run_cache_ctx.run_cache_service_requested {
        evict_node_metadata_for_untracked_rebuild(ctx, model);
    }
    adapter
        .cache_added(&ctx.env.empty_state(), Arc::clone(target))
        .map_err(|e| FsError::from_jinja_err(e, "WAP published relation cache"))?;
    eval(
        ctx,
        context,
        "apply_grants(this, config.get('grants'), should_revoke=__wap_target_exists and config.get('copy_grants', false))",
    )?;
    if model
        .deprecated_config
        .__warehouse_specific_config__
        .automatic_clustering
        == Some(true)
        && eval(ctx, context, "config.get('cluster_by')")?.is_true()
    {
        execute_sql(
            ctx,
            context,
            format!(
                "alter table {} resume recluster",
                target.render_self_as_str()
            ),
        )?;
    }
    if should_create_latest_version_pointer(model, ctx.runtime_config()) {
        let mut base = ctx.inner.base_context.clone();
        add_task_context(&mut base, model.common(), &ctx.thread_id);
        let result = materialize_latest_version_pointer(
            model,
            model.node_adapter(),
            ctx.runtime_config(),
            &ctx.inner.materialization_resolver,
            ctx.env.clone(),
            &base,
            &ctx.inner.arg.io,
        )?;
        cache_materialization_return_value(ctx.env.clone(), &result)
            .map_err(|e| FsError::from_jinja_err(e, "WAP version pointer cache"))?;
    }
    Ok(())
}

fn publish_model(model: &DbtModel, ctx: &TaskRunnerCtx) -> FsResult<()> {
    let unique_id = &model.common().unique_id;
    let wap = ctx
        .inner
        .wap_plan
        .model(unique_id)
        .ok_or_else(|| fs_err!(ErrorCode::Unexpected, "Missing WAP plan for '{unique_id}'"))?;
    let (target, candidate) = relations(wap)?;
    let adapter = ctx
        .env
        .get_base_adapter()
        .ok_or_else(|| fs_err!(ErrorCode::Unexpected, "Missing adapter for WAP publication"))?;
    let mut context = BTreeMap::new();
    let mut overrides = Vec::new();
    let audits = wap.audit_ids.iter().map(|id| {
        let status = ctx.inner.run_stats.get(id).map(|stat| stat.status.clone());
        (id.as_str(), status)
    });

    run_publication_steps(
        audits,
        &target.render_self_as_str(),
        &candidate.render_self_as_str(),
        |step| {
            match step {
                PublicationStep::Prepare => {
                    context = publication_context(model, ctx, &target, &candidate)?;
                    overrides = apply_node_overrides(
                        &adapter,
                        model.node_adapter(),
                        model
                            .__adapter_attr__
                            .snowflake_attr
                            .as_ref()
                            .and_then(|a| a.snowflake_warehouse.clone()),
                        &model.__base_attr__.database,
                        unique_id,
                    )?;
                }
                PublicationStep::SetQueryTag => {
                    let query_tag = eval(ctx, &context, "set_query_tag()")?;
                    context.insert("__wap_original_query_tag".to_string(), query_tag);
                }
                PublicationStep::Clone => {
                    let sql = eval(
                        ctx,
                        &context,
                        "dbt_snowflake.snowflake__create_or_replace_clone(this, __wap_candidate)",
                    )?
                    .to_string();
                    adapter.cancellation_token().check_cancellation()?;
                    // A failed submission can have committed without returning a response.
                    let response = execute_sql(ctx, &mut context, sql).map_err(|error| {
                        let message = format!(
                            "WAP publication failed for {}: {error}. Publication may have committed; \
                             inspect Snowflake query history before retrying. Working table: {}",
                            target.render_self_as_str(),
                            candidate.render_self_as_str()
                        );
                        Box::new(error.with_context(message))
                    })?;
                    ctx.inner.main_adapter_responses.insert(
                        unique_id.clone(),
                        response
                            .with("wap_published", true)
                            .with("wap_candidate", candidate.render_self_as_str()),
                    );
                }
                PublicationStep::Finalize => {
                    finalize_publication(model, ctx, &adapter, &mut context, &target)?;
                }
                PublicationStep::ResetQueryTag => {
                    eval(ctx, &context, "unset_query_tag(__wap_original_query_tag)")?;
                }
                PublicationStep::ResetOverrides => {
                    reset_node_overrides(&adapter, unique_id, &overrides)?;
                }
                PublicationStep::DropCandidate => {
                    eval(ctx, &context, "adapter.drop_relation(__wap_candidate)")?;
                }
            }
            Ok(())
        },
    )
    .inspect_err(|_| {
        // Session changes can have committed even if setting/restoring the query
        // tag failed. Never hand that uncertain connection to the next model.
        drop_thread_local_connection();
    })
}

pub(crate) fn interruption_message(response: Option<&AdapterResponse>) -> &'static str {
    if response.is_some_and(|response| {
        Value::from_serialize(response)
            .get_attr("wap_published")
            .is_ok_and(|value| value.is_true())
    }) {
        "WAP table was published; finalization was interrupted. Working table retained if present"
    } else {
        "WAP interrupted before completion; working table retained if created. \
         If publication was in flight, verify its outcome in Snowflake before retrying"
    }
}

pub(crate) fn report_retained_candidate(ctx: &TaskRunnerCtx, unique_id: &str) {
    if let Some(wap) = ctx.inner.wap_plan.model(unique_id)
        && let Ok((_, candidate)) = relations(wap)
    {
        emit_info_log_message(format!(
            "WAP working table retained if created: {} (model {unique_id})",
            candidate.render_self_as_str()
        ));
    }
}

#[cfg(test)]
mod clone_tests;
#[cfg(test)]
mod header_tests;
#[cfg(test)]
mod publication_tests;
#[cfg(test)]
mod stage_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};

    #[test]
    fn wap_cancellation_preserves_confirmed_publication_outcome() {
        let published = AdapterResponse::new().with("wap_published", true);
        assert!(interruption_message(Some(&published)).contains("was published"));
        assert!(interruption_message(Some(&published)).contains("finalization was interrupted"));
        assert!(interruption_message(None).contains("verify its outcome"));
        assert!(interruption_message(Some(&AdapterResponse::new())).contains("verify its outcome"));
    }

    #[test]
    fn wap_requires_actual_pass_for_every_audit() {
        assert!(require_passing_audits([("a", Some(NodeStatus::TestPassed))]).is_ok());
        for status in [
            None,
            Some(NodeStatus::TestWarned),
            Some(NodeStatus::Errored),
            Some(NodeStatus::SkippedUpstreamFailed),
            Some(NodeStatus::StaticallyCheckedDataTest),
            Some(NodeStatus::ReusedNoChanges("cached".to_string())),
        ] {
            assert!(
                require_passing_audits([
                    ("passed", Some(NodeStatus::TestPassed)),
                    ("blocking", status),
                ])
                .is_err()
            );
        }
        assert!(require_passing_audits([]).is_err());
    }

    fn objects(names: &[&str], kinds: &[&str], dynamic: &[&str]) -> RecordBatch {
        let columns = ["name", "kind", "is_dynamic"];
        let schema = Schema::new(
            columns
                .map(|name| Field::new(name, DataType::Utf8, false))
                .to_vec(),
        );
        RecordBatch::try_new(
            Arc::new(schema),
            [names, kinds, dynamic]
                .into_iter()
                .map(|values| Arc::new(StringArray::from(values.to_vec())) as _)
                .collect(),
        )
        .unwrap()
    }

    fn metadata_row(columns: &[(&str, Option<&str>)]) -> RecordBatch {
        let schema = Schema::new(
            columns
                .iter()
                .map(|(name, _)| Field::new(*name, DataType::Utf8, true))
                .collect::<Vec<_>>(),
        );
        RecordBatch::try_new(
            Arc::new(schema),
            columns
                .iter()
                .map(|(_, value)| Arc::new(StringArray::from(vec![*value])) as _)
                .collect(),
        )
        .unwrap()
    }

    fn table(kind: &str, overrides: &[(&str, Option<&str>)]) -> RecordBatch {
        let mut columns = vec![
            ("name", Some("PUBLIC")),
            ("kind", Some(kind)),
            ("is_external", Some("N")),
            ("is_event", Some("N")),
            ("is_hybrid", Some("N")),
            ("is_iceberg", Some("N")),
            ("is_immutable", Some("N")),
        ];
        for (name, value) in &mut columns {
            if let Some((_, replacement)) = overrides.iter().find(|(flag, _)| *flag == *name) {
                *value = *replacement;
            }
        }
        metadata_row(&columns)
    }

    #[test]
    fn wap_preflight_preserves_collisions_and_unsupported_targets() {
        use RelationExpectation::{Absent, ExistingTable, OptionalTable};
        let empty = objects(&[], &[], &[]);
        assert!(!inspect_relation_rows(&empty, "PUBLIC", OptionalTable).unwrap());
        assert!(inspect_relation_rows(&empty, "WORK", ExistingTable).is_err());
        let existing = objects(&["PUBLIC"], &["TABLE"], &["N"]);
        assert!(inspect_relation_rows(&existing, "PUBLIC", OptionalTable).unwrap());
        let ready = objects(&["PUBLIC", "WORK"], &["TABLE", "TABLE"], &["N", "N"]);
        assert!(inspect_relation_rows(&ready, "WORK", Absent).is_err());
        assert!(inspect_relation_rows(&ready, "WORK", ExistingTable).unwrap());
        for kind in ["VIEW", "TEMPORARY", "EXTERNAL TABLE"] {
            assert!(
                inspect_relation_rows(
                    &objects(&["PUBLIC"], &[kind], &["N"]),
                    "PUBLIC",
                    OptionalTable
                )
                .is_err()
            );
        }
        assert!(
            inspect_relation_rows(
                &objects(&["PUBLIC"], &["TABLE"], &["Y"]),
                "PUBLIC",
                OptionalTable
            )
            .is_err()
        );
        assert!(
            inspect_relation_rows(
                &objects(&["PUBLIC", "PUBLIC"], &["TABLE", "TABLE"], &["N", "N"]),
                "PUBLIC",
                OptionalTable
            )
            .is_err()
        );
    }

    #[test]
    fn wap_requires_native_permanent_or_transient_table_details() {
        for kind in ["TABLE", "TRANSIENT"] {
            assert!(inspect_table_rows(&table(kind, &[]), "PUBLIC").is_ok());
        }
        for kind in [
            "TEMPORARY",
            "EXTERNAL TABLE",
            "VIEW",
            "INTERACTIVE TABLE",
            "",
        ] {
            assert!(inspect_table_rows(&table(kind, &[]), "PUBLIC").is_err());
        }
        for flag in [
            "is_external",
            "is_event",
            "is_hybrid",
            "is_iceberg",
            "is_immutable",
        ] {
            assert!(inspect_table_rows(&table("TABLE", &[(flag, Some("Y"))]), "PUBLIC").is_err());
        }
        assert!(
            inspect_table_rows(
                &table("TABLE", &[("name", Some("PUBLIC_SUFFIX"))]),
                "PUBLIC"
            )
            .is_err()
        );
        assert!(
            inspect_table_rows(
                &objects(&["PUBLIC", "PUBLIC"], &["TABLE", "TABLE"], &["N", "N"]),
                "PUBLIC"
            )
            .is_err()
        );
    }

    #[test]
    fn wap_rejects_missing_or_unknown_type_metadata() {
        for flag in ["is_external", "is_event", "is_hybrid", "is_iceberg"] {
            let batch = table("TABLE", &[]);
            let indices = batch
                .schema()
                .fields()
                .iter()
                .enumerate()
                .filter_map(|(index, field)| (field.name() != flag).then_some(index))
                .collect::<Vec<_>>();
            assert!(inspect_table_rows(&batch.project(&indices).unwrap(), "PUBLIC").is_err());
            for value in [None, Some(""), Some("unknown")] {
                assert!(inspect_table_rows(&table("TABLE", &[(flag, value)]), "PUBLIC").is_err());
            }
            assert!(
                inspect_table_rows(&table("TABLE", &[(flag, Some("false"))]), "PUBLIC").is_ok()
            );
        }
        for field in ["name", "kind"] {
            assert!(inspect_table_rows(&table("TABLE", &[(field, None)]), "PUBLIC").is_err());
        }
        for value in [None, Some(""), Some("unknown"), Some("Y")] {
            let batch = metadata_row(&[
                ("name", Some("PUBLIC")),
                ("kind", Some("TABLE")),
                ("is_dynamic", value),
            ]);
            assert!(
                inspect_relation_rows(&batch, "PUBLIC", RelationExpectation::OptionalTable)
                    .is_err()
            );
        }
        let missing_dynamic = metadata_row(&[("name", Some("PUBLIC")), ("kind", Some("TABLE"))]);
        assert!(
            inspect_relation_rows(
                &missing_dynamic,
                "PUBLIC",
                RelationExpectation::OptionalTable
            )
            .is_err()
        );
    }

    #[test]
    fn wap_rejects_interactive_tables_and_unknown_interactive_flags() {
        for value in [None, Some(""), Some("unknown"), Some("Y")] {
            let batch = metadata_row(&[
                ("name", Some("PUBLIC")),
                ("kind", Some("TABLE")),
                ("is_dynamic", Some("N")),
                ("is_interactive", value),
            ]);
            assert!(
                inspect_relation_rows(&batch, "PUBLIC", RelationExpectation::OptionalTable)
                    .is_err()
            );
        }
        let supported = metadata_row(&[
            ("name", Some("PUBLIC")),
            ("kind", Some("TABLE")),
            ("is_dynamic", Some("N")),
            ("is_interactive", Some("N")),
        ]);
        assert!(
            inspect_relation_rows(&supported, "PUBLIC", RelationExpectation::OptionalTable)
                .unwrap()
        );
    }
}
