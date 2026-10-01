//! Builder utilities
//!
//!

use adbc_core::options::OptionValue;
use std::iter::{Chain, Flatten};

/// An iterator over the builder options.
///
/// `COUNT` is the number of `Option<T>` fields in the builder.
///
/// In the implementation of this trait, the `COUNT` options are iterated first.
/// All the `None` values are skipped. After that, the iterator iterates over the
/// other options which are stored in a `Vec<(T, OptionValue)`.
pub struct BuilderIter<T, const COUNT: usize>(
    #[allow(clippy::type_complexity)]
    Chain<
        Flatten<<[Option<(T, OptionValue)>; COUNT] as IntoIterator>::IntoIter>,
        <Vec<(T, OptionValue)> as IntoIterator>::IntoIter,
    >,
);

impl<T, const COUNT: usize> BuilderIter<T, COUNT> {
    pub(crate) fn new(
        fixed: [Option<(T, OptionValue)>; COUNT],
        other: Vec<(T, OptionValue)>,
    ) -> Self {
        Self(fixed.into_iter().flatten().chain(other))
    }
}

impl<T, const COUNT: usize> Iterator for BuilderIter<T, COUNT> {
    type Item = (T, OptionValue);

    fn next(&mut self) -> Option<Self::Item> {
        self.0.next()
    }
}
