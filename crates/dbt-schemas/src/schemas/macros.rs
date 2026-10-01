use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

use dbt_common::path::DbtPath;
use dbt_yaml::Value;
use minijinja::{
    ArgSpec,
    machinery::Span,
    macro_unit::{MacroInfo, MacroUnit},
};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;

use super::common::DocsConfig;
use super::nodes::{InternalDbtNode, NodePathKind};

/// Macro argument as defined in v12 manifest schema
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MacroArgument {
    pub name: String,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub type_: Option<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct MacroConfig {
    #[serde(default)]
    pub meta: BTreeMap<String, Value>,
    #[serde(
        default,
        serialize_with = "crate::schemas::serde::serialize_docs_with_nulls"
    )]
    pub docs: DocsConfig,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct DbtMacro {
    pub name: String,
    pub package_name: String,
    pub path: DbtPath,
    /// Package-root-relative path from the manifest (e.g. `macros/my_macro.sql`).
    /// Present for all macros including those loaded from a serialized manifest.
    pub original_file_path: DbtPath,
    /// Absolute on-disk path, set during parse. Empty for macros loaded from a
    /// serialized manifest without going through parse-state restoration.
    /// Use `has_absolute_path()` to check before accessing.
    #[serde(skip, default)]
    pub absolute_path: DbtPath,
    #[serde(skip_serializing, default)]
    pub span: Option<Span>,
    pub unique_id: String,
    pub macro_sql: String,
    pub depends_on: MacroDependsOn,
    pub description: String,
    pub meta: BTreeMap<String, Value>,
    pub docs: Option<DocsConfig>,
    #[serde(default)]
    pub config: MacroConfig,
    pub patch_path: Option<PathBuf>,
    pub funcsign: Option<String>,
    pub supported_languages: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<ArgSpec>,
    /// Macro arguments from YAML spec (used for manifest serialization via ManifestMacro)
    #[serde(skip)]
    pub arguments: Vec<MacroArgument>,
    #[serde(skip_serializing, default)]
    pub macro_name_span: Option<Span>,
    pub __other__: BTreeMap<String, Value>,
}

impl DbtMacro {
    pub fn has_absolute_path(&self) -> bool {
        !self.absolute_path.as_os_str().is_empty()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub struct MacroDependsOn {
    #[serde(default)]
    pub macros: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub struct DbtDocsMacro {
    pub name: String,
    pub package_name: String,
    pub path: DbtPath,
    pub original_file_path: DbtPath,
    pub unique_id: String,
    pub block_contents: String,
}

pub fn build_macro_units(
    nodes: &BTreeMap<String, DbtMacro>,
    project_root: &Path,
) -> BTreeMap<String, Vec<MacroUnit>> {
    let mut macros = BTreeMap::new();
    for (_, inner_macro) in nodes.iter() {
        let display_path = inner_macro
            .get_node_path(NodePathKind::Definition, project_root, project_root)
            .into_owned();
        macros
            .entry(inner_macro.package_name.clone())
            .or_insert(vec![])
            .push(MacroUnit {
                info: MacroInfo {
                    name: inner_macro.name.clone(),
                    path: display_path,
                    span: inner_macro.span.expect("span is required"),
                    funcsign: inner_macro.funcsign.clone(),
                    args: inner_macro.args.clone(),
                    unique_id: inner_macro.unique_id.clone(),
                    name_span: inner_macro.macro_name_span.expect("name_span is required"),
                },
                sql: inner_macro.macro_sql.clone(),
            });
    }
    macros
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_macro_config_default_serializes_docs_with_nulls() {
        // serialize_docs_with_nulls forces both fields to be present, including
        // node_color (normally Option-skipped) as an explicit null.
        let config = MacroConfig::default();
        let json = serde_json::to_value(&config).expect("serializes");
        let docs = json.get("docs").expect("docs key present");
        assert_eq!(
            docs.get("show").expect("show key"),
            &serde_json::Value::Bool(true),
            "docs.show should serialize as true (the default)"
        );
        assert_eq!(
            docs.get("node_color").expect("node_color key"),
            &serde_json::Value::Null,
            "docs.node_color should serialize as explicit null even when None"
        );
    }

    #[test]
    fn test_macro_supported_languages_serializes_as_strings() {
        let macro_ = DbtMacro {
            supported_languages: Some(vec!["sql".to_string(), "python".to_string()]),
            ..Default::default()
        };

        let json = serde_json::to_value(&macro_).expect("serializes");
        assert_eq!(
            json.get("supported_languages"),
            Some(&serde_json::json!(["sql", "python"]))
        );
    }
}
