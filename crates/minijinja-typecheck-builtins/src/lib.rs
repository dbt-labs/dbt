mod assets {
    #![allow(clippy::disallowed_methods)] // RustEmbed generates calls to std::path::Path::canonicalize
    use rust_embed::RustEmbed;

    #[derive(RustEmbed)]
    #[folder = "assets/"]
    pub struct Asset;
}

use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

fn load_definitions() -> Vec<Definition> {
    let filename = "builtins.sdf.yml".to_string();
    let asset = assets::Asset::get(filename.as_str());
    if let Some(asset) = asset {
        let input = std::str::from_utf8(&asset.data)
            .unwrap_or_else(|_| panic!("{filename}:: corrupted asset: non UTF-8"));

        dbt_yaml::Deserializer::from_str(input)
            .map(|doc| {
                Definition::deserialize(doc)
                    .unwrap_or_else(|e| panic!("{filename}:: corrupted asset: bad definition {e}"))
            })
            .collect()
    } else {
        vec![]
    }
}

static DEFINITIONS: OnceLock<Arc<Vec<Definition>>> = OnceLock::new();

pub fn get_definitions() -> Arc<Vec<Definition>> {
    Arc::clone(DEFINITIONS.get_or_init(|| Arc::new(load_definitions())))
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Definition {
    pub object: Option<Object>,
    pub alias: Option<Alias>,
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Object {
    pub id: String,
    pub attributes: Option<Vec<Attribute>>,
    pub call: Option<Call>,
    pub inherit_from: Option<String>,
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Alias {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Attribute {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Call {
    pub arguments: Vec<Argument>,
    pub return_type: String,
}

#[derive(Deserialize, PartialEq, Debug, Clone, Default, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Argument {
    pub name: String,
    #[serde(rename = "type")]
    pub type_: String,
    pub is_optional: bool,
}
