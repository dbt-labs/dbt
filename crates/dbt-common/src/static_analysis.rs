use dbt_error::ErrorCode;

use crate::{
    io_args::StaticAnalysisKind,
    tracing::dbt_emit::{emit_error_log_message, emit_error_log_message_package_scoped},
};

pub enum StaticAnalysisDeprecationOrigin<'a> {
    CliArg,
    ProjectConfig,
    NodeConfig { unique_id: &'a str },
}

#[inline]
pub fn normalize_static_analysis_kind(kind: StaticAnalysisKind) -> StaticAnalysisKind {
    match kind {
        StaticAnalysisKind::On => StaticAnalysisKind::Strict,
        other => other,
    }
}

#[inline]
pub fn is_static_analysis_off_or_baseline(kind: StaticAnalysisKind) -> bool {
    matches!(
        normalize_static_analysis_kind(kind),
        StaticAnalysisKind::Off | StaticAnalysisKind::Baseline
    )
}

#[inline]
pub fn is_strict_static_analysis(kind: StaticAnalysisKind) -> bool {
    normalize_static_analysis_kind(kind) == StaticAnalysisKind::Strict
}

#[inline]
pub fn is_baseline_static_analysis(kind: StaticAnalysisKind) -> bool {
    matches!(kind, StaticAnalysisKind::Baseline)
}

#[inline]
pub fn is_static_analysis_enabled(kind: StaticAnalysisKind) -> bool {
    matches!(
        kind,
        StaticAnalysisKind::Strict | StaticAnalysisKind::On | StaticAnalysisKind::Unsafe
    )
}

#[inline]
pub fn is_deprecated_static_analysis_kind(kind: StaticAnalysisKind) -> bool {
    matches!(kind, StaticAnalysisKind::On | StaticAnalysisKind::Unsafe)
}

/// Checks for deprecated static analysis values and emits appropriate warnings.
/// TODO: remove after May 1, 2026 when deprecated values are removed.
pub fn check_deprecated_static_analysis_kind(
    kind: StaticAnalysisKind,
    origin: StaticAnalysisDeprecationOrigin<'_>,
    dependency_package_name: Option<&str>,
) {
    if !is_deprecated_static_analysis_kind(kind) {
        return;
    }

    if let Some(package_name) = dependency_package_name {
        let message = match kind {
            StaticAnalysisKind::On => format!(
                "Package `{package_name}` uses deprecated `static_analysis: on` in dbt_project.yml or node config. \
                 It will be removed in May, 2026. Use `static_analysis: strict` instead."
            ),
            StaticAnalysisKind::Unsafe => format!(
                "Package `{package_name}` uses deprecated `static_analysis: unsafe` in dbt_project.yml or node config. \
                 It will be removed in May, 2026. Use `static_analysis: strict` instead."
            ),
            _ => return,
        };
        emit_error_log_message_package_scoped(
            ErrorCode::DeprecatedStaticAnalysisValue,
            message,
            package_name,
        );
        return;
    }

    let message = match (kind, origin) {
        (StaticAnalysisKind::On, StaticAnalysisDeprecationOrigin::CliArg) => {
            "`--static-analysis on` is deprecated and will be removed in May, 2026. \
             Use `--static-analysis strict` instead."
                .to_string()
        }
        (StaticAnalysisKind::Unsafe, StaticAnalysisDeprecationOrigin::CliArg) => {
            "`--static-analysis unsafe` is deprecated and will be removed in May, 2026. \
             Use `--static-analysis strict` instead."
                .to_string()
        }
        (StaticAnalysisKind::On, StaticAnalysisDeprecationOrigin::ProjectConfig) => {
            "`static_analysis: on` is deprecated and will be removed in May, 2026. \
             Use `static_analysis: strict` instead."
                .to_string()
        }
        (StaticAnalysisKind::Unsafe, StaticAnalysisDeprecationOrigin::ProjectConfig) => {
            "`static_analysis: unsafe` is deprecated and will be removed in May, 2026. \
             Use `static_analysis: strict` instead."
                .to_string()
        }
        (StaticAnalysisKind::On, StaticAnalysisDeprecationOrigin::NodeConfig { unique_id }) => {
            format!(
                "Node `{unique_id}` uses deprecated `static_analysis: on`. \
                 It will be removed in May, 2026. Use `static_analysis: strict` instead."
            )
        }
        (StaticAnalysisKind::Unsafe, StaticAnalysisDeprecationOrigin::NodeConfig { unique_id }) => {
            format!(
                "Node `{unique_id}` uses deprecated `static_analysis: unsafe`. \
                 It will be removed in May, 2026. Use `static_analysis: strict` instead."
            )
        }
        _ => return,
    };

    emit_error_log_message(ErrorCode::DeprecatedStaticAnalysisValue, message);
}
