//! Test utilities for working with generated protobuf metadata and events.
//!
//! This module is compiled in this crate's tests and in dependents that enable
//! the `test-utils` feature. Keep helpers broadly useful and focused on testing.

use std::collections::{HashMap, hash_map::DefaultHasher};
use std::hash::{Hash, Hasher};
use std::time::SystemTime;

use dbt_tracing::{
    LogRecordInfo, SeverityNumber, SpanEndInfo, SpanStartInfo, SpanStatus, StatusCode,
    TelemetryAttributes, TelemetryEventRecType, TelemetryOutputFlags, TelemetryRecord,
    serialize::envelope::to_nanos,
};
use fake::rand::SeedableRng;
use fake::rand::rngs::StdRng;
use fake::{Fake, Faker};

use crate::TelemetryEventTypeRegistry;

/// Generate a pseudo-random but deterministic seed from a stable string.
pub fn hash_seed(seed: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    seed.hash(&mut hasher);
    hasher.finish()
}

fn test_severity(seed: u64, values: &[SeverityNumber]) -> SeverityNumber {
    values[(seed as usize) % values.len()]
}

fn fake_elapsed_nanos_within_timestamp_range(rng: &mut StdRng, start_time: &SystemTime) -> u64 {
    let elapsed = Faker.fake_with_rng(rng);
    let max_elapsed = u64::MAX - to_nanos(start_time);

    if max_elapsed == u64::MAX {
        elapsed
    } else {
        elapsed % (max_elapsed + 1)
    }
}

/// Create all fake telemetry attributes from a registry that match the requested output flags.
pub fn create_all_fake_attributes(
    seed: &str,
    registry: &TelemetryEventTypeRegistry,
    output_flags: TelemetryOutputFlags,
) -> Vec<TelemetryAttributes> {
    let mut attributes = Vec::new();

    for event_type in registry.iter() {
        let faker = registry
            .get_faker(event_type)
            .unwrap_or_else(|| panic!("No faker defined for event type \"{event_type}\""));

        for attr_boxed in faker(seed) {
            let attrs = TelemetryAttributes::new(attr_boxed);

            if !attrs.output_flags().contains(output_flags) {
                continue;
            }

            attributes.push(attrs);
        }
    }

    attributes
}

/// Create one test telemetry record per fake attribute, including matching span start/end pairs.
pub fn create_all_fake_records(
    seed: &str,
    registry: &TelemetryEventTypeRegistry,
    output_flags: TelemetryOutputFlags,
) -> Vec<TelemetryRecord> {
    let mut records = Vec::new();

    create_all_fake_attributes(seed, registry, output_flags)
        .into_iter()
        .for_each(|attributes| match attributes.record_category() {
            TelemetryEventRecType::Span => {
                let span_start = create_test_span_start(seed, attributes);
                let span_end = create_test_span_end(seed, &span_start);
                records.push(span_start);
                records.push(span_end);
            }
            TelemetryEventRecType::Log => {
                records.push(create_test_log_record(seed, attributes));
            }
        });

    records
}

pub fn create_test_span_start(seed: &str, attributes: TelemetryAttributes) -> TelemetryRecord {
    let hashed_seed = hash_seed(seed);
    let mut rng = StdRng::seed_from_u64(hashed_seed);
    let trace_id = Faker.fake_with_rng(&mut rng);
    let span_id = Faker.fake_with_rng(&mut rng);
    let parent_span_id = Faker.fake_with_rng(&mut rng);
    let start_time = Faker.fake_with_rng(&mut rng);

    TelemetryRecord::SpanStart(SpanStartInfo {
        trace_id,
        span_id,
        parent_span_id: Some(parent_span_id),
        links: None,
        span_name: attributes.event_display_name(),
        start_time_unix_nano: SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(start_time),
        attributes,
        severity_number: test_severity(
            hashed_seed,
            &[
                SeverityNumber::Trace,
                SeverityNumber::Debug,
                SeverityNumber::Info,
                SeverityNumber::Warn,
            ],
        ),
        severity_text: ["TRACE", "DEBUG", "INFO", "WARN"][(hashed_seed % 4) as usize].to_string(),
    })
}

pub fn create_test_span_end(seed: &str, span_start: &TelemetryRecord) -> TelemetryRecord {
    let TelemetryRecord::SpanStart(span_start_info) = span_start else {
        panic!("Expected SpanStart record");
    };

    let hashed_seed = hash_seed(seed);
    let mut rng = StdRng::seed_from_u64(hashed_seed);
    let elapsed =
        fake_elapsed_nanos_within_timestamp_range(&mut rng, &span_start_info.start_time_unix_nano);

    TelemetryRecord::SpanEnd(SpanEndInfo {
        trace_id: span_start_info.trace_id,
        span_id: span_start_info.span_id,
        parent_span_id: span_start_info.parent_span_id,
        links: span_start_info.links.clone(),
        span_name: span_start_info.span_name.clone(),
        start_time_unix_nano: span_start_info.start_time_unix_nano,
        end_time_unix_nano: span_start_info.start_time_unix_nano
            + std::time::Duration::from_nanos(elapsed),
        attributes: span_start_info.attributes.clone(),
        status: Some(SpanStatus {
            code: [StatusCode::Unset, StatusCode::Ok, StatusCode::Error]
                [(hashed_seed % 3) as usize],
            message: Some(format!("status_{}", hashed_seed % 100)),
        }),
        severity_number: test_severity(
            hashed_seed,
            &[
                SeverityNumber::Trace,
                SeverityNumber::Debug,
                SeverityNumber::Info,
                SeverityNumber::Warn,
            ],
        ),
        severity_text: ["TRACE", "DEBUG", "INFO", "WARN"][(hashed_seed % 4) as usize].to_string(),
    })
}

