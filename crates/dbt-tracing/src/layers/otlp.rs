use std::borrow::Cow;

use crate::{
    LogRecordInfo, SpanEndInfo, SpanStartInfo, TelemetryOutputFlags,
    data_provider::DataProvider,
    error::{TracingError, TracingResult},
    layer::{ConsumerLayer, LogPreprocessorHook, TelemetryConsumer},
    serialize::otlp::{export_log, export_span},
    shutdown::{TelemetryShutdown, TelemetryShutdownItem},
};

use opentelemetry::{KeyValue, logs::LoggerProvider, trace::TracerProvider};
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::{SdkLogger, SdkLoggerProvider};
use opentelemetry_sdk::resource::EnvResourceDetector;
use opentelemetry_sdk::trace::{SdkTracer, SdkTracerProvider};
use opentelemetry_sdk::{logs as sdk_logs, trace as sdk_trace};
use opentelemetry_semantic_conventions::resource::{SERVICE_NAME, SERVICE_VERSION};

#[derive(Clone, Debug)]
pub struct OtlpResourceConfig {
    service_name: &'static str,
    service_version: &'static str,
    resource_attributes: Vec<KeyValue>,
}

impl OtlpResourceConfig {
    pub fn new(service_name: &'static str, service_version: &'static str) -> Self {
        Self {
            service_name,
            service_version,
            resource_attributes: Vec::new(),
        }
    }

    pub fn with_resource_attributes(
        mut self,
        resource_attributes: impl IntoIterator<Item = KeyValue>,
    ) -> Self {
        self.resource_attributes.extend(resource_attributes);
        self
    }

    fn into_resource_attributes(self) -> Vec<KeyValue> {
        let mut resource_attributes = self.resource_attributes;
        resource_attributes.extend([
            KeyValue::new(SERVICE_NAME, self.service_name),
            KeyValue::new(SERVICE_VERSION, self.service_version),
        ]);
        resource_attributes
    }
}

/// Build an OTLP layer with HTTP exporters. If exporters cannot be built,
/// it will return None.
pub fn build_otlp_layer(
    resource_config: OtlpResourceConfig,
    log_preprocessor_hook: Option<LogPreprocessorHook>,
) -> Option<(ConsumerLayer, Vec<TelemetryShutdownItem>)> {
    let layer = OTLPExporterLayer::new_with_http_export(resource_config, log_preprocessor_hook)?;

    let shutdown_items: Vec<TelemetryShutdownItem> = vec![
        Box::new(layer.tracer_provider()),
        Box::new(layer.logger_provider()),
    ];

    Some((Box::new(layer), shutdown_items))
}

/// A tracing layer that reads telemetry data and sends it over HTTP to OTLP endpoint
pub struct OTLPExporterLayer {
    tracer_provider: SdkTracerProvider,
    logger_provider: SdkLoggerProvider,
    tracer: SdkTracer,
    logger: SdkLogger,
    log_preprocessor_hook: Option<LogPreprocessorHook>,
}

impl OTLPExporterLayer {
    /// Creates a new OTLPExporterLayer from provided exporters
    pub(crate) fn new(
        trace_exporter: impl sdk_trace::SpanExporter + 'static,
        log_exporter: impl sdk_logs::LogExporter + 'static,
        resource_config: OtlpResourceConfig,
        log_preprocessor_hook: Option<LogPreprocessorHook>,
    ) -> Self {
        Self::new_with_exporters(
            trace_exporter,
            log_exporter,
            resource_config,
            true,
            log_preprocessor_hook,
        )
    }

    #[cfg(test)]
    pub(crate) fn new_for_tests(
        trace_exporter: impl sdk_trace::SpanExporter + 'static,
        log_exporter: impl sdk_logs::LogExporter + 'static,
        resource_config: OtlpResourceConfig,
        log_preprocessor_hook: Option<LogPreprocessorHook>,
    ) -> Self {
        // These tests validate OTLP layer filtering/serialization, not the OpenTelemetry
        // SDK's batch processor lifecycle. Using simple exporters avoids flaky shutdown
        // interactions when libtest runs adjacent tracing tests with high parallelism.
        Self::new_with_exporters(
            trace_exporter,
            log_exporter,
            resource_config,
            false,
            log_preprocessor_hook,
        )
    }

