//! Config field inheritance and merge semantics.
//!
//! Defines the [`DefaultTo`] trait and [`ReplaceIfNone`] marker, plus per-type
//! [`DefaultTo`] impls that encode each field type's merge strategy (replace-if-none,
//! union-merge, deep-merge, append, etc.). Also houses the [`Tags`] and [`Packages`]
//! newtypes that carry merge semantics for `StringOrArrayOfStrings` fields.

use std::collections::BTreeMap;

use dbt_common::serde_utils::Omissible;
use dbt_proc_macros::StringOrArrayNewtype;
use dbt_yaml::{Spanned, Verbatim};
use indexmap::IndexMap;
use schemars::JsonSchema;

use crate::schemas::common::{DbtQuoting, DocsConfig, Hooks, merge_meta, merge_tags, merge_vec};
use crate::schemas::properties::model_properties::{DataTestState, ModelState};
use crate::schemas::serde::{
    IndexesConfig, OmissibleGrantConfig, PrimaryKeyConfig, StringOrArrayOfStrings,
};

type YmlValue = dbt_yaml::Value;

/// Trait for fields that can inherit from a parent config value.
pub trait DefaultTo {
    fn inherit_from(&mut self, parent: &Self);
}

/// Marker trait for types that use replace-if-none semantics in DefaultTo.
///
/// Implement this for any type whose `Option<T>` fields should simply be replaced
/// by the parent value when the child is `None`. Types with custom merge logic
/// (DbtQuoting, DocsConfig, IndexMap meta, etc.) must NOT implement this marker —
/// they get their own `impl DefaultTo for Option<T>`.
pub trait ReplaceIfNone: Clone {}

impl<T: ReplaceIfNone> DefaultTo for Option<T> {
    fn inherit_from(&mut self, parent: &Self) {
        if self.is_none() {
            *self = parent.clone();
        }
    }
}

impl<T: Clone> DefaultTo for Omissible<T> {
    fn inherit_from(&mut self, parent: &Self) {
        if self.is_omitted() {
            *self = parent.clone();
        }
    }
}

impl<T: Clone + DefaultTo> DefaultTo for Verbatim<T> {
    fn inherit_from(&mut self, parent: &Self) {
        self.0.inherit_from(&parent.0);
    }
}

/// Specialized impl for hooks: append parent hooks rather than replace.
/// This impl is coherent because `Option<Hooks>` does NOT implement `ReplaceIfNone`,
/// so `Option<Hooks>` does NOT implement `DefaultTo` via the blanket above,
/// so `Verbatim<Option<Hooks>>` does NOT satisfy `T: DefaultTo` in the Verbatim blanket.
impl DefaultTo for Verbatim<Option<Hooks>> {
    fn inherit_from(&mut self, parent: &Self) {
        if let Some(parent_hooks) = &**parent {
            if let Some(child_hooks) = &mut **self {
                child_hooks.extend(parent_hooks);
            } else {
                *self = Verbatim::from(Some(parent_hooks.clone()));
            }
        }
    }
}

impl DefaultTo for IndexesConfig {
    fn inherit_from(&mut self, parent: &Self) {
        if self.is_none() {
            *self = parent.clone();
        }
    }
}

impl DefaultTo for PrimaryKeyConfig {
    fn inherit_from(&mut self, parent: &Self) {
        if self.is_none() {
            *self = parent.clone();
        }
    }
}

/// Field-level merge: each sub-field inherits from parent when unset.
impl DefaultTo for Option<DbtQuoting> {
    fn inherit_from(&mut self, parent: &Self) {
        if let Some(quoting) = self {
            if let Some(parent_quoting) = parent {
                quoting.default_to(parent_quoting);
            }
        } else {
            *self = *parent;
        }
    }
}

/// Field-level merge: `node_color` falls back to parent when child leaves it unset.
impl DefaultTo for Option<DocsConfig> {
    fn inherit_from(&mut self, parent: &Self) {
        if let Some(docs) = self {
            if let Some(parent_docs) = parent {
                if docs.node_color.is_none() {
                    docs.node_color = parent_docs.node_color.clone();
                }
            }
        } else {
            *self = parent.clone();
        }
    }
}

