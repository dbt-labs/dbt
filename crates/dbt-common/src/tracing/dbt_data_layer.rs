use dbt_telemetry::{CallTrace, Invocation, LogMessage, Unknown, create_process_event_data};
use dbt_tracing::TelemetryAttributes;

use dbt_tracing::layers::data_layer::{
    RootSpanTraceContext, TelemetryDataLayerConfig, UnstructuredLogAttributesInput,
    UnstructuredSpanAttributesInput,
};

/// Creates the dbt data layer configuration used by the data layer.
pub fn dbt_data_layer_config(
    fallback_trace_id: u128,
    fallback_parent_span_id: Option<u64>,
) -> TelemetryDataLayerConfig {
    TelemetryDataLayerConfig::new(
        fallback_trace_id,
        fallback_parent_span_id,
        dbt_unstructured_span_attributes,
        dbt_unstructured_log_attributes,
        dbt_root_span_trace_context,
    )
}

/// Creates the dbt-owned process span attributes stored before opening the process span.
pub fn dbt_process_span_attributes(package: &str) -> TelemetryAttributes {
    create_process_event_data(package).into()
}

/// Creates dbt fallback span attributes for spans without structured attributes.
fn dbt_unstructured_span_attributes(
    input: UnstructuredSpanAttributesInput<'_>,
) -> TelemetryAttributes {
    let file = input.location.file.clone();
    let line = input.location.line;

    if input.level == &tracing::Level::TRACE {
        if let Some(extra) = input.debug_extra_attrs {
            // Trace spans without explicit attributes considered dev internal.
            return CallTrace {
                name: input.name.to_string(),
                file,
                line,
                extra: extra
                    .into_iter()
                    .map(|(key, value)| (key, value.into()))
                    .collect(),
            }
            .into();
        }
    }

    Unknown {
        name: input.name.to_string(),
        file: file.as_deref().unwrap_or("<unknown>").to_string(),
        line: line.unwrap_or_default(),
    }
    .into()
}

/// Creates dbt fallback log attributes for events without structured attributes.
fn dbt_unstructured_log_attributes(input: UnstructuredLogAttributesInput) -> TelemetryAttributes {
    let location = input.location;

    LogMessage {
        code: None,
        code_name: None,
        dbt_core_event_code: None,
        original_severity_number: input.severity_number as i32,
        original_severity_text: input.severity_number.as_str().to_string(),
        package_name: None,
        unique_id: None,
        phase: None,
        file: location.file,
        line: location.line,
        relative_path: None,
        code_line: None,
        code_column: None,
        expanded_relative_path: None,
        expanded_line: None,
        expanded_column: None,
    }
    .into()
}

/// Extracts dbt root span trace context from invocation attributes, when present.
fn dbt_root_span_trace_context(attributes: &TelemetryAttributes) -> Option<RootSpanTraceContext> {
    let invocation = attributes.downcast_ref::<Invocation>()?;

    Some(RootSpanTraceContext {
        // We use protos to define event structures, which doesn't allow
        // storing u128/uuid directly, so we store UUID string and convert it back here.
        trace_id: uuid::Uuid::parse_str(&invocation.invocation_id)
            .expect("invocation_id Must be a valid UUID string")
            .as_u128(),
        parent_span_id: invocation.parent_span_id,
    })
}
