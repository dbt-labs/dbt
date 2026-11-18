//! Shared helpers for `MetadataAdapter::freshness_with_overrides` impls.
//!
//! The override path (`loaded_at_field` / `loaded_at_query`) is fundamentally
//! standard SQL plus timestamp parsing — only the bulk INFORMATION_SCHEMA-style
//! query differs across adapters. This module owns the parts that don't.

use crate::connection::{AdapterConnectionFactory, ConnectionGuard};
use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{
    Array, TimestampMicrosecondArray, TimestampMillisecondArray, TimestampNanosecondArray,
    TimestampSecondArray,
};
use dbt_adapter_engine::MapReduce;
use dbt_adbc::QueryCtx;
use dbt_common::ErrorCode;
use dbt_common::cancellation::{Cancellable, CancellationToken};
use dbt_common::tracing::dbt_emit::emit_warn_log_message;
use dbt_schemas::schemas::relations::base::BaseRelation;

use crate::adapter::adapter_impl::AdapterImpl;
use crate::errors::{AdapterError, AdapterErrorKind, AdapterResult, AsyncAdapterResult};
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

/// Relations that have a freshness override, paired with that override.
pub(crate) type OverrideTargets = Vec<(Arc<dyn BaseRelation>, FreshnessOverride)>;

/// Partition `relations` into bulk (no override) and per-override buckets.
///
/// Relations whose `semantic_fqn` appears in `overrides` are paired with their
/// override and collected into the second return value; all others go into the
/// first (bulk) return value.
pub(crate) fn partition_override_relations(
    relations: &[Arc<dyn BaseRelation>],
    overrides: &BTreeMap<String, FreshnessOverride>,
) -> (Vec<Arc<dyn BaseRelation>>, OverrideTargets) {
    let mut bulk = Vec::new();
    let mut targets = Vec::new();
    for relation in relations {
        if let Some(ovr) = overrides.get(&relation.semantic_fqn()) {
            targets.push((Arc::clone(relation), ovr.clone()));
        } else {
            bulk.push(Arc::clone(relation));
        }
    }
    (bulk, targets)
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
/// adding `WHERE {filter}` when set, then defensively downcasts the first
/// column over the common timestamp precisions (Snowflake returns ms,
/// BigQuery micros, etc.).
pub(crate) fn freshness_override_sql(
    relation: &Arc<dyn BaseRelation>,
    ovr: &FreshnessOverride,
) -> String {
    let rendered_relation = relation.render_self_as_str();
    match ovr {
        FreshnessOverride::Query(query) => render_this(query, &rendered_relation),
        FreshnessOverride::Field { field, filter } => match filter {
            Some(filter) => format!(
                "SELECT max({}) AS last_modified FROM {} WHERE {}",
                field, rendered_relation, filter
            ),
            None => format!(
                "SELECT max({}) AS last_modified FROM {}",
                field, rendered_relation
            ),
        },
    }
}

pub(crate) fn run_override_sql(
    adapter: &AdapterImpl,
    conn: &mut ConnectionGuard<'_>,
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
    conn: &mut ConnectionGuard<'_>,
    relation: &Arc<dyn BaseRelation>,
    ovr: &FreshnessOverride,
    token: CancellationToken,
) -> AdapterResult<FreshnessTaskResult> {
    let semantic_fqn = relation.semantic_fqn();
    let sql = freshness_override_sql(relation, ovr);
    run_override_sql(adapter, conn, semantic_fqn, &sql, token)
}

/// Run one override query per target, in parallel. A failed override query
/// leaves its source out of the result and emits a warning. Cancellation and
/// missing replay data still fail the whole call.
pub(crate) fn run_override_targets(
    adapter: &AdapterImpl,
    targets: OverrideTargets,
    token: CancellationToken,
) -> AsyncAdapterResult<'static, BTreeMap<String, MetadataFreshness>> {
    type Acc = BTreeMap<String, MetadataFreshness>;
    let factory = Box::new(AdapterConnectionFactory::new(adapter.engine().clone()));
    let adapter = adapter.clone();
    let token_clone = token.clone();
    let map_f = move |conn: &mut ConnectionGuard<'_>,
                      (relation, ovr): &(Arc<dyn BaseRelation>, FreshnessOverride)|
          -> AdapterResult<FreshnessTaskResult> {
        run_override_query(&adapter, conn, relation, ovr, token_clone.clone())
    };
    let reduce_f = move |acc: &mut Acc,
                         (relation, _ovr): (Arc<dyn BaseRelation>, FreshnessOverride),
                         res: AdapterResult<FreshnessTaskResult>|
          -> Result<(), Cancellable<AdapterError>> {
        match res {
            Ok(task_result) => apply_freshness_task_result(acc, task_result)?,
            Err(err) => warn_or_propagate_override_error(&relation.semantic_fqn(), err)?,
        }
        Ok(())
    };
    MapReduce::new(factory, Box::new(map_f), Box::new(reduce_f), None).run(Arc::new(targets), token)
}

