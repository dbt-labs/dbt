use std::cell::{Cell, RefCell};
use std::marker::PhantomData;

use super::{CONTEXT, Context};
use crate::handle::{Handle, TryCurrentError};
use crate::util::markers::SyncNotSend;

#[must_use]
pub struct SetCurrentGuard {
    /// The previous handle.
    prev: Option<Handle>,

    /// The depth for this guard.
    depth: usize,

    /// Don't let the type move across threads.
    _p: PhantomData<SyncNotSend>,
}

pub(super) struct HandleCell {
    /// Current handle
    handle: RefCell<Option<Handle>>,

    /// Tracks the number of nested calls to `try_set_current`.
    depth: Cell<usize>,
}

/// Sets this [`Handle`] as the current active [`Handle`].
///
/// [`Handle`]: crate::handle::Handle
pub(crate) fn try_set_current(handle: &Handle) -> Option<SetCurrentGuard> {
    CONTEXT.try_with(|ctx| ctx.set_current(handle)).ok()
}

pub(crate) fn with_current<F, R>(f: F) -> Result<R, TryCurrentError>
where
    F: FnOnce(&Handle) -> R,
{
    match CONTEXT.try_with(|ctx| ctx.current.handle.borrow().as_ref().map(f)) {
        Ok(Some(ret)) => Ok(ret),
        Ok(None) => Err(TryCurrentError::NoContext),
        Err(_access_error) => Err(TryCurrentError::ThreadLocalDestroyed),
    }
}

impl Context {
    pub(super) fn set_current(&self, handle: &Handle) -> SetCurrentGuard {
        let old_handle = self.current.handle.borrow_mut().replace(handle.clone());
        let depth = self.current.depth.get();

        assert!(depth != usize::MAX, "reached max `enter` depth");

        let depth = depth + 1;
        self.current.depth.set(depth);

        SetCurrentGuard {
            prev: old_handle,
            depth,
            _p: PhantomData,
        }
    }
}

impl HandleCell {
    pub(super) const fn new() -> HandleCell {
        HandleCell {
            handle: RefCell::new(None),
            depth: Cell::new(0),
        }
    }
}

impl Drop for SetCurrentGuard {
    fn drop(&mut self) {
        CONTEXT.with(|ctx| {
            let depth = ctx.current.depth.get();

            if depth != self.depth {
                if !std::thread::panicking() {
                    panic!(
                        "`EnterGuard` values dropped out of order. Guards returned by \
                         `Handle::enter()` must be dropped in the reverse order as they \
                         were acquired."
                    );
                } else {
                    // Just return... this will leave handles in a wonky state though...
                    return;
                }
            }

            *ctx.current.handle.borrow_mut() = self.prev.take();
            ctx.current.depth.set(depth - 1);
        });
    }
}
