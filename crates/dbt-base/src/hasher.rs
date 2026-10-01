use std::hash::{BuildHasher, DefaultHasher, Hasher, RandomState};

/// A distinct type wrapper around Rust's [DefaultHasher], that simply forwards
/// all hashing operations to it.
///
/// Note that we are not modifying the (SipHash13) hashing algorithm in any way,
/// only the *seed*. The purpose of this type is to make our own (drop-in
/// replacement) `HashMap` and `HashSet` types that is distinct from the
/// standard library's default types, which can then be enforced by clippy.
#[derive(Default)]
pub struct MaybeStableHasher(DefaultHasher);

impl Hasher for MaybeStableHasher {
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        self.0.write(bytes);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0.finish()
    }
}

/// A version of the default hasher that is stable across program runs for debug
/// builds, and DDoS-resistant for release builds.
#[derive(Clone, Default)]
#[allow(dead_code)]
pub struct MaybeStableHasherBuilder(RandomState);

impl BuildHasher for MaybeStableHasherBuilder {
    type Hasher = MaybeStableHasher;

    fn build_hasher(&self) -> Self::Hasher {
        #[cfg(any(debug_assertions, feature = "stable-hasher"))]
        {
            // This delegates to DefaultHasher::default(), which uses a fixed
            // seed of zero.
            MaybeStableHasher::default()
        }
        #[cfg(not(any(debug_assertions, feature = "stable-hasher")))]
        {
            // In release builds, we want a hasher with a random seed for DDoS
            // resistance -- just offload to RandomState as usual.
            MaybeStableHasher((self.0).build_hasher())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maybe_stable_hasher_consistency() {
        let mut hasher1 = MaybeStableHasher::default();
        let mut hasher2 = MaybeStableHasher::default();
        let data = b"test data";
        hasher1.write(data);
        hasher2.write(data);
        assert_eq!(hasher1.finish(), hasher2.finish());
    }

    #[test]
    fn test_maybe_stable_hasher_builder() {
        let builder1 = MaybeStableHasherBuilder::default();
        let builder2 = MaybeStableHasherBuilder::default();
        let mut hasher1 = builder1.build_hasher();
        let mut hasher2 = builder2.build_hasher();
        let data = b"another test";
        hasher1.write(data);
        hasher2.write(data);

        // The behavior differs based on build configuration:
        #[cfg(any(debug_assertions, feature = "stable-hasher"))]
        assert_eq!(hasher1.finish(), hasher2.finish());
        #[cfg(not(any(debug_assertions, feature = "stable-hasher")))]
        assert_ne!(hasher1.finish(), hasher2.finish());
    }
}
