use crate::proto::v1::public::events::fusion::phase::ExecutionPhase;
use crate::{DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryContext, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;
use serde_with::skip_serializing_none;

pub use crate::proto::v1::public::events::fusion::asset::AssetParsed;

impl StaticTelemetryEvent for AssetParsed {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Asset parsed ({})", self.display_path)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // Inject phase from context if not set
        if let Some(ctx_phase) = context.phase
            && self.phase() == ExecutionPhase::Unspecified
        {
            self.set_phase(ctx_phase);
        }

        // Inject unique_id if not set and provided by context
        if self.unique_id.is_none()
            && let Some(unique_id) = context.unique_id.as_ref()
        {
            self.unique_id = Some(unique_id.clone());
        }
    }
}

#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct AssetParsedJsonPayload {
    pub display_path: String,
}

impl ArrowSerializableTelemetryEvent for AssetParsed {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            phase: Some(self.phase()),
            name: Some(self.name.as_str().into()),
            unique_id: self.unique_id.as_deref().map(Into::into),
            package_name: Some(self.package_name.as_str().into()),
            relative_path: Some(self.relative_path.as_str().into()),
            json_payload: serde_json::to_string(&AssetParsedJsonPayload {
                display_path: self.display_path.clone(),
            })
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize json payload for event type \"{}\" to JSON",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: AssetParsedJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize json payload for event type \"{}\" from JSON: {}",
                    Self::full_name(),
                    e
                )
            })?;

        Ok(Self {
            phase: record.phase.map(|v| v as i32).ok_or_else(|| {
                format!("Missing `phase` for event type \"{}\"", Self::full_name())
            })?,
            name: record.name.as_deref().map(str::to_string).ok_or_else(|| {
                format!("Missing `name` for event type \"{}\"", Self::full_name())
            })?,
            relative_path: record
                .relative_path
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `relative_path` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            display_path: json_payload.display_path,
            unique_id: record.unique_id.as_deref().map(str::to_string),
            package_name: record
                .package_name
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `package_name` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
        })
    }
}