/// Field-level merge: each `state:` key falls back to the parent layer only when
/// this layer leaves it unset, so setting one key on a node does not drop the
/// others configured at a less specific layer (#16135).
impl DefaultTo for Option<ModelState> {
    fn inherit_from(&mut self, parent: &Self) {
        let Some(parent_state) = parent.as_ref() else {
            return;
        };
        let Some(state) = self.as_mut() else {
            *self = Some(parent_state.clone());
            return;
        };
        // destructured so a new `ModelState` field fails to compile until merged here
        let ModelState {
            lag_tolerance,
            require_fresh_data_from,
            evaluate_volatile_sql,
            pre_clone,
            execute_hooks_on_any_reuse,
            compare_unrendered_code,
        } = state;
        *lag_tolerance = lag_tolerance
            .take()
            .or_else(|| parent_state.lag_tolerance.clone());
        *require_fresh_data_from = require_fresh_data_from
            .take()
            .or_else(|| parent_state.require_fresh_data_from.clone());
        *evaluate_volatile_sql = evaluate_volatile_sql.or(parent_state.evaluate_volatile_sql);
        *pre_clone = pre_clone.take().or_else(|| parent_state.pre_clone.clone());
        *execute_hooks_on_any_reuse =
            execute_hooks_on_any_reuse.or(parent_state.execute_hooks_on_any_reuse);
        *compare_unrendered_code = compare_unrendered_code.or(parent_state.compare_unrendered_code);
    }
}

/// Field-level merge, as for [`Option<ModelState>`]: the subset of `state:` keys
/// data tests support merges key by key rather than as a whole object.
impl DefaultTo for Option<DataTestState> {
    fn inherit_from(&mut self, parent: &Self) {
        let Some(parent_state) = parent.as_ref() else {
            return;
        };
        let Some(state) = self.as_mut() else {
            *self = Some(parent_state.clone());
            return;
        };
        let DataTestState {
            require_fresh_data_from,
            evaluate_volatile_sql,
            compare_unrendered_code,
        } = state;
        *require_fresh_data_from = require_fresh_data_from
            .take()
            .or_else(|| parent_state.require_fresh_data_from.clone());
        *evaluate_volatile_sql = evaluate_volatile_sql.or(parent_state.evaluate_volatile_sql);
        *compare_unrendered_code = compare_unrendered_code.or(parent_state.compare_unrendered_code);
    }
}

/// Meta merge: child keys win, missing keys fall back to parent.
impl DefaultTo for Option<IndexMap<String, YmlValue>> {
    fn inherit_from(&mut self, parent: &Self) {
        *self = merge_meta(parent.clone(), self.take());
    }
}

// `#[derive(StringOrArrayNewtype)]` (defined in `dbt-proc-macros`) generates the
// `AsStringOrArrayOfStrings` impl and shared accessors for newtypes wrapping
// `Option<StringOrArrayOfStrings>`, plus `Serialize`/`Deserialize` impls that always
// normalize `Some` to an array. `DefaultTo::inherit_from` is written by hand per type
// since each has genuinely different merge semantics.

/// Tags: parent (less specific) values first, then child — matches dbt-core
/// additive inheritance order. Do not alphabetically sort (dbt-labs/dbt-core#15590).
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, StringOrArrayNewtype)]
#[serde(transparent)]
#[string_or_array(none_as_empty_list = true)]
pub struct Tags(pub Option<StringOrArrayOfStrings>);

impl DefaultTo for Tags {
    fn inherit_from(&mut self, parent: &Self) {
        let child_vec: Option<Vec<String>> = self.0.take().map(|v| v.into());
        let parent_vec: Option<Vec<String>> = parent.0.clone().map(|v| v.into());
        self.0 = merge_tags(parent_vec, child_vec).map(StringOrArrayOfStrings::ArrayOfStrings);
    }
}

/// Classifiers: unordered set-like config — union-merge, deduplicated and sorted.
/// Unlike [`Tags`], classifier order does not carry meaning, so `merge_vec` (sorted)
/// is used instead of `merge_tags` (parent-first, order-preserving).
#[derive(Debug, Clone, Default, PartialEq, Eq, JsonSchema, StringOrArrayNewtype)]
#[serde(transparent)]
#[string_or_array(none_as_empty_list = true)]
pub struct Classifiers(pub Option<StringOrArrayOfStrings>);

impl DefaultTo for Classifiers {
    fn inherit_from(&mut self, parent: &Self) {
        let child_vec: Option<Vec<String>> = self.0.take().map(|v| v.into());
        let parent_vec: Option<Vec<String>> = parent.0.clone().map(|v| v.into());
        self.0 = merge_vec(child_vec, parent_vec).map(StringOrArrayOfStrings::ArrayOfStrings);
    }
}

impl ReplaceIfNone for StringOrArrayOfStrings {}

