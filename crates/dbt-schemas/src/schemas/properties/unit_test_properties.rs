use std::collections::BTreeMap;

// Type aliases for clarity
type YmlValue = dbt_yaml::Value;

use crate::schemas::{
    common::{Expect, Given, verbatim_option_is_none},
    project::UnitTestConfig,
};
use dbt_common::io_args::StaticAnalysisOffReason;
use dbt_yaml::DbtSchema;
use dbt_yaml::Spanned;
use dbt_yaml::Verbatim;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct UnitTestProperties {
    pub config: Option<UnitTestConfig>,
    pub description: Option<String>,
    pub expect: Expect,
    pub given: Option<Vec<Given>>,
    pub model: Spanned<String>,
    pub name: String,
    pub overrides: Option<UnitTestOverrides>,
    #[serde(skip_deserializing, default)]
    pub static_analysis_off_reason: Option<StaticAnalysisOffReason>,
    pub versions: Option<YmlValue>,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct UnitTestOverrides {
    pub env_vars: Option<BTreeMap<String, YmlValue>>,
    // `macros` overrides (e.g. freezing `run_started_at`) are rendered by
    // hand at task-execution time with the real compile context (see
    // `apply_unit_test_overrides`), not eagerly during parsing: eager
    // rendering would round-trip a Jinja expression's return value through
    // `YmlValue` and lose non-scalar values (e.g. a datetime) before the
    // real render ever runs.
    #[serde(skip_serializing_if = "verbatim_option_is_none")]
    pub macros: Verbatim<Option<BTreeMap<String, YmlValue>>>,
    pub vars: Option<BTreeMap<String, YmlValue>>,
}
