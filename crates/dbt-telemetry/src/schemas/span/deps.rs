use std::borrow::Cow;

pub use crate::proto::v1::public::events::fusion::deps::{
    DepsAddPackage, DepsAllPackagesInstalled, DepsPackageInstalled, PackageType,
};
use crate::serialize::arrow::ArrowAttributes;
use dbt_tracing::{
    ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryEventRecType,
    TelemetryOutputFlags,
};
use prost::Name;
use serde_with::skip_serializing_none;

/// Internal struct used for serializing/deserializing subset of
/// DepsAddPackage fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct DepsAddPackageJsonPayload<'a> {
    pub package_type: PackageType,
    pub package_version: Option<Cow<'a, str>>,
}

impl StaticTelemetryEvent for DepsAddPackage {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Package \"{}\" added", self.package_name)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for DepsAddPackage {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            package_name: Some(Cow::Borrowed(self.package_name.as_str())),
            json_payload: serde_json::to_string(&DepsAddPackageJsonPayload {
                package_type: self.package_type(),
                package_version: self.package_version.as_deref().map(Cow::Borrowed),
            })
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON: {e:?}",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: DepsAddPackageJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json_payload for event type \"{}\"",
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
            package_type: json_payload.package_type as i32,
            package_version: json_payload.package_version.as_deref().map(str::to_string),
            dbt_core_event_code: record
                .dbt_core_event_code
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `dbt_core_event_code` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
        })
    }
}

impl StaticTelemetryEvent for DepsPackageInstalled {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!(
            "Package \"{}\" installed",
            self.package_name
                .as_deref()
                .unwrap_or_else(|| self.package_url_or_path.as_deref().unwrap_or("unknown"))
        )
    }

    fn has_sensitive_data(&self) -> bool {
        // Assumption is that all sensitive data, like secrets in package_url_or_path,
        // are stripped out before creating this event.
        false
    }
}

/// Internal struct used for serializing/deserializing subset of
/// DepsPackageInstalled fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct DepsPackageInstalledJsonPayload<'a> {
    pub package_type: PackageType,
    pub package_version: Option<Cow<'a, str>>,
    pub package_url_or_path: Option<Cow<'a, str>>,
}

impl ArrowSerializableTelemetryEvent for DepsPackageInstalled {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            dbt_core_event_code: Some(Cow::Borrowed(self.dbt_core_event_code.as_str())),
            package_name: self.package_name.as_deref().map(Cow::Borrowed),
            json_payload: serde_json::to_string(&DepsPackageInstalledJsonPayload {
                package_type: self.package_type(),
                package_version: self.package_version.as_deref().map(Cow::Borrowed),
                package_url_or_path: self.package_url_or_path.as_deref().map(Cow::Borrowed),
            })
            .unwrap_or_else(|e| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON: {e:?}",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: DepsPackageInstalledJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json_payload for event type \"{}\"",
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
            package_name: record.package_name.as_deref().map(str::to_string),
            package_type: json_payload.package_type as i32,
            package_version: json_payload.package_version.as_deref().map(str::to_string),
            package_url_or_path: json_payload
                .package_url_or_path
                .as_deref()
                .map(str::to_string),
            dbt_core_event_code: record
                .dbt_core_event_code
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `dbt_core_event_code` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
        })
    }
}

impl StaticTelemetryEvent for DepsAllPackagesInstalled {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        format!("Installing {} package(s)", self.package_count)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for DepsAllPackagesInstalled {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            // Data is serialized as JSON payload
            json_payload: serde_json::to_string(self)
                .unwrap_or_else(|e| {
                    panic!(
                        "Failed to serialize data in event type \"{}\" to JSON: {e:?}",
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
