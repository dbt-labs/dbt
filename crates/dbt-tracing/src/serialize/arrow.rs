//! Arrow serialization support for telemetry records using serde_arrow.

use crate::{
    IndexedTelemetryDeserializeError, LogRecordInfo, PartialLogRecordInfo, PartialSpanEndInfo,
    PartialSpanStartInfo, PartialTelemetryRecord, SeverityNumber, SpanEndInfo, SpanLinkInfo,
    SpanStartInfo, SpanStatus, StatusCode, TelemetryAttributes, TelemetryDeserializeError,
    TelemetryOutputFlags, TelemetryRecord, TelemetryRecordType,
    serialize::{
        envelope::to_nanos,
        traits::{ArrowAttributesSerialize, ArrowRegistryLookup},
    },
};
use arrow::{
    array::{Array, ArrayRef, ListArray, StructArray, new_null_array},
    compute::{CastOptions, cast_with_options},
    datatypes::{DataType, Field, FieldRef, Fields, Schema, TimeUnit},
    record_batch::RecordBatch,
    util::display::FormatOptions,
};
use arrow_schema::extension::Json as JsonExtensionType;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
// no serde_arrow schema tracing; we build schema manually
use std::{borrow::Cow, sync::Arc};
use std::{str::FromStr, time::SystemTime};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArrowSpanLink {
    /// Arrow doesn't support u128 natively, so this is stored as a hex string.
    pub trace_id: String,
    pub span_id: u64,
    /// JSON serialized attributes for the link.
    pub json_payload: String,
}

impl<'a> TryFrom<&'a SpanLinkInfo> for ArrowSpanLink {
    type Error = String;

    fn try_from(link: &'a SpanLinkInfo) -> Result<Self, Self::Error> {
        Ok(ArrowSpanLink {
            trace_id: format!("{:032x}", link.trace_id),
            span_id: link.span_id,
            json_payload: serde_json::to_string(&link.attributes)
                .map_err(|e| format!("Failed to serialize SpanLink attributes to JSON: {e}"))?,
        })
    }
}

/// A special type used to derive the schema for telemetry records (envelope) in arrow
/// serialization, as well as a intermediate representation for serialization and deserialization.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ArrowTelemetryRecord<'a, A> {
    pub record_type: TelemetryRecordType,
    /// Arrow doesn't support u128 natively, so this is stored as a hex string.
    pub trace_id: String,
    pub span_id: Option<u64>,
    pub event_id: Option<Cow<'a, str>>,
    pub span_name: Option<Cow<'a, str>>,
    pub parent_span_id: Option<u64>,
    pub links: Option<Vec<ArrowSpanLink>>,
    pub start_time_unix_nano: Option<u64>,
    pub end_time_unix_nano: Option<u64>,
    pub time_unix_nano: Option<u64>,
    pub severity_number: i32,
    pub severity_text: Cow<'a, str>,
    pub body: Option<Cow<'a, str>>,
    pub status_code: Option<u32>,
    pub status_message: Option<Cow<'a, str>>,
    pub event_type: Cow<'a, str>,
    pub attributes: A,
}

#[inline]
fn nanos_to_system_time(nanos: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(nanos)
}

