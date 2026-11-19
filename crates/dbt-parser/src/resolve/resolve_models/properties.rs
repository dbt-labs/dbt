//! Models in properties files: one entry for each version of a model.
//!
//! A model without versions is one entry, found under its name. A model with versions is one
//! entry for each version, found under the name of the version's file (`customers_v2`, or what
//! `defined_in` says), and one for the latest version, found under the model's name.

use crate::resolve::properties_index::{
    IndexCx, IndexedName, PackageProperties, PropertiesEntry, typed_again, validate_resource_name,
};
use crate::resolve::resolve_utils::deep_merge_yaml;
use dbt_common::FsResult;
use dbt_jinja_utils::jinja_environment::JinjaEnv;
use dbt_schemas::schemas::properties::MinimalSchemaValue;
use dbt_schemas::schemas::serde::FloatOrString;
use dbt_yaml::Verbatim;
use minijinja::Value as MinijinjaValue;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;

/// The names that the model `item` defines: one for each of its versions.
pub(crate) fn index_model(
    key: &'static str,
    item: &dbt_yaml::Value,
    cx: &IndexCx<'_>,
) -> FsResult<Vec<IndexedName>> {
    let model: MinimalSchemaValue = cx.typed(item)?;
    validate_resource_name(&model.name)?;
    let name_span = item
        .get("name")
        .map(|name| name.span().clone())
        .unwrap_or_default();
    Ok(collect_model_version_info(&model)
        .into_iter()
        .map(|(name, _)| IndexedName {
            key,
            name,
            name_span: name_span.clone(),
        })
        .collect())
}

/// Takes the entries under `models:`, by the name each is found under.
///
/// The entry of a version is a copy of its model's YAML, with the version's `config:` merged
/// into the model's. The versions of a model are read from its YAML again here: the index keeps
/// only where each is defined.
pub(crate) fn take_model_entries(
    properties: &mut PackageProperties,
    jinja_env: &JinjaEnv,
    base_ctx: &BTreeMap<String, MinijinjaValue>,
) -> FsResult<BTreeMap<String, PropertiesEntry>> {
    let mut entries = BTreeMap::new();
    // The name and the versions of each model with versions, by the file and the position of
    // the model.
    let mut versioned = BTreeMap::new();
    for indexed in properties.take_indexed("models") {
        let location = indexed.location;
        let item = properties.item(&location);
        let has_versions = item
            .get("versions")
            .is_some_and(|versions| !versions.is_null());
        let (name, version_info, schema_value) = if has_versions {
            let (model_name, versions) = match versioned.entry((location.file, location.item)) {
                Entry::Occupied(model) => model.into_mut(),
                Entry::Vacant(model) => {
                    let typed: MinimalSchemaValue = typed_again(item, jinja_env, base_ctx)?;
                    let versions = collect_model_version_info(&typed);
                    model.insert((typed.name, versions))
                }
            };
            let version_info = versions
                .iter()
                .find(|(name, _)| name == &indexed.name)
                .and_then(|(_, version_info)| version_info.clone());
            // Each version gets its own copy, so merging per version affects no sibling.
            let mut schema_value = item.clone();
            if let Some(version_info) = &version_info {
                merge_version_config_into_schema_value(
                    &mut schema_value,
                    &version_info.version_config,
                );
            }
            (model_name.clone(), version_info, schema_value)
        } else {
            (indexed.name.clone(), None, properties.take_item(&location))
        };
        entries.insert(
            indexed.name,
            PropertiesEntry {
                name,
                name_span: indexed.name_span,
                relative_path: properties.path(&location).to_path_buf(),
                schema_value,
                table_value: None,
                version_info,
                duplicate_paths: indexed.duplicate_paths,
            },
        );
    }
    Ok(entries)
}

#[derive(Debug, Clone)]
pub struct VersionInfo {
    pub version: String,
    pub latest_version: String,
    pub versioned_name: String,
    /// This version's raw `config:` mapping. Consumed only by
    /// [`merge_version_config_into_schema_value`]; a second merge would apply it twice.
    pub version_config: Verbatim<Option<dbt_yaml::Value>>,
    // TODO: Remove this and figure out more efficient way to handle this
    pub all_versions: BTreeMap<String, String>,
}

