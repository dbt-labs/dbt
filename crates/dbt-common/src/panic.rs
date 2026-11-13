//! Recovering from panics in code that cannot be changed to return errors.
//!
//! A process-wide panic hook that exits (as the CLI installs) runs before any
//! `catch_unwind` gets control, so a plain `catch_unwind` never recovers under
//! it. [`catch_unwind_quietly`] marks the current thread so such hooks can
//! check [`is_catching_panic`] and return, letting the panic unwind to the
//! caller.

use std::any::Any;
use std::cell::Cell;
use std::panic::{AssertUnwindSafe, catch_unwind};

thread_local! {
    static CATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Whether the current thread is inside [`catch_unwind_quietly`]. Panic hooks
/// that terminate the process should return early when this is true.
pub fn is_catching_panic() -> bool {
    CATCH_DEPTH.with(|depth| depth.get() > 0)
}

/// Runs `f`, converting a panic into `Err(message)`. Only use this around code
/// whose state is discarded on panic.
pub fn catch_unwind_quietly<R>(f: impl FnOnce() -> R) -> Result<R, String> {
    struct Guard;
    impl Drop for Guard {
        fn drop(&mut self) {
            CATCH_DEPTH.with(|depth| depth.set(depth.get() - 1));
        }
    }

    CATCH_DEPTH.with(|depth| depth.set(depth.get() + 1));
    let _guard = Guard;
    catch_unwind(AssertUnwindSafe(f)).map_err(|payload| panic_message(payload.as_ref()))
}

/// The message of a panic payload, for `&str` and `String` payloads.
pub fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catches_panic_and_tracks_depth() {
        assert!(!is_catching_panic());
        let result = catch_unwind_quietly(|| {
            assert!(is_catching_panic());
            panic!("boom {}", 1);
        });
        assert_eq!(result, Err::<(), _>("boom 1".to_string()));
        assert!(!is_catching_panic());
        assert_eq!(catch_unwind_quietly(|| 7), Ok(7));
    }
}
