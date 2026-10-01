pub use crate::proto::v1::public::events::fusion::query::{
    ConnectionLimitWait, QueryExecuted, QueryOutcome,
};
use prost::Name as _;
use serde_with::skip_serializing_none;
use std::borrow::Cow;

use crate::{DbtTelemetryContext, serialize::arrow::ArrowAttributes};
use dbt_tracing::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, SpanStatus, StaticTelemetryEvent,
    TelemetryContext, TelemetryEventRecType, TelemetryOutputFlags,
};

impl StaticTelemetryEvent for QueryExecuted {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        if let Some(unique_id) = &self.unique_id {
            return format!("Query executed ({unique_id})");
        }

        "Query executed".to_string()
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        match self.query_outcome() {
            QueryOutcome::Success => SpanStatus::succeeded().into(),
            QueryOutcome::Error => SpanStatus::failed(self.query_error_adapter_message()).into(),
            QueryOutcome::Canceled => SpanStatus::failed("canceled").into(),
            QueryOutcome::Unspecified => None,
        }
    }

    fn has_sensitive_data(&self) -> bool {
        // SQL queries may contain sensitive data, so we mark this event as containing sensitive data.
        true
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(QueryExecuted {
            sql: "<redacted>".to_string(), // Redact the SQL query
            query_description: None,       // Redact the query description
            ..self.clone()
        }))
    }

    fn with_context(&mut self, context: &TelemetryContext) {
        let Some(context) = context.downcast_ref::<DbtTelemetryContext>() else {
            return;
        };

        // TODO: as of tody we do not inject unique_id from tracing context into
        // query executed event - we assume that all adapter code knows better when
        // to set it or not. However, this may be changed as an laternative for QueryCtx
        // Inject unique_id if not set and provided by context
        // if self.unique_id.is_none() {
        //     self.unique_id = context.unique_id.clone();
        // }

        // Inject phase if not set and provided by context
        if self.phase.is_none()
            && let Some(p) = context.phase
        {
            self.phase = Some(p as i32);
        }
    }
}

/// Internal struct used for serializing/deserializing subset of
/// QueryExecuted fields as JSON payload in ArrowAttributes.
#[skip_serializing_none]
#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq, Debug)]
struct QueryExecutedJsonPayload {
    sql: String,
    query_description: Option<String>,
    query_error_adapter_message: Option<String>,
}

impl ArrowSerializableTelemetryEvent for QueryExecuted {
    type ArrowRecord<'a> = ArrowAttributes<'a>;
    fn to_arrow_record(&self) -> ArrowAttributes<'_> {
        ArrowAttributes {
            // Well-known fields for easier querying
            content_hash: Some(Cow::from(self.sql_hash.as_str())),
            adapter_type: Some(Cow::from(self.adapter_type.as_str())),
            query_id: self.query_id.as_deref().map(Cow::Borrowed),
            unique_id: self.unique_id.as_deref().map(Cow::Borrowed),
            query_outcome: Some(self.query_outcome()),
            phase: self.phase.map(|_| self.phase()),
            query_error_vendor_code: self
                .query_error_vendor_code
                .map(|_| self.query_error_vendor_code()),
            dbt_core_event_code: Some(Cow::from(self.dbt_core_event_code.as_str())),
            // The rest of the data is serialized as JSON payload
            json_payload: serde_json::to_string(&QueryExecutedJsonPayload {
                sql: self.sql.clone(),
                query_description: self.query_description.clone(),
                query_error_adapter_message: self.query_error_adapter_message.clone(),
            })
            .unwrap_or_else(|_| {
                panic!(
                    "Failed to serialize data in event type \"{}\" to JSON",
                    Self::full_name()
                )
            })
            .into(),
            ..Default::default()
        }
    }

    fn from_arrow_record(record: &ArrowAttributes) -> Result<Self, String> {
        let json_payload: QueryExecutedJsonPayload =
            serde_json::from_str(record.json_payload.as_ref().ok_or_else(|| {
                format!(
                    "Missing json payload for event type \"{}\"",
                    Self::full_name()
                )
            })?)
            .map_err(|e| {
                format!(
                    "Failed to deserialize data of event type \"{}\" from JSON payload: {}",
                    Self::full_name(),
                    e
                )
            })?;

        Ok(Self {
            sql: json_payload.sql,
            sql_hash: record
                .content_hash
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `content_hash` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            adapter_type: record
                .adapter_type
                .as_deref()
                .map(str::to_string)
                .ok_or_else(|| {
                    format!(
                        "Missing `adapter_type` for event type \"{}\"",
                        Self::full_name()
                    )
                })?,
            query_description: json_payload.query_description.clone(),
            query_id: record.query_id.as_deref().map(str::to_string),
            unique_id: record.unique_id.as_deref().map(str::to_string),
            query_outcome: record.query_outcome.map(|v| v as i32).ok_or_else(|| {
                format!(
                    "Missing `query_outcome` for event type \"{}\"",
                    Self::full_name()
                )
            })?,
            phase: record.phase.map(|v| v as i32),
            query_error_adapter_message: json_payload.query_error_adapter_message,
            query_error_vendor_code: record.query_error_vendor_code,
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

impl StaticTelemetryEvent for ConnectionLimitWait {
    const RECORD_CATEGORY: TelemetryEventRecType = TelemetryEventRecType::Span;
    const OUTPUT_FLAGS: TelemetryOutputFlags = TelemetryOutputFlags::ALL;

    fn event_display_name(&self) -> String {
        let active_nodes_suffix = if let Some(active_nodes) = self.active_nodes {
            format!(" ({active_nodes} active nodes)")
        } else {
            "".to_string()
        };

        let active_connections_suffix = if let Some(active_connections) = self.active_connections {
            format!(" ({active_connections} active connections)")
        } else {
            "".to_string()
        };

        format!("Connection limit wait{active_nodes_suffix}{active_connections_suffix}")
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }
}

impl ArrowSerializableTelemetryEvent for ConnectionLimitWait {
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
