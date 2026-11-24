use std::borrow::Cow;

use dbt_tracing::{LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryAttributes};

use super::super::{data_provider::DataProvider, layer::TelemetryMiddleware};
use crate::env_secrets::{env_secret_values, scrub_secrets};

/// Middleware that masks `DBT_ENV_SECRET*` values in every span and log record, so no
/// consumer (console, files, OTEL, the LSP) sees them. It covers log bodies, span names,
/// span status messages and the strings of the event attributes.
pub struct TelemetrySecretScrubber {
    secrets: Vec<String>,
}

impl TelemetrySecretScrubber {
    /// Snapshots secrets from the environment once, at construction.
    pub fn from_env() -> Self {
        Self::with_secrets(env_secret_values())
    }

    pub(crate) fn with_secrets(secrets: Vec<String>) -> Self {
        Self { secrets }
    }

    fn scrub(&self, text: &mut String) {
        if let Some(scrubbed) = scrub_secrets(text, &self.secrets) {
            *text = scrubbed.into_owned();
        }
    }

    fn scrub_attributes(&self, attributes: &mut TelemetryAttributes) {
        attributes.rewrite_strings(&|text| scrub_secrets(text, &self.secrets).map(Cow::into_owned));
    }
}

impl TelemetryMiddleware for TelemetrySecretScrubber {
    fn on_span_start(
        &self,
        mut span: SpanStartInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanStartInfo> {
        if !self.secrets.is_empty() {
            self.scrub(&mut span.span_name);
            self.scrub_attributes(&mut span.attributes);
        }
        Some(span)
    }

    fn on_span_end(
        &self,
        mut span: SpanEndInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<SpanEndInfo> {
        if !self.secrets.is_empty() {
            self.scrub(&mut span.span_name);
            if let Some(message) = span.status.as_mut().and_then(|s| s.message.as_mut()) {
                self.scrub(message);
            }
            self.scrub_attributes(&mut span.attributes);
        }
        Some(span)
    }

    fn on_log_record(
        &self,
        mut record: LogRecordInfo,
        _data_provider: &mut DataProvider<'_>,
    ) -> Option<LogRecordInfo> {
        if !self.secrets.is_empty() {
            self.scrub(&mut record.body);
            if let Some(span_name) = record.span_name.as_mut() {
                self.scrub(span_name);
            }
            self.scrub_attributes(&mut record.attributes);
        }
        Some(record)
    }
}
