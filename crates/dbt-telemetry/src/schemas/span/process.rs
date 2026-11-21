use crate::{attributes::DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryContext, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;

pub use crate::proto::v1::public::events::fusion::process::{Process, ThreadPoolWait};

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

impl StaticTelemetryEvent for ThreadPoolWait {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        "Thread pool wait".to_string()
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Attribute the wait to the closest node or batch evaluation, unless
        // the caller attributed it explicitly.
        if self.unique_id.is_none() && self.batch_id.is_none() {
            self.unique_id = context.unique_id.clone();
            self.batch_id = context.batch_id.clone();
        }
    }
}

/// Wait fields stored outside well-known Arrow columns.
#[derive(serde::Serialize, serde::Deserialize)]
struct ThreadPoolWaitJsonPayload {
    batch_id: String,
}

impl ArrowSerializableTelemetryEvent for ThreadPoolWait {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            unique_id: self.unique_id.as_deref().map(Into::into),
            json_payload: self.batch_id.as_ref().map(|batch_id| {
                serde_json::to_string(&ThreadPoolWaitJsonPayload {
                    batch_id: batch_id.clone(),
                })
                .unwrap_or_else(|_| {
                    panic!(
                        "Failed to serialize json payload for event type \"{}\" to JSON",
                        Self::full_name()
                    )
                })
            }),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let batch_id = record
            .json_payload
            .as_deref()
            .map(|payload| {
                serde_json::from_str::<ThreadPoolWaitJsonPayload>(payload)
                    .map(|payload| payload.batch_id)
                    .map_err(|error| {
                        format!(
                            "Failed to deserialize event type \"{}\": {error}",
                            Self::full_name()
                        )
                    })
            })
            .transpose()?;
        Ok(Self {
            unique_id: record.unique_id.as_deref().map(str::to_string),
            batch_id,
        })
    }
}
