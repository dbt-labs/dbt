use crate::schemas::data_tests::DataTests;
use crate::schemas::project::FunctionConfig;
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::DbtSchema;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

/// Function kind enum with same values as UDFKind
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum FunctionKind {
    #[serde(rename = "scalar")]
    #[default]
    Scalar,
    #[serde(rename = "aggregate")]
    Aggregate,
    #[serde(rename = "table")]
    Table,
}

/// Function volatility enum - defines the function's eligibility for certain optimizations
/// Matches the Python Volatility enum from dbt-core
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Volatility {
    /// Deterministic - An deterministic function will always return the same output when given the same input.
    #[serde(rename = "deterministic")]
    Deterministic,
    /// NonDeterministic - A non-deterministic function may change the return value from evaluation to evaluation.
    /// Multiple invocations of a non-deterministic function may return different results when used in the same query.
    #[serde(rename = "non-deterministic")]
    NonDeterministic,
    #[serde(rename = "stable")]
    Stable,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct FunctionArgument {
    pub name: Option<String>,
    pub data_type: Option<String>,
    pub description: Option<String>,
    pub default_value: Option<String>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct FunctionReturnType {
    pub data_type: Option<String>,
    pub description: Option<String>,
}

/// String identifiers for the languages a `dbt function` resource can be
/// authored in. These are the values stored on `FunctionProperties::language`
/// and on the corresponding manifest node.
///TODO: This should be an enum, not a bunch of constants.
pub const FUNCTION_LANGUAGE_SQL: &str = "sql";
pub const FUNCTION_LANGUAGE_PYTHON: &str = "python";
pub const FUNCTION_LANGUAGE_JAVASCRIPT: &str = "javascript";

fn default_language() -> Option<String> {
    Some(FUNCTION_LANGUAGE_SQL.to_string())
}

/// An overload of a function with different argument signatures.
/// Each overload references a separate SQL file (via `defined_in`) that
/// contains the function body for this overload.
#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, DbtSchema)]
pub struct FunctionOverload {
    pub defined_in: String,
    pub arguments: Option<Vec<FunctionArgument>>,
    pub returns: Option<FunctionReturnType>,
    pub description: Option<String>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct FunctionProperties {
    pub config: Option<FunctionConfig>,
    pub data_tests: Option<Vec<DataTests>>,
    pub description: Option<String>,
    pub identifier: Option<String>,
    pub name: String,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    pub tests: Option<Vec<DataTests>>,
    #[serde(default = "default_language")]
    pub language: Option<String>,
    pub returns: Option<FunctionReturnType>,
    pub arguments: Option<Vec<FunctionArgument>>,
    pub overloads: Option<Vec<FunctionOverload>>,
}

impl FunctionProperties {
    pub fn empty(name: String) -> Self {
        Self {
            name,
            config: None,
            data_tests: None,
            description: None,
            identifier: None,
            static_analysis_off_reason: None,
            tests: None,
            language: default_language(),
            returns: None,
            arguments: None,
            overloads: None,
        }
    }
}

impl crate::schemas::properties::properties::GetConfig<FunctionConfig> for FunctionProperties {
    fn get_config(&self) -> Option<&FunctionConfig> {
        self.config.as_ref()
    }
}

impl crate::schemas::properties::properties::GetConfig<crate::schemas::project::ModelConfig>
    for FunctionProperties
{
    fn get_config(&self) -> Option<&crate::schemas::project::ModelConfig> {
        // Functions don't have ModelConfig, return None
        None
    }
}
