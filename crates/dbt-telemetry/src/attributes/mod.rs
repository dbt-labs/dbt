//! Extensible telemetry attributes system.
//!
//! This module provides the infrastructure for defining custom telemetry event data types
//! also known as "attributes" that can be used in telemetry records, enabling downstream
//! users to extend the telemetry system with their own attribute types.

mod context;
mod registry;

pub use context::DbtTelemetryContext;
use dbt_tracing::{AnyTelemetryEvent, ArrowSerializableTelemetryEvent};
pub use registry::TelemetryEventTypeRegistry;