/// Handle a failed override query. Cancellation and missing replay data are
/// returned so that a cancel stops the run and replay can fall back. Any other
/// error is a warning: the source is left out of the result, so dbt State
/// treats it as changed.
pub(crate) fn warn_or_propagate_override_error(
    semantic_fqn: &str,
    err: AdapterError,
) -> AdapterResult<()> {
    match err.kind() {
        AdapterErrorKind::Cancelled | AdapterErrorKind::ReplayDataMissing => Err(err),
        _ => {
            emit_warn_log_message(
                ErrorCode::StateServiceWarn,
                format!(
                    "dbt State freshness override query (loaded_at_field/loaded_at_query) \
                     failed for {semantic_fqn}: {err}; omitting freshness for this source"
                ),
            );
            Ok(())
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::relation::create_relation;
    use dbt_adapter_core::AdapterType;
    use dbt_schemas::dbt_types::RelationType;
    use dbt_schemas::schemas::common::ResolvedQuoting;

    fn make_relation() -> Arc<dyn BaseRelation> {
        relation_named("users")
    }

    fn relation_named(identifier: &str) -> Arc<dyn BaseRelation> {
        create_relation(
            AdapterType::Snowflake,
            "db".to_string(),
            "raw".to_string(),
            Some(identifier.to_string()),
            Some(RelationType::View),
            ResolvedQuoting::default(),
        )
        .unwrap()
        .into()
    }

    #[test]
    fn partition_routes_all_to_bulk_when_no_overrides() {
        let relations = vec![relation_named("a"), relation_named("b")];
        let (bulk, targets) = partition_override_relations(&relations, &BTreeMap::new());
        assert_eq!(bulk.len(), 2);
        assert!(targets.is_empty());
    }

    #[test]
    fn partition_routes_override_relation_to_override_bucket() {
        let relation = relation_named("src");
        let fqn = relation.semantic_fqn();
        let overrides = BTreeMap::from([(
            fqn.clone(),
            FreshnessOverride::Field {
                field: "updated_at".into(),
                filter: None,
            },
        )]);
        let (bulk, targets) = partition_override_relations(&[relation], &overrides);
        assert!(bulk.is_empty());
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0.semantic_fqn(), fqn);
        assert!(
            matches!(&targets[0].1, FreshnessOverride::Field { field, .. } if field == "updated_at")
        );
    }

    #[test]
    fn partition_mixed_relations_correctly_sorted() {
        let bulk1 = relation_named("bulk1");
        let ovr = relation_named("ovr");
        let bulk2 = relation_named("bulk2");
        let fqn_ovr = ovr.semantic_fqn();
        let relations = vec![Arc::clone(&bulk1), ovr, Arc::clone(&bulk2)];
        let overrides = BTreeMap::from([(
            fqn_ovr.clone(),
            FreshnessOverride::Query("SELECT max(ts) FROM {{ this }}".into()),
        )]);

        let (bulk, targets) = partition_override_relations(&relations, &overrides);

        assert_eq!(bulk.len(), 2);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].0.semantic_fqn(), fqn_ovr);
        // bulk order is preserved
        assert_eq!(bulk[0].semantic_fqn(), bulk1.semantic_fqn());
        assert_eq!(bulk[1].semantic_fqn(), bulk2.semantic_fqn());
    }

    #[test]
    fn field_override_without_filter_has_no_where_clause() {
        let relation = make_relation();
        let sql = freshness_override_sql(
            &relation,
            &FreshnessOverride::Field {
                field: "updated_at".to_string(),
                filter: None,
            },
        );
        assert_eq!(
            sql,
            format!(
                "SELECT max(updated_at) AS last_modified FROM {}",
                relation.render_self_as_str()
            )
        );
    }

    #[test]
    fn field_override_with_filter_adds_where_clause() {
        let relation = make_relation();
        let sql = freshness_override_sql(
            &relation,
            &FreshnessOverride::Field {
                field: "updated_at".to_string(),
                filter: Some("deleted_at is null".to_string()),
            },
        );
        assert_eq!(
            sql,
            format!(
                "SELECT max(updated_at) AS last_modified FROM {} WHERE deleted_at is null",
                relation.render_self_as_str()
            )
        );
    }

    #[test]
    fn override_error_is_warned_and_skipped() {
        let err = AdapterError::new(AdapterErrorKind::UnexpectedResult, "bad loaded_at_field");
        assert!(warn_or_propagate_override_error("db.raw.users", err).is_ok());
    }

    #[test]
    fn override_cancellation_and_missing_replay_data_are_propagated() {
        for kind in [
            AdapterErrorKind::Cancelled,
            AdapterErrorKind::ReplayDataMissing,
        ] {
            let err = AdapterError::new(kind, "stop");
            let got =
                warn_or_propagate_override_error("db.raw.users", err).expect_err("must propagate");
            assert_eq!(got.kind(), kind);
        }
    }
}
