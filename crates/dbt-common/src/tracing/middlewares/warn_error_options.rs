use std::sync::Arc;

use dbt_error::ErrorCode;
use dbt_tracing::{LogRecordInfo, SeverityNumber};

use crate::{
    tracing::{
        TracingConfigProvider, data_provider::DataProvider, fs_error_log::get_log_message,
        layer::TelemetryMiddleware,
    },
    warn_error_options::{ErrorCtx, WarnErrorDecision, WarnErrorOptions, is_fusion_only_warning},
};

pub struct TelemetryWarnErrorOptionsMiddleware {
    config_provider: Arc<dyn TracingConfigProvider>,
    /// Withholds upgrades of warnings with no dbt-core counterpart. Set once,
    /// from the replay mode known at tracing init time.
    skip_fusion_only_upgrades: bool,
}

impl TelemetryWarnErrorOptionsMiddleware {
    pub fn new(
        config_provider: Arc<dyn TracingConfigProvider>,
        skip_fusion_only_upgrades: bool,
    ) -> Self {
        Self {
            config_provider,
            skip_fusion_only_upgrades,
        }
    }
}

impl TelemetryMiddleware for TelemetryWarnErrorOptionsMiddleware {
    fn on_log_record(
        &self,
        mut record: LogRecordInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        let Some(log_message) = get_log_message(&record.attributes) else {
            return Some(record);
        };
        if record.severity_number != SeverityNumber::Warn {
            return Some(record);
        }
        let Some(code) = log_message
            .code
            .and_then(|code| u16::try_from(code).ok())
            .and_then(|code| ErrorCode::try_from(code).ok())
        else {
            return Some(record);
        };

        let error_ctx = ErrorCtx::from_dependency_package_name(log_message.package_name.as_deref());
        let mut decision = WarnErrorDecision::Retain;
        self.config_provider.with_warn_error_options(
            &mut |warn_error_options: &WarnErrorOptions| {
                decision = warn_error_options.decision_for_error_code_with_context(code, error_ctx);
            },
        );

        // Silencing stays intact; only the upgrade is withheld.
        if self.skip_fusion_only_upgrades
            && decision == WarnErrorDecision::UpgradeToError
            && is_fusion_only_warning(code)
        {
            decision = WarnErrorDecision::Retain
        };

        match decision {
            WarnErrorDecision::Silence => None,
            WarnErrorDecision::Retain => Some(record),
            WarnErrorDecision::UpgradeToError => {
                record.severity_number = SeverityNumber::Error;
                record.severity_text = SeverityNumber::Error.as_str().to_string();
                Some(record)
            }
        }
    }
}
