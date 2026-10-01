/// Utility module to help with string interning.
use crate::utils::OnDrop;

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::sync::Arc;

thread_local! {
    static STRING_KEY_CACHE: RefCell<HashSet<Arc<str>>> = Default::default();
    static USE_STRING_KEY_CACHE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn use_string_cache() -> impl Drop {
    let was_enabled = USE_STRING_KEY_CACHE.with(|flag| {
        let was_enabled = flag.get();
        flag.set(true);
        was_enabled
    });
    OnDrop::new(move || {
        if !was_enabled {
            USE_STRING_KEY_CACHE.with(|flag| flag.set(false));
            STRING_KEY_CACHE.with(|cache| cache.borrow_mut().clear());
        }
    })
}

#[inline(always)]
pub(crate) fn try_intern(s: &str) -> Arc<str> {
    // strings longer than 16 bytes are never interned or if we're at
    // depth 0.  (serialization code outside of internal serialization)
    // not checking for depth can cause a memory leak.
    if s.len() > 16 || !USE_STRING_KEY_CACHE.with(|flag| flag.get()) {
        return Arc::from(s);
    }

    STRING_KEY_CACHE.with(|cache| {
        let mut set = cache.borrow_mut();
        match set.get(s) {
            Some(stored) => stored.clone(),
            None => {
                let rv: Arc<str> = Arc::from(s.to_string());
                set.insert(rv.clone());
                rv
            }
        }
    })
}
