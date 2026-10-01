//! Registry for telemetry attribute types.

use arrow_schema::{DataType, Field, Fields, extension::Json as JsonExtensionType};
use dbt_tracing::{
    StaticName,
    serialize::traits::{
        ArrowRegistryLookup, JsonRegistryLookup, TelemetryAttributeDeserializeError,
    },
};
use serde::de::DeserializeOwned;
use std::{collections::HashMap, sync::LazyLock};

use super::AnyTelemetryEvent;
use crate::{
    attributes::ArrowSerializableTelemetryEvent,
    schemas::{
        AdapterConnectionClose, AdapterConnectionOpen, ArtifactWritten, AssetParsed, CallTrace,
        CompiledCode, CompiledCodeInline, ConnectionLimitWait, DepsAddPackage,
        DepsAllPackagesInstalled, DepsPackageInstalled, GenericOpExecuted, GenericOpItemProcessed,
        HookProcessed, Invocation, ListItemOutput, LogMessage, NodeEvaluated, NodeProcessed,
        OnboardingScreenShown, PackageUpdate, PhaseExecuted, Process, ProgressMessage,
        QueryExecuted, ShowDataOutput, ShowResult, StateModifiedDiff, Unknown, UserLogMessage,
    },
    serialize::arrow::ArrowAttributes as DbtTelemetryArrowAttributes,
};

/// Helper function that converts trait deserializer method to one compatible with the registry.
fn arrow_deserialize_for_type<T>(
    attrs: &T::ArrowRecord<'_>,
) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>
where
    T: AnyTelemetryEvent + ArrowSerializableTelemetryEvent,
{
    T::from_arrow_record(attrs)
        .map(|t| Box::new(t) as Box<dyn AnyTelemetryEvent>)
        .map_err(TelemetryAttributeDeserializeError::malformed)
}

/// Helper function that deserializes JSON attributes to a concrete telemetry event.
fn json_deserialize_for_type<T>(
    attrs: &serde_json::Value,
) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>
where
    T: AnyTelemetryEvent + DeserializeOwned,
{
    T::deserialize(attrs)
        .map(|t| Box::new(t) as Box<dyn AnyTelemetryEvent>)
        .map_err(|err| {
            TelemetryAttributeDeserializeError::malformed(format!(
                "Failed to deserialize JSON attributes: {err}"
            ))
        })
}

/// Helper function to create a faker for a given type.
/// Returns multiple variants of the message:
/// - One using Faker
/// - One using Default::default()
#[cfg(any(test, feature = "test-utils"))]
fn faker_for_type<T>(seed: &str) -> Vec<Box<dyn AnyTelemetryEvent>>
where
    T: AnyTelemetryEvent + fake::Dummy<fake::Faker> + Default,
{
    use fake::rand::SeedableRng;
    use fake::rand::rngs::StdRng;
    use fake::{Fake, Faker};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    // Generate pseudo-random but deterministic values for testing
    fn hash_seed(seed: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        seed.hash(&mut hasher);
        hasher.finish()
    }

    let hashed_seed = hash_seed(seed);
    let mut rng = StdRng::seed_from_u64(hashed_seed);

    let mut variants = Vec::new();

    let faker_variant: T = Faker.fake_with_rng(&mut rng);
    variants.push(Box::new(faker_variant) as Box<dyn AnyTelemetryEvent>);

    let default_variant = T::default();
    variants.push(Box::new(default_variant) as Box<dyn AnyTelemetryEvent>);

    variants
}