impl<'a> TryFrom<&'a TelemetryRecord>
    for ArrowTelemetryRecord<'a, Box<dyn ArrowAttributesSerialize + 'a>>
{
    type Error = String;

    fn try_from(value: &'a TelemetryRecord) -> Result<Self, Self::Error> {
        let event_type = value.attributes().event_type();

        let attributes =
            value.attributes().inner().to_arrow().ok_or_else(|| {
                format!("Missing arrow serializer for event type \"{event_type}\"")
            })?;

        let arrow_record = match value {
            TelemetryRecord::SpanStart(span) => ArrowTelemetryRecord {
                record_type: value.into(),
                trace_id: format!("{:032x}", span.trace_id),
                span_id: Some(span.span_id),
                event_id: None,
                span_name: Some(Cow::Borrowed(span.span_name.as_str())),
                parent_span_id: span.parent_span_id,
                links: span
                    .links
                    .as_deref()
                    .map(|links| {
                        links
                            .iter()
                            .map(ArrowSpanLink::try_from)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?,
                start_time_unix_nano: Some(to_nanos(&span.start_time_unix_nano)),
                end_time_unix_nano: None,
                time_unix_nano: None,
                severity_number: span.severity_number as i32,
                severity_text: Cow::Borrowed(span.severity_text.as_ref()),
                body: None,
                status_code: None,
                status_message: None,
                event_type: event_type.into(),
                attributes,
            },
            TelemetryRecord::SpanEnd(span) => ArrowTelemetryRecord {
                record_type: value.into(),
                trace_id: format!("{:032x}", span.trace_id),
                span_id: Some(span.span_id),
                event_id: None,
                span_name: Some(Cow::Borrowed(span.span_name.as_str())),
                parent_span_id: span.parent_span_id,
                links: span
                    .links
                    .as_deref()
                    .map(|links| {
                        links
                            .iter()
                            .map(ArrowSpanLink::try_from)
                            .collect::<Result<Vec<_>, _>>()
                    })
                    .transpose()?,
                start_time_unix_nano: Some(to_nanos(&span.start_time_unix_nano)),
                end_time_unix_nano: Some(to_nanos(&span.end_time_unix_nano)),
                time_unix_nano: None,
                severity_number: span.severity_number as i32,
                severity_text: Cow::Borrowed(span.severity_text.as_ref()),
                body: None,
                status_code: span.status.as_ref().map(|s| s.code as u32),
                status_message: span
                    .status
                    .as_ref()
                    .and_then(|s| s.message.as_deref().map(Cow::Borrowed)),
                event_type: event_type.into(),
                attributes,
            },
            TelemetryRecord::LogRecord(log) => ArrowTelemetryRecord {
                record_type: value.into(),
                trace_id: format!("{:032x}", log.trace_id),
                span_id: log.span_id,
                event_id: Some(log.event_id.to_string().into()),
                span_name: log.span_name.as_deref().map(Cow::Borrowed),
                parent_span_id: None,
                links: None,
                start_time_unix_nano: None,
                end_time_unix_nano: None,
                time_unix_nano: Some(to_nanos(&log.time_unix_nano)),
                severity_number: log.severity_number as i32,
                severity_text: Cow::Borrowed(log.severity_text.as_ref()),
                body: Some(Cow::Borrowed(log.body.as_str())),
                status_code: None,
                status_message: None,
                event_type: event_type.into(),
                attributes,
            },
        };

        Ok(arrow_record)
    }
}

fn deserialize_record_from_arrow<R>(
    arrow: ArrowTelemetryRecord<'_, R::ArrowAttributes<'_>>,
    registry: &R,
) -> Result<TelemetryRecord, TelemetryDeserializeError>
where
    R: ArrowRegistryLookup,
{
    let ArrowTelemetryRecord {
        record_type,
        trace_id,
        span_id,
        event_id,
        span_name,
        parent_span_id,
        links,
        start_time_unix_nano,
        end_time_unix_nano,
        time_unix_nano,
        severity_number,
        severity_text,
        body,
        status_code,
        status_message,
        event_type,
        attributes,
    } = arrow;

    let trace_id = u128::from_str_radix(&trace_id, 16).map_err(|e| {
        TelemetryDeserializeError::invalid_envelope(format!("Invalid trace_id: {e}"))
    })?;
    let event_type = event_type.into_owned();

    let links = if let Some(arrow_links) = links {
        let mut span_links = Vec::with_capacity(arrow_links.len());
        for link in arrow_links {
            let trace_id = u128::from_str_radix(&link.trace_id, 16).map_err(|e| {
                TelemetryDeserializeError::invalid_envelope(format!(
                    "Invalid trace_id in SpanLink: {e}"
                ))
            })?;
            span_links.push(SpanLinkInfo {
                trace_id,
                span_id: link.span_id,
                attributes: serde_json::from_str(&link.json_payload).map_err(|e| {
                    TelemetryDeserializeError::invalid_envelope(format!(
                        "Failed to deserialize SpanLink attributes from JSON: {e}"
                    ))
                })?,
            });
        }
        Some(span_links)
    } else {
        None
    };

    match record_type {
        TelemetryRecordType::SpanStart => {
            let span_id = span_id.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope("Missing span_id for SpanStart record")
            })?;
            let span_name = span_name
                .ok_or_else(|| {
                    TelemetryDeserializeError::invalid_envelope(
                        "Missing span_name for SpanStart record",
                    )
                })?
                .into_owned();
            let start_time_unix_nano = start_time_unix_nano.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope(
                    "Missing start_time_unix_nano for SpanStart record",
                )
            })?;
            let severity_text = severity_text.into_owned();

            let severity_number = SeverityNumber::try_from(severity_number).map_err(|_| {
                TelemetryDeserializeError::invalid_envelope("Invalid severity_number")
            })?;
            let start_time_unix_nano = nanos_to_system_time(start_time_unix_nano);

            let deserialized_attributes =
                match registry.deserialize_arrow_attributes(&event_type, &attributes) {
                    Ok(attributes) => TelemetryAttributes::new(attributes),
                    Err(err) => {
                        let record = PartialTelemetryRecord::SpanStart(PartialSpanStartInfo {
                            trace_id,
                            span_id,
                            span_name,
                            parent_span_id,
                            links,
                            start_time_unix_nano,
                            severity_number,
                            severity_text,
                            event_type,
                            attributes: arrow_attributes_json(&attributes)?,
                        });
                        return Err(TelemetryDeserializeError::from_attribute_error(record, err));
                    }
                };

            Ok(TelemetryRecord::SpanStart(SpanStartInfo {
                trace_id,
                span_id,
                span_name,
                parent_span_id,
                links,
                start_time_unix_nano,
                severity_number,
                severity_text,
                attributes: deserialized_attributes,
            }))
        }
        TelemetryRecordType::SpanEnd => {
            let span_id = span_id.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope("Missing span_id for SpanEnd record")
            })?;
            let span_name = span_name
                .ok_or_else(|| {
                    TelemetryDeserializeError::invalid_envelope(
                        "Missing span_name for SpanEnd record",
                    )
                })?
                .into_owned();
            let start_time_unix_nano = start_time_unix_nano.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope(
                    "Missing start_time_unix_nano for SpanEnd record",
                )
            })?;
            let end_time_unix_nano = end_time_unix_nano.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope(
                    "Missing end_time_unix_nano for SpanEnd record",
                )
            })?;
            let severity_text = severity_text.into_owned();

            let status = if status_code.is_some() || status_message.is_some() {
                Some(SpanStatus {
                    code: StatusCode::from_repr(status_code.unwrap_or(0) as u8)
                        .unwrap_or(StatusCode::Unset),
                    message: status_message.map(Cow::into_owned),
                })
            } else {
                None
            };
            let severity_number = SeverityNumber::try_from(severity_number).map_err(|_| {
                TelemetryDeserializeError::invalid_envelope("Invalid severity_number")
            })?;
            let start_time_unix_nano = nanos_to_system_time(start_time_unix_nano);
            let end_time_unix_nano = nanos_to_system_time(end_time_unix_nano);

            let deserialized_attributes =
                match registry.deserialize_arrow_attributes(&event_type, &attributes) {
                    Ok(attributes) => TelemetryAttributes::new(attributes),
                    Err(err) => {
                        let record = PartialTelemetryRecord::SpanEnd(PartialSpanEndInfo {
                            trace_id,
                            span_id,
                            span_name,
                            parent_span_id,
                            links,
                            start_time_unix_nano,
                            end_time_unix_nano,
                            status,
                            severity_number,
                            severity_text,
                            event_type,
                            attributes: arrow_attributes_json(&attributes)?,
                        });
                        return Err(TelemetryDeserializeError::from_attribute_error(record, err));
                    }
                };

            Ok(TelemetryRecord::SpanEnd(SpanEndInfo {
                trace_id,
                span_id,
                span_name,
                parent_span_id,
                links,
                start_time_unix_nano,
                end_time_unix_nano,
                status,
                severity_number,
                severity_text,
                attributes: deserialized_attributes,
            }))
        }
        TelemetryRecordType::LogRecord => {
            let time_unix_nano = time_unix_nano.ok_or_else(|| {
                TelemetryDeserializeError::invalid_envelope("Missing time_unix_nano for LogRecord")
            })?;
            let body = body
                .ok_or_else(|| {
                    TelemetryDeserializeError::invalid_envelope("Missing body for LogRecord")
                })?
                .into_owned();
            let severity_text = severity_text.into_owned();

            let event_id = uuid::Uuid::from_str(
                event_id
                    .ok_or_else(|| TelemetryDeserializeError::invalid_envelope("Missing event_id"))?
                    .as_ref(),
            )
            .map_err(|e| {
                TelemetryDeserializeError::invalid_envelope(format!(
                    "Failed to deserialize `event_id`: {e}"
                ))
            })?;
            let span_name = span_name.map(|name| name.into_owned());
            let severity_number = SeverityNumber::try_from(severity_number).map_err(|_| {
                TelemetryDeserializeError::invalid_envelope("Invalid severity_number")
            })?;
            let time_unix_nano = nanos_to_system_time(time_unix_nano);

            let deserialized_attributes =
                match registry.deserialize_arrow_attributes(&event_type, &attributes) {
                    Ok(attributes) => TelemetryAttributes::new(attributes),
                    Err(err) => {
                        let record = PartialTelemetryRecord::LogRecord(PartialLogRecordInfo {
                            time_unix_nano,
                            trace_id,
                            span_id,
                            event_id,
                            span_name,
                            severity_number,
                            severity_text,
                            body,
                            event_type,
                            attributes: arrow_attributes_json(&attributes)?,
                        });
                        return Err(TelemetryDeserializeError::from_attribute_error(record, err));
                    }
                };

            Ok(TelemetryRecord::LogRecord(LogRecordInfo {
                time_unix_nano,
                trace_id,
                span_id,
                event_id,
                span_name,
                severity_number,
                severity_text,
                body,
                attributes: deserialized_attributes,
            }))
        }
    }
}

