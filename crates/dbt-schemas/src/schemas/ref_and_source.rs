use dbt_common::CodeLocationWithFile;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use super::serde::NodeVersion;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, DbtSchema)]
#[serde(rename_all = "snake_case")]
pub struct DbtRef {
    pub name: String,
    pub package: Option<String>,
    pub version: Option<NodeVersion>,
    #[serde(skip_serializing)]
    pub location: Option<CodeLocationWithFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DbtSourceWrapper {
    pub source: Vec<String>,
    pub location: Option<CodeLocationWithFile>,
}

pub fn serialize_dbt_function_refs<S>(
    function_refs: &[DbtRef],
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    function_refs
        .iter()
        .map(|function_ref| match &function_ref.package {
            Some(package) => vec![package.clone(), function_ref.name.clone()],
            None => vec![function_ref.name.clone()],
        })
        .collect::<Vec<_>>()
        .serialize(serializer)
}

pub fn deserialize_dbt_function_refs<'de, D>(deserializer: D) -> Result<Vec<DbtRef>, D::Error>
where
    D: Deserializer<'de>,
{
    match dbt_yaml::Value::deserialize(deserializer)? {
        dbt_yaml::Value::Null(_) => Ok(Vec::new()),
        dbt_yaml::Value::Sequence(values, _) => values
            .into_iter()
            .map(deserialize_dbt_function_ref)
            .collect::<Result<Vec<_>, _>>()
            .map_err(de::Error::custom),
        _ => Err(de::Error::custom(
            "expected an array of function dependencies",
        )),
    }
}

fn deserialize_dbt_function_ref(value: dbt_yaml::Value) -> Result<DbtRef, String> {
    match value {
        dbt_yaml::Value::Mapping(_, _) => {
            let mut function_ref =
                dbt_yaml::from_value::<DbtRef>(value).map_err(|err| err.to_string())?;
            function_ref.location = None;
            Ok(function_ref)
        }
        dbt_yaml::Value::Sequence(args, _) => match args.len() {
            1 => Ok(DbtRef {
                name: function_arg_to_string(&args[0])?,
                package: None,
                version: None,
                location: None,
            }),
            2 => Ok(DbtRef {
                name: function_arg_to_string(&args[1])?,
                package: Some(function_arg_to_string(&args[0])?),
                version: None,
                location: None,
            }),
            len => Err(format!(
                "function dependencies must be [name] or [package, name], got {len} items"
            )),
        },
        _ => Err("function dependencies must be arrays or objects".to_string()),
    }
}

fn function_arg_to_string(value: &dbt_yaml::Value) -> Result<String, String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .ok_or_else(|| "function dependency arguments must be strings".to_string())
}

impl Serialize for DbtSourceWrapper {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.source.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for DbtSourceWrapper {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let source = Vec::deserialize(deserializer)?;
        Ok(DbtSourceWrapper {
            source,
            location: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[derive(Debug, Serialize, Deserialize)]
    struct TestFunctionDeps {
        #[serde(
            default,
            serialize_with = "serialize_dbt_function_refs",
            deserialize_with = "deserialize_dbt_function_refs"
        )]
        functions: Vec<DbtRef>,
    }

    fn function_ref(name: &str, package: Option<&str>) -> DbtRef {
        DbtRef {
            name: name.to_string(),
            package: package.map(ToOwned::to_owned),
            version: None,
            location: None,
        }
    }

    #[test]
    fn deserialize_core_style_function_dependencies() {
        let parsed: TestFunctionDeps = serde_json::from_value(json!({
            "functions": [["is_positive_int"], ["utils", "normalize_int"]]
        }))
        .unwrap();

        assert_eq!(
            parsed.functions,
            vec![
                function_ref("is_positive_int", None),
                function_ref("normalize_int", Some("utils"))
            ]
        );
    }

    #[test]
    fn deserialize_legacy_fusion_function_dependencies() {
        let parsed: TestFunctionDeps = serde_json::from_value(json!({
            "functions": [{"name": "is_positive_int", "package": null, "version": null}]
        }))
        .unwrap();

        assert_eq!(
            parsed.functions,
            vec![function_ref("is_positive_int", None)]
        );
    }

    #[test]
    fn serialize_function_dependencies_in_core_shape() {
        let value = serde_json::to_value(TestFunctionDeps {
            functions: vec![
                function_ref("is_positive_int", None),
                function_ref("normalize_int", Some("utils")),
            ],
        })
        .unwrap();

        assert_eq!(
            value,
            json!({
                "functions": [["is_positive_int"], ["utils", "normalize_int"]]
            })
        );
    }
}
