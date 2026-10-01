//! Shared helpers for `MetadataAdapter::freshness_with_overrides` impls.
//!
//! The override path (`loaded_at_field` / `loaded_at_query`) is fundamentally
//! standard SQL plus timestamp parsing — only the bulk INFORMATION_SCHEMA-style
//! query differs across adapters. This module owns the parts that don't.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{
    Array, TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray,
};
use dbt_adbc::{Connection, QueryCtx};
use dbt_common::cancellation::CancellationToken;
use dbt_schemas::schemas::relations::base::BaseRelation;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult};
use crate::metadata::{FreshnessOverride, MetadataFreshness};

/// Task input for a `freshness_with_overrides` MapReduce pass.
///
/// Adapters partition the input relations into a single `Bulk` task plus one
/// `Override` task per source-with-override, then run them through one
/// MapReduce so they share a threadpool — matching the dbt-core run-cache
/// plugin's parallelism model.
#[derive(Clone)]
pub(crate) enum FreshnessTask {
    Bulk(Vec<Arc<dyn BaseRelation>>),
    Override(Arc<dyn BaseRelation>, FreshnessOverride),
}

/// Result of one `FreshnessTask`.
pub(crate) enum FreshnessTaskResult {
    /// Pre-built per-relation freshness map from the adapter-specific bulk
    /// metadata query.
    Bulk(BTreeMap<String, MetadataFreshness>),
    /// `(semantic_fqn, last_modified_epoch_ms)`. `None` epoch = no rows / null
    /// timestamp — downstream uses absence-from-map as the signal.
    Override(String, Option<i64>),
}

/// Substitute `{{ this }}` (with optional surrounding whitespace) in `template`
/// with `rendered_relation`. Mirrors the dbt-core plugin's `loaded_at_query`
/// rendering, which in practice only requires `this`. Avoids a full Jinja env
/// for what is effectively a single-placeholder substitution in user-supplied
/// SQL.
pub(crate) fn render_this(template: &str, rendered_relation: &str) -> String {
    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end_offset) = template[i + 2..].find("}}") {
                let inner = &template[i + 2..i + 2 + end_offset];
                if inner.trim() == "this" {
                    out.push_str(rendered_relation);
                    i += 2 + end_offset + 2;
                    continue;
                }
            }
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

/// Run one per-source override query and parse its single timestamp scalar.
///
/// Renders `{{ this }}` for `FreshnessOverride::Query` or builds
/// `SELECT max({field}) FROM {relation}` for `FreshnessOverride::Field`,
/// then defensively downcasts the first column over the common timestamp
/// precisions (Snowflake returns ms, BigQuery micros, etc.).
pub(crate) fn freshness_override_sql(
    relation: &Arc<dyn BaseRelation>,
    ovr: &FreshnessOverride,
) -> String {
    let rendered_relation = relation.render_self_as_str();
    match ovr {
        FreshnessOverride::Query(query) => render_this(query, &rendered_relation),
        FreshnessOverride::Field(field) => format!(
            "SELECT max({}) AS last_modified FROM {}",
            field, rendered_relation
        ),
    }
}

pub(crate) fn run_override_sql(
    adapter: &AdapterImpl,
    conn: &mut dyn Connection,
    semantic_fqn: String,
    sql: &str,
    token: CancellationToken,
) -> AdapterResult<FreshnessTaskResult> {
    let ctx = QueryCtx::default().with_desc("Source freshness override (loaded_at_field/query)");
    let (_resp, agate_table) = adapter.query(&ctx, conn, sql, None, token)?;
    let batch = agate_table.original_record_batch();
    if batch.num_rows() == 0 || batch.num_columns() == 0 {
        return Ok(FreshnessTaskResult::Override(semantic_fqn, None));
    }
    let col = batch.column(0);
    // A NULL scalar (e.g. `max(loaded_at)` over an empty / all-null column)
    // signals "no freshness info" — same as a 0-row result — so callers can
    // distinguish absence from a real timestamp via the `Option` epoch.
    if col.is_null(0) {
        return Ok(FreshnessTaskResult::Override(semantic_fqn, None));
    }
    let epoch_ms = if let Some(ts) = col.as_any().downcast_ref::<TimestampMillisecondArray>() {
        ts.value(0)
    } else if let Some(ts) = col.as_any().downcast_ref::<TimestampMicrosecondArray>() {
        ts.value(0) / 1_000
    } else if let Some(ts) = col.as_any().downcast_ref::<TimestampNanosecondArray>() {
        ts.value(0) / 1_000_000
    } else if let Some(ts) = col.as_any().downcast_ref::<TimestampSecondArray>() {
        ts.value(0) * 1_000
    } else {
        return Err(AdapterError::new(
            AdapterErrorKind::UnexpectedResult,
            format!("freshness override returned non-timestamp first column for {semantic_fqn}"),
        ));
    };
    Ok(FreshnessTaskResult::Override(semantic_fqn, Some(epoch_ms)))
}

pub(crate) fn run_override_query(
    adapter: &AdapterImpl,
    conn: &mut dyn Connection,
    relation: &Arc<dyn BaseRelation>,
    ovr: &FreshnessOverride,
    token: CancellationToken,
) -> AdapterResult<FreshnessTaskResult> {
    let semantic_fqn = relation.semantic_fqn();
    let sql = freshness_override_sql(relation, ovr);
    run_override_sql(adapter, conn, semantic_fqn, &sql, token)
}

/// Merge one `FreshnessTaskResult` into a freshness accumulator. Use as the
/// body of a `MapReduce` reduce closure. Sources can't be views, so override
/// results are written with `is_view=false`.
pub(crate) fn apply_freshness_task_result(
    acc: &mut BTreeMap<String, MetadataFreshness>,
    res: FreshnessTaskResult,
) -> AdapterResult<()> {
    match res {
        FreshnessTaskResult::Bulk(bulk_acc) => {
            acc.extend(bulk_acc);
        }
        FreshnessTaskResult::Override(name, Some(epoch_ms)) => {
            acc.insert(name, MetadataFreshness::from_millis(epoch_ms, false)?);
        }
        FreshnessTaskResult::Override(_, None) => {
            // No rows / null timestamp → "no freshness info" for this source.
        }
    }
    Ok(())
}
