use std::fmt;

use dbt_telemetry::AssetParsed;

use super::{
    color::{CYAN, GREEN, MaybeColored},
    duration::FixedWidthDuration,
    layout::RightAligned,
};

/// Displays an asset parsing start message using legacy style.
/// TODO: either remove any formatting for parsing start, or unify style
/// with other debug level spans messages.
pub struct AssetParsedStart<'a> {
    pub asset: &'a AssetParsed,
    pub colorize: bool,
}

impl fmt::Display for AssetParsedStart<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}",
            MaybeColored::new(&GREEN, RightAligned("Parsing"), self.colorize),
            self.asset.display_path
        )
    }
}

/// Displays an asset parsing end message.
pub struct AssetParsedEnd<'a> {
    pub asset: &'a AssetParsed,
    pub duration: std::time::Duration,
    pub colorize: bool,
}

impl fmt::Display for AssetParsedEnd<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Finished parsing {} [{}]",
            MaybeColored::new(&CYAN, &self.asset.display_path, self.colorize),
            FixedWidthDuration(self.duration)
        )
    }
}
