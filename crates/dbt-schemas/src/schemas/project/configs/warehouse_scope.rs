//! Warehouse-key applicability and validation.

use dbt_common::ErrorCode;
use dbt_common::fs_err;
use dbt_common::tracing::dbt_emit::emit_warn_log_from_fs_error;

use crate::schemas::project::configs::common::WarehouseSpecificNodeConfig;

/// Status of a warehouse config key for a resource type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyStatus {
    /// Applied without a warning.
    Valid,
    /// Applied with a deprecation warning.
    Stale,
    /// The shared resolved config accepts this key, but `dbt_project.yml` does not.
    ResolvedOnly,
    /// Warned and ignored.
    Invalid,
    /// Not a warehouse key; use normal unknown-key handling.
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WarningEmission {
    Emit,
    Suppress,
}

/// Maps resolved config types to their resource type for conformance tests.
#[allow(dead_code)]
pub(crate) trait WarehouseConfigResource {
    const NODE_TYPE: dbt_telemetry::NodeType;
}

pub fn resolved_surface_key_status(resource: dbt_telemetry::NodeType, key: &str) -> KeyStatus {
    match WarehouseSpecificNodeConfig::key_status(resource, key) {
        KeyStatus::ResolvedOnly => KeyStatus::Stale,
        status => status,
    }
}

pub fn project_surface_key_status(resource: dbt_telemetry::NodeType, key: &str) -> KeyStatus {
    match WarehouseSpecificNodeConfig::key_status(resource, key) {
        KeyStatus::ResolvedOnly => KeyStatus::Invalid,
        status => status,
    }
}

/// Warns for deprecated keys and removes invalid keys before deserialization.
pub fn warn_and_strip_deprecated_warehouse_keys(
    mapping: &mut dbt_yaml::Mapping,
    context: Option<&str>,
    resource: dbt_telemetry::NodeType,
    warning_emission: WarningEmission,
    status_for: impl Fn(&str) -> KeyStatus,
) {
    mapping.retain(|key, _value| {
        let dbt_yaml::Value::String(key_str, span) = key else {
            return true;
        };
        match status_for(key_str) {
            KeyStatus::Valid | KeyStatus::Unknown => true,
            KeyStatus::Stale | KeyStatus::ResolvedOnly => {
                if warning_emission == WarningEmission::Emit {
                    emit_deprecation_warning(key_str, context, resource, span, true);
                }
                true
            }
            KeyStatus::Invalid => {
                if warning_emission == WarningEmission::Emit {
                    emit_deprecation_warning(key_str, context, resource, span, false);
                }
                false
            }
        }
    });
}

fn emit_deprecation_warning(
    key: &str,
    context: Option<&str>,
    resource: dbt_telemetry::NodeType,
    span: &dbt_yaml::Span,
    still_applied: bool,
) {
    let outcome = if still_applied {
        "deprecated and may stop being supported in a future release. It is still applied."
    } else {
        "not a valid config here and is ignored."
    };
    let scope = match context {
        Some(context) => format!(" (under `{context}`)"),
        None => String::new(),
    };
    let err = fs_err!(
        code => ErrorCode::DeprecatedConfigKey,
        loc => span.clone(),
        "Config `{}`{} for resource type `{}` is {}",
        key,
        scope,
        resource.as_static_ref(),
        outcome,
    );
    emit_warn_log_from_fs_error(*err);
}

#[cfg(test)]
mod tests {
    use super::{
        KeyStatus, WarehouseConfigResource, WarningEmission, project_surface_key_status,
        resolved_surface_key_status, warn_and_strip_deprecated_warehouse_keys,
    };
    use crate::schemas::project::configs::common::WarehouseSpecificNodeConfig as Whc;
    use crate::schemas::project::configs::data_test_config::{
        DataTestConfig, ProjectDataTestConfig,
    };
    use crate::schemas::project::configs::function_config::{
        FunctionConfig, ProjectFunctionConfig,
    };
    use crate::schemas::project::configs::model_config::{ModelConfig, ProjectModelConfig};
    use crate::schemas::project::configs::seed_config::{ProjectSeedConfig, SeedConfig};
    use crate::schemas::project::configs::snapshot_config::{
        ProjectSnapshotConfig, SnapshotConfig,
    };
    use crate::schemas::project::configs::source_config::{ProjectSourceConfig, SourceConfig};
    use crate::schemas::project::configs::unit_test_config::{
        ProjectUnitTestConfig, UnitTestConfig,
    };
    use dbt_telemetry::NodeType;
    use std::collections::HashSet;

