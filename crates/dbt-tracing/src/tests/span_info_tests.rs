use crate::{
    AnyTelemetryEvent, SpanStatus, TelemetryAttributes, TelemetryEventRecType,
    TelemetryOutputFlags,
    emit::{create_info_span, create_root_info_span},
    init::create_tracing_subcriber_with_layer,
    layer::ConsumerLayer,
    span_info::{
        get_root_span_ref, record_current_span_status_from_attrs, record_span_status,
        record_span_status_from_attrs, record_span_status_with_attrs,
    },
};
use dbt_base::{HashMap, hashmap};
use serde::Serialize;
use tracing_subscriber::{Registry, registry::LookupSpan as _};

use super::mocks::{TestLayer, test_data_layer};

#[derive(Debug, Clone, PartialEq, Serialize)]
struct TestStatusEvent {
    name: String,
    note: Option<String>,
    status: Option<SpanStatus>,
}

impl TestStatusEvent {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            note: None,
            status: None,
        }
    }
}

impl AnyTelemetryEvent for TestStatusEvent {
    fn event_type(&self) -> &'static str {
        "v1.test.events.fusion.TestStatusEvent"
    }

    fn event_display_name(&self) -> String {
        self.name.clone()
    }

    fn get_span_status(&self) -> Option<SpanStatus> {
        self.status.clone()
    }

    fn record_category(&self) -> TelemetryEventRecType {
        TelemetryEventRecType::Span
    }

    fn output_flags(&self) -> TelemetryOutputFlags {
        TelemetryOutputFlags::empty()
    }

    fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|rhs| rhs == self)
    }

    fn has_sensitive_data(&self) -> bool {
        false
    }

    fn clone_without_sensitive_data(&self) -> Option<Box<dyn AnyTelemetryEvent>> {
        Some(Box::new(self.clone()))
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }

    fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
        Box::new(self.clone())
    }

    fn to_json(&self) -> Result<serde_json::Value, String> {
        serde_json::to_value(self).map_err(|err| format!("Failed to serialize: {err}"))
    }
}