fn arrow_attributes_json<A>(attributes: &A) -> Result<JsonValue, TelemetryDeserializeError>
where
    A: Serialize,
{
    serde_json::to_value(attributes).map_err(|e| {
        TelemetryDeserializeError::invalid_envelope(format!(
            "Failed to convert Arrow attributes to JSON: {e}"
        ))
    })
}

fn large_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::LargeUtf8, nullable)
}

fn dict_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(
        name,
        DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8)),
        nullable,
    )
}

fn json_large_utf8_field(name: &str, nullable: bool) -> Field {
    Field::new(name, DataType::LargeUtf8, nullable)
        .with_extension_type(JsonExtensionType::default())
}

/// Creates an Arrow schema for telemetry records.
///
/// This generates the Arrow schema definition that can be used to serialize
/// telemetry records to Parquet or other Arrow-compatible formats.
///
/// It returns two schemas:
/// 1. `serialisable_schema`: Used to convert Vec<Struct> -> RecordBatch with timestamp fields as `u64`.
///    This is a current limitation of the `serde_arrow` library, which doesn't support serializing
///    `SystemTime` or `Timestamp` types directly. These RecordBatches are never returned or stored,
///    they are only an intermediate step in the serialization process.
/// 2. `schema_with_timestamps`: The final schema with timestamp fields converted to `Timestamp(NANOSECOND)`.
///
/// Callers that need to reuse schemas should cache the returned fields.
///
/// # Returns
///
/// Returns two vectors of Arrow field references that define the schema structure,
/// or an error if schema generation fails.
fn create_arrow_schema<R>() -> (Vec<FieldRef>, Vec<FieldRef>)
where
    R: ArrowRegistryLookup,
{
    // ArrowSpanLink struct fields
    let span_link_fields = Fields::from(vec![
        dict_utf8_field("trace_id", false),
        Field::new("span_id", DataType::UInt64, false),
        json_large_utf8_field("json_payload", true),
    ]);

    let attributes_fields = R::arrow_attributes_fields();

    // Top-level fields for ArrowTelemetryRecord
    let serialisable_schema: Vec<FieldRef> = vec![
        dict_utf8_field("record_type", false).into(),
        dict_utf8_field("trace_id", false).into(),
        Arc::new(Field::new("span_id", DataType::UInt64, true)),
        large_utf8_field("event_id", true).into(),
        large_utf8_field("span_name", true).into(),
        Arc::new(Field::new("parent_span_id", DataType::UInt64, true)),
        Arc::new(Field::new(
            "links",
            DataType::List(Arc::new(Field::new(
                "item",
                DataType::Struct(span_link_fields),
                false,
            ))),
            true,
        )),
        Arc::new(Field::new("start_time_unix_nano", DataType::UInt64, true)),
        Arc::new(Field::new("end_time_unix_nano", DataType::UInt64, true)),
        Arc::new(Field::new("time_unix_nano", DataType::UInt64, true)),
        Arc::new(Field::new("severity_number", DataType::Int32, false)),
        dict_utf8_field("severity_text", false).into(),
        large_utf8_field("body", true).into(),
        Arc::new(Field::new("status_code", DataType::UInt32, true)),
        large_utf8_field("status_message", true).into(),
        dict_utf8_field("event_type", false).into(),
        Arc::new(Field::new(
            "attributes",
            DataType::Struct(attributes_fields),
            false,
        )),
    ];

    // Convert timestamp columns from u64 to Timestamp(NANOSECOND)
    let schema_with_timestamps: Vec<FieldRef> = serialisable_schema
        .iter()
        .map(|f| {
            if f.name() == "start_time_unix_nano"
                || f.name() == "end_time_unix_nano"
                || f.name() == "time_unix_nano"
            {
                Arc::new(Field::new(
                    f.name(),
                    DataType::Timestamp(TimeUnit::Nanosecond, None),
                    true,
                ))
            } else {
                f.clone()
            }
        })
        .collect();

    (serialisable_schema, schema_with_timestamps)
}

