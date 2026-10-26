use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;
use ustr::Ustr;

/// Owned version of [Ident].
pub type Identifier = Ident<'static>;

/// A cheaply-clonable type that implements SQL identifier semantics.
///
/// An [Ident] is essentially a read-only string that is case preserving, but
/// with case-insensitive equality semantics.
///
/// [Ident] objects are suitable for use as keys in hash maps or lookup
/// tables.
#[derive(Clone, Eq)]
pub enum Ident<'a> {
    Owned(&'static str),
    Borrowed(&'a str),
}

impl PartialEq for Ident<'_> {
    fn eq(&self, other: &Self) -> bool {
        // Fast path: pointer equality
        if self.as_ptr() == other.as_ptr() {
            return true;
        }
        self.name().eq_ignore_ascii_case(other.name())
    }
}

impl Ord for Ident<'_> {
    /// O(n) zero-copy case-insensitive lexicographic comparison.
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        for (l, r) in self
            .name()
            .chars()
            .map(|c| c.to_ascii_lowercase())
            .zip(other.name().chars().map(|c| c.to_ascii_lowercase()))
        {
            match l.cmp(&r) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        }

        self.name().len().cmp(&other.name().len())
    }
}

impl PartialOrd for Ident<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl std::hash::Hash for Ident<'_> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        for c in self.name().chars() {
            c.to_ascii_lowercase().hash(state);
        }
    }
}

impl Serialize for Ident<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.name().serialize(serializer)
    }
}

impl<'a> Deserialize<'a> for Ident<'static> {
    fn deserialize<D: Deserializer<'a>>(deserializer: D) -> Result<Self, D::Error> {
        String::deserialize(deserializer).map(Ident::new)
    }
}

impl Default for Ident<'static> {
    fn default() -> Self {
        Ident::Borrowed("")
    }
}

impl std::fmt::Debug for Ident<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self.name(), f)
    }
}

impl std::fmt::Display for Ident<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name())
    }
}

impl Ident<'_> {
    /// Creates a new owned identifier.
    pub fn new(value: impl AsRef<str>) -> Self {
        Ident::Owned(Ustr::from(value.as_ref()).into())
    }

    /// Returns a reference to the inner string value.
    pub fn name(&self) -> &str {
        match self {
            Ident::Owned(name) => name,
            Ident::Borrowed(name) => name,
        }
    }

    /// Consumes this [Ident] and returns the inner `Arc<str>` value. May incur
    /// an allocation if this [Ident] is borrowed.
    pub fn into_inner(self) -> Arc<str> {
        match self {
            Ident::Owned(name) => name.into(),
            Ident::Borrowed(name) => name.to_string().into(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.name().is_empty()
    }

    /// Converts this [Ident] into an owned variant. If this [Ident] is already
    /// owned, returns itself.
    pub fn to_owned(&self) -> Ident<'static> {
        match self {
            Ident::Owned(name) => Ident::Owned(name),
            Ident::Borrowed(name) => Ident::Owned(Ustr::from(name).into()),
        }
    }

    pub fn to_value(&self) -> String {
        self.name().to_string()
    }

    pub fn to_ascii_lowercase(&self) -> String {
        self.name().to_ascii_lowercase()
    }

    pub fn to_ascii_uppercase(&self) -> String {
        self.name().to_ascii_uppercase()
    }

    pub fn as_str(&self) -> &str {
        self.name()
    }

    pub fn as_path(&self) -> &Path {
        Path::new(self.name())
    }

    /// Case-insenstiive comparison of the inner string.
    ///
    /// This is equivalent to `self == other` for `Ident` types.
    pub fn matches(&self, str_value: impl AsRef<str>) -> bool {
        self.name().eq_ignore_ascii_case(str_value.as_ref())
    }

    /// Case-sensitive comparison of the inner string.
    pub fn matches_exact(&self, str_value: impl AsRef<str>) -> bool {
        self.name() == str_value.as_ref()
    }

    // Internal: Get the raw pointer to the inner string value.
    fn as_ptr(&self) -> *const u8 {
        match self {
            Ident::Owned(name) => name.as_ptr(),
            Ident::Borrowed(name) => name.as_ptr(),
        }
    }
}

impl From<String> for Ident<'static> {
    fn from(value: String) -> Self {
        Ident::Owned(Ustr::from(value.as_str()).into())
    }
}

impl From<Arc<str>> for Ident<'static> {
    fn from(value: Arc<str>) -> Self {
        Ident::Owned(Ustr::from(&value).into())
    }
}

impl<'a> From<Ident<'a>> for String {
    fn from(value: Ident<'a>) -> Self {
        value.name().to_string()
    }
}

impl<'a> From<&Ident<'a>> for String {
    fn from(value: &Ident<'a>) -> Self {
        value.name().to_string()
    }
}

impl<'a> From<&'a String> for Ident<'a> {
    fn from(value: &'a String) -> Self {
        Ident::Borrowed(value.as_str())
    }
}

impl<'a> From<&'a str> for Ident<'a> {
    fn from(value: &'a str) -> Self {
        Ident::Borrowed(value)
    }
}

impl<'a> From<Cow<'a, str>> for Ident<'a> {
    fn from(value: Cow<'a, str>) -> Self {
        match value {
            Cow::Borrowed(value) => Ident::Borrowed(value),
            Cow::Owned(value) => Ident::Owned(Ustr::from(value.as_str()).into()),
        }
    }
}

impl<'a> From<&Ident<'a>> for Ident<'a> {
    fn from(value: &Ident<'a>) -> Self {
        value.clone()
    }
}

impl AsRef<str> for Ident<'_> {
    fn as_ref(&self) -> &str {
        self.name()
    }
}

impl AsRef<Path> for Ident<'_> {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}
