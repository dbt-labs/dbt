use crate::proto::v1::public::events::fusion::{asset::AssetParsed, phase::ExecutionPhase};

impl AssetParsed {
    /// Creates a new `AssetParsed` with phase coming from context.
    ///
    /// Arguments:
    /// * `package_name` - Package name that owns this asset.
    /// * `name` - Asset name (typically the file stem).
    /// * `relative_path` - Path to the asset relative to the project root.
    /// * `display_path` - Display path used for CLI progress output.
    /// * `unique_id` - Optional unique identifier for this asset.
    pub fn new_with_phase_from_context(
        package_name: String,
        name: String,
        relative_path: String,
        display_path: String,
        unique_id: Option<String>,
    ) -> Self {
        Self {
            package_name,
            name,
            relative_path,
            display_path,
            unique_id,
            phase: ExecutionPhase::Unspecified as i32,
        }
    }
}
