use serde::{Deserialize, Serialize};

use crate::schemas::{
    manifest::{
        DbtSavedQuery,
        common::SourceFileMetadata,
        saved_query::{SavedQueryExport, SavedQueryExportConfig, SavedQueryParams},
    },
    project::ExportConfigExportAs,
};

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[allow(non_camel_case_types)]
pub enum SemanticManifestSavedQueryExportConfigExportAs {
    #[default]
    table,
    view,
}

impl From<ExportConfigExportAs> for SemanticManifestSavedQueryExportConfigExportAs {
    fn from(value: ExportConfigExportAs) -> Self {
        match value {
            ExportConfigExportAs::table => SemanticManifestSavedQueryExportConfigExportAs::table,
            ExportConfigExportAs::view => SemanticManifestSavedQueryExportConfigExportAs::view,
            _ => {
                // not sure how to handle ExportConfigExportAs::cache
                SemanticManifestSavedQueryExportConfigExportAs::table
            }
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifestSavedQueryExportConfig {
    pub export_as: SemanticManifestSavedQueryExportConfigExportAs,
    pub schema_name: Option<String>,
    pub alias: Option<String>,
}

impl From<SavedQueryExportConfig> for SemanticManifestSavedQueryExportConfig {
    fn from(value: SavedQueryExportConfig) -> Self {
        SemanticManifestSavedQueryExportConfig {
            export_as: value.export_as.clone().into(),
            schema_name: value.schema_name.clone(),
            alias: value.alias,
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifestSavedQueryExport {
    pub name: String,
    pub config: SemanticManifestSavedQueryExportConfig,
}

impl From<SavedQueryExport> for SemanticManifestSavedQueryExport {
    fn from(value: SavedQueryExport) -> Self {
        SemanticManifestSavedQueryExport {
            name: value.name.clone(),
            config: value.config.into(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SemanticManifestSavedQuery {
    pub name: String,
    pub description: Option<String>,
    pub label: Option<String>,
    pub metadata: Option<SourceFileMetadata>,
    pub tags: Vec<String>,

    pub query_params: SavedQueryParams,
    pub exports: Vec<SemanticManifestSavedQueryExport>,
}

impl From<DbtSavedQuery> for SemanticManifestSavedQuery {
    fn from(model: DbtSavedQuery) -> Self {
        SemanticManifestSavedQuery {
            name: model.__common_attr__.name,
            description: model.__common_attr__.description,
            label: model.__saved_query_attr__.label,
            metadata: model.__saved_query_attr__.metadata,
            tags: model.__common_attr__.tags,
            query_params: model.__saved_query_attr__.query_params,
            exports: model
                .__saved_query_attr__
                .exports
                .iter()
                .map(|e| e.clone().into())
                .collect(),
        }
    }
}