/// Macro to generate faker functions for messages with oneof fields.
///
/// This macro creates a function that returns multiple variants of a message:
/// - One using Faker
/// - One for each variant of each specified oneof field
///
/// The oneof variants are iterated separately (not combined), so if you have
/// two oneof fields with 3 and 4 variants respectively, you'll get:
/// 1 (faker) + 3 (first oneof) + 4 (second oneof) = 8 variants total
///
/// # Usage
///
/// ```ignore
/// faker_for_type_with_oneofs!(
///     faker_for_my_message,          // Name of the function to generate
///     MyMessage,                      // The message type
///     MyOneofEnum => field_name,      // First oneof: enum type => field name
///     AnotherOneofEnum => other_field // Second oneof: enum type => field name
/// );
/// ```
#[cfg(any(test, feature = "test-utils"))]
macro_rules! faker_for_type_with_oneofs {
    ($fn_name:ident, $message_type:ty $(, $oneof_enum:ty => $field_name:ident)*) => {
        #[allow(unused_imports)]
        fn $fn_name(seed: &str) -> Vec<Box<dyn AnyTelemetryEvent>> {
            use fake::rand::SeedableRng;
            use fake::rand::rngs::StdRng;
            use fake::{Fake, Faker};
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            // Generate pseudo-random but deterministic values for testing
            fn hash_seed(seed: &str) -> u64 {
                let mut hasher = DefaultHasher::new();
                seed.hash(&mut hasher);
                hasher.finish()
            }

            let hashed_seed = hash_seed(seed);
            let mut rng = StdRng::seed_from_u64(hashed_seed);

            let mut variants = Vec::new();

            // First variant: using Faker
            let faker_variant: $message_type = Faker.fake_with_rng(&mut rng);
            variants.push(Box::new(faker_variant) as Box<dyn AnyTelemetryEvent>);

            // Additional variants: one for each oneof enum variant
            // For each oneof field specified, iterate over its variants
            $(
                for oneof_variant in <$oneof_enum as strum::IntoEnumIterator>::iter() {
                    let mut base = <$message_type>::default();
                    base.$field_name = Some(oneof_variant);
                    variants.push(Box::new(base) as Box<dyn AnyTelemetryEvent>);
                }
            )*

            variants
        }
    };
}

pub type ArrowDeserializerFn =
    for<'a> fn(
        &DbtTelemetryArrowAttributes<'a>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>;
