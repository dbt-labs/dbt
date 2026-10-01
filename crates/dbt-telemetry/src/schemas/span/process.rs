use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;

pub use crate::proto::v1::public::events::fusion::process::Process;

/// Creates a new instance of `Process` with the current process information.
pub fn create_process_event_data(package: &str) -> Process {
    Process {
        package: package.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        host_os: std::env::consts::OS.to_string(),
        host_arch: std::env::consts::ARCH.to_string(),
    }
}

impl StaticTelemetryEvent for Process {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "{} process ({}-{})",
            self.package, self.host_arch, self.host_os
        )
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for Process {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            json_payload: serde_json::to_string(self)
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
                .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
            format!(
                "Missing json payload for event type \"{}\"",
                Self::full_name()
            )
        })?)
        .map_err(|e| {
            format!(
                "Failed to deserialize event type \"{}\" from JSON: {}",
                Self::full_name(),
                e
            )
        })
    }
}
