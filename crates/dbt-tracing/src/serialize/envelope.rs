//! Serde helpers for telemetry record envelope fields (ids and timestamps).

use serde::de::{self, Visitor};
use serde::{Deserializer, Serializer};
use std::fmt;
use std::time::SystemTime;

/// Custom serialization for trace_id as 32-character hexadecimal string
pub fn serialize_trace_id<S>(trace_id: &u128, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("{trace_id:032x}"))
}

/// Custom deserialization for trace_id from hexadecimal string
pub fn deserialize_trace_id<'de, D>(deserializer: D) -> Result<u128, D::Error>
where
    D: Deserializer<'de>,
{
    struct TraceIdVisitor;

    impl<'de> Visitor<'de> for TraceIdVisitor {
        type Value = u128;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a 32-character hexadecimal string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u128::from_str_radix(value, 16)
                .map_err(|_| E::custom(format!("invalid trace_id hex string: {value}")))
        }
    }

    deserializer.deserialize_str(TraceIdVisitor)
}

/// Custom serialization for span_id as 16-character hexadecimal string
pub fn serialize_span_id<S>(span_id: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(&format!("{span_id:016x}"))
}

/// Custom deserialization for span_id from hexadecimal string
pub fn deserialize_span_id<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    struct SpanIdVisitor;

    impl<'de> Visitor<'de> for SpanIdVisitor {
        type Value = u64;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a 16-character hexadecimal string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u64::from_str_radix(value, 16)
                .map_err(|_| E::custom(format!("invalid span_id hex string: {value}")))
        }
    }

    deserializer.deserialize_str(SpanIdVisitor)
}

/// Custom serialization for optional span_id as 16-character hexadecimal string
pub fn serialize_optional_span_id<S>(
    span_id: &Option<u64>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match span_id {
        Some(id) => serializer.serialize_some(&format!("{id:016x}")),
        None => serializer.serialize_none(),
    }
}

/// Custom deserialization for optional span_id from hexadecimal string
pub fn deserialize_optional_span_id<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
where
    D: Deserializer<'de>,
{
    struct OptionalSpanIdVisitor;

    impl<'de> Visitor<'de> for OptionalSpanIdVisitor {
        type Value = Option<u64>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an optional 16-character hexadecimal string")
        }

        fn visit_none<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_unit<E>(self) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            Ok(None)
        }

        fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
        where
            D: Deserializer<'de>,
        {
            deserialize_span_id(deserializer).map(Some)
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            u64::from_str_radix(value, 16)
                .map(Some)
                .map_err(|_| E::custom(format!("invalid span_id hex string: {value}")))
        }
    }

    deserializer.deserialize_option(OptionalSpanIdVisitor)
}

#[inline]
pub fn to_nanos(timestamp: &SystemTime) -> u64 {
    timestamp
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}

/// Custom serialization for timestamps as string
pub fn serialize_timestamp<S>(timestamp: &SystemTime, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    serializer.serialize_str(to_nanos(timestamp).to_string().as_str())
}

/// Custom deserialization for timestamps from string
pub fn deserialize_timestamp<'de, D>(deserializer: D) -> Result<SystemTime, D::Error>
where
    D: Deserializer<'de>,
{
    struct TimestampVisitor;

    impl<'de> Visitor<'de> for TimestampVisitor {
        type Value = SystemTime;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("a timestamp as string")
        }

        fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
        where
            E: de::Error,
        {
            value
                .parse::<u64>()
                .map_err(|_| E::custom(format!("invalid timestamp string: {value}")))
                .map(|nanos| SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(nanos))
        }
    }

    deserializer.deserialize_str(TimestampVisitor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn test_trace_id_serialization() {
        let trace_id: u128 = 0x123456789abcdef0123456789abcdef0;
        let serializer = serde_json::value::Serializer;
        let result = serialize_trace_id(&trace_id, serializer).unwrap();
        if let serde_json::Value::String(s) = result {
            assert_eq!(s, "123456789abcdef0123456789abcdef0");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_trace_id_serialization_small_number() {
        let trace_id: u128 = 1;
        let serializer = serde_json::value::Serializer;
        let result = serialize_trace_id(&trace_id, serializer).unwrap();
        if let serde_json::Value::String(s) = result {
            assert_eq!(s, "00000000000000000000000000000001");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_trace_id_deserialization() {
        let json_str = "\"123456789abcdef0123456789abcdef0\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let trace_id = deserialize_trace_id(deserializer).unwrap();
        assert_eq!(trace_id, 0x123456789abcdef0123456789abcdef0);
    }

    #[test]
    fn test_trace_id_deserialization_small_number() {
        let json_str = "\"00000000000000000000000000000001\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let trace_id = deserialize_trace_id(deserializer).unwrap();
        assert_eq!(trace_id, 1);
    }

    #[test]
    fn test_span_id_serialization() {
        let span_id: u64 = 0x123456789abcdef0;
        let serializer = serde_json::value::Serializer;
        let result = serialize_span_id(&span_id, serializer).unwrap();
        if let serde_json::Value::String(s) = result {
            assert_eq!(s, "123456789abcdef0");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_span_id_deserialization() {
        let json_str = "\"123456789abcdef0\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let span_id = deserialize_span_id(deserializer).unwrap();
        assert_eq!(span_id, 0x123456789abcdef0);
    }

    #[test]
    fn test_optional_span_id_serialization_some() {
        let span_id: Option<u64> = Some(0x123456789abcdef0);
        let serializer = serde_json::value::Serializer;
        let result = serialize_optional_span_id(&span_id, serializer).unwrap();
        if let serde_json::Value::String(s) = result {
            assert_eq!(s, "123456789abcdef0");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_optional_span_id_serialization_none() {
        let span_id: Option<u64> = None;
        let serializer = serde_json::value::Serializer;
        let result = serialize_optional_span_id(&span_id, serializer).unwrap();
        assert_eq!(result, serde_json::Value::Null);
    }

    #[test]
    fn test_optional_span_id_deserialization_some() {
        let json_str = "\"123456789abcdef0\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let span_id = deserialize_optional_span_id(deserializer).unwrap();
        assert_eq!(span_id, Some(0x123456789abcdef0));
    }

    #[test]
    fn test_optional_span_id_deserialization_none() {
        let json_str = "null";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let span_id = deserialize_optional_span_id(deserializer).unwrap();
        assert_eq!(span_id, None);
    }

    #[test]
    fn test_timestamp_serialization() {
        let timestamp =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(1234567890123456700);
        let serializer = serde_json::value::Serializer;
        let result = serialize_timestamp(&timestamp, serializer).unwrap();
        if let serde_json::Value::String(s) = result {
            assert_eq!(s, "1234567890123456700");
        } else {
            panic!("Expected string value");
        }
    }

    #[test]
    fn test_timestamp_deserialization() {
        let json_str = "\"1234567890123456700\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let timestamp = deserialize_timestamp(deserializer).unwrap();
        let expected =
            SystemTime::UNIX_EPOCH + std::time::Duration::from_nanos(1234567890123456700);
        assert_eq!(timestamp, expected);
    }

    #[test]
    fn test_invalid_trace_id_deserialization() {
        let json_str = "\"invalid_hex\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let result = deserialize_trace_id(deserializer);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_span_id_deserialization() {
        let json_str = "\"invalid_hex\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let result = deserialize_span_id(deserializer);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_timestamp_deserialization() {
        let json_str = "\"not_a_number\"";
        let deserializer = &mut serde_json::Deserializer::from_str(json_str);
        let result = deserialize_timestamp(deserializer);
        assert!(result.is_err());
    }
}
