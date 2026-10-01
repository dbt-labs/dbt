use std::collections::HashSet;

use dbt_common::io_args::{ClapResourceType, EvalArgs, FsCommand, IoArgs};

use dbt_schemas::schemas::selection_override::SelectionOverride;

#[derive(Clone, Default, Debug)]
pub struct SchedulerArgs {
    pub command: FsCommand,
    pub io: IoArgs,
    pub resource_types: Vec<ClapResourceType>,
    pub exclude_resource_types: Vec<ClapResourceType>,
    /// A set of unique ids to be exluded when scheduling.
    /// This is more efficient than relying on exclusions from resolved selectors.
    /// REVIEW: If we can make expanding exclude selectors fast, then this will not be needed.
    pub exclude_unique_ids: HashSet<String>,
    /// An externally supplied node set that replaces the computed selection outright.
    ///
    /// When set, none of the selection inputs above are consulted: the supplied ids stand in for
    /// whatever `--select`, `--exclude`, `--resource-type` and `--exclude-resource-type` would
    /// have produced. Only the schedulability filters still apply, since a node this engine cannot
    /// build has no runnable task whatever its provenance.
    pub selection_override: Option<SelectionOverride>,
}

impl SchedulerArgs {
    pub fn from_eval_args(arg: &EvalArgs) -> Self {
        Self {
            command: arg.command,
            io: arg.io.clone(),
            resource_types: arg.resource_types.clone(),
            exclude_resource_types: arg.exclude_resource_types.clone(),
            exclude_unique_ids: Default::default(),
            selection_override: None,
        }
    }

    pub fn from_eval_args_with_exclude_unique_ids(
        arg: &EvalArgs,
        exclude_unique_ids: HashSet<String>,
    ) -> Self {
        Self {
            command: arg.command,
            io: arg.io.clone(),
            resource_types: arg.resource_types.clone(),
            exclude_resource_types: arg.exclude_resource_types.clone(),
            exclude_unique_ids,
            selection_override: None,
        }
    }

    pub fn from_eval_args_with_exclude_unique_ids_and_selection_override(
        arg: &EvalArgs,
        exclude_unique_ids: HashSet<String>,
        selection_override: Option<SelectionOverride>,
    ) -> Self {
        Self {
            selection_override,
            ..Self::from_eval_args_with_exclude_unique_ids(arg, exclude_unique_ids)
        }
    }
}