/// Column types merge: parent keys fill in missing child keys.
impl DefaultTo for Option<BTreeMap<Spanned<String>, String>> {
    fn inherit_from(&mut self, parent: &Self) {
        match (self, parent) {
            (Some(child), Some(parent)) => {
                for (key, value) in parent {
                    child.entry(key.clone()).or_insert_with(|| value.clone());
                }
            }
            (child @ None, Some(parent)) => {
                *child = Some(parent.clone());
            }
            (_, None) => {}
        }
    }
}

/// Packages: append parent values before child values, no dedup.
#[derive(Debug, Clone, Default, PartialEq, JsonSchema, StringOrArrayNewtype)]
#[serde(transparent)]
#[string_or_array(none_as_empty_list = true)]
pub struct Packages(pub Option<StringOrArrayOfStrings>);

impl DefaultTo for Packages {
    fn inherit_from(&mut self, parent: &Self) {
        let child_vec: Option<Vec<String>> = self.0.take().map(|p| p.into());
        let parent_vec: Option<Vec<String>> = parent.0.clone().map(|p| p.into());
        let merged = match (parent_vec, child_vec) {
            (None, None) => None,
            (Some(mut p), Some(c)) => {
                p.extend(c);
                Some(p)
            }
            (Some(p), None) => Some(p),
            (None, Some(c)) => Some(c),
        };
        self.0 = merged.map(StringOrArrayOfStrings::ArrayOfStrings);
    }
}

/// Grants merge: DictKeyAppend semantics — `+key` extends, plain key clobbers.
impl DefaultTo for OmissibleGrantConfig {
    fn inherit_from(&mut self, parent: &Self) {
        use crate::schemas::serde::OmissibleGrantConfig;

        match (self.as_mut(), parent.as_ref()) {
            (None, Some(parent_map)) => {
                *self = OmissibleGrantConfig(Omissible::Present(parent_map.clone()));
            }
            (Some(child_map), Some(parent_map)) => {
                let child_grants = &mut child_map.0;
                let parent_grants = &parent_map.0;

                let child_keys: Vec<String> = child_grants.keys().cloned().collect();

                for (parent_key, parent_value) in parent_grants.iter() {
                    let child_has_key = child_grants.contains_key(parent_key)
                        || child_grants.contains_key(&format!("+{}", parent_key));
                    if !child_has_key {
                        child_grants.insert(parent_key.clone(), parent_value.clone());
                    }
                }

                for child_key in child_keys {
                    if child_key.starts_with('+') {
                        let actual_key = child_key.trim_start_matches('+');
                        if let Some(child_value) = child_grants.swap_remove(&child_key) {
                            let child_array: Vec<String> = child_value.into();
                            if let Some(parent_value) = parent_grants.get(actual_key) {
                                let mut merged: Vec<String> = parent_value.clone().into();
                                merged.extend(child_array);
                                child_grants.insert(
                                    actual_key.to_string(),
                                    StringOrArrayOfStrings::ArrayOfStrings(merged),
                                );
                            } else {
                                child_grants.insert(
                                    actual_key.to_string(),
                                    StringOrArrayOfStrings::ArrayOfStrings(child_array),
                                );
                            }
                        }
                    }
                }
            }
            (Some(child_map), None) => {
                let child_grants = &mut child_map.0;
                let keys: Vec<String> = child_grants
                    .keys()
                    .filter(|k| k.starts_with('+'))
                    .cloned()
                    .collect();
                for key in keys {
                    let actual_key = key.trim_start_matches('+').to_string();
                    if let Some(value) = child_grants.swap_remove(&key) {
                        child_grants.insert(actual_key, value);
                    }
                }
            }
            (None, None) => {}
        }
    }
}

/// Delegate `DefaultTo` for `WarehouseSpecificNodeConfig` to its `ResolvableConfig::default_to`.
/// This allows `__warehouse_specific_config__` fields to participate in `default_to_fields`.
impl DefaultTo for crate::schemas::project::configs::common::WarehouseSpecificNodeConfig {
    fn inherit_from(&mut self, parent: &Self) {
        use crate::schemas::project::ResolvableConfig;
        ResolvableConfig::default_to(self, parent);
    }
}

// ---------------------------------------------------------------------------
// ReplaceIfNone implementations for all config field types.
// ---------------------------------------------------------------------------

// Primitives
impl ReplaceIfNone for bool {}
impl ReplaceIfNone for String {}
impl ReplaceIfNone for i32 {}
impl ReplaceIfNone for i64 {}
impl ReplaceIfNone for u64 {}
impl ReplaceIfNone for f64 {}