    #[allow(dead_code)]
    #[derive(dbt_proc_macros::WarehouseScope)]
    struct SyntheticScope {
        #[warehouse(valid(Model), stale(Seed), invalid(Source))]
        example: Option<String>,
    }

    #[test]
    fn derive_emits_each_lifecycle_state() {
        assert_eq!(
            SyntheticScope::key_status(NodeType::Model, "example"),
            KeyStatus::Valid
        );
        assert_eq!(
            SyntheticScope::key_status(NodeType::Seed, "example"),
            KeyStatus::Stale
        );
        assert_eq!(
            SyntheticScope::key_status(NodeType::Source, "example"),
            KeyStatus::Invalid
        );
        assert_eq!(
            SyntheticScope::key_status(NodeType::Snapshot, "example"),
            KeyStatus::ResolvedOnly
        );
    }

    #[test]
    fn resolved_only_is_surface_specific() {
        assert_eq!(
            Whc::key_status(NodeType::Source, "immutable_where"),
            KeyStatus::ResolvedOnly
        );
        assert_eq!(
            resolved_surface_key_status(NodeType::Source, "immutable_where"),
            KeyStatus::Stale
        );
        assert_eq!(
            project_surface_key_status(NodeType::Source, "immutable_where"),
            KeyStatus::Invalid
        );
    }

    #[test]
    fn known_keys_are_never_unknown() {
        for key in Whc::all_keys() {
            for resource in [
                NodeType::Model,
                NodeType::Seed,
                NodeType::Snapshot,
                NodeType::Source,
                NodeType::Test,
                NodeType::UnitTest,
                NodeType::Function,
            ] {
                assert_ne!(
                    Whc::key_status(resource, key),
                    KeyStatus::Unknown,
                    "warehouse key `{key}` reported Unknown for {resource:?}"
                );
            }
        }
    }

    #[test]
    fn unrecognized_key_is_unknown_everywhere() {
        assert_eq!(
            Whc::key_status(NodeType::Model, "not_a_real_warehouse_key"),
            KeyStatus::Unknown
        );
    }

    fn synthetic_status(key: &str) -> KeyStatus {
        match key {
            "a_valid_key" => KeyStatus::Valid,
            "a_stale_key" => KeyStatus::Stale,
            "an_invalid_key" => KeyStatus::Invalid,
            _ => KeyStatus::Unknown,
        }
    }

    fn mapping_with_keys(keys: &[&str]) -> dbt_yaml::Mapping {
        let mut mapping = dbt_yaml::Mapping::new();
        for key in keys {
            mapping.insert(
                dbt_yaml::Value::string(key.to_string()),
                dbt_yaml::Value::string("some_value".to_string()),
            );
        }
        mapping
    }

    #[test]
    fn valid_and_unknown_keys_are_left_untouched() {
        let mut mapping = mapping_with_keys(&["a_valid_key", "a_truly_unrecognized_key"]);
        warn_and_strip_deprecated_warehouse_keys(
            &mut mapping,
            Some("test"),
            NodeType::Model,
            WarningEmission::Emit,
            synthetic_status,
        );
        assert!(mapping.contains_key("a_valid_key"));
        assert!(mapping.contains_key("a_truly_unrecognized_key"));
    }

    #[test]
    fn stale_key_is_kept_not_dropped() {
        let mut mapping = mapping_with_keys(&["a_stale_key"]);
        warn_and_strip_deprecated_warehouse_keys(
            &mut mapping,
            Some("test"),
            NodeType::Model,
            WarningEmission::Emit,
            synthetic_status,
        );
        assert_eq!(
            mapping.get("a_stale_key"),
            Some(&dbt_yaml::Value::string("some_value".to_string())),
            "a Stale key's value must survive unchanged -- the canary's entire premise is that \
             behavior does not change while a key is staged for removal"
        );
    }