pub type JsonDeserializerFn =
    fn(
        &serde_json::Value,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError>;
#[cfg(any(test, feature = "test-utils"))]
pub type FakerFn = fn(&str) -> Vec<Box<dyn AnyTelemetryEvent>>;

/// Registry for telemetry attribute type deserializers.
///
/// This registry maps event type identifiers to deserializer functions,
/// enabling runtime dispatch during deserialization.
#[derive(Clone, Default)]
pub struct TelemetryEventTypeRegistry {
    arrow_deserializers: HashMap<&'static str, ArrowDeserializerFn>,
    json_deserializers: HashMap<&'static str, JsonDeserializerFn>,

    #[cfg(any(test, feature = "test-utils"))]
    fakers: HashMap<&'static str, FakerFn>,
}

static PUBLIC_TELEMETRY_EVENT_REGISTRY: LazyLock<TelemetryEventTypeRegistry> = LazyLock::new(
    || {
        let mut registry = TelemetryEventTypeRegistry::new();

        // Register span event types
        registry.register(
            CallTrace::FULL_NAME,
            arrow_deserialize_for_type::<CallTrace>,
            json_deserialize_for_type::<CallTrace>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<CallTrace>,
        );
        registry.register(
            Invocation::FULL_NAME,
            arrow_deserialize_for_type::<Invocation>,
            json_deserialize_for_type::<Invocation>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<Invocation>,
        );
        registry.register(
            Process::FULL_NAME,
            arrow_deserialize_for_type::<Process>,
            json_deserialize_for_type::<Process>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<Process>,
        );
        registry.register(
            PhaseExecuted::FULL_NAME,
            arrow_deserialize_for_type::<PhaseExecuted>,
            json_deserialize_for_type::<PhaseExecuted>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<PhaseExecuted>,
        );
        registry.register(
            AssetParsed::FULL_NAME,
            arrow_deserialize_for_type::<AssetParsed>,
            json_deserialize_for_type::<AssetParsed>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<AssetParsed>,
        );
        registry.register(
            OnboardingScreenShown::FULL_NAME,
            arrow_deserialize_for_type::<OnboardingScreenShown>,
            json_deserialize_for_type::<OnboardingScreenShown>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<OnboardingScreenShown>,
        );

        // Needs a custom faker due to oneof fields
        #[cfg(any(test, feature = "test-utils"))]
        faker_for_type_with_oneofs!(
            faker_for_node_evaluated,
            NodeEvaluated,
            crate::proto::v1::public::events::fusion::node::node_evaluated::NodeOutcomeDetail => node_outcome_detail
        );
        registry.register(
            DepsAddPackage::FULL_NAME,
            arrow_deserialize_for_type::<DepsAddPackage>,
            json_deserialize_for_type::<DepsAddPackage>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<DepsAddPackage>,
        );
        registry.register(
            DepsPackageInstalled::FULL_NAME,
            arrow_deserialize_for_type::<DepsPackageInstalled>,
            json_deserialize_for_type::<DepsPackageInstalled>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<DepsPackageInstalled>,
        );
        registry.register(
            DepsAllPackagesInstalled::FULL_NAME,
            arrow_deserialize_for_type::<DepsAllPackagesInstalled>,
            json_deserialize_for_type::<DepsAllPackagesInstalled>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<DepsAllPackagesInstalled>,
        );
        registry.register(
            GenericOpExecuted::FULL_NAME,
            arrow_deserialize_for_type::<GenericOpExecuted>,
            json_deserialize_for_type::<GenericOpExecuted>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<GenericOpExecuted>,
        );
        registry.register(
            GenericOpItemProcessed::FULL_NAME,
            arrow_deserialize_for_type::<GenericOpItemProcessed>,
            json_deserialize_for_type::<GenericOpItemProcessed>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<GenericOpItemProcessed>,
        );
        registry.register(
            HookProcessed::FULL_NAME,
            arrow_deserialize_for_type::<HookProcessed>,
            json_deserialize_for_type::<HookProcessed>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<HookProcessed>,
        );
        registry.register(
            NodeEvaluated::FULL_NAME,
            arrow_deserialize_for_type::<NodeEvaluated>,
            json_deserialize_for_type::<NodeEvaluated>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_node_evaluated,
        );
        // Needs a custom faker due to oneof fields
        #[cfg(any(test, feature = "test-utils"))]
        faker_for_type_with_oneofs!(
            faker_for_node_processed,
            NodeProcessed,
            crate::proto::v1::public::events::fusion::node::node_processed::NodeOutcomeDetail => node_outcome_detail
        );
        registry.register(
            NodeProcessed::FULL_NAME,
            arrow_deserialize_for_type::<NodeProcessed>,
            json_deserialize_for_type::<NodeProcessed>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_node_processed,
        );
        registry.register(
            ArtifactWritten::FULL_NAME,
            arrow_deserialize_for_type::<ArtifactWritten>,
            json_deserialize_for_type::<ArtifactWritten>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ArtifactWritten>,
        );
        registry.register(
            Unknown::FULL_NAME,
            arrow_deserialize_for_type::<Unknown>,
            json_deserialize_for_type::<Unknown>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<Unknown>,
        );
        registry.register(
            QueryExecuted::FULL_NAME,
            arrow_deserialize_for_type::<QueryExecuted>,
            json_deserialize_for_type::<QueryExecuted>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<QueryExecuted>,
        );
        registry.register(
            ConnectionLimitWait::FULL_NAME,
            arrow_deserialize_for_type::<ConnectionLimitWait>,
            json_deserialize_for_type::<ConnectionLimitWait>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ConnectionLimitWait>,
        );

        // Register log attributes
        registry.register(
            LogMessage::FULL_NAME,
            arrow_deserialize_for_type::<LogMessage>,
            json_deserialize_for_type::<LogMessage>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<LogMessage>,
        );
        registry.register(
            StateModifiedDiff::FULL_NAME,
            arrow_deserialize_for_type::<StateModifiedDiff>,
            json_deserialize_for_type::<StateModifiedDiff>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<StateModifiedDiff>,
        );
        registry.register(
            UserLogMessage::FULL_NAME,
            arrow_deserialize_for_type::<UserLogMessage>,
            json_deserialize_for_type::<UserLogMessage>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<UserLogMessage>,
        );
        registry.register(
            ProgressMessage::FULL_NAME,
            arrow_deserialize_for_type::<ProgressMessage>,
            json_deserialize_for_type::<ProgressMessage>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ProgressMessage>,
        );
        registry.register(
            CompiledCodeInline::FULL_NAME,
            arrow_deserialize_for_type::<CompiledCodeInline>,
            json_deserialize_for_type::<CompiledCodeInline>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<CompiledCodeInline>,
        );
        registry.register(
            CompiledCode::FULL_NAME,
            arrow_deserialize_for_type::<CompiledCode>,
            json_deserialize_for_type::<CompiledCode>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<CompiledCode>,
        );
        registry.register(
            ListItemOutput::FULL_NAME,
            arrow_deserialize_for_type::<ListItemOutput>,
            json_deserialize_for_type::<ListItemOutput>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ListItemOutput>,
        );
        registry.register(
            ShowDataOutput::FULL_NAME,
            arrow_deserialize_for_type::<ShowDataOutput>,
            json_deserialize_for_type::<ShowDataOutput>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ShowDataOutput>,
        );
        registry.register(
            ShowResult::FULL_NAME,
            arrow_deserialize_for_type::<ShowResult>,
            json_deserialize_for_type::<ShowResult>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<ShowResult>,
        );
        registry.register(
            PackageUpdate::FULL_NAME,
            arrow_deserialize_for_type::<PackageUpdate>,
            json_deserialize_for_type::<PackageUpdate>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<PackageUpdate>,
        );
        registry.register(
            AdapterConnectionOpen::FULL_NAME,
            arrow_deserialize_for_type::<AdapterConnectionOpen>,
            json_deserialize_for_type::<AdapterConnectionOpen>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<AdapterConnectionOpen>,
        );
        registry.register(
            AdapterConnectionClose::FULL_NAME,
            arrow_deserialize_for_type::<AdapterConnectionClose>,
            json_deserialize_for_type::<AdapterConnectionClose>,
            #[cfg(any(test, feature = "test-utils"))]
            faker_for_type::<AdapterConnectionClose>,
        );

        registry
    },
);

impl TelemetryEventTypeRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Default::default()
    }

    /// Register an event type with explicit event type string.
    pub fn register(
        &mut self,
        event_type: &'static str,
        arrow_deserializer: ArrowDeserializerFn,
        json_deserializer: JsonDeserializerFn,
        #[cfg(any(test, feature = "test-utils"))] faker: FakerFn,
    ) {
        self.arrow_deserializers
            .insert(event_type, arrow_deserializer);
        self.json_deserializers
            .insert(event_type, json_deserializer);

        #[cfg(any(test, feature = "test-utils"))]
        self.fakers.insert(event_type, faker);
    }

    /// Get an arrow deserializer for the given event type.
    pub fn get_arrow_deserializer(&self, event_type: &str) -> Option<ArrowDeserializerFn> {
        self.arrow_deserializers.get(event_type).copied()
    }

    /// Get a JSON deserializer for the given event type.
    pub fn get_json_deserializer(&self, event_type: &str) -> Option<JsonDeserializerFn> {
        self.json_deserializers.get(event_type).copied()
    }

    /// Get a faker function for the given event type.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn get_faker(&self, event_type: &str) -> Option<FakerFn> {
        self.fakers.get(event_type).copied()
    }

    /// Create a registry with all built-in attribute types from this crate.
    pub fn public() -> &'static Self {
        &PUBLIC_TELEMETRY_EVENT_REGISTRY
    }

    /// Merge another registry into this one.
    ///
    /// This is useful for combining the public registry with custom attribute types.
    pub fn merge(&mut self, other: TelemetryEventTypeRegistry) {
        self.arrow_deserializers.extend(other.arrow_deserializers);
        self.json_deserializers.extend(other.json_deserializers);
        #[cfg(any(test, feature = "test-utils"))]
        self.fakers.extend(other.fakers);
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static str> {
        self.arrow_deserializers.keys().copied()
    }
}