    fn new_with_exporters(
        trace_exporter: impl sdk_trace::SpanExporter + 'static,
        log_exporter: impl sdk_logs::LogExporter + 'static,
        resource_config: OtlpResourceConfig,
        use_batch_exporters: bool,
        log_preprocessor_hook: Option<LogPreprocessorHook>,
    ) -> Self {
        let service_name = resource_config.service_name;

        // Set up resource with service information
        let resource = Resource::builder()
            .with_detectors(&[Box::new(EnvResourceDetector::new())])
            .with_attributes(resource_config.into_resource_attributes())
            .build();

        // Initialize a tracer provider.
        let tracer_provider = if use_batch_exporters {
            SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(trace_exporter)
                .build()
        } else {
            SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .with_simple_exporter(trace_exporter)
                .build()
        };

        // Initialize a logger provider.
        let logger_provider = if use_batch_exporters {
            SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_batch_exporter(log_exporter)
                .build()
        } else {
            SdkLoggerProvider::builder()
                .with_resource(resource)
                .with_simple_exporter(log_exporter)
                .build()
        };

        // Get tracer
        let tracer = tracer_provider.tracer(service_name);

        // Get root logger
        let logger = logger_provider.logger(service_name);

        OTLPExporterLayer {
            tracer_provider,
            logger_provider,
            tracer,
            logger,
            log_preprocessor_hook,
        }
    }

    /// Creates a new OTLPExporterLayer with HTTP exporters (binary protocol)
    ///
    /// If endpoint is not reachable or exporters fail to build, it will return None.
    ///
    /// Reads the OTLP endpoint from either:
    /// - the environment variable `OTEL_EXPORTER_OTLP_ENDPOINT` - works for logs & traces,
    ///   and assumes default routes: `/v1/logs` for logs and `/v1/traces` for traces.
    /// - the environment variable `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` - can
    ///   be used to specify a full endpoint for traces, with non-default routes.
    /// - the environment variable `OTEL_EXPORTER_OTLP_LOGS_ENDPOINT` - can
    ///   be used to specify a full endpoint for logs, with non-default routes.
    pub(crate) fn new_with_http_export(
        resource_config: OtlpResourceConfig,
        log_preprocessor_hook: Option<LogPreprocessorHook>,
    ) -> Option<Self> {
        // Add OTLP trace HTTP exporter
        let trace_exporter = match opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .build()
        {
            Ok(http_exporter) => http_exporter,
            Err(_) => return None,
        };

        // Create OTLP logger exporter
        let log_exporter = match opentelemetry_otlp::LogExporterBuilder::new()
            .with_http()
            .with_protocol(opentelemetry_otlp::Protocol::HttpBinary)
            .build()
        {
            Ok(http_exporter) => http_exporter,
            Err(_) => return None,
        };

        Some(Self::new(
            trace_exporter,
            log_exporter,
            resource_config,
            log_preprocessor_hook,
        ))
    }

    pub(crate) fn tracer_provider(&self) -> SdkTracerProvider {
        // Cheap, it's really an arc
        self.tracer_provider.clone()
    }

    pub(crate) fn logger_provider(&self) -> SdkLoggerProvider {
        // Cheap, it's really an arc
        self.logger_provider.clone()
    }
}

impl TelemetryShutdown for SdkTracerProvider {
    fn shutdown(&mut self) -> TracingResult<()> {
        SdkTracerProvider::shutdown(self).map_err(|otel_error| {
            TracingError::shutdown(format!(
                "Failed to gracefully shutdown OTLP trace exporter: {otel_error}"
            ))
        })
    }
}

impl TelemetryShutdown for SdkLoggerProvider {
    fn shutdown(&mut self) -> TracingResult<()> {
        SdkLoggerProvider::shutdown(self).map_err(|otel_error| {
            TracingError::shutdown(format!(
                "Failed to gracefully shutdown OTLP log exporter: {otel_error}"
            ))
        })
    }
}

impl TelemetryConsumer for OTLPExporterLayer {
    fn is_span_enabled(&self, span: &SpanStartInfo) -> bool {
        span.attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_OTLP)
    }

    fn is_log_enabled(&self, log_record: &LogRecordInfo) -> bool {
        log_record
            .attributes
            .output_flags()
            .contains(TelemetryOutputFlags::EXPORT_OTLP)
    }

    // We record spans to OTLP only when they are closed, so we don't need to do anything on new span
    fn on_span_end(&self, span: &SpanEndInfo, _: &mut DataProvider<'_>) {
        export_span(&self.tracer, span);
    }

    fn on_log_record(&self, record: &LogRecordInfo, _: &mut DataProvider<'_>) {
        let record = self
            .log_preprocessor_hook
            .map_or(Cow::Borrowed(record), |hook| hook(record));
        export_log(&self.logger, record.as_ref());
    }
}