    #[test]
    fn invalid_key_is_removed() {
        let mut mapping = mapping_with_keys(&["an_invalid_key", "a_valid_key"]);
        warn_and_strip_deprecated_warehouse_keys(
            &mut mapping,
            Some("test"),
            NodeType::Model,
            WarningEmission::Emit,
            synthetic_status,
        );
        assert!(!mapping.contains_key("an_invalid_key"));
        assert!(mapping.contains_key("a_valid_key"));
    }

    #[test]
    fn non_string_keys_are_left_untouched() {
        let mut mapping = dbt_yaml::Mapping::new();
        mapping.insert(
            dbt_yaml::Value::from(1i64),
            dbt_yaml::Value::string("x".to_string()),
        );
        warn_and_strip_deprecated_warehouse_keys(
            &mut mapping,
            Some("test"),
            NodeType::Model,
            WarningEmission::Emit,
            synthetic_status,
        );
        assert_eq!(mapping.len(), 1);
    }

    /// Extracts warehouse fields from a project config's generated schema.
    fn declared_warehouse_keys<T: schemars::JsonSchema>() -> HashSet<String> {
        let root = schemars::r#gen::SchemaGenerator::default().into_root_schema_for::<T>();
        let object = root
            .schema
            .object
            .expect("Project<X>Config should generate an object schema");
        let all: HashSet<String> = Whc::all_keys().iter().map(|s| s.to_string()).collect();
        object
            .properties
            .into_keys()
            .filter_map(|k| {
                let stripped = k.strip_prefix('+').unwrap_or(&k).to_string();
                all.contains(&stripped).then_some(stripped)
            })
            .collect()
    }

    /// Resolved-only fields that intentionally lack a project-config counterpart.
    const KNOWN_DEPARTURES: &[(NodeType, &str)] = &[
        // Generic data tests synthesize config(description=...) from their YAML config.
        (NodeType::Test, "description"),
        // ClickHouse seed materialization reads these resolved fields directly.
        (NodeType::Seed, "engine"),
        (NodeType::Seed, "order_by"),
        // Snapshots share adapter table-creation settings with models.
        (NodeType::Snapshot, "change_tracking"),
        (NodeType::Snapshot, "data_retention_time_in_days"),
        (NodeType::Snapshot, "max_data_extension_time_in_days"),
        (NodeType::Snapshot, "storage_serialization_policy"),
        (NodeType::Snapshot, "target_file_size"),
        (NodeType::Snapshot, "iceberg_version"),
        (NodeType::Snapshot, "engine"),
        (NodeType::Snapshot, "order_by"),
        (NodeType::Snapshot, "ttl"),
        (NodeType::Snapshot, "settings"),
        (NodeType::Snapshot, "query_settings"),
        (NodeType::Snapshot, "projections"),
        (NodeType::Snapshot, "partition_by_config"),
        (NodeType::Snapshot, "distribute_by_config"),
        (NodeType::Snapshot, "primary_key_config"),
    ];

    #[test]
    fn table_matches_project_config_field_lists() {
        let cases: [(NodeType, HashSet<String>); 7] = [
            (
                ModelConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectModelConfig>(),
            ),
            (
                SourceConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectSourceConfig>(),
            ),
            (
                SeedConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectSeedConfig>(),
            ),
            (
                SnapshotConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectSnapshotConfig>(),
            ),
            (
                DataTestConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectDataTestConfig>(),
            ),
            (
                UnitTestConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectUnitTestConfig>(),
            ),
            (
                FunctionConfig::NODE_TYPE,
                declared_warehouse_keys::<ProjectFunctionConfig>(),
            ),
        ];

        let mut mismatches = Vec::new();
        for (resource, declared) in &cases {
            for key in Whc::all_keys() {
                let status = Whc::key_status(*resource, key);
                let should_be_valid = declared.contains(*key);
                let is_project_accepted = matches!(status, KeyStatus::Valid | KeyStatus::Stale);
                if should_be_valid != is_project_accepted
                    && !KNOWN_DEPARTURES.contains(&(*resource, *key))
                {
                    mismatches.push(format!(
                        "{resource:?}.{key}: table says {status:?}, but Project<X>Config {}",
                        if should_be_valid {
                            "declares it"
                        } else {
                            "does not declare it"
                        }
                    ));
                }
            }
        }

        assert!(
            mismatches.is_empty(),
            "warehouse_scope table disagrees with Project<X>Config field lists:\n{}",
            mismatches.join("\n")
        );
    }
}
