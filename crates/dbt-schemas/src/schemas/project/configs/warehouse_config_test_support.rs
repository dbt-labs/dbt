//! Shared test checks for configs that keep their warehouse options in a
//! [`LazyWarehouseSpecificNodeConfig`].

use dbt_adapter_core::AdapterType;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::schemas::project::ResolvableConfig;
use crate::schemas::project::configs::common::LazyWarehouseSpecificNodeConfig;

/// Deserializes `yaml` through the owned (`into_typed`) or borrowed (`to_typed`) `dbt_yaml`
/// path, and returns the config with the keys reported as unused.
pub fn typed_from_yaml<T: DeserializeOwned>(yaml: &str, borrowed: bool) -> (T, Vec<String>) {
    let value: dbt_yaml::Value = dbt_yaml::from_str(yaml).unwrap();
    let mut unused_keys = Vec::new();
    let collect_unused = |_: dbt_yaml::Path<'_>, key: &dbt_yaml::Value, _: &dbt_yaml::Value| {
        unused_keys.push(key.as_str().unwrap().to_string())
    };
    let no_transform = |_: &dbt_yaml::Value| -> Result<
        Option<dbt_yaml::Value>,
        Box<dyn std::error::Error + 'static + Send + Sync>,
    > { Ok(None) };
    let config = if borrowed {
        value.to_typed(collect_unused, no_transform).unwrap()
    } else {
        value.into_typed(collect_unused, no_transform).unwrap()
    };
    (config, unused_keys)
}

/// Checks, through both `dbt_yaml` paths, that a flattened warehouse key added to `base_yaml`
/// is allocated and serialized back under the same key, that an unknown key is still reported,
/// and that `base_yaml` alone leaves the warehouse config unallocated.
pub fn assert_warehouse_config_allocates_only_when_set<T: DeserializeOwned + Serialize>(
    base_yaml: &str,
    warehouse_config: impl Fn(&T) -> &LazyWarehouseSpecificNodeConfig,
) {
    for borrowed in [false, true] {
        let (config, unused_keys) = typed_from_yaml::<T>(
            &format!("{base_yaml}snowflake_warehouse: wh\nnot_a_config: 1\n"),
            borrowed,
        );
        assert_eq!(unused_keys, ["not_a_config"]);
        let warehouse_specific = warehouse_config(&config)
            .get()
            .expect("a set warehouse option is allocated");
        assert_eq!(
            warehouse_specific.snowflake_warehouse.as_deref(),
            Some("wh")
        );
        let serialized = dbt_yaml::to_value(&config).unwrap();
        assert_eq!(
            serialized.get("snowflake_warehouse").unwrap().as_str(),
            Some("wh")
        );

        let (config, unused_keys) = typed_from_yaml::<T>(base_yaml, borrowed);
        assert!(unused_keys.is_empty());
        assert!(warehouse_config(&config).get().is_none());
    }
}

/// Checks that alias canonicalization leaves a default config unallocated, and that the
/// Databricks `catalog` alias moves to `database`, freeing a warehouse config left at the default
/// and keeping one with other warehouse options allocated.
pub fn assert_catalog_alias_frees_default_warehouse_config<T: ResolvableConfig<T>>(
    warehouse_config: impl Fn(&T) -> &LazyWarehouseSpecificNodeConfig,
    database: impl Fn(&T) -> Option<&str>,
) {
    let mut config = T::default();
    config.canonicalize_adapter_aliases(AdapterType::Databricks);
    assert!(warehouse_config(&config).get().is_none());
    assert_eq!(database(&config), None);

    let (mut config, _) = typed_from_yaml::<T>("catalog: main\n", false);
    config.canonicalize_adapter_aliases(AdapterType::Databricks);
    assert_eq!(database(&config), Some("main"));
    assert!(warehouse_config(&config).get().is_none());

    let (mut config, _) = typed_from_yaml::<T>("catalog: main\nfile_format: delta\n", false);
    config.canonicalize_adapter_aliases(AdapterType::Databricks);
    assert_eq!(database(&config), Some("main"));
    let warehouse_specific = warehouse_config(&config).get().unwrap();
    assert_eq!(warehouse_specific.catalog, None);
    assert_eq!(warehouse_specific.file_format.as_deref(), Some("delta"));
}
