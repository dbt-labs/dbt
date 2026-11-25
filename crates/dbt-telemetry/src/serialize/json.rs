use dbt_tracing::{
    IndexedTelemetryDeserializeError, TelemetryAttributes, TelemetryOutputFlags, TelemetryRecord,
    serialize::json::{deserialize_from_json_value, deserialize_json_lines},
};
use serde_json::{Value as JsonValue, json};

use crate::{
    GenericOpExecuted, NodeEvaluated, NodeOutcome, TelemetryEventTypeRegistry,
    test_utils::{create_all_fake_attributes, create_all_fake_records, create_test_log_record},
};

#[test]
fn json_roundtrip_all_public_record_types() {
    let registry = TelemetryEventTypeRegistry::public();
    let records = public_json_records("json_roundtrip_seed");

    assert!(!records.is_empty(), "expected JSON-exported public records");
    assert!(
        records
            .iter()
            .any(|record| matches!(record, TelemetryRecord::SpanStart(_))),
        "expected at least one public span start record"
    );
    assert!(
        records
            .iter()
            .any(|record| matches!(record, TelemetryRecord::SpanEnd(_))),
        "expected at least one public span end record"
    );
    assert!(
        records
            .iter()
            .any(|record| matches!(record, TelemetryRecord::LogRecord(_))),
        "expected at least one public log record"
    );

    let json_lines = records
        .iter()
        .map(serialize_record)
        .collect::<Vec<_>>()
        .join("\n");

    let (from_lines, errors) = deserialize_json_lines(&json_lines, registry);
    assert_no_deserialize_errors(&errors);
    assert_eq!(from_lines, records);
}

#[test]
fn json_deserialization_accepts_public_registry_variations() {
    let registry = TelemetryEventTypeRegistry::public();
    let mut record = public_log_record("json_variation_seed");
    let TelemetryRecord::LogRecord(log_record) = &mut record else {
        panic!("expected public log record");
    };
    log_record.span_id = None;
    log_record.span_name = None;
    let base_value = serialize_record_value(&record);

    let variations = vec![
        (
            "extra_top_level_field",
            value_with_extra_top_level_field(&base_value),
        ),
        (
            "extra_attribute_field",
            value_with_extra_attribute_field(&base_value),
        ),
        (
            "omitted_optional_envelope_fields",
            value_without_optional_envelope_fields(&base_value),
        ),
    ];

    for (name, value) in variations {
        let deserialized = deserialize_from_json_value(value, registry)
            .unwrap_or_else(|err| panic!("expected success for {name}: {err}"));
        assert_eq!(deserialized, record, "variation {name} mismatch");
    }
}

#[test]
fn json_deserialization_rejects_required_public_envelope_fields() {
    let registry = TelemetryEventTypeRegistry::public();
    let record = public_log_record("json_required_field_seed");
    let base_value = serialize_record_value(&record);

    for field in ["event_type", "attributes"] {
        let mut value = base_value.clone();
        value.as_object_mut().unwrap().remove(field);

        let err = deserialize_from_json_value(value, registry).unwrap_err();
        assert!(
            err.to_string()
                .contains(&format!("missing field `{field}`")),
            "expected missing {field} error, got: {err}"
        );
    }
}

#[test]
fn json_deserialization_rejects_invalid_public_ids() {
    let registry = TelemetryEventTypeRegistry::public();
    let record = public_log_record("json_invalid_id_seed");

    let mut invalid_trace_id = serialize_record_value(&record);
    invalid_trace_id["trace_id"] = json!("not-hex");
    let err = deserialize_from_json_value(invalid_trace_id, registry).unwrap_err();
    assert!(err.to_string().contains("invalid trace_id"));

    let mut invalid_span_id = serialize_record_value(&record);
    invalid_span_id["span_id"] = json!("not-hex");
    let err = deserialize_from_json_value(invalid_span_id, registry).unwrap_err();
    assert!(err.to_string().contains("invalid span_id"));
}

#[test]
fn severity_number_json_matches_pbjson_shapes() {
    use crate::proto::v1::public::events::fusion::compat::SeverityNumber;

    assert_eq!(
        serde_json::to_value(SeverityNumber::Warn).unwrap(),
        json!(SeverityNumber::Warn as i32)
    );
    assert_eq!(
        serde_json::from_value::<SeverityNumber>(json!("SEVERITY_NUMBER_WARN")).unwrap(),
        SeverityNumber::Warn
    );
    assert_eq!(
        serde_json::from_value::<SeverityNumber>(json!(123_456)).unwrap(),
        SeverityNumber::Unspecified
    );
}

fn public_json_records(seed: &str) -> Vec<TelemetryRecord> {
    create_all_fake_records(
        seed,
        TelemetryEventTypeRegistry::public(),
        TelemetryOutputFlags::empty(),
    )
}

