/// Default terminal width used for formatting when actual terminal width is unavailable
pub(super) const DEFAULT_TERMINAL_WIDTH: usize = 60;

/// Maximum length for displaying schema information in formatted outputs
pub(super) const MAX_QUALIFIER_DISPLAY_LEN: usize = 200;

/// Minimum width allocated for node type display in formatted outputs
pub(super) const MIN_NODE_TYPE_WIDTH: usize = 5; // Length of "model"

// Schema suffix for unit tests
pub(super) const UNIT_TEST_SCHEMA_SUFFIX: &str = "_dbt_test__audit";

/// Width for action labels
pub(super) const ACTION_WIDTH: usize = 10;

/// Width of the duration value rendered by `format_duration_fixed_width`
pub(super) const DURATION_WIDTH: usize = 7;

/// Column where a group header's verdict starts inside the right-aligned action column,
/// so member lines branch from directly beneath that verdict.
pub(super) const MEMBER_TREE_INDENT: usize = ACTION_WIDTH - "Failed".len();

/// Selected nodes delimeter title
pub(in crate::tracing) const SELECTED_NODES_TITLE: &str = "Selected nodes";
