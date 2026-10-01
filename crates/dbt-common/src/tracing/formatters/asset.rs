use dbt_telemetry::AssetParsed;

use super::{
    color::{CYAN, GREEN, maybe_apply_color},
    duration::format_duration_fixed_width,
    layout::right_align_static_action,
};

/// Format an asset parsing start message using legacy style.
/// TODO: either remove any formatting for parsing start, or unify style
/// with other debug level spans messages.
pub fn format_asset_parsed_start(asset: &AssetParsed, colorize: bool) -> String {
    let action = right_align_static_action("Parsing");
    let action = maybe_apply_color(&GREEN, &action, colorize);
    format!("{} {}", action, asset.display_path)
}

/// Format an asset parsing end message.
pub fn format_asset_parsed_end(
    asset: &AssetParsed,
    duration: std::time::Duration,
    colorize: bool,
) -> String {
    let duration_formatted = format_duration_fixed_width(duration);
    let path_formatted = maybe_apply_color(&CYAN, &asset.display_path, colorize);

    format!(
        "Finished parsing {} [{}]",
        path_formatted, duration_formatted
    )
}
