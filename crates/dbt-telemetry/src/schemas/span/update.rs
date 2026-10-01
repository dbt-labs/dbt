pub use crate::proto::v1::public::events::fusion::update::PackageUpdate;
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent,
    TelemetryEventRecType, TelemetryOutputFlags,
};
use prost::Name as _;

// Our shorthand `StaticTelemetryEvent` trait requires arrow trait implementation,
// and since this one is not exported to Parquet, we have to implement the
// `AnyTelemetryEvent` methods directly.

impl StaticTelemetryEvent for PackageUpdate {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Update: {} -> {}", self.package, self.version)
    }

    fn has_sensitive_data(&self) -> bool {
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        // PackageUpdate is considered sensitive as it may carry sensitive path
        // in the `exe_path` field. We strip it out here.
        Some(Box::new(PackageUpdate {
            package: self.package.clone(),
            version: self.version.clone(),
            exe_path: None,
        }))
    }
}

impl ArrowSerializableTelemetryEvent for PackageUpdate {
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
