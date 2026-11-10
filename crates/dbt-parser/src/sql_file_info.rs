//! This module contains the SqlFileInfo struct, which is used to collect details about processed sql files.

use dbt_frontend_common::error::CodeLocation;
use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
use dbt_schemas::schemas::{common::DbtChecksum, project::ResolvableConfig, serde::NodeVersion};
use minijinja::{ArgSpec, machinery::Span};

/// Collected details about processed sql files
#[derive(Debug, Clone)]
pub struct SqlFileInfo<T: ResolvableConfig<T>> {
    /// e.g. source('a', 'b') — live sources that become runtime `depends_on` entries.
    pub sources: Vec<(String, String, CodeLocation)>,
    /// Sources discovered by static AST analysis of dead Jinja branches.
    ///
    /// Included for schema fetching and lineage, but must NOT be promoted to
    /// `depends_on.nodes` at resolve time — they were never actually executed.
    pub static_sources: Vec<(String, String, CodeLocation)>,
    /// e.g. ref('a', 'b', 'c')
    pub refs: Vec<(String, Option<String>, Option<NodeVersion>, CodeLocation)>,
    /// true if `this` is referenced in this .sql file, otherwise false
    pub this: bool,
    /// e.g. metric('a', 'b')
    pub metrics: Vec<(String, Option<String>)>,
    /// Explicit SQL `{{ config(...) }}` calls only, in source order and unmerged; see
    /// `ProjectConfigResolver::fold_inline_calls`.
    ///
    /// This intentionally excludes the initial project/properties config.
    pub config_calls: Vec<Box<T>>,
    /// e.g. tests
    pub tests: Vec<(String, Span)>,
    /// e.g. macros
    pub macros: Vec<(String, Span, Option<String>, Vec<ArgSpec>)>,
    /// e.g. materializations
    pub materializations: Vec<(String, String, Span)>,
    /// e.g. docs
    pub docs: Vec<(String, Span)>,
    /// e.g. snapshots
    pub snapshots: Vec<(String, Span)>,
    /// e.g. functions
    pub functions: Vec<(String, Option<String>, CodeLocation)>,
    /// e.g. checksums
    pub checksum: DbtChecksum,
    /// true if `execute` flag exists in this .sql file, otherwise false
    pub execute: bool,
}

impl<T: ResolvableConfig<T>> Default for SqlFileInfo<T> {
    fn default() -> Self {
        Self {
            sources: Vec::new(),
            static_sources: Vec::new(),
            refs: Vec::new(),
            this: false,
            metrics: Vec::new(),
            config_calls: Vec::new(),
            tests: Vec::new(),
            macros: Vec::new(),
            materializations: Vec::new(),
            docs: Vec::new(),
            snapshots: Vec::new(),
            functions: Vec::new(),
            checksum: DbtChecksum::default(),
            execute: false,
        }
    }
}

impl<T: ResolvableConfig<T>> SqlFileInfo<T> {
    /// Collects rendering artifacts from a list of SqlResources.
    ///
    /// `ConfigCall` items (from inline `{{ config(...) }}` calls) are kept in `config_calls`,
    /// unmerged. No project/properties config merging or `finalize()` happens here; fold the
    /// calls with `ProjectConfigResolver::fold_inline_calls` and resolve the node after this.
    pub fn from_sql_resources(
        resources: Vec<SqlResource<T>>,
        checksum: DbtChecksum,
        execute: bool,
    ) -> Self {
        let mut sources = Vec::new();
        let mut static_sources = Vec::new();
        let mut refs = Vec::new();
        let mut this = false;
        let mut metrics = Vec::new();
        let mut config_calls = Vec::new();
        let mut tests = Vec::new();
        let mut macros = Vec::new();
        let mut materializations = Vec::new();
        let mut docs = Vec::new();
        let mut snapshots = Vec::new();
        let mut functions = Vec::new();

        for resource in resources {
            match resource {
                SqlResource::Source(source) => sources.push(source),
                SqlResource::StaticSource(source) => static_sources.push(source),
                SqlResource::Ref(reference) => refs.push(reference),
                SqlResource::This => this = true,
                SqlResource::Function(function) => functions.push(function),
                SqlResource::Metric(metric) => metrics.push(metric),
                SqlResource::ConfigCall(resource_config) => config_calls.push(resource_config),
                SqlResource::Test(name, span, _, _) => tests.push((name, span)),
                SqlResource::Macro(name, span, func_sign, args, _) => {
                    macros.push((name, span, func_sign, args))
                }
                SqlResource::Materialization(name, adapter, _, span, _) => {
                    materializations.push((name, adapter, span))
                }
                SqlResource::Doc(name, span) => docs.push((name, span)),
                SqlResource::Snapshot(name, span, _) => snapshots.push((name, span)),
            }
        }

        SqlFileInfo {
            sources,
            static_sources,
            refs,
            this,
            metrics,
            config_calls,
            tests,
            macros,
            materializations,
            docs,
            snapshots,
            functions,
            checksum,
            execute,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SqlFileInfo;
    use dbt_jinja_utils::phases::parse::sql_resource::SqlResource;
    use dbt_schemas::schemas::common::DbtChecksum;
    use dbt_schemas::schemas::project::ModelConfig;

    #[test]
    fn config_calls_are_kept_unmerged_in_source_order() {
        // No config calls => none recorded.
        let info =
            SqlFileInfo::<ModelConfig>::from_sql_resources(vec![], DbtChecksum::default(), false);
        assert!(info.config_calls.is_empty());

        let call1 = ModelConfig {
            alias: Some("a1".to_string()),
            schema: dbt_common::serde_utils::Omissible::Present(Some("s1".to_string())),
            ..Default::default()
        };
        let call2 = ModelConfig {
            alias: Some("a2".to_string()),
            ..Default::default()
        };

        let info = SqlFileInfo::from_sql_resources(
            vec![
                SqlResource::ConfigCall(Box::new(call1)),
                SqlResource::ConfigCall(Box::new(call2)),
            ],
            DbtChecksum::default(),
            false,
        );

        let aliases: Vec<_> = info
            .config_calls
            .iter()
            .map(|call| call.alias.as_deref())
            .collect();
        assert_eq!(aliases, [Some("a1"), Some("a2")]);
        // The later call does not inherit the earlier call's schema.
        assert!(info.config_calls[1].schema.is_omitted());
    }
}
