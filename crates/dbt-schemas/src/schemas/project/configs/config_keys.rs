use dbt_yaml::Value as YmlValue;
use serde::Serialize;
use std::collections::HashSet;

/// Trait for config types to provide their valid field names.
/// This is used to determine if a key is a valid config attribute
/// or a custom user-defined meta key.
pub trait ConfigKeys: Serialize + Default {
    /// Returns a set of all valid field names for this config type.
    /// Field names should not include the '+' prefix.
    ///
    /// This default implementation serializes a default instance of the type
    /// and extracts all keys from the resulting YAML mapping.
    fn valid_field_names() -> HashSet<String> {
        let default_instance = Self::default();
        let serialized = dbt_yaml::to_value(&default_instance)
            .expect("Failed to serialize config for field extraction");

        let mut field_names = HashSet::new();

        if let YmlValue::Mapping(map, _) = serialized {
            for (key, _) in map {
                if let YmlValue::String(key_str, _) = key {
                    field_names.insert(key_str);
                }
            }
        }

        field_names
    }
}