// dbt_yaml types
impl<T: Clone> ReplaceIfNone for Spanned<T> {}
// Free-form YmlValue fields (e.g. ClickHouse dictionary `lifetime`/`range`/`update_lag`)
// replace wholesale — no deep merge.
impl ReplaceIfNone for YmlValue {}

// std collections used as replace-if-none fields.
// BTreeMap<String, YmlValue> is distinct from BTreeMap<Spanned<String>, String>
// (column_types, handled by a special DefaultTo impl).
impl ReplaceIfNone for BTreeMap<String, YmlValue> {}
impl<T: Clone> ReplaceIfNone for Vec<T> {}
// IndexMap<String, String> for labels/resource_tags (replace-if-none).
// IndexMap<String, YmlValue> for meta uses a custom merge — do NOT add ReplaceIfNone for it.
impl ReplaceIfNone for IndexMap<String, String> {}

// dbt_common types
impl ReplaceIfNone for dbt_common::io_args::ComputeArg {}

// crate::schemas::common types
impl ReplaceIfNone for crate::schemas::common::Access {}
impl ReplaceIfNone for crate::schemas::common::ClusterConfig {}
impl ReplaceIfNone for dbt_adapter_core::AdapterType {}
// `propagate` states a node's complete set of propagation targets, so a child's
// list replaces the parent's rather than unioning with it -- matching `adapter`,
// its single-valued sibling, not `Tags`.
impl ReplaceIfNone for crate::schemas::serde::AdapterTypeOrArray {}
impl ReplaceIfNone for crate::schemas::serde::RefreshableConfig {}
impl ReplaceIfNone for crate::schemas::common::DbtBatchSize {}
impl ReplaceIfNone for crate::schemas::common::DbtContract {}
impl ReplaceIfNone for crate::schemas::common::DbtIncrementalStrategy {}
impl ReplaceIfNone for crate::schemas::common::DbtMaterialization {}
impl ReplaceIfNone for crate::schemas::common::DbtUniqueKey {}
impl ReplaceIfNone for crate::schemas::common::HardDeletes {}
impl ReplaceIfNone for crate::schemas::common::OnConfigurationChange {}
impl ReplaceIfNone for crate::schemas::common::OnError {}
impl ReplaceIfNone for crate::schemas::common::OnSchemaChange {}
impl ReplaceIfNone for crate::schemas::common::PartitionConfig {}
impl ReplaceIfNone for crate::schemas::common::PersistDocsConfig {}
impl ReplaceIfNone for crate::schemas::common::Schedule {}
impl ReplaceIfNone for crate::schemas::common::RowFilterConfig {}
impl ReplaceIfNone for crate::schemas::common::SchemaOrigin {}
impl ReplaceIfNone for crate::schemas::common::Severity {}
impl ReplaceIfNone for crate::schemas::common::StoreFailuresAs {}
impl ReplaceIfNone for crate::schemas::common::SyncConfig {}

// crate::schemas::serde types
impl ReplaceIfNone for crate::schemas::serde::PartitionsConfig {}
impl ReplaceIfNone for crate::schemas::serde::QueryTag {}
impl ReplaceIfNone for crate::schemas::serde::StringOrInteger {}

// crate::schemas::manifest types
impl ReplaceIfNone for crate::schemas::manifest::GrantAccessToTarget {}

// crate::schemas::properties types
impl ReplaceIfNone for crate::schemas::properties::FunctionKind {}
impl ReplaceIfNone for crate::schemas::properties::Volatility {}

// crate::schemas::properties::model_properties types
impl ReplaceIfNone for crate::schemas::properties::model_properties::ModelFreshness {}

// Config-internal types
impl ReplaceIfNone for crate::schemas::project::configs::check_config::SelectionFilterOn {}
impl ReplaceIfNone for crate::schemas::project::configs::function_config::FunctionSnowflakeConfig {}
impl ReplaceIfNone for crate::schemas::project::configs::model_config::DataLakeObjectCategory {}
impl ReplaceIfNone for crate::schemas::project::configs::model_config::LatestVersionPointer {}
impl ReplaceIfNone for crate::schemas::project::configs::saved_query_config::ExportConfigExportAs {}
impl ReplaceIfNone for crate::schemas::project::configs::saved_query_config::SavedQueryCache {}
impl ReplaceIfNone for crate::schemas::project::configs::snapshot_config::SnapshotMetaColumnNames {}
