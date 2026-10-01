//! `DeferRelation` mirrors dbt-core's `DeferRelation` dataclass
//! (`core/dbt/artifacts/resources/v1/components.py::DeferRelation`).
//!
//! When a project is run with `--defer --state <prior_manifest>`, dbt-core
//! attaches a `defer_relation` to every refable node (model / seed / snapshot)
//! that has a counterpart in the prior-state manifest. It surfaces in two
//! places:
//!
//! 1. The persisted `manifest.json` — each refable node carries an optional
//!    `defer_relation` field with the prior-state identity + config.
//! 2. The Jinja `graph.nodes[*].defer_relation` runtime value — used by
//!    macros like `dbt clone` and `get_fixture_sql`.
//!
//! Generic over the per-resource config type (`ManifestModelConfig`,
//! `ManifestSeedConfig`, `ManifestSnapshotConfig`) so each manifest node
//! type carries the correctly-typed config dict.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::schemas::manifest::manifest_nodes::{
    ManifestModelConfig, ManifestSeedConfig, ManifestSnapshotConfig,
};
use crate::schemas::{DbtModel, DbtSeed, DbtSnapshot};

type YmlValue = dbt_yaml::Value;

// Note: deliberately *no* `#[skip_serializing_none]` — dbt-core emits
// `null` for None Option fields (`compiled_code`, `database`, `relation_name`)
// rather than omitting them, and we match that wire shape.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DeferRelation<C> {
    pub database: Option<String>,
    pub schema: String,
    pub alias: String,
    pub relation_name: Option<String>,
    /// Lowercase resource type as in dbt-core's `NodeType` enum
    /// (e.g. `"model"`, `"seed"`, `"snapshot"`).
    pub resource_type: String,
    pub name: String,
    /// Always emitted, defaults to empty string when absent — matches
    /// dbt-core where `description: str` is non-optional.
    #[serde(default)]
    pub description: String,
    /// `None` for seeds (mirrors dbt-core's
    /// `compiled_code=node.compiled_code if not isinstance(node, SeedNode) else None`).
    pub compiled_code: Option<String>,
    #[serde(default)]
    pub meta: IndexMap<String, YmlValue>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub config: Option<C>,
}

/// Build a `DeferRelation` for a model from the prior-state representation.
/// Mirrors dbt-core's `Manifest.merge_from_artifact` algorithm. (#1366)
pub fn defer_relation_for_model(deferred: &DbtModel) -> DeferRelation<ManifestModelConfig> {
    let base = &deferred.__base_attr__;
    let common = &deferred.__common_attr__;
    DeferRelation {
        database: (!base.database.is_empty()).then(|| base.database.clone()),
        schema: base.schema.clone(),
        alias: base.alias.clone(),
        relation_name: base.relation_name.clone(),
        resource_type: "model".to_string(),
        name: common.name.clone(),
        description: common.description.clone().unwrap_or_default(),
        // TODO: DbtModel doesn't currently round-trip `compiled_code` through
        // state load (see `manifest_model_to_dbt_model`). When it does, pull
        // it from the deferred node here. For now this is None, matching a
        // parse-only state manifest; a compile-state manifest will diverge.
        compiled_code: None,
        meta: common.meta.clone(),
        tags: common.tags.clone(),
        config: Some(deferred.deprecated_config.clone().into()),
    }
}

pub fn defer_relation_for_snapshot(
    deferred: &DbtSnapshot,
) -> DeferRelation<ManifestSnapshotConfig> {
    let base = &deferred.__base_attr__;
    let common = &deferred.__common_attr__;
    DeferRelation {
        database: (!base.database.is_empty()).then(|| base.database.clone()),
        schema: base.schema.clone(),
        alias: base.alias.clone(),
        relation_name: base.relation_name.clone(),
        resource_type: "snapshot".to_string(),
        name: common.name.clone(),
        description: common.description.clone().unwrap_or_default(),
        compiled_code: deferred.compiled_code.clone(),
        meta: common.meta.clone(),
        tags: common.tags.clone(),
        config: Some(deferred.deprecated_config.clone().into()),
    }
}

pub fn defer_relation_for_seed(deferred: &DbtSeed) -> DeferRelation<ManifestSeedConfig> {
    let base = &deferred.__base_attr__;
    let common = &deferred.__common_attr__;
    DeferRelation {
        database: (!base.database.is_empty()).then(|| base.database.clone()),
        schema: base.schema.clone(),
        alias: base.alias.clone(),
        relation_name: base.relation_name.clone(),
        resource_type: "seed".to_string(),
        name: common.name.clone(),
        description: common.description.clone().unwrap_or_default(),
        // dbt-core hardcodes None for seeds in `merge_from_artifact`.
        compiled_code: None,
        meta: common.meta.clone(),
        tags: common.tags.clone(),
        config: Some(deferred.deprecated_config.clone().into()),
    }
}
