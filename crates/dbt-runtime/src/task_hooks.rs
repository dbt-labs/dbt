use std::marker::PhantomData;

use crate::task::SpawnLocation;
use crate::task::id::Id;

/// The task hooks a runtime shares with every task it spawns.
///
/// Upstream this hangs off the *scheduler* handle, not the blocking pool, and
/// carries `task_spawn_callback` / `before_poll_callback` / `after_poll_callback`
/// as well. Only the terminate hook applies to blocking work.
#[derive(Clone, Default)]
pub(crate) struct TaskHooks {
    pub(crate) task_terminate_callback: Option<TaskCallback>,
}

pub struct TaskMeta<'a> {
    /// The opaque ID of the task.
    pub(crate) id: Id,
    /// The source code location where the task was spawned.
    pub(crate) spawned_at: SpawnLocation,
    pub(crate) _phantom: PhantomData<&'a ()>,
}

impl<'a> TaskMeta<'a> {
    /// Return the opaque ID of the task.
    pub fn id(&self) -> Id {
        self.id
    }

    /// Return the source code location where the task was spawned.
    pub fn spawned_at(&self) -> &'static std::panic::Location<'static> {
        self.spawned_at.0
    }
}

/// Runs on specific task-related events
pub type TaskCallback = std::sync::Arc<dyn Fn(&TaskMeta<'_>) + Send + Sync>;