fn dict_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(
        name,
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        nullable,
    )
}

fn large_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::LargeUtf8, nullable)
}

fn json_large_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::LargeUtf8, nullable)
        .with_extension_type(JsonExtensionType::default())
}

fn create_arrow_attributes_fields() -> Fields {
    Fields::from(vec![
        // JSON blob for non well-known attributes
        json_large_utf8_field("json_payload", true),
        // Well-known common fields
        dict_utf8_field("name", true),
        dict_utf8_field("database", true),
        dict_utf8_field("schema", true),
        dict_utf8_field("identifier", true),
        dict_utf8_field("dbt_core_event_code", true),
        // Phase
        dict_utf8_field("phase", true),
        // Node fields
        dict_utf8_field("unique_id", true),
        dict_utf8_field("materialization", true),
        dict_utf8_field("custom_materialization", true),
        dict_utf8_field("node_type", true),
        dict_utf8_field("node_outcome", true),
        dict_utf8_field("node_error_type", true),
        dict_utf8_field("node_cancel_reason", true),
        dict_utf8_field("node_skip_reason", true),
        Field::new("sao_enabled", DataType::Boolean, true),
        // CallTrace/Unknown fields
        dict_utf8_field("dev_name", true),
        // Fusion origin code location fields (debug only)
        dict_utf8_field("file", true),
        Field::new("line", DataType::UInt32, true),
        // Log fields
        Field::new("code", DataType::UInt32, true),
        dict_utf8_field("code_name", true),
        Field::new("original_severity_number", DataType::Int32, true),
        dict_utf8_field("original_severity_text", true),
        dict_utf8_field("package_name", true),
        // Artifact or node paths & location
        large_utf8_field("relative_path", true),
        Field::new("code_line", DataType::UInt32, true),
        Field::new("code_column", DataType::UInt32, true),
        dict_utf8_field("artifact_type", true),
        // Query fields
        large_utf8_field("query_id", true),
        dict_utf8_field("query_outcome", true),
        dict_utf8_field("adapter_type", true),
        Field::new("query_error_vendor_code", DataType::Int32, true),
        // Content hash (e.g. CAS hash for artifacts stored in CAS)
        large_utf8_field("content_hash", true),
        // List command output fields
        dict_utf8_field("output_format", true),
        large_utf8_field("content", true),
        // Node processing duration
        Field::new("duration_ms", DataType::UInt64, true),
        // Number of rows affected by this event
        Field::new("rows_affected", DataType::UInt64, true),
        // Group identifier for model notifications
        large_utf8_field("group", true),
    ])
}

