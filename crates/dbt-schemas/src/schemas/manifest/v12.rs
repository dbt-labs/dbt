use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use serde::ser::{SerializeMap, Serializer};

use crate::schemas::{
    macros::DbtDocsMacro,
    manifest::{
        DbtNode, ManifestMetadata,
        manifest::serialize_with_resource_type,
        manifest_nodes::{
            ManifestExposure, ManifestFunction, ManifestMacro, ManifestMetric, ManifestSavedQuery,
            ManifestSemanticModel, ManifestSource, ManifestUnitTest,
        },
    },
};

use super::{DbtSelector, ManifestGroup};

struct StreamingYamlMap<'a, T> {
    map: &'a BTreeMap<String, T>,
}

impl<'a, T> Serialize for StreamingYamlMap<'a, T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = serializer.serialize_map(Some(self.map.len()))?;
        for (k, v) in self.map {
            let yml = dbt_yaml::to_value(v).map_err(serde::ser::Error::custom)?;
            m.serialize_entry(k, &yml)?;
        }
        m.end()
    }
}

struct StreamingYamlMapWithResourceType<'a, T> {
    resource_type: &'static str,
    map: &'a BTreeMap<String, T>,
}

impl<'a, T> Serialize for StreamingYamlMapWithResourceType<'a, T>
where
    T: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = serializer.serialize_map(Some(self.map.len()))?;
        for (k, v) in self.map {
            let yml = dbt_yaml::to_value(v).map_err(serde::ser::Error::custom)?;
            let yml = serialize_with_resource_type(yml, self.resource_type);
            m.serialize_entry(k, &yml)?;
        }
        m.end()
    }
}

#[derive(Debug, Default, Deserialize, Clone)]
pub struct DbtManifestV12 {
    pub metadata: ManifestMetadata,
    pub nodes: BTreeMap<String, DbtNode>,
    pub sources: BTreeMap<String, ManifestSource>,
    pub macros: BTreeMap<String, ManifestMacro>,
    pub unit_tests: BTreeMap<String, ManifestUnitTest>,
    pub docs: BTreeMap<String, DbtDocsMacro>,
    pub semantic_models: BTreeMap<String, ManifestSemanticModel>,
    pub saved_queries: BTreeMap<String, ManifestSavedQuery>,
    pub exposures: BTreeMap<String, ManifestExposure>,
    pub metrics: BTreeMap<String, ManifestMetric>,
    #[serde(default)]
    pub functions: BTreeMap<String, ManifestFunction>,
    pub child_map: BTreeMap<String, Vec<String>>,
    pub parent_map: BTreeMap<String, Vec<String>>,
    pub group_map: BTreeMap<String, Vec<String>>,
    pub disabled: BTreeMap<String, Vec<YmlValue>>,
    pub selectors: BTreeMap<String, DbtSelector>,
    pub groups: BTreeMap<String, ManifestGroup>,
}

impl DbtManifestV12 {
    pub fn into_map_compiled_sql(&self) -> HashMap<&str, Option<&str>> {
        self.nodes
            .iter()
            .filter_map(|(id, node)| match node {
                DbtNode::Model(model) => {
                    Some((id.as_str(), model.__base_attr__.compiled_code.as_deref()))
                }
                DbtNode::Test(test) => {
                    Some((id.as_str(), test.__base_attr__.compiled_code.as_deref()))
                }
                DbtNode::Snapshot(snapshot) => {
                    Some((id.as_str(), snapshot.__base_attr__.compiled_code.as_deref()))
                }
                DbtNode::Seed(seed) => {
                    Some((id.as_str(), seed.__base_attr__.compiled_code.as_deref()))
                }
                DbtNode::Operation(_operation) => None,
                DbtNode::Function(_function) => None,
                DbtNode::Analysis(analysis) => {
                    Some((id.as_str(), analysis.__base_attr__.compiled_code.as_deref()))
                }
                DbtNode::Check(check) => {
                    Some((id.as_str(), check.__base_attr__.compiled_code.as_deref()))
                }
            })
            .collect::<HashMap<_, _>>()
    }
}

impl Serialize for DbtManifestV12 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut m = serializer.serialize_map(Some(16))?;

        // `ManifestMetadata` must stay in its native shape (not flattened) so downstream tooling
        // can read `metadata.__base__.*` (notably `dbt_schema_version`).
        m.serialize_entry("metadata", &self.metadata)?;

        m.serialize_entry("nodes", &StreamingYamlMap { map: &self.nodes })?;
        m.serialize_entry(
            "sources",
            &StreamingYamlMapWithResourceType {
                resource_type: "source",
                map: &self.sources,
            },
        )?;
        m.serialize_entry(
            "macros",
            &StreamingYamlMapWithResourceType {
                resource_type: "macro",
                map: &self.macros,
            },
        )?;
        m.serialize_entry(
            "unit_tests",
            &StreamingYamlMapWithResourceType {
                resource_type: "unit_test",
                map: &self.unit_tests,
            },
        )?;

        m.serialize_entry(
            "docs",
            &StreamingYamlMapWithResourceType {
                resource_type: "doc",
                map: &self.docs,
            },
        )?;

        m.serialize_entry(
            "semantic_models",
            &StreamingYamlMapWithResourceType {
                resource_type: "semantic_model",
                map: &self.semantic_models,
            },
        )?;
        m.serialize_entry(
            "saved_queries",
            &StreamingYamlMapWithResourceType {
                resource_type: "saved_query",
                map: &self.saved_queries,
            },
        )?;
        m.serialize_entry(
            "exposures",
            &StreamingYamlMapWithResourceType {
                resource_type: "exposure",
                map: &self.exposures,
            },
        )?;
        m.serialize_entry(
            "metrics",
            &StreamingYamlMapWithResourceType {
                resource_type: "metric",
                map: &self.metrics,
            },
        )?;
        m.serialize_entry(
            "functions",
            &StreamingYamlMapWithResourceType {
                resource_type: "function",
                map: &self.functions,
            },
        )?;

        let child_map = dbt_yaml::to_value(&self.child_map).map_err(serde::ser::Error::custom)?;
        let parent_map = dbt_yaml::to_value(&self.parent_map).map_err(serde::ser::Error::custom)?;
        let group_map = dbt_yaml::to_value(&self.group_map).map_err(serde::ser::Error::custom)?;
        let disabled = dbt_yaml::to_value(&self.disabled).map_err(serde::ser::Error::custom)?;
        let selectors = dbt_yaml::to_value(&self.selectors).map_err(serde::ser::Error::custom)?;

        m.serialize_entry("child_map", &child_map)?;
        m.serialize_entry("parent_map", &parent_map)?;
        m.serialize_entry("group_map", &group_map)?;
        m.serialize_entry("disabled", &disabled)?;
        m.serialize_entry("selectors", &selectors)?;
        m.serialize_entry(
            "groups",
            &StreamingYamlMapWithResourceType {
                resource_type: "group",
                map: &self.groups,
            },
        )?;

        m.end()
    }
}
