use crate::SeverityNumber;
use tracing::Level;

pub fn log_level_to_severity(level: &Level) -> (SeverityNumber, &'static str) {
    match *level {
        Level::ERROR => (SeverityNumber::Error, "ERROR"),
        Level::WARN => (SeverityNumber::Warn, "WARN"),
        Level::INFO => (SeverityNumber::Info, "INFO"),
        Level::DEBUG => (SeverityNumber::Debug, "DEBUG"),
        Level::TRACE => (SeverityNumber::Trace, "TRACE"),
    }
}
