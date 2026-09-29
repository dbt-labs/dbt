use crate::handle::Handle;
use crate::task::{self, Task, TaskHarnessScheduleHooks};

/// `task::Schedule` implementation that does nothing (except some bookkeeping
/// in test-util builds). This is unique to the blocking scheduler as tasks
/// scheduled are not really futures but blocking operations.
///
/// We avoid storing the task by forgetting it in `bind` and re-materializing it
/// in `release`.
pub(crate) struct BlockingSchedule {
    hooks: TaskHarnessScheduleHooks,
}

impl BlockingSchedule {
    pub(crate) fn new(handle: &Handle) -> Self {
        BlockingSchedule {
            hooks: TaskHarnessScheduleHooks {
                task_terminate_callback: handle.inner.hooks().task_terminate_callback.clone(),
            },
        }
    }
}

impl task::Schedule for BlockingSchedule {
    fn release(&self, _task: &Task<Self>) -> Option<Task<Self>> {
        None
    }

    fn schedule(&self, _task: task::Notified<Self>) {
        unreachable!();
    }

    fn hooks(&self) -> TaskHarnessScheduleHooks {
        TaskHarnessScheduleHooks {
            task_terminate_callback: self.hooks.task_terminate_callback.clone(),
        }
    }
}