#[derive(Clone, Debug)]
pub struct TelemetryArrowSchemas {
    serialisable_schema: Vec<FieldRef>,
    schema_with_timestamps: Vec<FieldRef>,
}

impl TelemetryArrowSchemas {
    pub fn new<R>() -> Self
    where
        R: ArrowRegistryLookup,
    {
        let (serialisable_schema, schema_with_timestamps) = create_arrow_schema::<R>();

        Self {
            serialisable_schema,
            schema_with_timestamps,
        }
    }

    pub fn serialisable_schema(&self) -> &[FieldRef] {
        &self.serialisable_schema
    }

    pub fn schema_with_timestamps(&self) -> &[FieldRef] {
        &self.schema_with_timestamps
    }
}

/// Serializes telemetry records to an Arrow RecordBatch.
///
/// Converts a slice of telemetry records into an Arrow RecordBatch that can be
/// written to Parquet files or other Arrow-compatible storage formats.
///
/// Top-level envelope datetime fields are converted to Timestamp(NANOSECOND) type.
///
/// # Arguments
///
/// * `records` - Slice of telemetry records to serialize
/// * `schemas` - Cached Arrow schemas for the registry used by the caller
///
/// # Returns
///
/// Returns an Arrow RecordBatch containing the serialized records, or an error
/// if serialization fails.
///
/// # Examples
///
/// ```rust
/// use dbt_tracing::serialize::arrow::{serialize_to_arrow, TelemetryArrowSchemas};
/// use dbt_tracing::TelemetryRecord;
///
/// let records: Vec<TelemetryRecord> = vec![/* ... */];
/// let schemas = TelemetryArrowSchemas::new::<MyRegistry>();
/// let batch = serialize_to_arrow(&records, &schemas).expect("Failed to serialize");
/// ```
pub fn serialize_to_arrow(
    records: &[TelemetryRecord],
    schemas: &TelemetryArrowSchemas,
) -> Result<RecordBatch, Box<dyn std::error::Error>> {
    let mut errors: Vec<String> = Vec::new();

    let arrow_records: Vec<ArrowTelemetryRecord<'_, Box<dyn ArrowAttributesSerialize + '_>>> =
        records
            .iter()
            .filter(|r| {
                // Only include records with serializable attributes
                r.attributes()
                    .output_flags()
                    .contains(TelemetryOutputFlags::EXPORT_PARQUET)
            })
            .filter_map(|r| {
                ArrowTelemetryRecord::try_from(r)
                    .map_err(|e| errors.push(e))
                    .ok()
            })
            .collect();

    if !errors.is_empty() {
        // As of today, this should never happen because we filter out records with non-serializable attributes
        // above via export flags and this is the only realistic error case.
        return Err(Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Failed to serialize some records: {}", errors.join("; ")),
        )));
    }

    // Serialize with the temporary schema (timestamp fields as u64),
    // see `create_arrow_schema` for details.
    let batch = serde_arrow::to_record_batch(schemas.serialisable_schema(), &arrow_records)?;

    let mut columns = batch.columns().to_vec();

    // Convert timestamp columns from u64 to Timestamp(NANOSECOND),
    // this is zero-copy, just metadata change.
    let schema_with_timestamps = schemas.schema_with_timestamps();
    for (i, field) in schema_with_timestamps.iter().enumerate() {
        if let DataType::Timestamp(TimeUnit::Nanosecond, None) = field.data_type()
            && let Some(column) = columns.get(i)
        {
            columns[i] = cast_with_options(
                column,
                &DataType::Timestamp(TimeUnit::Nanosecond, None),
                &CastOptions {
                    safe: false,
                    format_options: FormatOptions::new().with_display_error(false),
                },
            )?
        }
    }

    Ok(RecordBatch::try_new(
        Schema::new(schema_with_timestamps.to_vec()).into(),
        columns,
    )?)
}

