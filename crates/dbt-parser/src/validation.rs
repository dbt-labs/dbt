use dbt_common::{
    ErrorCode, FsError,
    io_args::StaticAnalysisKind,
    static_analysis::{StaticAnalysisDeprecationOrigin, check_deprecated_static_analysis_kind},
    tracing::dbt_emit::emit_warn_log_from_fs_error,
};
use dbt_schemas::schemas::project::ResolvedConfig;

/// Warns when a node config has `static_analysis` set to a deprecated or overridden value.
pub fn check_node_static_analysis(
    config: &impl ResolvedConfig,
    global_override: Option<StaticAnalysisKind>,
    unique_id: &str,
    dependency_package_name: Option<&str>,
) {
    if let Some(spanned) = config.get_static_analysis() {
        let kind = *spanned;
        if kind != global_override.unwrap_or_default() {
            check_deprecated_static_analysis_kind(
                kind,
                StaticAnalysisDeprecationOrigin::NodeConfig { unique_id },
                dependency_package_name,
            );
        }
    }
}

/// Warns when a Python model or function has `static_analysis` set to a value that enables
/// analysis, since static analysis is not supported for Python.
pub fn warn_python_static_analysis(kind: StaticAnalysisKind, unique_id: &str) {
    if matches!(kind, StaticAnalysisKind::On | StaticAnalysisKind::Strict) {
        emit_warn_log_from_fs_error(FsError::new(
            ErrorCode::InvalidConfig,
            format!(
                "Python model '{unique_id}' has static_analysis set to '{kind}', but static \
                analysis is not supported for Python models. Setting will be ignored.",
            ),
        ));
    }
}