fn public_log_record(seed: &str) -> TelemetryRecord {
    let attributes = create_all_fake_attributes(
        seed,
        TelemetryEventTypeRegistry::public(),
        TelemetryOutputFlags::empty(),
    )
    .into_iter()
    .find(|attrs| {
        matches!(
            attrs.record_category(),
            dbt_tracing::TelemetryEventRecType::Log
        )
    })
    .expect("expected at least one public JSON log attribute");

    create_test_log_record(seed, attributes)
}

fn serialize_record(record: &TelemetryRecord) -> String {
    serde_json::to_string(&record.as_ref())
        .unwrap_or_else(|err| panic!("failed to serialize {:?}: {err}", record.attributes()))
}

fn serialize_record_value(record: &TelemetryRecord) -> JsonValue {
    serde_json::to_value(record.as_ref())
        .unwrap_or_else(|err| panic!("failed to serialize {:?}: {err}", record.attributes()))
}

fn assert_no_deserialize_errors(errors: &[IndexedTelemetryDeserializeError]) {
    assert!(
        errors.is_empty(),
        "expected all JSON lines to deserialize, got errors:\n{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn value_with_extra_top_level_field(value: &JsonValue) -> JsonValue {
    let mut value = value.clone();
    value["__test_extra_top_level"] = json!(true);
    value
}

fn value_with_extra_attribute_field(value: &JsonValue) -> JsonValue {
    let mut value = value.clone();
    value["attributes"]["__test_extra_attribute"] = json!("ignored");
    value
}

fn value_without_optional_envelope_fields(value: &JsonValue) -> JsonValue {
    let mut value = value.clone();
    let object = value.as_object_mut().unwrap();
    object.remove("span_id");
    object.remove("span_name");
    value
}

fn has_string(json: &JsonValue) -> bool {
    match json {
        JsonValue::String(_) => true,
        JsonValue::Array(items) => items.iter().any(has_string),
        JsonValue::Object(fields) => fields.values().any(has_string),
        JsonValue::Null | JsonValue::Bool(_) | JsonValue::Number(_) => false,
    }
}

/// `rewrite_strings` goes through an event's JSON form. Every public event must come back
/// from that trip unchanged, or its strings could not be rewritten.
#[test]
fn rewrite_strings_rebuilds_every_public_event_unchanged() {
    let all_events = create_all_fake_attributes(
        "rewrite_strings_seed",
        TelemetryEventTypeRegistry::public(),
        TelemetryOutputFlags::empty(),
    );
    assert!(!all_events.is_empty(), "expected public events");
    for original in all_events {
        let json = original.inner().to_json().expect("the event serializes");
        let mut rewritten = original.clone();
        let changed = rewritten.rewrite_strings(&|text| Some(text.to_string()));
        assert_eq!(rewritten, original, "{}", original.event_type());
        // An event that can't be rebuilt is left as it is, so it also compares equal. Only
        // the return value tells the two apart.
        assert!(
            changed || !has_string(&json),
            "{} was not rebuilt",
            original.event_type()
        );
    }
}

fn replace_text(text: &str, from: &str) -> Option<String> {
    text.contains(from).then(|| text.replace(from, "*****"))
}

/// An enum is written to JSON by name. Rewriting that name would make the event
/// unparsable, so it is kept and the other strings are still rewritten.
#[test]
fn rewrite_strings_keeps_a_value_that_would_not_parse() {
    let mut event = TelemetryAttributes::from(NodeEvaluated {
        unique_id: "model.pkg.SUCCESS_rate".to_string(),
        node_outcome: NodeOutcome::Success as i32,
        ..Default::default()
    });
    let json = event.inner().to_json().expect("the event serializes");
    assert!(json.to_string().contains("NODE_OUTCOME_SUCCESS"), "{json}");

    assert!(event.rewrite_strings(&|text| replace_text(text, "SUCCESS")));

    let event = event
        .downcast_ref::<NodeEvaluated>()
        .expect("the event keeps its type");
    assert_eq!(event.unique_id, "model.pkg.*****_rate");
    assert_eq!(event.node_outcome(), NodeOutcome::Success);
}

/// JSON escapes quotes and backslashes, so the text to rewrite does not appear as it is in
/// the event's JSON text.
#[test]
fn rewrite_strings_finds_text_that_json_escapes() {
    let secret = r#"p"ss\word"#;
    let mut event = TelemetryAttributes::from(GenericOpExecuted {
        operation_id: "login".to_string(),
        display_action: format!("Logging in with {secret}"),
        item_count_total: None,
    });

    assert!(event.rewrite_strings(&|text| replace_text(text, secret)));

    let event = event
        .downcast_ref::<GenericOpExecuted>()
        .expect("the event keeps its type");
    assert_eq!(event.display_action, "Logging in with *****");
}

#[test]
fn rewrite_strings_reports_no_change_without_a_match() {
    let original = TelemetryAttributes::from(GenericOpExecuted {
        operation_id: "login".to_string(),
        display_action: "Logging in".to_string(),
        item_count_total: None,
    });
    let mut event = original.clone();
    assert!(!event.rewrite_strings(&|text| replace_text(text, "s3cr3t")));
    assert_eq!(event, original);
}