/// Deserializes telemetry records from an Arrow RecordBatch.
///
/// Converts an Arrow RecordBatch (typically read from a Parquet file) back into
/// telemetry records.
///
/// # Arguments
///
/// * `batch` - Arrow RecordBatch to deserialize from
/// * `registry` - Registry of telemetry event types for deserialization
///
/// # Returns
///
/// Returns successfully deserialized telemetry records and per-row errors with
/// one-based Arrow row indexes.
///
/// # Errors
///
/// This function returns an outer error when the RecordBatch cannot be decoded
/// as telemetry records, including incompatible schemas, schema normalization
/// failures, or Arrow/serde decoding failures.
///
/// # Per-row errors
///
/// The second tuple element contains row-level errors for records that could be
/// projected from the batch but could not become typed telemetry records. Each
/// error includes the one-based Arrow row index:
/// - Required envelope fields are missing, such as span IDs, timestamps, event
///   IDs, or log bodies.
/// - Envelope field values are invalid, such as malformed trace IDs, span link
///   payloads, UUIDs, or severity numbers.
/// - The event type is unknown to the supplied registry.
/// - The event type is known but its attributes are malformed.
///
/// # Examples
///
/// ```rust
/// use dbt_tracing::serialize::arrow::{deserialize_from_arrow, TelemetryArrowSchemas};
/// use arrow::record_batch::RecordBatch;
///
/// let batch: RecordBatch = /* read from file */;
/// let (records, errors) = deserialize_from_arrow(&batch, MyRegistry).expect("Failed to deserialize");
/// ```
pub fn deserialize_from_arrow<R>(
    batch: &RecordBatch,
    registry: &R,
) -> Result<(Vec<TelemetryRecord>, Vec<IndexedTelemetryDeserializeError>), Box<dyn std::error::Error>>
where
    R: ArrowRegistryLookup,
{
    let schemas = TelemetryArrowSchemas::new::<R>();
    let temp_batch = normalize_batch(batch, schemas.serialisable_schema())?;

    let arrow_records: Vec<ArrowTelemetryRecord<'_, R::ArrowAttributes<'_>>> =
        serde_arrow::from_record_batch(&temp_batch)
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

    let mut records = Vec::with_capacity(arrow_records.len());
    let mut errors = Vec::new();

    for (idx, record) in arrow_records.into_iter().enumerate() {
        match deserialize_record_from_arrow(record, registry) {
            Ok(record) => records.push(record),
            Err(err) => errors.push(IndexedTelemetryDeserializeError::new(idx + 1, err)),
        }
    }

    Ok((records, errors))
}

/// Normalizes incoming `RecordBatch` to make it compatible with the
/// serde_arrow schema used by telemetry deserialization.
///
/// * Columns are matched by name.
/// * Missing required columns and incompatible type conversions are accumulated
///   and reported together.
/// * Missing nullable columns are filled with nulls.
/// * Extra columns present in the input batch are ignored.
/// * String-like columns (plain or dictionary) are accepted without casting
///   so long as both sides are string compatible.
/// * other mismatches are cast to the expected type when possible.
fn normalize_batch(
    batch: &RecordBatch,
    serialisable_schema: &[FieldRef],
) -> Result<RecordBatch, Box<dyn std::error::Error>> {
    let batch_schema = batch.schema();

    let mut missing_columns = Vec::new();
    let mut type_errors = Vec::new();
    let mut normalized_columns = Vec::with_capacity(serialisable_schema.len());

    for expected_field in serialisable_schema.iter() {
        let expected_field = expected_field.as_ref();
        let Some((index, actual_field)) = batch_schema.column_with_name(expected_field.name())
        else {
            if expected_field.is_nullable() {
                let array = new_null_array(expected_field.data_type(), batch.num_rows());
                normalized_columns.push(Some(NormalizedColumn {
                    field: Arc::new(expected_field.clone()),
                    array,
                    metadata_changed: true,
                }));
            } else {
                missing_columns.push(expected_field.name().to_string());
                normalized_columns.push(None);
            }
            continue;
        };

        let column = batch.column(index);
        match normalize_column(expected_field.name(), column, expected_field, actual_field) {
            Ok(normalized) => normalized_columns.push(Some(normalized)),
            Err(mut errors) => {
                type_errors.append(&mut errors);
                normalized_columns.push(None);
            }
        }
    }

    if !missing_columns.is_empty() || !type_errors.is_empty() {
        let mut parts = Vec::new();
        if !missing_columns.is_empty() {
            parts.push(format!("missing columns: {}", missing_columns.join(", ")));
        }
        if !type_errors.is_empty() {
            parts.push(format!("incompatible columns: {}", type_errors.join("; ")));
        }
        return Err(Box::new(arrow::error::ArrowError::SchemaError(
            parts.join("; "),
        )));
    }

    let normalized: Vec<NormalizedColumn> = normalized_columns
        .into_iter()
        .map(|opt| opt.expect("errors handled above"))
        .collect();

    let fields: Vec<FieldRef> = normalized.iter().map(|col| col.field.clone()).collect();
    let arrays: Vec<ArrayRef> = normalized.into_iter().map(|col| col.array).collect();

    Ok(RecordBatch::try_new(Schema::new(fields).into(), arrays)?)
}

struct NormalizedColumn {
    field: FieldRef,
    array: ArrayRef,
    metadata_changed: bool,
}

