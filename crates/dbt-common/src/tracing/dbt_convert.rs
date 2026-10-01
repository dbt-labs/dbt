use crate::io_args::LogLevel;

pub fn log_level_filter_to_tracing(level_filter: &LogLevel) -> tracing::level_filters::LevelFilter {
    match *level_filter {
        LogLevel::Off => tracing::level_filters::LevelFilter::OFF,
        LogLevel::Error => tracing::level_filters::LevelFilter::ERROR,
        LogLevel::Warn => tracing::level_filters::LevelFilter::WARN,
        LogLevel::Info => tracing::level_filters::LevelFilter::INFO,
        LogLevel::Debug => tracing::level_filters::LevelFilter::DEBUG,
        LogLevel::Trace => tracing::level_filters::LevelFilter::TRACE,
    }
}