impl ArrowRegistryLookup for TelemetryEventTypeRegistry {
    type ArrowAttributes<'a> = DbtTelemetryArrowAttributes<'a>;

    fn arrow_attributes_fields() -> Fields {
        create_arrow_attributes_fields()
    }

    fn deserialize_arrow_attributes(
        &self,
        event_type: &str,
        attributes: &Self::ArrowAttributes<'_>,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError> {
        let deserializer = self
            .get_arrow_deserializer(event_type)
            .ok_or_else(|| TelemetryAttributeDeserializeError::unknown_event_type(event_type))?;
        deserializer(attributes)
    }
}

impl JsonRegistryLookup for TelemetryEventTypeRegistry {
    fn deserialize_json_attributes(
        &self,
        event_type: &str,
        attributes: &serde_json::Value,
    ) -> Result<Box<dyn AnyTelemetryEvent>, TelemetryAttributeDeserializeError> {
        let deserializer = self
            .get_json_deserializer(event_type)
            .ok_or_else(|| TelemetryAttributeDeserializeError::unknown_event_type(event_type))?;
        deserializer(attributes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    // Smoke test: ensure the registry contains all message types we expose
    // under v1.public.events.fusion subpackages that dbt-telemetry supports.
    #[test]
    fn registry_covers_fusion_subpackage_messages() {
        // Enumerate all top-level messages and filter to supported fusion subpackages.
        let all = crate::test_utils::all_message_full_names();

        // We intentionally maintain a opt out list of known non-top-level
        // messages that are not first-class events in the registry. This
        // way we ensure that when new ones are added, they will be
        // automatically picked up by this test.
        let expected: HashSet<String> = all
            .into_iter()
            .filter(|n| n.starts_with("v1.public.events.fusion."))
            .filter(|n| {
                ![
                    // Ignore helper/embedded types that are not first-class events in the registry.
                    // dev
                    "v1.public.events.fusion.dev.DebugValue",
                    // invocation helpers
                    "v1.public.events.fusion.invocation.InvocationEvalArgs",
                    "v1.public.events.fusion.invocation.InvocationMetrics",
                    // node
                    "v1.public.events.fusion.node.NodeCacheDetail",
                    "v1.public.events.fusion.node.NodeEvaluationDetail",
                    "v1.public.events.fusion.node.NodeSkipUpstreamDetail",
                    "v1.public.events.fusion.node.SourceFreshnessDetail",
                    "v1.public.events.fusion.node.TestEvaluationDetail",
                ]
                .contains(&n.as_str())
            })
            .collect();

        // Build the actual set from the registry keys.
        let registry = TelemetryEventTypeRegistry::public();
        let actual: HashSet<String> = registry.iter().map(str::to_string).collect();

        // Compute missing ones and report the full list for easier debugging.
        let mut missing: Vec<String> = expected.difference(&actual).cloned().collect();
        missing.sort();
        let mut unexpected: Vec<String> = actual.difference(&expected).cloned().collect();
        unexpected.sort();

        assert!(
            missing.is_empty(),
            "Missing deserializers for: {}",
            missing.join(", ")
        );
        assert!(
            unexpected.is_empty(),
            "Unexpected deserializers for: {}",
            unexpected.join(", ")
        );
    }

    /// Test that verifies faker functions for types with oneofs return more than 2 variants.
    ///
    /// # How this test works
    ///
    /// This is a very naive test, but better than nothing. It works by:
    /// 1. Using dbt-telemetry test utilities to get all messages with oneofs
    /// 2. For each registered event type, checking if it's in the oneof list
    /// 3. If it has oneofs, verifying the faker returns > 2 variants
    ///
    /// Regular types return exactly 2 variants (Faker + Default), while types with
    /// oneofs should return at least 3 (Faker + N oneof variants where N >= 1).
    ///
    /// # False positives
    ///
    /// This test may produce false positives for single-field oneofs (where the oneof
    /// has only one variant). In such cases, you'd get Faker + 1 oneof variant = 2 total,
    /// which would fail this test. However, single-field oneofs are generally a code smell
    /// and should be avoided - use an optional field instead.
    ///
    /// If you encounter this, consider refactoring the proto definition to avoid
    /// single-variant oneofs, as they add unnecessary complexity.
    #[test]
    fn faker_functions_with_oneofs_return_multiple_variants() {
        let oneofs = crate::test_utils::message_oneofs();
        let registry = TelemetryEventTypeRegistry::public();

        // Track which types we've verified have oneofs
        let mut types_with_oneofs_verified = Vec::new();

        for event_type in registry.iter() {
            let faker = registry
                .get_faker(event_type)
                .unwrap_or_else(|| panic!("No faker for event type \"{event_type}\""));

            let variants = faker("test_seed_for_oneof_check");

            // Check if this type has oneofs according to proto metadata
            if oneofs.contains_key(event_type) {
                assert!(
                    variants.len() > 1 + oneofs.get(event_type).unwrap().len(),
                    "Type \"{event_type}\" has oneof fields {:?} but faker returned only {} variants. \
                    Expected > 2 (Faker + K oneof fields * 2+ oneof variants each). \
                    If this is a single-variant oneof, consider refactoring to use an optional field instead.",
                    oneofs.get(event_type),
                    variants.len()
                );
                types_with_oneofs_verified.push(event_type);
            } else {
                // Types without oneofs should return exactly 2 variants (Faker + Default)
                assert_eq!(
                    variants.len(),
                    2,
                    "Type \"{event_type}\" has no oneofs but faker returned {} variants (expected 2)",
                    variants.len()
                );
            }
        }

        // Verify we actually tested some types with oneofs
        assert!(
            !types_with_oneofs_verified.is_empty(),
            "No types with oneofs were verified. Expected at least NodeEvaluated to have oneofs."
        );
    }
}
