//! A structured tracing library built on top of the [`tracing`] crate.
//!
//! `dbt-tracing` extends `tracing` with *fully structured* telemetry: spans and
//! events are described by typed, self-describing event structs instead of
//! ad-hoc key/value fields. On top of that it provides more ergonomic emit and
//! span APIs, a data layer that materializes events into trace/log record
//! envelopes, middleware that can filter and transform events in flight, and
//! consumer infrastructure for convenient storage and export. It owns the
//! generic telemetry API, the record envelopes, status/location metadata, and
//! the serialization registry traits.
//!
//! The library is independent of any concrete event taxonomy. It defines the
//! traits, record types, generic serialization, and dbt-agnostic output layers,
//! but ships no dbt-specific event schemas or user-facing formatters. Users
//! provide those: Fusion and dbt-core define their event types in
//! `dbt-telemetry` (generated from protobuf), and the dbt/Fusion integration
//! layer — CLI config, user-facing formatting, and export assembly — lives in
//! `dbt-common::tracing`.
//!
//! [`tracing`]: https://docs.rs/tracing

pub mod async_tracing;
pub mod attributes;
pub mod background_writer;
pub mod constants;
pub mod convert;
pub mod data_provider;
mod debug_value;
pub mod emit;
pub mod error;
pub mod event_info;
pub mod filter;
pub mod init;
pub mod layer;
pub mod layers;
pub mod metrics;
pub mod reload;
pub mod rotating_file_writer;
pub mod schemas;
pub mod serialize;
mod shared;
pub mod shared_writer;
pub mod shutdown;
pub mod span_info;
mod static_name;

pub use debug_value::DebugValue;
pub use static_name::StaticName;

pub use attributes::{
    AnyTelemetryEvent, ArrowSerializableTelemetryEvent, StaticTelemetryEvent, TelemetryAttributes,
    TelemetryContext, TelemetryEventRecType, TelemetryOutputFlags,
};
pub use schemas::{
    LogRecordInfo, RecordCodeLocation, SeverityNumber, SpanEndInfo, SpanLinkInfo, SpanStartInfo,
    SpanStatus, StatusCode, TelemetryRecord, TelemetryRecordRef, TelemetryRecordType,
};
pub use serialize::deserialize::{
    IndexedTelemetryDeserializeError, PartialLogRecordInfo, PartialSpanEndInfo,
    PartialSpanStartInfo, PartialTelemetryRecord, TelemetryDeserializeError,
};

#[cfg(test)]
mod tests;

#[cfg(any(test, feature = "test-utils"))]
pub mod test_support;