#[test]
fn test_record_span_attrs_and_status() {
    const CHILD_STATUS_ERR: &str = "child failure";
    const ATTRS_NOTE: &str = "mutated via attrs";
    const ATTRS_FAILURE: &str = "attrs failure";
    const CURRENT_FAILURE: &str = "current failure";
    const CURRENT_NOTE: &str = "from current";

    let trace_id = rand::random::<u128>();

    let (test_layer, _, span_ends, _) = TestLayer::new();

    // Init telemetry using internal API allowing to set thread local subscriber.
    // This avoids collisions with other unit tests
    let subscriber = create_tracing_subcriber_with_layer(
        tracing::level_filters::LevelFilter::TRACE,
        test_data_layer(
            trace_id,
            None, // parent_span_id not needed in tests
            false,
            std::iter::empty(),
            std::iter::once(Box::new(test_layer) as ConsumerLayer),
        ),
        &[],
    )
    .expect("test tracing filter directives must be valid");

    tracing::subscriber::with_default(subscriber, || {
        let root_span = create_root_info_span(TestStatusEvent::new("root"));
        {
            let _root_scope = root_span.enter();

            let child_status = create_info_span(TestStatusEvent::new("child_status"));
            {
                let _child_scope = child_status.enter();
                record_span_status(&child_status, Some(CHILD_STATUS_ERR));
            }
            drop(child_status);

            let child_attrs = create_info_span(TestStatusEvent::new("child_attrs"));
            {
                let _child_scope = child_attrs.enter();
                record_span_status_with_attrs(
                    &child_attrs,
                    |attrs| {
                        let event = attrs
                            .downcast_mut::<TestStatusEvent>()
                            .expect("child span should use test event");
                        event.note = Some(ATTRS_NOTE.to_string());
                        event.status = Some(SpanStatus::succeeded());
                    },
                    None,
                );

                let grandchild_from_attrs =
                    create_info_span(TestStatusEvent::new("grandchild_from_attrs"));
                {
                    let _gc_scope = grandchild_from_attrs.enter();
                    record_span_status_from_attrs(&grandchild_from_attrs, |attrs| {
                        let event = attrs
                            .downcast_mut::<TestStatusEvent>()
                            .expect("grandchild span should use test event");
                        event.note = Some(ATTRS_FAILURE.to_string());
                        event.status = Some(SpanStatus::failed(ATTRS_FAILURE));
                    });
                }
                drop(grandchild_from_attrs);

                let grandchild_current =
                    create_info_span(TestStatusEvent::new("grandchild_current"));
                {
                    let _gc_scope = grandchild_current.enter();

                    let root_name = tracing::dispatcher::get_default(|dispatch| {
                        let registry = dispatch.downcast_ref::<Registry>()?;
                        let current_span = dispatch.current_span();
                        let current_id = current_span.id()?;
                        let current_span_ref = registry.span(current_id)?;
                        let root_ref = get_root_span_ref(current_span_ref);
                        let extensions = root_ref.extensions();
                        let attrs = extensions.get::<TelemetryAttributes>()?;
                        let event = attrs.downcast_ref::<TestStatusEvent>()?;
                        Some(event.name.clone())
                    })
                    .expect("current span must have a root span in the active registry");
                    assert_eq!(root_name, "root");

                    record_current_span_status_from_attrs(|attrs| {
                        let event = attrs
                            .downcast_mut::<TestStatusEvent>()
                            .expect("grandchild span should use test event");
                        event.note = Some(CURRENT_NOTE.to_string());
                        event.status = Some(SpanStatus::failed(CURRENT_FAILURE));
                    });
                }
                drop(grandchild_current);
            }
            drop(child_attrs);
        }
        drop(root_span);
    });

    let span_ends = {
        let guard = span_ends.lock().expect("span ends lock poisoned");
        guard.clone()
    };

    let mut observed: HashMap<String, (Option<SpanStatus>, TestStatusEvent)> = hashmap::new();

    for span_end in &span_ends {
        if let Some(event) = span_end.attributes.downcast_ref::<TestStatusEvent>() {
            observed.insert(event.name.clone(), (span_end.status.clone(), event.clone()));
        } else {
            panic!("Unexpected type attributes!");
        }
    }

    assert_eq!(observed.len(), 5, "expected five spans to be captured");

    let root_entry = observed
        .get("root")
        .expect("root span should be present in captured data");
    assert_eq!(
        root_entry.0, None,
        "root span should not have explicit status"
    );

    let child_status_entry = observed
        .get("child_status")
        .expect("child_status span missing");
    assert_eq!(
        child_status_entry.0,
        Some(SpanStatus::failed(CHILD_STATUS_ERR)),
        "record_span_status should set failure status"
    );
    assert_eq!(
        child_status_entry.1.note, None,
        "record_span_status should not mutate attributes"
    );

    let child_attrs_entry = observed
        .get("child_attrs")
        .expect("child_attrs span missing");
    assert_eq!(
        child_attrs_entry.0,
        Some(SpanStatus::succeeded()),
        "record_span_status_with_attrs should set success status"
    );
    assert_eq!(
        child_attrs_entry.1.note,
        Some(ATTRS_NOTE.to_string()),
        "record_span_status_with_attrs should mutate attributes"
    );

    let from_attrs_entry = observed
        .get("grandchild_from_attrs")
        .expect("grandchild_from_attrs span missing");
    assert_eq!(
        from_attrs_entry.0,
        Some(SpanStatus::failed(ATTRS_FAILURE)),
        "record_span_status_from_attrs should use inferred status"
    );
    assert_eq!(
        from_attrs_entry.1.note,
        Some(ATTRS_FAILURE.to_string()),
        "record_span_status_from_attrs should persist attribute updates"
    );
    assert_eq!(
        from_attrs_entry.1.status,
        Some(SpanStatus::failed(ATTRS_FAILURE)),
        "attributes should retain inferred status"
    );

    let current_entry = observed
        .get("grandchild_current")
        .expect("grandchild_current span missing");
    assert_eq!(
        current_entry.0,
        Some(SpanStatus::failed(CURRENT_FAILURE)),
        "record_current_span_status_from_attrs should set status based on current span"
    );
    assert_eq!(
        current_entry.1.note,
        Some(CURRENT_NOTE.to_string()),
        "record_current_span_status_from_attrs should persist attribute changes"
    );
    assert_eq!(
        current_entry.1.status,
        Some(SpanStatus::failed(CURRENT_FAILURE)),
        "attributes should retain status after current-span update"
    );
}