/// Deep-merges a version's `config:` onto the model-level `config:` in that version's own clone of
/// the schema.yml entry, per `deep_merge(target.config, unparsed_version.config)` in dbt-mantle
/// `core/dbt/parser/schemas.py:1093`. Both the rendered and unrendered pipelines read
/// `mpe.schema_value`, so merging here keeps them from disagreeing.
fn merge_version_config_into_schema_value(
    schema_value: &mut dbt_yaml::Value,
    version_config: &Verbatim<Option<dbt_yaml::Value>>,
) {
    let Some(version_config) = (*version_config).as_ref() else {
        return;
    };
    if !version_config.is_mapping() {
        return;
    }
    let Some(schema_mapping) = schema_value.as_mapping_mut() else {
        return;
    };
    match schema_mapping.get_mut("config") {
        Some(model_config) => deep_merge_yaml(model_config, version_config),
        None => {
            schema_mapping.insert(
                dbt_yaml::Value::string("config".to_string()),
                version_config.clone(),
            );
        }
    }
}

// Collect and build a properites config for all versions of a model
fn collect_model_version_info(model: &MinimalSchemaValue) -> Vec<(String, Option<VersionInfo>)> {
    if let Some(versions) = &model.versions {
        let mut version_entries = versions
            .iter()
            .map(|v| {
                let version = match &v.v {
                    dbt_yaml::Value::String(s, _) => Some(s.to_string()),
                    dbt_yaml::Value::Number(n, _) => Some(n.to_string()),
                    _ => None,
                }
                .unwrap_or_else(|| {
                    panic!("Version '{:?}' does not meet the required format", v.v);
                });

                let versioned_name = format!("{}_v{}", model.name, version);

                let defined_in = v
                    .defined_in
                    .as_deref()
                    .map(|s| s.strip_suffix(".sql").unwrap_or(s).to_string());

                let version_config = v.config.clone();

                (
                    version,
                    defined_in.unwrap_or(versioned_name),
                    version_config,
                )
            })
            .collect::<Vec<_>>();
        let latest_version = model
            .latest_version
            .clone()
            .map(|v| match v {
                FloatOrString::String(s) => s,
                FloatOrString::Number(n) => n.to_string(),
            })
            .unwrap_or_else(|| {
                // Try parsing as numbers first
                let numeric_versions: Vec<_> = version_entries
                    .iter()
                    .filter_map(|(v, _, _)| v.parse::<f32>().ok())
                    .collect();

                if numeric_versions.len() == version_entries.len() {
                    // If all versions are numeric, use highest number
                    numeric_versions
                        .iter()
                        .reduce(|a, b| if a > b { a } else { b })
                        .map(|n| n.to_string())
                        .expect("Versions should not be empty")
                } else {
                    // Otherwise use lexicographically last
                    version_entries
                        .iter()
                        .map(|(v, _, _)| v)
                        .max()
                        .unwrap()
                        .clone()
                }
            });

        // Find the config for the latest version from existing version entries
        let latest_version_config = version_entries
            .iter()
            .find(|(v, _, _)| v == &latest_version)
            .map(|(_, _, config)| config.clone())
            .unwrap_or_else(|| Verbatim::from(None));

        // Only add the latest version by model.name if it's not already in the list (as in, defined by a defined_in)
        if !version_entries.iter().any(|(_, d, _)| d == &model.name) {
            // how do I get the config for the latest version?
            version_entries.push((
                latest_version.clone(),
                model.name.clone(),
                latest_version_config,
            ));
        }
        version_entries
            .iter()
            .map(|(v, d, config)| {
                (
                    d.clone(),
                    Some(VersionInfo {
                        version: v.clone(),
                        latest_version: latest_version.clone(),
                        versioned_name: d.clone(),
                        all_versions: version_entries
                            .iter()
                            .map(|(v, d, _)| (v.clone(), d.clone()))
                            .collect(),
                        version_config: config.clone(),
                    }),
                )
            })
            .collect()
    } else {
        vec![(model.name.clone(), None)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resolve::properties_index::PropertiesFile;
    use crate::resolve::resolve_models::Models;
    use std::path::Path;
    use std::sync::Arc;

    /// The entries of the models in `files`, each a path and its content.
    async fn model_entries(files: &[(&str, &str)]) -> FsResult<BTreeMap<String, PropertiesEntry>> {
        let jinja_env = Arc::new(JinjaEnv::new(minijinja::Environment::new()));
        let base_ctx = Arc::new(BTreeMap::new());
        let mut properties = PackageProperties::default();
        for (path, input) in files {
            let cx = IndexCx::new(&jinja_env, &base_ctx);
            let file = PropertiesFile::read(
                input,
                Path::new(path),
                &Path::new("/project").join(path),
                None,
                &["models"],
                &[],
            )?;
            properties.index_file(&[&Models], file, &cx).await?;
        }
        take_model_entries(&mut properties, &jinja_env, &base_ctx)
    }

    fn materialized(entry: &PropertiesEntry) -> Option<&str> {
        entry
            .schema_value
            .get("config")?
            .get("materialized")?
            .as_str()
    }

    #[dbt_runtime::test]
    async fn a_model_without_versions_is_one_entry() -> FsResult<()> {
        let entries = model_entries(&[
            (
                "a.yml",
                "models:\n  - name: orders\n    description: first\n",
            ),
            (
                "b.yml",
                "models:\n  - name: orders\n    description: second\n",
            ),
        ])
        .await?;
        assert_eq!(entries.keys().collect::<Vec<_>>(), ["orders"]);
        let orders = &entries["orders"];
        assert_eq!(orders.name, "orders");
        assert!(orders.version_info.is_none());
        assert!(orders.name_span.is_valid());
        assert_eq!(orders.relative_path, Path::new("a.yml"));
        assert_eq!(orders.duplicate_paths, [Path::new("b.yml")]);
        assert_eq!(
            orders
                .schema_value
                .get("description")
                .and_then(|d| d.as_str()),
            Some("first")
        );
        Ok(())
    }

    #[dbt_runtime::test]
    async fn a_model_with_versions_is_one_entry_for_each_version() -> FsResult<()> {
        let entries = model_entries(&[(
            "a.yml",
            "models:
  - name: customers
    latest_version: 2
    config:
      materialized: view
      schema: core
    versions:
      - v: 1
      - v: 2
        defined_in: customers_latest.sql
        config:
          materialized: table
",
        )])
        .await?;
        // One entry for each version, by its file, and one for the latest by the model's name.
        assert_eq!(
            entries.keys().collect::<Vec<_>>(),
            ["customers", "customers_latest", "customers_v1"]
        );
        for (key, version) in [
            ("customers", "2"),
            ("customers_latest", "2"),
            ("customers_v1", "1"),
        ] {
            let entry = &entries[key];
            assert_eq!(entry.name, "customers");
            let version_info = entry.version_info.as_ref().expect("a version");
            assert_eq!(version_info.version, version);
            assert_eq!(version_info.latest_version, "2");
            assert_eq!(version_info.versioned_name, key);
            assert_eq!(version_info.all_versions.len(), 2);
        }
        // A version's `config:` is merged into its own copy of the model's.
        assert_eq!(materialized(&entries["customers_v1"]), Some("view"));
        assert_eq!(materialized(&entries["customers_latest"]), Some("table"));
        assert_eq!(materialized(&entries["customers"]), Some("table"));
        let schema = |entry: &PropertiesEntry| {
            entry
                .schema_value
                .get("config")?
                .get("schema")?
                .as_str()
                .map(str::to_string)
        };
        assert_eq!(
            schema(&entries["customers_latest"]).as_deref(),
            Some("core")
        );
        Ok(())
    }

    #[dbt_runtime::test]
    async fn a_name_with_a_space_is_an_error() {
        let error = model_entries(&[("a.yml", "models:\n  - name: my model\n")])
            .await
            .expect_err("the name has a space");
        assert_eq!(
            error.context,
            "Resource name 'my model' contains forbidden characters"
        );
    }
}
