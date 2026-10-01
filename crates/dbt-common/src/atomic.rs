use arc_swap::{ArcSwap, ArcSwapOption};
use std::sync::Arc;

#[derive(Debug)]
pub struct Atomic<T> {
    value: ArcSwap<T>,
}

impl<T> Atomic<T> {
    pub fn new(value: Arc<T>) -> Self {
        Self {
            value: ArcSwap::new(value),
        }
    }

    /// Loads the underlying value atomically.
    pub fn load(&self) -> Arc<T> {
        self.value.load_full()
    }

    /// Sets the underlying value atomically.
    pub fn store(&self, value: Arc<T>) {
        self.value.store(value)
    }
}

#[derive(Debug)]
pub struct AtomicOption<T> {
    value: ArcSwapOption<T>,
}

impl<T> AtomicOption<T> {
    pub fn new(value: Option<Arc<T>>) -> Self {
        Self {
            value: ArcSwapOption::new(value),
        }
    }

    pub fn empty() -> Self {
        Self {
            value: ArcSwapOption::empty(),
        }
    }

    /// Loads the underlying value atomically.
    pub fn load(&self) -> Option<Arc<T>> {
        self.value.load_full()
    }

    /// Sets the underlying value atomically.
    pub fn store(&self, value: Option<Arc<T>>) {
        self.value.store(value)
    }
}

impl<T> Drop for AtomicOption<T> {
    fn drop(&mut self) {
        self.store(None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_option_strong_count() {
        let value = Arc::new(0);

        let atomic_value = AtomicOption::empty();
        atomic_value.store(Some(value.clone()));

        assert_eq!(3, Arc::strong_count(&atomic_value.load().unwrap()));
        assert_eq!(2, Arc::strong_count(&value));
        assert_eq!(3, Arc::strong_count(&atomic_value.load().unwrap()));
    }

    #[test]
    fn test_atomic_option_strong_count_2() {
        let value = Arc::new(0);

        let atomic_value = AtomicOption::empty();
        atomic_value.store(Some(value.clone()));

        let arc_value = atomic_value.load().unwrap();
        assert_eq!(3, Arc::strong_count(&arc_value));
        assert_eq!(3, Arc::strong_count(&value));
        assert_eq!(3, Arc::strong_count(&arc_value));
    }

    #[test]
    fn test_atomic_option_stress_concurrency() {
        let r = Arc::new(AtomicOption::empty());

        for _ in 1..10 {
            let mut threads = Vec::new();
            for i in 1..1000 {
                let r = r.clone();
                let thread = std::thread::spawn(move || {
                    let inner_r = Arc::new(123);

                    std::thread::sleep(std::time::Duration::from_millis(100));
                    if i % 2 == 0 {
                        r.store(None);
                    }
                    r.store(Some(inner_r));
                    if let Some(value) = r.load() {
                        assert_eq!(Arc::new(123), value)
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    r.store(None);
                });
                threads.push(thread);
            }

            for thread in threads {
                thread.join().expect("thread should not have failed")
            }
        }

        let value = r.load();
        assert!(value.is_none());
    }
}
