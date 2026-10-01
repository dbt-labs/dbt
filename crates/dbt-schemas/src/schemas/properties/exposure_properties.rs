use crate::schemas::manifest::common::DbtOwner;
use crate::schemas::nodes::ExposureType;
use dbt_yaml::{DbtSchema, Spanned};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use crate::schemas::project::ExposureConfig;

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Debug, Clone, DbtSchema)]
pub struct ExposureProperties {
    pub config: Option<ExposureConfig>,
    pub depends_on: Option<Vec<Spanned<String>>>,
    pub description: Option<String>,
    pub label: Option<String>,
    pub maturity: Option<String>,
    pub name: String,
    pub owner: DbtOwner,
    #[serde(rename = "type")]
    pub type_: ExposureType,
    pub url: Option<String>,
}
