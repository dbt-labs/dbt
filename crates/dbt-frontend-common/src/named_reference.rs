use std::hash::{Hash, Hasher};
use std::{fmt, ops};

use serde::Serialize;

use crate::FullyQualifiedName;

/// Wrapper type for references to named SQL entities, such as tables, views,
/// functions, etc.
#[derive(Debug, Clone, Serialize)]
pub struct NamedReference<T> {
    inner_ref: T,
    pub is_prefix: bool,
}

impl<T> NamedReference<T> {
    pub fn new(reference: T, is_prefix: bool) -> Self {
        NamedReference {
            inner_ref: reference,
            is_prefix,
        }
    }

    pub fn into_inner(self) -> T {
        self.inner_ref
    }

    pub fn map<F, U>(self, f: F) -> NamedReference<U>
    where
        F: FnOnce(T) -> U,
    {
        NamedReference {
            inner_ref: f(self.inner_ref),
            is_prefix: self.is_prefix,
        }
    }
}

impl<T> AsRef<T> for NamedReference<T> {
    fn as_ref(&self) -> &T {
        &self.inner_ref
    }
}

impl<T> ops::Deref for NamedReference<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.inner_ref
    }
}

impl<T> fmt::Display for NamedReference<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.inner_ref)
    }
}

impl<T> From<T> for NamedReference<T> {
    fn from(value: T) -> Self {
        NamedReference {
            inner_ref: value,
            is_prefix: false,
        }
    }
}

pub fn named_references_into_inner(
    named_references: impl Iterator<Item = NamedReference<FullyQualifiedName>>,
) -> Vec<FullyQualifiedName> {
    named_references.map(|r| r.inner_ref).collect()
}

pub fn to_default_named_references(
    names: impl Iterator<Item = FullyQualifiedName>,
) -> Vec<NamedReference<FullyQualifiedName>> {
    names.map(|name| name.into()).collect()
}

impl From<NamedReference<String>> for String {
    fn from(source: NamedReference<String>) -> Self {
        source.inner_ref
    }
}

impl From<NamedReference<FullyQualifiedName>> for FullyQualifiedName {
    fn from(source: NamedReference<FullyQualifiedName>) -> Self {
        source.inner_ref
    }
}

impl<T> Ord for NamedReference<T>
where
    T: Ord,
{
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.inner_ref.cmp(&other.inner_ref)
    }
}

impl<T> PartialOrd for NamedReference<T>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        match self.inner_ref.partial_cmp(&other.inner_ref) {
            Some(core::cmp::Ordering::Equal) => Some(core::cmp::Ordering::Equal),
            ord => ord,
        }
    }
}

impl<T> Eq for NamedReference<T> where T: Eq {}

impl<T> PartialEq for NamedReference<T>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        self.inner_ref == other.inner_ref
    }
}

impl<T> Hash for NamedReference<T>
where
    T: Eq + PartialEq + Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.inner_ref.hash(state);
    }
}
