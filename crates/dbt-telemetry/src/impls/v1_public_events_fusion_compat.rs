use crate::proto::v1::public::events::fusion::compat::SeverityNumber as ProtoSeverityNumber;
use dbt_tracing::SeverityNumber;
use serde::{Deserialize, Deserializer, de};
use std::fmt;

/// Convert the proto-derived severity into the tracing-library `SeverityNumber`.
///
/// Both enums share the same variants and discriminants; this bridges the
/// generated proto type used for `LogMessage` serde to the crate-owned type.
impl From<ProtoSeverityNumber> for SeverityNumber {
    fn from(value: ProtoSeverityNumber) -> Self {
        match value {
            ProtoSeverityNumber::Unspecified => SeverityNumber::Unspecified,
            ProtoSeverityNumber::Trace => SeverityNumber::Trace,
            ProtoSeverityNumber::Debug => SeverityNumber::Debug,
            ProtoSeverityNumber::Info => SeverityNumber::Info,
            ProtoSeverityNumber::Warn => SeverityNumber::Warn,
            ProtoSeverityNumber::Error => SeverityNumber::Error,
        }
    }
}

impl<'de> Deserialize<'de> for ProtoSeverityNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(SeverityNumberVisitor)
    }
}

struct SeverityNumberVisitor;

impl de::Visitor<'_> for SeverityNumberVisitor {
    type Value = ProtoSeverityNumber;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a severity number enum name or integer")
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(i32::try_from(value)
            .ok()
            .and_then(|value| ProtoSeverityNumber::try_from(value).ok())
            .unwrap_or(ProtoSeverityNumber::Unspecified))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(i32::try_from(value)
            .ok()
            .and_then(|value| ProtoSeverityNumber::try_from(value).ok())
            .unwrap_or(ProtoSeverityNumber::Unspecified))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        ProtoSeverityNumber::from_str_name(value)
            .ok_or_else(|| de::Error::unknown_variant(value, SEVERITY_NUMBER_NAMES))
    }
}

const SEVERITY_NUMBER_NAMES: &[&str] = &[
    "SEVERITY_NUMBER_UNSPECIFIED",
    "SEVERITY_NUMBER_TRACE",
    "SEVERITY_NUMBER_DEBUG",
    "SEVERITY_NUMBER_INFO",
    "SEVERITY_NUMBER_WARN",
    "SEVERITY_NUMBER_ERROR",
];
