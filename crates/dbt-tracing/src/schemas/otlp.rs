//! Types/models matching the OpenTelemetry OTLP proto specs used by
//! dbt-fusion for telemetry.
//!
//! These are not generated or imported from opentelemetry-proto for three reasons:
//! 1. Only a handful of native models from the OTLP spec are used as is
//! 2. We derive JSON Schema for these structs to allow for validation and documentation generation
//! 3. We subset some of the enums as we are not using all of the values defined in the OTLP spec

use dbt_yaml::JsonSchema;
#[cfg(any(test, feature = "test-utils"))]
use fake::Dummy;
use schemars::JsonSchema_repr;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use strum::FromRepr;

#[cfg_attr(any(test, feature = "test-utils"), derive(Dummy))]
#[derive(
    Serialize_repr, Deserialize_repr, Debug, JsonSchema_repr, Clone, Copy, PartialEq, Eq, FromRepr,
)]
#[repr(u8)]
#[derive(Default)]
pub enum StatusCode {
    /// The default status.
    #[default]
    Unset = 0,
    /// The Span has been validated by an Application developer or Operator to
    /// have completed successfully.
    Ok = 1,
    /// The Span contains an error.
    Error = 2,
}

/// The final status of a span
#[cfg_attr(any(test, feature = "test-utils"), derive(Dummy))]
#[derive(Serialize, Deserialize, Debug, JsonSchema, Clone, Default, PartialEq, Eq)]
pub struct SpanStatus {
    pub message: Option<String>,
    pub code: StatusCode,
}

impl SpanStatus {
    pub fn succeeded() -> Self {
        Self {
            message: None,
            code: StatusCode::Ok,
        }
    }

    pub fn failed(message: &str) -> Self {
        Self {
            message: Some(message.to_string()),
            code: StatusCode::Error,
        }
    }
}
