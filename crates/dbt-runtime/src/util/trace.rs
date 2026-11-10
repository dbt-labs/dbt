use std::marker::PhantomData;
use std::mem;

use tracing::instrument::{Instrument, Instrumented};

use crate::task::SpawnLocation;

#[derive(Copy, Clone)]
pub(crate) struct SpawnMeta<'a> {
    /// The name of the task
    pub(crate) name: Option<&'a str>,
    /// The original size of the future or function being spawned
    pub(crate) original_size: usize,
    /// The source code location where the task was spawned.
    ///
    /// This is wrapped in a type that may be empty when `tokio_unstable` is
    /// not enabled.
    pub(crate) spawned_at: SpawnLocation,
    _pd: PhantomData<&'a ()>,
}

impl<'a> SpawnMeta<'a> {
    /// Create new spawn meta with a name and original size (before possible auto-boxing)
    #[track_caller]
    #[expect(dead_code)]
    pub(crate) fn new(name: Option<&'a str>, original_size: usize) -> Self {
        Self {
            name,
            original_size,
            spawned_at: SpawnLocation::capture(),
            _pd: PhantomData,
        }
    }

    /// Create a new unnamed spawn meta with the original size (before possible auto-boxing)
    #[track_caller]
    pub(crate) fn new_unnamed(original_size: usize) -> Self {
        Self {
            name: None,
            original_size,
            spawned_at: SpawnLocation::capture(),
            _pd: PhantomData,
        }
    }
}

#[inline]
pub(crate) fn blocking_task<Fn, Fut>(
    task: Fut,
    spawn_meta: SpawnMeta<'_>,
    id: u64,
) -> Instrumented<Fut> {
    let fn_size = mem::size_of::<Fn>();
    let original_size = if spawn_meta.original_size != fn_size {
        Some(spawn_meta.original_size)
    } else {
        None
    };

    let span = tracing::trace_span!(
        target: "dbt-runtime",
        "runtime.spawn",
        kind = %"blocking",
        task.name = %spawn_meta.name.unwrap_or_default(),
        task.id = id,
        "fn" = %std::any::type_name::<Fn>(),
        original_size.bytes = original_size,
        size.bytes = fn_size,
        loc.file = spawn_meta.spawned_at.0.file(),
        loc.line = spawn_meta.spawned_at.0.line(),
        loc.col = spawn_meta.spawned_at.0.column(),
    );
    task.instrument(span)
}
