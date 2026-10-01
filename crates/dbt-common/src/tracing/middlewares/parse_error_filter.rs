use crate::collections::{HashMap, HashSet};
use dbt_error::ErrorCode;
use dbt_tracing::{LogRecordInfo, SeverityNumber, SpanStartInfo};

use super::super::{
    data_provider::DataProvider,
    dbt_metrics::{FusionMetricKey, InvocationMetricKey},
    fs_error_log::{FsErrorLog, get_log_message_mut},
    layer::TelemetryMiddleware,
};

/// Private type to store set of package names that have already emitted deprecation warnings
/// in data provider.
struct PackageWithLogsSet(HashSet<String>);
/// Private type to store which error codes have been seen for which packages, to allow filtering
/// repeated deprecation. `None` package name is used for the current project.
struct SeenErrorCodesByPackage(HashMap<u32, HashSet<Option<String>>>);

/// Middleware that adjust parsing error severity based on hiddne config set by env vars and
/// catches dependency-related errors and aggregates them into a single error message.
pub struct TelemetryParsingErrorFilter {
    /// If true, all schema parsing issues will be shown as warnings instead of errors.
    beta_parsing: bool,
    /// If true, dependency package parsing issues will be shown as warnings instead of errors.
    beta_package_parsing: bool,
    /// If true, all deprecation errors/warnings from dependency packages will be shown,
    /// not just the first one.
    show_all_deprecations: bool,
}

impl TelemetryParsingErrorFilter {
    pub fn new(show_all_deprecations: bool) -> Self {
        let beta_parsing = match std::env::var("DBT_ENGINE_BETA_PARSING") {
            Ok(val) => val == "1",
            Err(_) => false, // default to false (strict mode on)
        };
        let beta_package_parsing = match std::env::var("DBT_ENGINE_BETA_PACKAGE_PARSING") {
            Ok(val) => val == "1",
            Err(_) => true, // default to true (strict mode off for packages)
        };

        Self {
            beta_parsing,
            beta_package_parsing,
            show_all_deprecations,
        }
    }
}

impl TelemetryMiddleware for TelemetryParsingErrorFilter {
    fn on_span_start(
        &self,
        span: SpanStartInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        // If our configuration requires filtering repeated deprecations from packages,
        // initialize the set to track seen deprecations.
        if !self.show_all_deprecations && span.parent_span_id.is_none() {
            data_provider.init_root(PackageWithLogsSet(HashSet::default()));
            data_provider.init_root(SeenErrorCodesByPackage(HashMap::default()));
        }

        Some(span)
    }
    fn on_log_record(
        &self,
        mut record: LogRecordInfo,
        data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        if let Some(error_log) = record.attributes.downcast_mut::<FsErrorLog>()
            && error_log.is_parsing_error()
        {
            let log_message = error_log.get_log_message_mut();

            let (downgrade_to_warn, downgrade_message_suffix) = if let Some(package_name) =
                log_message.package_name.as_ref()
            {
                // If we are filtering repeated deprecations from packages,
                // check if this is a deprecation message and if we've seen it before.
                if !self.show_all_deprecations {
                    let mut seen = false;
                    data_provider.with_root_mut::<PackageWithLogsSet>(|seen_package_names| {
                        // This will check if we've seen this package name before and
                        // insert it into the set if not in one go.
                        seen = !seen_package_names.0.insert(package_name.to_string());
                    });

                    if seen {
                        // We've seen this deprecation message before, filter it out.
                        return None;
                    }

                    // Not seen before, replace with a general message
                    log_message.code = Some(ErrorCode::PackageParsingCompatibility as u32);
                    log_message.code_name =
                        Some(ErrorCode::PackageParsingCompatibility.name().to_string());
                    record.body = format!(
                        "Package `{package_name}` issued one or more compatibility warnings. To display all warnings associated with this package, run with `--show-all-deprecations`."
                    );
                }

                // for package-related logs, two env vars control downgrading
                (
                    self.beta_parsing || self.beta_package_parsing,
                    "(will error post preview)",
                )
            } else {
                // for local logs, only the main env var controls downgrading
                (self.beta_parsing, "(will error post beta)")
            };

            debug_assert_eq!(
                record.severity_number,
                SeverityNumber::Error,
                "Do not emit deprecation messages as non-errors"
            );
            let original_severity: SeverityNumber = log_message.original_severity_number().into();
            debug_assert_eq!(
                original_severity,
                SeverityNumber::Error,
                "Do not emit deprecation messages as non-errors"
            );

            if downgrade_to_warn {
                record.severity_number = SeverityNumber::Warn;
                record.severity_text = SeverityNumber::Warn.as_str().to_string();
                // Append a note that this is a downgraded message
                record.body = format!("{} {}", record.body, downgrade_message_suffix);
            }

            // Increment autofix counter
            data_provider.increment_metric(
                FusionMetricKey::InvocationMetric(InvocationMetricKey::AutoFixSuggestions),
                1,
            );

            return Some(record);
        }

        if !self.show_all_deprecations
            && let Some(log_message) = get_log_message_mut(&mut record.attributes)
            && let Some(code) = log_message.code
            && code == ErrorCode::DeprecatedStaticAnalysisValue as u32
        {
            let package_name = log_message.package_name.as_ref();
            let mut seen = false;
            data_provider.with_root_mut::<SeenErrorCodesByPackage>(|packages_by_code| {
                let packages = packages_by_code.0.entry(code).or_default();
                seen = !packages.insert(package_name.map(|s| s.to_string()));
            });
            if seen {
                return None;
            }
        }

        // Not a log message we know how to handle, return as is.
        Some(record)
    }
}