/// Validates and normalizes a single column (and any nested children) such that
/// it can be safely deserialized by serde_arrow into the expected type.
///
/// * String-like columns (`Utf8`, `LargeUtf8`, or dictionaries of those types)
///   are accepted without casting.
/// * Non-nullable fields in the batch are allowed in place of nullable expected
///   fields, but not vice versa.
/// * Struct and list columns are normalized recursively on their children.
/// * All other mismatches are cast to the expected type; failures return a
///   descriptive error message.
fn normalize_column(
    path: &str,
    array: &ArrayRef,
    expected_field: &Field,
    actual_field: &Field,
) -> Result<NormalizedColumn, Vec<String>> {
    let expected_type = expected_field.data_type();
    let actual_type = actual_field.data_type();

    if is_string_like(expected_type) && is_string_like(actual_type) {
        let (field, metadata_changed) =
            reconcile_field(expected_field, actual_field, actual_type.clone());
        return Ok(NormalizedColumn {
            field,
            array: array.clone(),
            metadata_changed,
        });
    }

    match expected_type {
        DataType::Struct(_) => normalize_struct_column(path, array, expected_field, actual_field),
        DataType::List(expected_child_field) => normalize_list_column(
            path,
            array,
            expected_field,
            actual_field,
            expected_child_field,
        ),
        _ => normalize_primitive_column(path, array, expected_field, actual_field),
    }
}

fn normalize_struct_column(
    path: &str,
    array: &ArrayRef,
    expected_field: &Field,
    actual_field: &Field,
) -> Result<NormalizedColumn, Vec<String>> {
    let struct_array = array
        .as_any()
        .downcast_ref::<StructArray>()
        .ok_or_else(|| {
            vec![format!(
                "field {path}: expected Struct but found {:?}",
                array.data_type()
            )]
        })?;

    let DataType::Struct(expected_fields) = expected_field.data_type() else {
        unreachable!("expected_field should be Struct");
    };

    let DataType::Struct(actual_fields) = actual_field.data_type() else {
        return Err(vec![format!(
            "field {path}: expected Struct but found {:?}",
            actual_field.data_type()
        )]);
    };

    let mut child_arrays = Vec::with_capacity(expected_fields.len());
    let mut child_fields = Vec::with_capacity(expected_fields.len());
    let mut errors = Vec::new();
    let mut needs_rebuild = false;

    for expected_child in expected_fields.iter() {
        let child_path = format!("{path}.{}", expected_child.name());
        let Some((child_index, actual_child_field)) = actual_fields
            .iter()
            .enumerate()
            .find(|(_, actual_child)| actual_child.name() == expected_child.name())
        else {
            if expected_child.is_nullable() {
                child_arrays.push(new_null_array(
                    expected_child.data_type(),
                    struct_array.len(),
                ));
                child_fields.push(expected_child.clone());
                needs_rebuild = true;
            } else {
                errors.push(format!("field {child_path}: missing required field"));
            }
            continue;
        };

        let child_array = struct_array.column(child_index);
        match normalize_column(
            &child_path,
            child_array,
            expected_child.as_ref(),
            actual_child_field.as_ref(),
        ) {
            Ok(child_column) => {
                if !Arc::ptr_eq(&child_column.array, child_array) || child_column.metadata_changed {
                    needs_rebuild = true;
                }
                child_arrays.push(child_column.array);
                child_fields.push(child_column.field);
            }
            Err(mut child_errors) => errors.append(&mut child_errors),
        }
    }

    if !errors.is_empty() {
        return Err(errors);
    }

    if child_arrays.len() != expected_fields.len() {
        return Err(vec![format!(
            "field {path}: unable to normalize because some children were missing"
        )]);
    }

    let child_fields_struct: Fields = child_fields.clone().into();
    let (new_field, parent_metadata_changed) = reconcile_field(
        expected_field,
        actual_field,
        DataType::Struct(child_fields_struct.clone()),
    );

    let array: ArrayRef = if needs_rebuild {
        Arc::new(StructArray::new(
            child_fields_struct,
            child_arrays,
            struct_array.logical_nulls(),
        ))
    } else {
        array.clone()
    };

    Ok(NormalizedColumn {
        field: new_field,
        array,
        metadata_changed: parent_metadata_changed || needs_rebuild,
    })
}

fn normalize_list_column(
    path: &str,
    array: &ArrayRef,
    expected_field: &Field,
    actual_field: &Field,
    expected_child_field: &FieldRef,
) -> Result<NormalizedColumn, Vec<String>> {
    let list_array = array.as_any().downcast_ref::<ListArray>().ok_or_else(|| {
        vec![format!(
            "field {path}: expected List but found {:?}",
            array.data_type()
        )]
    })?;

    let values = list_array.values();
    let actual_child_field = match actual_field.data_type() {
        DataType::List(field) => field.clone(),
        other => {
            return Err(vec![format!(
                "field {path}: expected List but found {:?}",
                other
            )]);
        }
    };
    let child_path = format!("{path}[]");
    let child_column = normalize_column(
        &child_path,
        values,
        expected_child_field.as_ref(),
        actual_child_field.as_ref(),
    )?;

    let needs_rebuild = child_column.metadata_changed || !Arc::ptr_eq(&child_column.array, values);

    let array: ArrayRef = if needs_rebuild {
        Arc::new(ListArray::new(
            child_column.field.clone(),
            list_array.offsets().clone(),
            child_column.array.clone(),
            list_array.nulls().cloned(),
        ))
    } else {
        array.clone()
    };

    let (new_field, parent_metadata_changed) = reconcile_field(
        expected_field,
        actual_field,
        DataType::List(child_column.field),
    );

    Ok(NormalizedColumn {
        field: new_field,
        array,
        metadata_changed: parent_metadata_changed || needs_rebuild,
    })
}

