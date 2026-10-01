use std::sync::atomic::{AtomicUsize, Ordering};

/// `AtomicUsize` for use in metrics.
#[derive(Debug, Default)]
pub(crate) struct MetricAtomicUsize {
    value: AtomicUsize,
}

impl MetricAtomicUsize {
    #[expect(dead_code)]
    pub(crate) fn new(value: usize) -> Self {
        Self {
            value: AtomicUsize::new(value),
        }
    }

    pub(crate) fn load(&self, ordering: Ordering) -> usize {
        self.value.load(ordering)
    }

    #[expect(dead_code)]
    pub(crate) fn store(&self, val: usize, ordering: Ordering) {
        self.value.store(val, ordering)
    }

    pub(crate) fn increment(&self) -> usize {
        self.value.fetch_add(1, Ordering::Relaxed)
    }

    pub(crate) fn decrement(&self) -> usize {
        self.value.fetch_sub(1, Ordering::Relaxed)
    }
}
