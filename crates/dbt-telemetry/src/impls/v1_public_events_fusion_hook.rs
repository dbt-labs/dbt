use crate::proto::v1::public::events::fusion::hook::{HookOutcome, HookProcessed, HookType};

impl HookType {
    pub const fn as_static_str(&self) -> &'static str {
        match self {
            Self::Unspecified => "hook",
            Self::OnRunStart => "on-run-start",
            Self::OnRunEnd => "on-run-end",
            Self::PreHook => "pre-hook",
            Self::PostHook => "post-hook",
        }
    }
}

impl HookProcessed {
    /// Creates a new `HookProcessed` event indicating start of an on-run hook.
    ///
    /// This helper sets stable start fields used for on-run hooks:
    /// - `hook_outcome` starts as `Unspecified`)
    /// - `dbt_core_event_code` is `Q032` (start event)
    /// - `phase` is left unset and resolved from tracing context
    /// - `hook_index` is the 0-based position within the combined (project & deps) on-run hook list
    pub fn start_on_run(
        package_name: &str,
        name: &str,
        hook_type: HookType,
        hook_index: u32,
        unique_id: &str,
    ) -> Self {
        debug_assert!(
            matches!(hook_type, HookType::OnRunStart | HookType::OnRunEnd),
            "start_on_run is only valid for on-run-start/on-run-end hooks"
        );

        Self::new(
            package_name.to_string(),
            Some(name.to_string()),
            hook_type,
            hook_index,
            unique_id.to_string(),
            HookOutcome::Unspecified,
            "Q032".to_string(),
            crate::ExecutionPhase::Unspecified,
        )
    }
}
