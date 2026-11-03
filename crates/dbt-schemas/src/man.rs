use crate::schemas::dbt_catalogs_v2::catalogs_v2_json_schema;
use crate::schemas::dbt_cloud::DbtCloudConfig;
use crate::schemas::packages::DbtPackages;
use crate::schemas::profiles::DbtProfiles;
use crate::schemas::project::DbtProject;
use crate::schemas::project::{
    KeyStatus, WarehouseSpecificNodeConfig, project_surface_key_status, resolved_surface_key_status,
};
use crate::schemas::properties::DbtPropertiesFile;
use crate::schemas::selectors::SelectorFile;
use dbt_common::ErrorCode;
use dbt_common::FsResult;
use dbt_common::err;
use dbt_common::io_args::EvalArgs;
use dbt_common::io_args::JsonSchemaTypes;
use dbt_common::tracing::dbt_emit::println;
use dbt_telemetry::NodeType;
use dbt_tracing::TelemetryRecord;

use strum::IntoEnumIterator;

use schemars::schema::*;
use schemars::schema::{InstanceType, Schema, SchemaObject};

use serde_json::to_string_pretty;

pub async fn execute_man_command(arg: &EvalArgs) -> FsResult<()> {
    // create an error if arg.schema.is_empty
    if arg.schema.is_empty() {
        let available_schemas: Vec<String> = JsonSchemaTypes::iter()
            .map(|s| s.to_string().to_lowercase())
            .collect();
        return err!(
            ErrorCode::InvalidArgument,
            "Please provide a --schema <SCHEMA>, where <SCHEMA> is one of {}",
            available_schemas.join(", ")
        );
    }
    for schema_type in &arg.schema {
        dbt_yaml::maybe_transformable::set_generate_pre_transformation_schema(schema_type.is_pre());
        let generator = schema_type.get_schema_settings().into_generator();
        match schema_type {
            JsonSchemaTypes::Profile(_) => {
                let mut schema = generator.into_root_schema_for::<DbtProfiles>();
                deny_additional_properties_in_root(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Project(_) => {
                let mut schema = generator.into_root_schema_for::<DbtProject>();
                deny_additional_properties_in_root(&mut schema);
                restrict_project_warehouse_config_properties(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Selector(_) => {
                let mut schema = generator.into_root_schema_for::<SelectorFile>();
                deny_additional_properties_in_root(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Schema(_) => {
                let mut schema = generator.into_root_schema_for::<DbtPropertiesFile>();
                deny_additional_properties_in_root(&mut schema);
                restrict_warehouse_config_properties(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::DbtCloud(_) => {
                let mut schema = generator.into_root_schema_for::<DbtCloudConfig>();
                deny_additional_properties_in_root(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Packages(_) => {
                let mut schema = generator.into_root_schema_for::<DbtPackages>();
                deny_additional_properties_in_root(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Dependencies(_) => {
                let mut schema = generator.into_root_schema_for::<DbtPackages>();
                deny_additional_properties_in_root(&mut schema);
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Telemetry(_) => {
                let schema = generator.into_root_schema_for::<TelemetryRecord>();
                println(to_string_pretty(&schema)?);
            }
            JsonSchemaTypes::Catalogs(_) => {
                // Built from the `catalogs.yml` parser's descriptor tables
                // (see `dbt_catalogs_v2`), not a parallel serde type tree, so
                // the schema cannot drift from the validation rules.
                println(to_string_pretty(&catalogs_v2_json_schema())?);
            }
        };
    }

    Ok(())
}

fn restrict_warehouse_config_properties(root: &mut RootSchema) {
    restrict_warehouse_config_properties_with(
        root,
        CONFIG_DEFINITIONS,
        false,
        resolved_surface_key_status,
    );
}

fn restrict_project_warehouse_config_properties(root: &mut RootSchema) {
    restrict_warehouse_config_properties_with(
        root,
        PROJECT_CONFIG_DEFINITIONS,
        true,
        project_surface_key_status,
    );
}

const PROJECT_CONFIG_DEFINITIONS: &[(&str, NodeType)] = &[
    ("ProjectModelConfig", NodeType::Model),
    ("ProjectSeedConfig", NodeType::Seed),
    ("ProjectSnapshotConfig", NodeType::Snapshot),
    ("ProjectSourceConfig", NodeType::Source),
    ("ProjectDataTestConfig", NodeType::Test),
    ("ProjectUnitTestConfig", NodeType::UnitTest),
    ("ProjectFunctionConfig", NodeType::Function),
];

const CONFIG_DEFINITIONS: &[(&str, NodeType)] = &[
    ("ModelConfig", NodeType::Model),
    ("SeedConfig", NodeType::Seed),
    ("SnapshotConfig", NodeType::Snapshot),
    ("SourceConfig", NodeType::Source),
    ("DataTestConfig", NodeType::Test),
    ("UnitTestConfig", NodeType::UnitTest),
    ("FunctionConfig", NodeType::Function),
];

fn restrict_warehouse_config_properties_with(
    root: &mut RootSchema,
    definitions: &[(&str, NodeType)],
    plus_prefixed: bool,
    status_for: impl Fn(NodeType, &str) -> KeyStatus,
) {
    for (definition_name, resource) in definitions {
        let Some(Schema::Object(schema)) = root.definitions.get_mut(*definition_name) else {
            continue;
        };
        let Some(object) = schema.object.as_mut() else {
            continue;
        };
        object.properties.retain(|key, _| {
            let warehouse_key = if plus_prefixed {
                key.strip_prefix('+')
            } else {
                Some(key.as_str())
            };
            match warehouse_key {
                Some(warehouse_key) => {
                    !WarehouseSpecificNodeConfig::all_keys().contains(&warehouse_key)
                        || status_for(*resource, warehouse_key) != KeyStatus::Invalid
                }
                None => true,
            }
        });
    }
}

/// Recursively modifies all object schemas in a `RootSchema`
/// to set "additionalProperties": false, unless the current path includes "meta"
pub fn deny_additional_properties_in_root(root: &mut RootSchema) {
    let mut path = Vec::new();
    deny_additional_properties_in_schema_object(&mut root.schema, &mut path);

    for (_name, def_schema) in root.definitions.iter_mut() {
        deny_additional_properties(def_schema, &mut path);
    }
}

// Applies the logic to a SchemaObject (used at the root)
fn deny_additional_properties_in_schema_object(
    schema_obj: &mut SchemaObject,
    path: &mut Vec<String>,
) {
    let mut schema = Schema::Object(schema_obj.clone());
    deny_additional_properties(&mut schema, path);
    if let Schema::Object(new_obj) = schema {
        *schema_obj = new_obj;
    }
}

// Recursively modifies the schema to set "additionalProperties": false
fn deny_additional_properties(schema: &mut Schema, path: &mut Vec<String>) {
    match schema {
        Schema::Object(SchemaObject {
            instance_type: Some(single_or_many),
            object: Some(validation),
            ..
        }) => {
            let types = match single_or_many {
                SingleOrVec::Single(boxed) => vec![*boxed.clone()],
                SingleOrVec::Vec(v) => v.clone(),
            };

            if types.contains(&InstanceType::Object)
                && !path.contains(&"meta".to_string())
                && !path.contains(&"column_types".to_string())
                && !path.contains(&"grants".to_string())
            {
                match validation
                    .additional_properties
                    .as_ref()
                    .map(|s| *s.clone())
                {
                    Some(Schema::Object(_)) => {}
                    _ => {
                        validation.additional_properties = Some(Box::new(Schema::Bool(false)));
                    }
                }
            }

            for (key, subschema) in validation.properties.iter_mut() {
                path.push(key.clone());
                deny_additional_properties(subschema, path);
                path.pop();
            }

            for (_key, subschema) in validation.pattern_properties.iter_mut() {
                deny_additional_properties(subschema, path);
            }
        }

        Schema::Object(SchemaObject {
            subschemas: Some(sub),
            ..
        }) => {
            for subschemas in sub
                .all_of
                .iter_mut()
                .chain(sub.any_of.iter_mut())
                .chain(sub.one_of.iter_mut())
            {
                for sub_schema in subschemas {
                    deny_additional_properties(sub_schema, path);
                }
            }
        }

        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::r#gen::SchemaSettings;

    fn assert_object_definitions(root: &RootSchema, definitions: &[(&str, NodeType)]) {
        for (name, _) in definitions {
            assert!(
                matches!(root.definitions.get(*name), Some(Schema::Object(_))),
                "missing object schema definition {name}"
            );
        }
    }

    #[test]
    fn schema_removes_invalid_warehouse_keys_per_resource() {
        let generator = SchemaSettings::draft07().into_generator();
        let mut root = generator.into_root_schema_for::<DbtPropertiesFile>();
        assert_object_definitions(&root, CONFIG_DEFINITIONS);

        restrict_warehouse_config_properties_with(
            &mut root,
            CONFIG_DEFINITIONS,
            false,
            |resource, key| {
                if resource == NodeType::Source && key == "immutable_where" {
                    KeyStatus::Invalid
                } else {
                    KeyStatus::Valid
                }
            },
        );

        let has_property = |definition: &str, property: &str| {
            let Schema::Object(schema) = &root.definitions[definition] else {
                panic!("{definition} must be an object schema");
            };
            schema
                .object
                .as_ref()
                .is_some_and(|object| object.properties.contains_key(property))
        };

        assert!(!has_property("SourceConfig", "immutable_where"));
        assert!(has_property("ModelConfig", "immutable_where"));
    }

    #[test]
    fn project_schema_removes_invalid_warehouse_keys_per_resource() {
        let generator = SchemaSettings::draft07().into_generator();
        let mut root = generator.into_root_schema_for::<DbtProject>();
        assert_object_definitions(&root, PROJECT_CONFIG_DEFINITIONS);

        restrict_warehouse_config_properties_with(
            &mut root,
            PROJECT_CONFIG_DEFINITIONS,
            true,
            |resource, key| {
                if resource == NodeType::Model && key == "partition_by" {
                    KeyStatus::Invalid
                } else {
                    KeyStatus::Valid
                }
            },
        );

        let Schema::Object(schema) = &root.definitions["ProjectModelConfig"] else {
            panic!("ProjectModelConfig must be an object schema");
        };
        assert!(
            !schema
                .object
                .as_ref()
                .is_some_and(|object| object.properties.contains_key("+partition_by"))
        );
    }
}