pub fn create_test_log_record(seed: &str, attributes: TelemetryAttributes) -> TelemetryRecord {
    let hashed_seed = hash_seed(seed);
    let mut rng = StdRng::seed_from_u64(hashed_seed);
    let trace_id = Faker.fake_with_rng(&mut rng);
    let span_id = Faker.fake_with_rng(&mut rng);
    let log_time = Faker.fake_with_rng(&mut rng);

    TelemetryRecord::LogRecord(LogRecordInfo {
        time_unix_nano: SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(log_time),
        trace_id,
        span_id: Some(span_id),
        event_id: Faker.fake_with_rng(&mut rng),
        span_name: Some(attributes.event_display_name()),
        severity_number: test_severity(
            hashed_seed,
            &[
                SeverityNumber::Error,
                SeverityNumber::Warn,
                SeverityNumber::Info,
                SeverityNumber::Debug,
            ],
        ),
        severity_text: ["ERROR", "WARN", "INFO", "DEBUG"][(hashed_seed % 4) as usize].to_string(),
        body: format!("Log message {}", hashed_seed % 10000),
        attributes,
    })
}

/// Return fully-qualified names (e.g. `v1.public.events.fusion.log.LogMessage`)
/// for all top-level messages across all packages.
///
/// Notes:
/// - Uses the pre-generated `dbtlabs_proto_public.bin` emitted by `xtask protogen`.
/// - Only considers top-level messages (nested message types are ignored).
pub fn all_message_full_names() -> Vec<String> {
    // FileDescriptorSet generated in `src/gen/dbtlabs_proto_public.bin` alongside rust code.
    // This is a stable artifact that doesn't affect production code paths.
    let bytes = include_bytes!("gen/dbtlabs_proto_public.bin");
    let fds = match prost_types::FileDescriptorSet::decode(bytes.as_ref()) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    let mut out = Vec::new();
    for file in fds.file.iter() {
        let package = file.package.as_deref().unwrap_or("");
        // Only enumerate top-level messages in the file (ignore nested types).
        for m in file.message_type.iter() {
            if let Some(name) = m.name.as_deref() {
                out.push(format!("{package}.{name}"));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Return a map of fully-qualified message names to their oneof field names.
///
/// For each message that has oneof fields, this returns a mapping from the
/// message's fully-qualified name to a vector of oneof field names within that message.
///
/// Notes:
/// - Uses the pre-generated `dbtlabs_proto_public.bin` emitted by `xtask protogen`.
/// - Only considers top-level messages (nested message types are ignored).
/// - Messages without oneof fields are excluded from the result.
/// - Filters out synthetic oneofs (proto3 optional fields) which have names starting with `_`.
///
/// # Example
///
/// ```
/// use dbt_telemetry::test_utils::message_oneofs;
///
/// let oneofs = message_oneofs();
/// if let Some(fields) = oneofs.get("v1.public.events.fusion.node.NodeEvaluated") {
///     assert!(fields.contains(&"node_outcome_detail".to_string()));
/// }
/// ```
pub fn message_oneofs() -> HashMap<String, Vec<String>> {
    let bytes = include_bytes!("gen/dbtlabs_proto_public.bin");
    let fds = match prost_types::FileDescriptorSet::decode(bytes.as_ref()) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };

    let mut result = HashMap::new();

    for file in fds.file.iter() {
        let package = file.package.as_deref().unwrap_or("");

        // Only consider top-level messages
        for message in file.message_type.iter() {
            if let Some(message_name) = message.name.as_deref() {
                let full_name = format!("{package}.{message_name}");

                // Check if this message has any oneof declarations
                if !message.oneof_decl.is_empty() {
                    let mut oneof_names = Vec::new();
                    for oneof in message.oneof_decl.iter() {
                        if let Some(oneof_name) = oneof.name.as_deref() {
                            // Filter out synthetic oneofs for proto3 optional fields
                            // These have names starting with '_' (e.g., "_field_name")
                            if !oneof_name.starts_with('_') {
                                oneof_names.push(oneof_name.to_string());
                            }
                        }
                    }

                    // Only include messages with real (non-synthetic) oneofs
                    if !oneof_names.is_empty() {
                        result.insert(full_name, oneof_names);
                    }
                }
            }
        }
    }

    result
}

// Prost types needed for decoding the descriptor set
use prost::Message as _;
use prost_types as _;
