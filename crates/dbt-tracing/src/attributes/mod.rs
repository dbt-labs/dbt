//! Extensible telemetry attributes system.
//!
//! This module provides the infrastructure for defining custom telemetry event
//! data types also known as "attributes" that can be used in telemetry records,
//! enabling downstream users to extend the telemetry system with their own
//! attribute types.

mod context;
mod export;
mod traits;
mod wrapper;

pub use context::TelemetryContext;
pub use export::TelemetryOutputFlags;
pub use traits::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryEventRecType,
};
pub use wrapper::TelemetryAttributes;
