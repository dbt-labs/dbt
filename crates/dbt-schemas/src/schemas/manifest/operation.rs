use std::any::Any;
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::nodes::{InternalDbtNode, InternalDbtNodeAttributes, IntrospectionKind};
use crate::schemas::{CommonAttributes, NodeBaseAttributes};
use dbt_adapter_core::AdapterType;
use dbt_telemetry::NodeType;

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtOperation {
    pub __common_attr__: CommonAttributes,
    #[serde(default)]
    pub __base_attr__: NodeBaseAttributes,
    pub __other__: BTreeMap<String, YmlValue>,
}

impl InternalDbtNode for DbtOperation {
    fn common(&self) -> &CommonAttributes {
        &self.__common_attr__
    }

    fn base(&self) -> &NodeBaseAttributes {
        &self.__base_attr__
    }

    fn base_mut(&mut self) -> &mut NodeBaseAttributes {
        &mut self.__base_attr__
    }

    fn common_mut(&mut self) -> &mut CommonAttributes {
        &mut self.__common_attr__
    }

    fn resource_type(&self) -> NodeType {
        NodeType::Operation
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn serialize_inner(
        &self,
        mode: crate::schemas::serialization_utils::SerializationMode,
    ) -> YmlValue {
        crate::schemas::serialization_utils::serialize_with_mode(self, mode)
    }

    fn has_same_config(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        // Operations don't have config to compare
        matches!(
            other.as_any().downcast_ref::<DbtOperation>(),
            Some(_other_operation)
        )
    }

    fn has_same_content(&self, other: &dyn InternalDbtNode, _adapter_type: AdapterType) -> bool {
        if let Some(other_operation) = other.as_any().downcast_ref::<DbtOperation>() {
            self.__common_attr__.raw_code == other_operation.__common_attr__.raw_code
        } else {
            false
        }
    }

    fn set_detected_introspection(&mut self, _introspection: IntrospectionKind) {
        // Operations don't use introspection
    }
}

impl InternalDbtNodeAttributes for DbtOperation {
    fn search_name(&self) -> String {
        self.__common_attr__.name.clone()
    }

    fn selector_string(&self) -> String {
        self.__common_attr__.fqn.join(".")
    }

    fn serialized_config(&self) -> YmlValue {
        // Operations don't have config, return empty map
        dbt_yaml::to_value(BTreeMap::<String, YmlValue>::new())
            .expect("Failed to serialize to YAML")
    }
}
