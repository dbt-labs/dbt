mod location;
mod otlp;
mod record;
mod severity;

pub use location::RecordCodeLocation;
pub use otlp::{SpanStatus, StatusCode};
pub use record::{
    LogRecordInfo, SpanEndInfo, SpanLinkInfo, SpanStartInfo, TelemetryRecord, TelemetryRecordRef,
    TelemetryRecordType,
};
pub use severity::SeverityNumber;
