use chrono::{DateTime, SecondsFormat, Utc};
use serde::Serializer;

pub fn serialize<S>(dt: &Option<pbjson_types::Timestamp>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match dt {
        Some(ts) => {
            let datetime = DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
                .ok_or_else(|| serde::ser::Error::custom("invalid timestamp"))?;
            serializer.serialize_str(&datetime.to_rfc3339_opts(SecondsFormat::Micros, true))
        }
        None => serializer.serialize_none(),
    }
}