fn normalize_primitive_column(
    path: &str,
    array: &ArrayRef,
    expected_field: &Field,
    actual_field: &Field,
) -> Result<NormalizedColumn, Vec<String>> {
    let expected_type = expected_field.data_type();
    if array.data_type() == expected_type {
        let (field, metadata_changed) =
            reconcile_field(expected_field, actual_field, expected_type.clone());
        return Ok(NormalizedColumn {
            field,
            array: array.clone(),
            metadata_changed,
        });
    }

    match cast_array(array, expected_type) {
        Ok(casted) => {
            let (field, _) = reconcile_field(expected_field, actual_field, expected_type.clone());
            Ok(NormalizedColumn {
                field,
                array: casted,
                metadata_changed: true,
            })
        }
        Err(err) => Err(vec![format!(
            "field {path}: cannot cast from {:?} to {:?}: {err}",
            array.data_type(),
            expected_type
        )]),
    }
}

fn reconcile_field(
    expected_field: &Field,
    actual_field: &Field,
    data_type: DataType,
) -> (FieldRef, bool) {
    let updated = expected_field.clone().with_data_type(data_type);
    let metadata_changed = updated != *actual_field;
    (Arc::new(updated), metadata_changed)
}

fn is_string_like(data_type: &DataType) -> bool {
    match data_type {
        DataType::Utf8 | DataType::LargeUtf8 => true,
        DataType::Dictionary(_, value) => is_string_like(value.as_ref()),
        _ => false,
    }
}

