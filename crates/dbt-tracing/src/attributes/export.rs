//! Output flags for telemetry events.
//!
//! These flags control which mediums an event is output/exported to.

use bitflags::bitflags;

bitflags! {
    /// Flags that determine where a telemetry event should be exported and/or output.
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct TelemetryOutputFlags: u32 {
        /// Export event to Parquet files.
        const EXPORT_PARQUET = 1;
        /// Export event to an OTLP backend.
        const EXPORT_OTLP = 1 << 1;
        /// Export event in JSONL fornmat (can be a separate file or stdout).
        const EXPORT_JSONL = 1 << 2;

        /// Output event to console (stdout/stderr).
        const OUTPUT_CONSOLE = 1 << 3;
        /// Output event to a human-readable log file (e.g. `dbt.log`).
        const OUTPUT_LOG_FILE = 1 << 4;

        // Helpful combinations of flags

        /// Alias that has both JSONL and OTLP flags, but no other flags.
        const EXPORT_JSONL_AND_OTLP = Self::EXPORT_JSONL.bits() | Self::EXPORT_OTLP.bits();

        /// Alias that has all export flags set – event goes to all machine readable mediums.
        const EXPORT_ALL = Self::EXPORT_PARQUET.bits() | Self::EXPORT_OTLP.bits() | Self::EXPORT_JSONL.bits();

        /// Alias that has all output flags set – event goes to all human readable mediums.
        const OUTPUT_ALL = Self::OUTPUT_CONSOLE.bits() | Self::OUTPUT_LOG_FILE.bits();

        /// Alias that has all export and output flags set – event goes to all mediums.
        const ALL = Self::EXPORT_ALL.bits() | Self::OUTPUT_ALL.bits();
    }
}

impl Default for TelemetryOutputFlags {
    fn default() -> Self {
        Self::empty()
    }
}
