/// Utilities for handling different serialization modes (Artifact vs Jinja)
use serde::Serialize;

type YmlValue = dbt_yaml::Value;

/// Serialization mode determines how Option fields are handled
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SerializationMode {
    /// Strip None/null fields to reduce size (for manifests, JSON output)
    OmitNone,
    /// Keep None fields as null for accessibility (for list command, Jinja context)
    KeepNone,
}

/// Serialize a value with the specified mode
///
/// Note: This assumes config structs do NOT have #[skip_serializing_none],
/// so None values are serialized as null by default.
pub fn serialize_with_mode<T: Serialize>(value: &T, mode: SerializationMode) -> YmlValue {
    let yml_value = dbt_yaml::to_value(value).expect("Failed to serialize to YAML");

    match mode {
        SerializationMode::KeepNone => yml_value, // Keep everything including nulls
        SerializationMode::OmitNone => strip_null_fields(yml_value), // Remove nulls to save space
    }
}

/// Recursively strips null values from a YmlValue to reduce artifact size
fn strip_null_fields(value: YmlValue) -> YmlValue {
    match value {
        YmlValue::Mapping(map, span) => {
            let filtered_map: dbt_yaml::Mapping = map
                .into_iter()
                .filter_map(|(k, v)| {
                    // Skip null values
                    if matches!(v, YmlValue::Null(_)) {
                        None
                    } else {
                        // Recursively process non-null values
                        Some((k, strip_null_fields(v)))
                    }
                })
                .collect();
            YmlValue::Mapping(filtered_map, span)
        }
        YmlValue::Sequence(seq, span) => {
            let processed_seq = seq.into_iter().map(strip_null_fields).collect();
            YmlValue::Sequence(processed_seq, span)
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indexmap::IndexMap;
    use serde::Serialize;

    // NOTE: No #[skip_serializing_none] - we want None to serialize as null
    #[derive(Serialize)]
    struct TestConfig {
        pub full_refresh: Option<bool>,
        pub enabled: Option<bool>,
        pub required_field: String,
    }

    #[derive(Serialize)]
    struct TestNode {
        pub name: String,
        #[serde(rename = "config")]
        pub deprecated_config: TestConfig,
    }

    /// Config with meta field using serialize_none_as_empty_map
    #[derive(Serialize)]
    struct TestConfigWithMeta {
        pub enabled: Option<bool>,
        #[serde(serialize_with = "crate::schemas::serde::serialize_none_as_empty_map")]
        pub meta: Option<IndexMap<String, YmlValue>>,
    }

    #[derive(Serialize)]
    struct TestNodeWithMeta {
        pub name: String,
        #[serde(rename = "config")]
        pub deprecated_config: TestConfigWithMeta,
    }

    #[test]
    fn test_omit_none_mode_strips_null() {
        let node = TestNode {
            name: "test".to_string(),
            deprecated_config: TestConfig {
                full_refresh: None,
                enabled: Some(true),
                required_field: "value".to_string(),
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // In OmitNone mode, null fields should be stripped
        if let YmlValue::Mapping(map, _) = result {
            if let Some(YmlValue::Mapping(config_map, _)) =
                map.get(YmlValue::string("config".to_string()))
            {
                // full_refresh should be missing (stripped)
                assert!(!config_map.contains_key(YmlValue::string("full_refresh".to_string())));
                // enabled should be present
                assert!(config_map.contains_key(YmlValue::string("enabled".to_string())));
            }
        }
    }

    #[test]
    fn test_keep_none_mode_keeps_null() {
        let node = TestNode {
            name: "test".to_string(),
            deprecated_config: TestConfig {
                full_refresh: None,
                enabled: Some(true),
                required_field: "value".to_string(),
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::KeepNone);

        // In KeepNone mode, null fields should be kept
        if let YmlValue::Mapping(map, _) = result {
            if let Some(YmlValue::Mapping(config_map, _)) =
                map.get(YmlValue::string("config".to_string()))
            {
                // full_refresh should be present as null
                assert!(config_map.contains_key(YmlValue::string("full_refresh".to_string())));
                // Check that it's Null
                assert!(matches!(
                    config_map.get(YmlValue::string("full_refresh".to_string())),
                    Some(YmlValue::Null(_))
                ));
                // enabled should be present
                assert!(config_map.contains_key(YmlValue::string("enabled".to_string())));
            }
        }
    }

    /// Test that meta field is always serialized as an empty map when None,
    /// even in OmitNone mode. This is required for Jinja macros that access
    /// node.config.meta.get(...) to work correctly.
    #[test]
    fn test_meta_serializes_as_empty_map_when_none() {
        let node = TestNodeWithMeta {
            name: "test_model".to_string(),
            deprecated_config: TestConfigWithMeta {
                enabled: Some(true),
                meta: None, // This should serialize as {} not be stripped
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // Verify meta is present as an empty map, not stripped
        if let YmlValue::Mapping(map, _) = result {
            let config = map
                .get(YmlValue::string("config".to_string()))
                .expect("config should be present");
            if let YmlValue::Mapping(config_map, _) = config {
                // meta should be present (not stripped)
                assert!(
                    config_map.contains_key(YmlValue::string("meta".to_string())),
                    "meta should be present even when None"
                );
                // meta should be an empty mapping
                let meta = config_map
                    .get(YmlValue::string("meta".to_string()))
                    .unwrap();
                assert!(
                    matches!(meta, YmlValue::Mapping(m, _) if m.is_empty()),
                    "meta should be an empty map, got: {:?}",
                    meta
                );
            } else {
                panic!("config should be a mapping");
            }
        } else {
            panic!("result should be a mapping");
        }
    }

    /// Test that meta field serializes correctly when it has values
    #[test]
    fn test_meta_serializes_with_values() {
        let mut meta = IndexMap::new();
        meta.insert(
            "schema_prod".to_string(),
            YmlValue::string("custom_schema".to_string()),
        );

        let node = TestNodeWithMeta {
            name: "test_model".to_string(),
            deprecated_config: TestConfigWithMeta {
                enabled: Some(true),
                meta: Some(meta),
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // Verify meta is present with the correct value
        if let YmlValue::Mapping(map, _) = result {
            let config = map
                .get(YmlValue::string("config".to_string()))
                .expect("config should be present");
            if let YmlValue::Mapping(config_map, _) = config {
                let meta = config_map
                    .get(YmlValue::string("meta".to_string()))
                    .expect("meta should be present");
                if let YmlValue::Mapping(meta_map, _) = meta {
                    assert_eq!(meta_map.len(), 1);
                    let schema_prod = meta_map
                        .get(YmlValue::string("schema_prod".to_string()))
                        .expect("schema_prod should be present");
                    assert_eq!(
                        schema_prod.as_str(),
                        Some("custom_schema"),
                        "schema_prod should have correct value"
                    );
                } else {
                    panic!("meta should be a mapping");
                }
            } else {
                panic!("config should be a mapping");
            }
        } else {
            panic!("result should be a mapping");
        }
    }

    // --- Tests for serialize_none_as_empty_list (tags field) ---

    use crate::schemas::serde::StringOrArrayOfStrings;

    /// Config with tags field using serialize_none_as_empty_list
    #[derive(Serialize)]
    struct TestConfigWithTags {
        pub enabled: Option<bool>,
        #[serde(serialize_with = "crate::schemas::nodes::serialize_none_as_empty_list")]
        pub tags: Option<StringOrArrayOfStrings>,
    }

    #[derive(Serialize)]
    struct TestNodeWithTags {
        pub name: String,
        #[serde(rename = "config")]
        pub deprecated_config: TestConfigWithTags,
    }

    /// Test that tags field is always serialized as an empty list when None,
    /// even in OmitNone mode. This is required for Jinja macros that call
    /// obj.config.tags.extend(...) to work correctly.
    /// See: https://github.com/dbt-labs/dbt-fusion/issues/1198
    #[test]
    fn test_tags_serializes_as_empty_list_when_none() {
        let node = TestNodeWithTags {
            name: "test_model".to_string(),
            deprecated_config: TestConfigWithTags {
                enabled: Some(true),
                tags: None, // This should serialize as [] not be stripped
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // Verify tags is present as an empty list, not stripped
        if let YmlValue::Mapping(map, _) = result {
            let config = map
                .get(YmlValue::string("config".to_string()))
                .expect("config should be present");
            if let YmlValue::Mapping(config_map, _) = config {
                // tags should be present (not stripped)
                assert!(
                    config_map.contains_key(YmlValue::string("tags".to_string())),
                    "tags should be present even when None"
                );
                // tags should be an empty sequence
                let tags = config_map
                    .get(YmlValue::string("tags".to_string()))
                    .unwrap();
                assert!(
                    matches!(tags, YmlValue::Sequence(s, _) if s.is_empty()),
                    "tags should be an empty list, got: {:?}",
                    tags
                );
            } else {
                panic!("config should be a mapping");
            }
        } else {
            panic!("result should be a mapping");
        }
    }

    /// Test that tags field serializes correctly when it has an array of values
    #[test]
    fn test_tags_serializes_with_array_values() {
        let node = TestNodeWithTags {
            name: "test_model".to_string(),
            deprecated_config: TestConfigWithTags {
                enabled: Some(true),
                tags: Some(StringOrArrayOfStrings::ArrayOfStrings(vec![
                    "tag1".to_string(),
                    "tag2".to_string(),
                ])),
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // Verify tags is present with the correct values
        if let YmlValue::Mapping(map, _) = result {
            let config = map
                .get(YmlValue::string("config".to_string()))
                .expect("config should be present");
            if let YmlValue::Mapping(config_map, _) = config {
                let tags = config_map
                    .get(YmlValue::string("tags".to_string()))
                    .expect("tags should be present");
                if let YmlValue::Sequence(tags_seq, _) = tags {
                    assert_eq!(tags_seq.len(), 2);
                    assert_eq!(tags_seq[0].as_str(), Some("tag1"));
                    assert_eq!(tags_seq[1].as_str(), Some("tag2"));
                } else {
                    panic!("tags should be a sequence");
                }
            } else {
                panic!("config should be a mapping");
            }
        } else {
            panic!("result should be a mapping");
        }
    }

    /// Test that tags field serializes a single string as a list with one element
    #[test]
    fn test_tags_serializes_single_string_as_list() {
        let node = TestNodeWithTags {
            name: "test_model".to_string(),
            deprecated_config: TestConfigWithTags {
                enabled: Some(true),
                tags: Some(StringOrArrayOfStrings::String("single_tag".to_string())),
            },
        };

        let result = serialize_with_mode(&node, SerializationMode::OmitNone);

        // Verify tags is present as a list with one element
        if let YmlValue::Mapping(map, _) = result {
            let config = map
                .get(YmlValue::string("config".to_string()))
                .expect("config should be present");
            if let YmlValue::Mapping(config_map, _) = config {
                let tags = config_map
                    .get(YmlValue::string("tags".to_string()))
                    .expect("tags should be present");
                if let YmlValue::Sequence(tags_seq, _) = tags {
                    assert_eq!(
                        tags_seq.len(),
                        1,
                        "single string should become a list with one element"
                    );
                    assert_eq!(tags_seq[0].as_str(), Some("single_tag"));
                } else {
                    panic!("tags should be a sequence, got: {:?}", tags);
                }
            } else {
                panic!("config should be a mapping");
            }
        } else {
            panic!("result should be a mapping");
        }
    }
}