fn cast_array(
    array: &ArrayRef,
    data_type: &DataType,
) -> Result<ArrayRef, Box<dyn std::error::Error>> {
    Ok(cast_with_options(
        array.as_ref(),
        data_type,
        &CastOptions {
            safe: false,
            format_options: FormatOptions::new().with_display_error(false),
        },
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AnyTelemetryEvent, LogRecordInfo, TelemetryAttributes, TelemetryEventRecType};
    use arrow::datatypes::Fields;
    use serde::{Deserialize, Serialize};
    use std::{any::Any, time::SystemTime};

    const TEST_EVENT_TYPE: &str = "v1.test.events.fusion.MockEvent";

    #[derive(Debug, Clone, PartialEq, Serialize)]
    struct MockEvent {
        value: String,
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
    struct MockArrowAttributes<'a> {
        value: Cow<'a, str>,
    }

    impl AnyTelemetryEvent for MockEvent {
        fn event_type(&self) -> &'static str {
            TEST_EVENT_TYPE
        }

        fn event_display_name(&self) -> String {
            "mock event".to_string()
        }

        fn record_category(&self) -> TelemetryEventRecType {
            TelemetryEventRecType::Log
        }

        fn output_flags(&self) -> TelemetryOutputFlags {
            TelemetryOutputFlags::EXPORT_PARQUET
        }

        fn event_eq(&self, other: &dyn AnyTelemetryEvent) -> bool {
            other.as_any().downcast_ref::<Self>() == Some(self)
        }

        fn has_sensitive_data(&self) -> bool {
            false
        }

        fn as_any(&self) -> &dyn Any {
            self
        }

        fn as_any_mut(&mut self) -> &mut dyn Any {
            self
        }

        fn clone_box(&self) -> Box<dyn AnyTelemetryEvent> {
            Box::new(self.clone())
        }

        fn to_json(&self) -> Result<serde_json::Value, String> {
            serde_json::to_value(self).map_err(|e| e.to_string())
        }

        fn to_arrow(&self) -> Option<Box<dyn ArrowAttributesSerialize + '_>> {
            Some(Box::new(MockArrowAttributes {
                value: Cow::Borrowed(self.value.as_str()),
            }))
        }
    }

    struct MockRegistry;

    impl ArrowRegistryLookup for MockRegistry {
        type ArrowAttributes<'a> = MockArrowAttributes<'a>;

        fn arrow_attributes_fields() -> Fields {
            Fields::from(vec![large_utf8_field("value", false)])
        }

        fn deserialize_arrow_attributes(
            &self,
            event_type: &str,
            attributes: &Self::ArrowAttributes<'_>,
        ) -> Result<
            Box<dyn AnyTelemetryEvent>,
            crate::serialize::traits::TelemetryAttributeDeserializeError,
        > {
            if event_type != TEST_EVENT_TYPE {
                return Err(
                    crate::serialize::traits::TelemetryAttributeDeserializeError::unknown_event_type(
                        event_type,
                    ),
                );
            }
            if attributes.value == "malformed" {
                return Err(
                    crate::serialize::traits::TelemetryAttributeDeserializeError::malformed(
                        "bad mock value",
                    ),
                );
            }

            Ok(Box::new(MockEvent {
                value: attributes.value.clone().into_owned(),
            }))
        }
    }

    fn mock_arrow_record(
        event_type: &'static str,
        value: &'static str,
        event_id: Option<uuid::Uuid>,
    ) -> ArrowTelemetryRecord<'static, MockArrowAttributes<'static>> {
        ArrowTelemetryRecord {
            record_type: TelemetryRecordType::LogRecord,
            trace_id: format!("{:032x}", 42),
            span_id: Some(7),
            event_id: event_id.map(|id| Cow::Owned(id.to_string())),
            span_name: Some(Cow::Borrowed("mock span")),
            parent_span_id: None,
            links: None,
            start_time_unix_nano: None,
            end_time_unix_nano: None,
            time_unix_nano: Some(12345),
            severity_number: SeverityNumber::Info as i32,
            severity_text: Cow::Borrowed("INFO"),
            body: Some(Cow::Borrowed("mock body")),
            status_code: None,
            status_message: None,
            event_type: Cow::Borrowed(event_type),
            attributes: MockArrowAttributes {
                value: Cow::Borrowed(value),
            },
        }
    }

    fn batch_from_arrow_records(
        records: Vec<ArrowTelemetryRecord<'static, MockArrowAttributes<'static>>>,
    ) -> RecordBatch {
        let schemas = TelemetryArrowSchemas::new::<MockRegistry>();
        serde_arrow::to_record_batch(schemas.serialisable_schema(), &records).unwrap()
    }

    fn batch_without_column(batch: &RecordBatch, column_name: &str) -> RecordBatch {
        let remove_index = batch.schema().index_of(column_name).unwrap();
        let fields = batch
            .schema()
            .fields()
            .iter()
            .enumerate()
            .filter_map(|(idx, field)| (idx != remove_index).then_some(field.clone()))
            .collect::<Vec<_>>();
        let columns = batch
            .columns()
            .iter()
            .enumerate()
            .filter_map(|(idx, column)| (idx != remove_index).then_some(column.clone()))
            .collect::<Vec<_>>();

        RecordBatch::try_new(Schema::new(fields).into(), columns).unwrap()
    }

    #[test]
    fn test_generic_arrow_roundtrip() {
        let record = TelemetryRecord::LogRecord(LogRecordInfo {
            time_unix_nano: SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(42),
            trace_id: 0x123456789abcdef0123456789abcdef0,
            span_id: Some(7),
            event_id: uuid::Uuid::from_u128(1),
            span_name: Some("mock span".to_string()),
            severity_number: SeverityNumber::Info,
            severity_text: "INFO".to_string(),
            body: "mock body".to_string(),
            attributes: TelemetryAttributes::new(Box::new(MockEvent {
                value: "mock value".to_string(),
            })),
        });

        let schemas = TelemetryArrowSchemas::new::<MockRegistry>();
        let batch = serialize_to_arrow(std::slice::from_ref(&record), &schemas).unwrap();
        let (deserialized, errors) = deserialize_from_arrow(&batch, &MockRegistry).unwrap();

        assert!(
            errors.is_empty(),
            "unexpected deserialize errors: {errors:?}"
        );
        assert_eq!(deserialized, vec![record]);
        assert_eq!(batch.num_rows(), 1);
        assert!(batch.schema().field_with_name("attributes").is_ok());
    }

    #[test]
    fn test_arrow_deserialize_mixed_success_and_malformed_rows() {
        let batch = batch_from_arrow_records(vec![
            mock_arrow_record(TEST_EVENT_TYPE, "valid", Some(uuid::Uuid::from_u128(1))),
            mock_arrow_record(TEST_EVENT_TYPE, "malformed", Some(uuid::Uuid::from_u128(2))),
        ]);

        let (records, errors) = deserialize_from_arrow(&batch, &MockRegistry).unwrap();

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].attributes().event_type(), TEST_EVENT_TYPE);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].index(), 2);
        let TelemetryDeserializeError::MalformedEvent { record, message } = errors[0].error()
        else {
            panic!("expected malformed event error");
        };
        let PartialTelemetryRecord::LogRecord(record) = record.as_ref() else {
            panic!("expected log partial record");
        };
        assert_eq!(record.event_type, TEST_EVENT_TYPE);
        assert_eq!(record.attributes["value"], "malformed");
        assert!(message.contains("bad mock value"));
    }

    #[test]
    fn test_arrow_unknown_event_type_preserves_partial_attributes() {
        let unknown_event_type = "v1.test.events.fusion.Unknown";
        let batch = batch_from_arrow_records(vec![mock_arrow_record(
            unknown_event_type,
            "unknown value",
            Some(uuid::Uuid::from_u128(1)),
        )]);

        let (records, errors) = deserialize_from_arrow(&batch, &MockRegistry).unwrap();

        assert!(records.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].index(), 1);
        let TelemetryDeserializeError::UnknownEventType { record } = errors[0].error() else {
            panic!("expected unknown event type error");
        };
        let PartialTelemetryRecord::LogRecord(record) = record.as_ref() else {
            panic!("expected log partial record");
        };
        assert_eq!(record.event_type, unknown_event_type);
        assert_eq!(record.attributes["value"], "unknown value");
    }

    #[test]
    fn test_arrow_missing_required_row_envelope_field_is_invalid_envelope() {
        let batch =
            batch_from_arrow_records(vec![mock_arrow_record(TEST_EVENT_TYPE, "valid", None)]);

        let (records, errors) = deserialize_from_arrow(&batch, &MockRegistry).unwrap();

        assert!(records.is_empty());
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].index(), 1);
        assert!(matches!(
            errors[0].error(),
            TelemetryDeserializeError::InvalidEnvelope { message }
                if message.contains("Missing event_id")
        ));
    }

    #[test]
    fn test_arrow_schema_incompatibility_returns_outer_error() {
        let batch = batch_from_arrow_records(vec![mock_arrow_record(
            TEST_EVENT_TYPE,
            "valid",
            Some(uuid::Uuid::from_u128(1)),
        )]);
        let batch = batch_without_column(&batch, "event_type");

        let err = deserialize_from_arrow(&batch, &MockRegistry).unwrap_err();

        assert!(err.to_string().contains("missing columns: event_type"));
    }
}
