//! Public events that no human-readable sink renders.
//!
//! Each case emits fake instances of the event, the way the telemetry registry generates
//! them, and pins that every sink stays empty at its most verbose.

use dbt_telemetry::{
    AdapterConnectionClose, AdapterConnectionOpen, ArtifactWritten, CallTrace, ConnectionLimitWait,
    OnboardingScreenShown, PackageUpdate, Process, TelemetryEventTypeRegistry, Unknown,
};
use dbt_tracing::{StaticName, TelemetryAttributes, TelemetryEventRecType};

use super::support::{Case, DBT_LOG_TRACE, TERMINAL, TEXT_DEBUG};
use crate::tracing::emit::{create_info_span, emit_info_event};

pub(super) fn cases(cases: &mut Vec<Case>) {
    // Debug data, not user-facing text.
    unrendered::<CallTrace>(cases);
    unrendered::<Unknown>(cases);
    // Process attributes are only read for the invocation summary.
    unrendered::<Process>(cases);
    unrendered::<OnboardingScreenShown>(cases);
    // Artifacts are reported by the legacy JSON log only.
    unrendered::<ArtifactWritten>(cases);
    unrendered::<ConnectionLimitWait>(cases);
    unrendered::<PackageUpdate>(cases);
    unrendered::<AdapterConnectionOpen>(cases);
    unrendered::<AdapterConnectionClose>(cases);
}

/// Adds a case per fake instance of `T`, expecting every sink to stay empty.
fn unrendered<T: StaticName>(cases: &mut Vec<Case>) {
    let event_type = T::FULL_NAME;
    let fake = TelemetryEventTypeRegistry::public()
        .get_faker(event_type)
        .unwrap_or_else(|| panic!("{event_type} is not a public event"));
    let name = event_type
        .rsplit('.')
        .next()
        .expect("event type has a name");

    for (index, event) in fake(event_type).into_iter().enumerate() {
        let event = TelemetryAttributes::new(event);
        cases.push(
            Case::new(format!("{name}, fake #{index}"))
                .run(move |_| match event.record_category() {
                    TelemetryEventRecType::Span => drop(create_info_span(event.clone())),
                    TelemetryEventRecType::Log => emit_info_event(event.clone(), None),
                })
                .expect_no_output(DBT_LOG_TRACE)
                .expect_no_output(TEXT_DEBUG)
                .expect_no_output(TERMINAL),
        );
    }
}
