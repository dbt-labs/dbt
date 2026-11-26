use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::num::NonZero;

use dbt_adapter_core::{AdapterType, quote_char};

use super::is_keyword_ignore_ascii_case;
use super::tokenizer::QuotingStyle;

pub mod bigquery;
pub mod clickhouse;
pub mod databricks;
pub mod duckdb;
pub mod redshift;
pub mod snowflake;
pub mod spark;

/// An identifier and its provenance (from quoted or an plain identifier in the input source).
///
/// The name [Ident::Unquoted] means that the identifier was *quoted* in the
/// input source, but we call it "unquoted" because we have removed the
/// quoting characters and unescaped any escaped characters in the token.
///
/// This makes manipulating identifiers easier, because we don't have to
/// worry about escaping and quoting until we want to format the identifier
/// using the [Ident::display] method that takes a [AdapterType] and quotes the
/// identifier if necessary according to the specific dialect rules.
///
/// This also lets us preserve intentional quoting in the input source, even
/// if the identifier could have been represented as a plain identifier. For
/// example, the identifier `"MyTable"` (with quotes) will be represented
/// as `Unquoted('"', "MyTable")`, even though it could have been represented
/// as `Plain("MyTable")`. This is important for dialects like Snowflake
/// where unquoted identifiers are normalized to uppercase, so the quotes
/// are necessary to preserve the original casing.
#[derive(Debug, Clone)]
pub enum Ident {
    /// Identifier that was not quoted in the input source.
    Plain(String),
    /// Identifier that was quoted in the input source and has been unescaped.
    Unquoted(QuotingStyle, String),
}

impl AsRef<str> for Ident {
    fn as_ref(&self) -> &str {
        match self {
            Ident::Unquoted(_, s) => s.as_ref(),
            Ident::Plain(s) => s.as_ref(),
        }
    }
}

impl Ident {
    pub fn new(s: impl Into<String>, backend: AdapterType) -> Self {
        let s: String = s.into();
        if must_be_quoted(&s, backend) {
            Ident::Unquoted(canonical_quote(backend), s)
        } else {
            Ident::Plain(s)
        }
    }

    /// Create an [Ident] from an unescaped string that must always be rendered
    /// in the backend's canonical quotes. Escaping happens on [Ident::display].
    ///
    /// ```ignore
    /// let id = Ident::from_unquoted(r"a\b`c", AdapterType::Bigquery);
    /// assert_eq!(id.display(AdapterType::Bigquery).to_string(), r"`a\\b\`c`");
    /// ```
    pub fn from_unquoted(s: impl Into<String>, backend: AdapterType) -> Self {
        Ident::unquoted(canonical_quote(backend), s)
    }

    pub fn plain(s: impl Into<String>) -> Self {
        Ident::Plain(s.into())
    }

    pub fn unquoted(quote: QuotingStyle, s: impl Into<String>) -> Self {
        Ident::Unquoted(quote, s.into())
    }

    pub fn from_string(s: impl Into<String>, must_quote: bool, backend: AdapterType) -> Self {
        let s = s.into();
        if must_quote || must_be_quoted(&s, backend) {
            Ident::Unquoted(canonical_quote(backend), s)
        } else {
            Ident::Plain(s)
        }
    }

    pub fn display(&self, backend: AdapterType) -> IdentDisplay<'_> {
        IdentDisplay(self, backend)
    }

    /// Like [Ident::display], but takes ownership of the identifier, so the result can
    /// outlive the expression that created the [Ident] (e.g. be returned from a closure).
    pub fn into_display(self, backend: AdapterType) -> IdentDisplayOwned {
        IdentDisplayOwned(self, backend)
    }

    /// Converts the identifier to a string losing any quoting information.
    ///
    /// This is bad because quoting governs how an identifier is interpreted
    /// regarding case-sensitivity and allowed characters, so you should worry
    /// every time you use this method. [Ident::display] will render the identifier
    /// with quotes if necessary according to the backend's dialect rules.
    pub(crate) fn to_string_lossy(&self) -> &String {
        match self {
            Ident::Unquoted(_, s) => s,
            Ident::Plain(s) => s,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.as_ref().is_empty()
    }

    /// Compare and hash the identifier by how it renders for `backend`.
    ///
    /// Equality and ordering need the [AdapterType] because quoting and escaping
    /// rules differ between backends, and [Ident] doesn't store one.
    ///
    /// Two [IdentKeyBorrowed]s are equal when [Ident::display] would render the same text,
    /// and they order like those rendered strings, without allocating them.
    ///
    /// XXX: should we try to implement warehouse equivalence instead?
    pub fn key(&self, backend: AdapterType) -> IdentKeyBorrowed<'_> {
        IdentKeyBorrowed(self, backend)
    }

    /// Like [Ident::key], but takes ownership of the identifier, so the key can be
    /// stored, e.g. in a map.
    pub fn into_key(self, backend: AdapterType) -> IdentKey {
        IdentKey(self, backend)
    }

    /// The characters [Ident::display] writes for `backend`: the opening quote,
    /// the escaped identifier, and the closing quote.
    #[inline]
    fn rendered_chars(&self, backend: AdapterType) -> impl Iterator<Item = char> + '_ {
        let (opening, s, closing, quote) = match self {
            Ident::Plain(s) => ("", s.as_str(), "", None),
            Ident::Unquoted(style, s) => {
                let closing = style.closing();
                let quote = closing.chars().next();
                (style.opening(), s.as_str(), closing, quote)
            }
        };
        opening
            .chars()
            .chain(Self::escaped_identifier_chars(s, backend, quote))
            .chain(closing.chars())
    }

    /// The characters of [escaped_identifier], one at a time, without allocating.
    ///
    /// With no `quote`, as for a plain identifier, nothing is escaped.
    #[inline]
    fn escaped_identifier_chars(
        id: &str,
        backend: AdapterType,
        quote: Option<char>,
    ) -> impl Iterator<Item = char> + '_ {
        id.chars().flat_map(move |c| {
            let (first, second) = match quote {
                None => (c, None),
                Some(q) => match backend {
                    AdapterType::Bigquery if c == '\\' => ('\\', Some('\\')),
                    AdapterType::Bigquery if c == q => ('\\', Some(q)),
                    AdapterType::ClickHouse if c == '\\' => ('\\', Some('\\')),
                    _ if c == q => (q, Some(q)),
                    _ => (c, None),
                },
            };
            std::iter::once(first).chain(second)
        })
    }
}

pub struct IdentDisplay<'a>(&'a Ident, AdapterType);

impl fmt::Display for IdentDisplay<'_> {
    #[inline(never)]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use fmt::Write;
        self.0
            .rendered_chars(self.1)
            .try_for_each(|c| f.write_char(c))
    }
}

impl IdentDisplay<'_> {
    #[inline]
    pub fn adapter_type(&self) -> AdapterType {
        self.1
    }
    #[inline]
    pub fn inner(&self) -> &Ident {
        self.0
    }
}

/// An owning counterpart of [IdentDisplay], created by [Ident::into_display].
pub struct IdentDisplayOwned(Ident, AdapterType);

impl fmt::Display for IdentDisplayOwned {
    #[inline]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.display(self.1).fmt(f)
    }
}

impl IdentDisplayOwned {
    #[inline]
    pub fn adapter_type(&self) -> AdapterType {
        self.1
    }
    #[inline]
    pub fn into_inner(self) -> Ident {
        self.0
    }
}

/// An [Ident] bound to an [AdapterType] for comparison and hashing, created by
/// [Ident::key].
///
/// See [IdentKey] for the owned version.
#[derive(Debug, Clone, Copy)]
pub struct IdentKeyBorrowed<'a>(&'a Ident, AdapterType);

impl<'a> IdentKeyBorrowed<'a> {
    /// Clone the identifier into an [IdentKey].
    #[inline]
    pub fn into_owned(self) -> IdentKey {
        IdentKey(self.0.clone(), self.1)
    }

    /// View this key as the type an [IdentKey] lends through [Borrow], to look
    /// up maps and sets keyed by [IdentKey].
    #[inline]
    pub fn as_dyn(&self) -> &(dyn AsIdentKey + 'a) {
        self
    }
}

impl PartialEq for IdentKeyBorrowed<'_> {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for IdentKeyBorrowed<'_> {}

impl PartialOrd for IdentKeyBorrowed<'_> {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IdentKeyBorrowed<'_> {
    #[inline(never)]
    fn cmp(&self, other: &Self) -> Ordering {
        // Comparing chars orders like comparing the UTF-8 rendered strings.
        self.0
            .rendered_chars(self.1)
            .cmp(other.0.rendered_chars(other.1))
    }
}

impl Hash for IdentKeyBorrowed<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let mut buf = [0; 4];
        for c in self.0.rendered_chars(self.1) {
            state.write(c.encode_utf8(&mut buf).as_bytes());
        }
        // Like str's Hash, end with a byte UTF-8 never contains, so a tuple of keys
        // hashes differently from the same text split another way.
        state.write_u8(0xff);
    }
}

/// An owned [IdentKeyBorrowed], created by [Ident::into_key] or
/// [IdentKeyBorrowed::into_owned]. It compares and hashes the same way.
#[derive(Debug, Clone)]
pub struct IdentKey(Ident, AdapterType);

impl IdentKey {
    #[inline]
    pub fn borrowed(&self) -> IdentKeyBorrowed<'_> {
        IdentKeyBorrowed(&self.0, self.1)
    }
}

impl PartialEq for IdentKey {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.borrowed().eq(&other.borrowed())
    }
}

impl Eq for IdentKey {}

impl PartialOrd for IdentKey {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for IdentKey {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.borrowed().cmp(&other.borrowed())
    }
}

impl Hash for IdentKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.borrowed().hash(state)
    }
}

/// The form both [IdentKey] and [IdentKeyBorrowed] can be borrowed as.
pub trait AsIdentKey {
    fn as_ident_key(&self) -> IdentKeyBorrowed<'_>;
}

impl AsIdentKey for IdentKey {
    #[inline]
    fn as_ident_key(&self) -> IdentKeyBorrowed<'_> {
        self.borrowed()
    }
}

impl AsIdentKey for IdentKeyBorrowed<'_> {
    #[inline]
    fn as_ident_key(&self) -> IdentKeyBorrowed<'_> {
        *self
    }
}

impl<'a> Borrow<dyn AsIdentKey + 'a> for IdentKey {
    #[inline]
    fn borrow(&self) -> &(dyn AsIdentKey + 'a) {
        self
    }
}

impl PartialEq for dyn AsIdentKey + '_ {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.as_ident_key() == other.as_ident_key()
    }
}

impl Eq for dyn AsIdentKey + '_ {}

impl PartialOrd for dyn AsIdentKey + '_ {
    #[inline]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for dyn AsIdentKey + '_ {
    #[inline]
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_ident_key().cmp(&other.as_ident_key())
    }
}

impl Hash for dyn AsIdentKey + '_ {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_ident_key().hash(state)
    }
}

pub fn max_identifier_length(adapter_type: AdapterType) -> Option<NonZero<usize>> {
    use AdapterType::*;
    match adapter_type {
        Postgres => {
            // SAFETY: literal 63 is never 0
            Some(unsafe { NonZero::new_unchecked(63) })
        }
        Redshift => {
            // SAFETY: literal 127 is never 0
            Some(unsafe { NonZero::new_unchecked(127) })
        }
        Snowflake | Bigquery | Databricks | Spark | DuckDB | Salesforce | Fabric | ClickHouse
        | Exasol | Athena | Starburst | Trino | Datafusion | Dremio | Oracle | LakeCompute
        | MotherDuck => None,
    }
}

// TODO: implement a separate struct Idents that can be used as (BTree|Hash)(Map|Set) keys
// (a separate struct that binds the backend is needed because comparing Idents is backend-dependent)
//
// ## PostgreSQL
//
// The identifiers FOO, foo, and "foo" are considered the same by PostgreSQL, but "Foo" and "FOO"
// are different from these three and each other.
//
// https://www.postgresql.org/docs/current/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS

/// The canonical quoting style used to quote identifiers in this backend's dialect.
///
/// Always inline because the backend is often known at compile and
/// constant propagation eliminates the match entirely.
#[inline(always)]
pub const fn canonical_quote(backend: AdapterType) -> QuotingStyle {
    use AdapterType::*;
    match backend {
        Bigquery | Databricks | Spark | Athena => QuotingStyle::Backtick,
        ClickHouse | Exasol | Snowflake | Redshift | Postgres | Salesforce | DuckDB
        | LakeCompute | MotherDuck => QuotingStyle::Double,
        // https://learn.microsoft.com/en-us/sql/t-sql/statements/set-quoted-identifier-transact-sql?view=sql-server-ver17
        Fabric => QuotingStyle::Double,
        // Fabric => QuotingStyle::Bracketed,
        _ => QuotingStyle::Double,
    }
}

/// Returns true if the given character is a valid character for an
/// unquoted identifier in this backend's dialect.
pub fn is_valid_ident_char(c: char, backend: AdapterType) -> bool {
    use AdapterType::*;
    match backend {
        Bigquery => c.is_alphanumeric() || ['_', '-', '$'].contains(&c),
        Snowflake => {
            // TODO: revert this once
            // https://github.com/sdf-labs/sdf/issues/3328 is fixed:
            // c.is_alphanumeric() || ['_', '`', '@'].contains(&c)
            c != '.' && c != quote_char(backend) && !c.is_whitespace() && c != '/' && c != ';'
        }
        Athena // TODO: check these fallbacks against documentation of these dialects
            | Postgres
            | Databricks
            | Spark
            | Redshift
            | Salesforce
            | DuckDB
            | Fabric
            | ClickHouse
            | Starburst
            | Trino
            | Datafusion
            | Dremio
            | Oracle
            | LakeCompute
            | MotherDuck
            | Exasol => c.is_alphanumeric() || c == '_',
    }
}

/// Returns true iff the identifier absolutely MUST be quoted when formatting
/// to source code form in this backend's dialect.
///
/// For instance, if an column name contains a `-` character, it must be quoted,
/// because `-` is not a valid character in an unquoted identifier. It would be
/// a syntax error to write:
///
///     SELECT my-column FROM tbl;
///
/// But in PostgreSQL, the query above is valid if the identifier is quoted:
///
///    SELECT "my-column" FROM tbl;
///
/// IMPORTANT: an identifier can't represent user intention to quote or not quote.
/// If the user wrote `"MyTable"` (with quotes) in the input source, we should carry
/// that intention in a `Indent::Unquoted(Double, "MyTable")` value, even though
/// `must_be_quoted("MyTable", _)` returns false for all backends.
pub fn must_be_quoted(id: &str, backend: AdapterType) -> bool {
    let mut chars = id.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return true, // Empty identifiers MUST be quoted
    };

    // If the first character is not [_a-zA-Z], the identifier MUST be quoted
    if first != '_' && !first.is_ascii_alphabetic() {
        return true;
    }

    // Look for invalid characters on the rest of the identifier.
    // The first character is already checked above.
    let has_invalid_char = chars.any(|c| {
        !is_valid_ident_char(c, backend)
            // BigQuery allows hyphens in unquoted identifiers in certain
            // contexts (e.g. table names), but we still quote them here.
            // Snowflake requires hyphenated identifiers to be quoted.
            || (matches!(backend, AdapterType::Bigquery | AdapterType::Snowflake) && c == '-')
    });
    // Invalid characters MUST be in a quoted identifier (sometimes escaped)
    if has_invalid_char {
        return true;
    }

    // Reserved keywords MUST to be quoted to avoid confusing the lexer/parser
    is_keyword_ignore_ascii_case(id, backend).is_some()
}

/// Reduce `name` to a bare, injection-safe identifier for `backend`, dropping any
/// character the backend won't accept unquoted.
///
/// DuckDB: keep only ASCII alphanumerics and `_` — used for ATTACH aliases so the
/// routing database name matches the attached alias exactly, and so a catalog/alias
/// name can never break out of a quoted identifier or inject SQL. We filter over
/// `bytes()`: the continuation bytes of a multi-byte UTF-8 scalar always have their
/// high bit set, so `is_ascii_alphanumeric` rejects them and no interior byte leaks
/// through. Other backends currently pass the name through unchanged.
pub fn sanitize_identifier(name: &str, backend: AdapterType) -> String {
    match backend {
        AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::MotherDuck => name
            .bytes()
            .filter(|&b| b.is_ascii_alphanumeric() || b == b'_')
            .map(char::from)
            .collect(),
        _ => name.to_string(),
    }
}

/// Escape all occurrences of `quote` in `s` with the backend's escape sequence for that quote.
///
/// No wrapping quotes are added. `\` might be duplicated depending on the backend's escaping
/// rules.
///
/// Always inline to avoid passing [Cow] over the stack where possible.
#[inline(never)]
pub fn escaped_identifier<'a>(id: Cow<'a, str>, backend: AdapterType, quote: char) -> Cow<'a, str> {
    let escaped = || Ident::escaped_identifier_chars(&id, backend, Some(quote));
    // Keep `id` when there is nothing to escape.
    if escaped().eq(id.chars()) {
        id
    } else {
        Cow::Owned(escaped().collect())
    }
}

/// Define an `Ident` newtype over [Ident] that is bound to one [AdapterType],
/// so callers don't pass the backend to every constructor and when formatting.
///
/// Expands to items in the invoking module, e.g. `ident::bigquery::Ident`:
///
/// ```ignore
/// define_adapter_ident!(AdapterType::Bigquery);
/// ```
macro_rules! define_adapter_ident {
    ($backend:expr) => {
        /// [Ident](super::Ident) bound to a single backend.
        #[derive(Debug, Clone)]
        pub struct Ident($crate::ident::Ident);

        impl AsRef<str> for Ident {
            #[inline]
            fn as_ref(&self) -> &str {
                self.0.as_ref()
            }
        }

        impl Ident {
            #[inline]
            pub fn new(s: impl Into<String>) -> Self {
                Self($crate::ident::Ident::new(s, $backend))
            }

            #[inline]
            pub fn from_unquoted(s: impl Into<String>) -> Self {
                Self($crate::ident::Ident::from_unquoted(s, $backend))
            }

            #[inline]
            pub fn plain(s: impl Into<String>) -> Self {
                Self($crate::ident::Ident::plain(s))
            }

            #[inline]
            pub fn from_string(s: impl Into<String>, must_quote: bool) -> Self {
                Self($crate::ident::Ident::from_string(s, must_quote, $backend))
            }

            pub fn into_inner(self) -> $crate::ident::Ident {
                self.0
            }

            #[inline]
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl ::std::fmt::Display for Ident {
            #[inline]
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
                ::std::fmt::Display::fmt(&self.0.display($backend), f)
            }
        }
    };
}

pub(crate) use define_adapter_ident;

#[cfg(test)]
mod sanitize_tests {
    use super::*;
    use strum::IntoEnumIterator;

    const P: Option<QuotingStyle> = None;
    const S: Option<QuotingStyle> = Some(QuotingStyle::Single);
    const D: Option<QuotingStyle> = Some(QuotingStyle::Double);
    const B: Option<QuotingStyle> = Some(QuotingStyle::Backtick);
    const U: Option<QuotingStyle> = Some(QuotingStyle::UAndDouble);

    /// Identifiers as (quoting style, name), where `P` means [Ident::Plain].
    #[rustfmt::skip]
    const KEY_IDENTS: &[(Option<QuotingStyle>, &str)] = &[
        (P, ""),     (S, ""),     (D, ""),     (B, ""),     (U, ""),
        (P, "a"),    (S, "a"),    (D, "a"),    (B, "a"),    (U, "a"),
        (P, "A"),    (S, "A"),    (D, "A"),    (B, "A"),    (U, "A"),
        (P, "z"),    (S, "z"),    (D, "z"),    (B, "z"),    (U, "z"),
        (P, "a\"b"), (S, "a\"b"), (D, "a\"b"), (B, "a\"b"), (U, "a\"b"),
        (P, "a`b"),  (S, "a`b"),  (D, "a`b"),  (B, "a`b"),  (U, "a`b"),
        (P, "a'b"),  (S, "a'b"),  (D, "a'b"),  (B, "a'b"),  (U, "a'b"),
        (P, "a\\b"), (S, "a\\b"), (D, "a\\b"), (B, "a\\b"), (U, "a\\b"),
        (P, "日本"), (S, "日本"), (D, "日本"), (B, "日本"), (U, "日本"),
    ];

    fn key_ident(&(style, name): &(Option<QuotingStyle>, &str)) -> Ident {
        match style {
            None => Ident::plain(name),
            Some(style) => Ident::unquoted(style, name),
        }
    }

    #[test]
    fn key_compares_like_rendered_strings() {
        use std::hash::BuildHasher;
        let hasher = std::collections::hash_map::RandomState::new();
        for backend in AdapterType::iter() {
            for a in KEY_IDENTS.iter().map(key_ident) {
                let rendered = a.display(backend).to_string();
                let ctx = format!("{backend:?} {a:?}");
                assert_eq!(
                    a.rendered_chars(backend).collect::<String>(),
                    rendered,
                    "{ctx}"
                );
                for b in KEY_IDENTS.iter().map(key_ident) {
                    let other = b.display(backend).to_string();
                    let (ka, kb) = (a.key(backend), b.key(backend));
                    assert_eq!(ka.cmp(&kb), rendered.cmp(&other), "{ctx} {b:?}");
                    assert_eq!(ka == kb, rendered == other, "{ctx} {b:?}");
                    if ka == kb {
                        assert_eq!(hasher.hash_one(ka), hasher.hash_one(kb), "{ctx} {b:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn ident_key_maps_can_be_queried_with_borrowed_keys() {
        use std::collections::{BTreeMap, HashMap};
        let at = AdapterType::Snowflake;
        let mut hash_map = HashMap::new();
        let mut btree_map = BTreeMap::new();
        for (i, row) in KEY_IDENTS.iter().enumerate() {
            let key = key_ident(row).into_key(at);
            hash_map.insert(key.clone(), i);
            btree_map.insert(key, i);
        }
        for row in KEY_IDENTS {
            let ident = key_ident(row);
            let borrowed = ident.key(at);
            let owned = ident.clone().into_key(at);
            assert_eq!(
                hash_map.get(borrowed.as_dyn()),
                hash_map.get(&owned),
                "{ident:?}"
            );
            assert_eq!(
                btree_map.get(borrowed.as_dyn()),
                btree_map.get(&owned),
                "{ident:?}"
            );
            assert!(hash_map.contains_key(borrowed.as_dyn()), "{ident:?}");
        }
        let missing = Ident::plain("missing");
        assert_eq!(hash_map.get(missing.key(at).as_dyn()), None);
        assert_eq!(btree_map.get(missing.key(at).as_dyn()), None);
    }

    #[test]
    fn snowflake_hyphen_must_be_quoted() {
        assert!(must_be_quoted("GIFT-CARDS", AdapterType::Snowflake));
        assert!(!must_be_quoted("GIFT_CARDS", AdapterType::Snowflake));
    }

    #[test]
    fn from_unquoted_always_quotes_and_escapes() {
        let id = Ident::from_unquoted(r"a\b`c", AdapterType::Bigquery);
        assert_eq!(id.display(AdapterType::Bigquery).to_string(), r"`a\\b\`c`");
    }

    #[test]
    fn from_unquoted_always_quotes_and_escapes_per_backend() {
        let quoted = |id: &str, backend: AdapterType| {
            Ident::from_unquoted(id, backend)
                .display(backend)
                .to_string()
        };
        assert_eq!(quoted("orders", AdapterType::DuckDB), "\"orders\"");
        assert_eq!(quoted("od\"d", AdapterType::DuckDB), "\"od\"\"d\"");
        assert_eq!(quoted("col", AdapterType::Bigquery), "`col`");
        assert_eq!(
            quoted(r"odd\`name", AdapterType::Bigquery),
            r"`odd\\\`name`"
        );
        assert_eq!(
            quoted(r#"we\"ird"#, AdapterType::ClickHouse),
            r#""we\\""ird""#
        );
        assert_eq!(quoted("trail\\", AdapterType::ClickHouse), r#""trail\\""#);
    }

    #[test]
    fn duckdb_sanitize_strips_sql_metacharacters() {
        let s = |n: &str| sanitize_identifier(n, AdapterType::DuckDB);
        assert_eq!(s("foo\";DROP TABLE bar;--"), "fooDROPTABLEbar");
        assert_eq!(s("my.catalog.name"), "mycatalogname");
        assert_eq!(s("'); DROP"), "DROP");
        assert_eq!(s("a b\tc\n"), "abc");
        assert_eq!(s("under_score123"), "under_score123");
        // Multi-byte Unicode is dropped entirely (no stray continuation bytes).
        assert_eq!(s("café—x"), "cafx");
    }

    #[test]
    fn motherduck_sanitize_matches_duckdb() {
        let s = |n: &str| sanitize_identifier(n, AdapterType::MotherDuck);
        assert_eq!(s("foo\";DROP TABLE bar;--"), "fooDROPTABLEbar");
        assert_eq!(s("my.catalog.name"), "mycatalogname");
        assert_eq!(s("café—x"), "cafx");
    }
}

#[cfg(test)]
mod quote_ident_tests {
    use super::*;

    pub fn quote_ident(adapter_type: AdapterType, identifier: &str) -> String {
        let quote = quote_char(adapter_type);
        let escaped = escaped_identifier(identifier.into(), adapter_type, quote);
        format!("{quote}{escaped}{quote}")
    }

    const IDENTIFIERS: &[&str] = &[
        "",
        "a",
        "A",
        "abc",
        "ABC",
        "MixedCase",
        "snake_case",
        "_leading_underscore",
        "trailing_underscore_",
        "with1digits",
        "1leading_digit",
        "123",
        "with space",
        " leading_space",
        "trailing_space ",
        "  ",
        "tab\there",
        "new\nline",
        "carriage\rreturn",
        "hyphen-ated",
        "GIFT-CARDS",
        "dot.ted",
        "my.catalog.name",
        "dollar$sign",
        "at@sign",
        "hash#tag",
        "percent%",
        "star*",
        "plus+minus-",
        "slash/and\\back",
        "colon:semi;",
        "comma,here",
        "paren(thesis)",
        "brackets[x]",
        "braces{x}",
        "angle<brackets>",
        "question?",
        "bang!",
        "tilde~",
        "caret^",
        "pipe|",
        "ampersand&",
        "equals=",
        "\"",
        "\"\"",
        "\"\"\"",
        "a\"b",
        "\"leading",
        "trailing\"",
        "\"wrapped\"",
        "a\"\"b",
        "`",
        "``",
        "```",
        "a`b",
        "`leading",
        "trailing`",
        "`wrapped`",
        "a``b",
        "'",
        "''",
        "a'b",
        "'wrapped'",
        "\\",
        "\\\\",
        "\\\\\\",
        "a\\b",
        "\\leading",
        "trailing\\",
        "a\\\\b",
        "\\\"",
        "\\`",
        "\\'",
        "\"\\",
        "`\\",
        "\\\"\\\"",
        "\\`\\`",
        "mix\"of`quotes'and\\backslashes",
        "foo\";DROP TABLE bar;--",
        "foo`;DROP TABLE bar;--",
        "'); DROP",
        "Robert'); DROP TABLE Students;--",
        "select",
        "SELECT",
        "from",
        "table",
        "order",
        "group",
        "null",
        "true",
        "café",
        "café—x",
        "日本語",
        "naïve",
        "ñandú",
        "emoji😀name",
        "😀",
        "ünï\"cödé`",
        "zero\u{200b}width",
        "nul\0byte",
        "very_long_identifier_name_that_goes_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_and_on_end",
    ];

    fn assert_equivalent(adapter_type: AdapterType, identifier: &str) {
        assert_eq!(
            quote_ident(adapter_type, identifier),
            Ident::from_unquoted(identifier, adapter_type)
                .display(adapter_type)
                .to_string(),
            "adapter_type={adapter_type:?} identifier={identifier:?}",
        );
    }

    /// Adapters where both functions are expected to agree.
    ///
    /// Left out on purpose:
    /// - `Dremio` and `Oracle`: `quote_char` is `todo!()` and panics.
    /// - `ClickHouse` and `Athena`: `quote_char` and `canonical_quote` disagree
    ///   about the quote character (see the ignored tests below).
    const COMPARABLE_ADAPTERS: &[AdapterType] = &[
        AdapterType::Snowflake,
        AdapterType::Bigquery,
        AdapterType::Databricks,
        AdapterType::Redshift,
        AdapterType::Spark,
        AdapterType::DuckDB,
        AdapterType::Postgres,
        AdapterType::Salesforce,
        AdapterType::Fabric,
        AdapterType::Exasol,
        AdapterType::Starburst,
        AdapterType::Trino,
        AdapterType::Datafusion,
        AdapterType::LakeCompute,
        AdapterType::MotherDuck,
    ];

    #[test]
    fn quote_ident_matches_from_unquoted_for_all_adapters_and_identifiers() {
        for &adapter_type in COMPARABLE_ADAPTERS {
            for identifier in IDENTIFIERS {
                assert_equivalent(adapter_type, identifier);
            }
        }
    }

    #[test]
    fn comparable_adapters_cover_every_adapter_but_the_known_exceptions() {
        let excluded = [
            AdapterType::Dremio,
            AdapterType::Oracle,
            AdapterType::ClickHouse,
            AdapterType::Athena,
        ];
        for (adapter_type, _) in AdapterType::iter_with_names() {
            assert!(
                COMPARABLE_ADAPTERS.contains(&adapter_type) != excluded.contains(&adapter_type),
                "{adapter_type:?} must be in exactly one of COMPARABLE_ADAPTERS or the exclusions",
            );
        }
    }

    macro_rules! equivalence_tests {
        ($($name:ident => $adapter_type:expr),* $(,)?) => {
            $(
                #[test]
                fn $name() {
                    for identifier in IDENTIFIERS {
                        assert_equivalent($adapter_type, identifier);
                    }
                }
            )*
        };
    }

    equivalence_tests! {
        quote_ident_matches_snowflake => AdapterType::Snowflake,
        quote_ident_matches_bigquery => AdapterType::Bigquery,
        quote_ident_matches_databricks => AdapterType::Databricks,
        quote_ident_matches_redshift => AdapterType::Redshift,
        quote_ident_matches_spark => AdapterType::Spark,
        quote_ident_matches_duckdb => AdapterType::DuckDB,
        quote_ident_matches_postgres => AdapterType::Postgres,
        quote_ident_matches_salesforce => AdapterType::Salesforce,
        quote_ident_matches_fabric => AdapterType::Fabric,
        quote_ident_matches_exasol => AdapterType::Exasol,
        quote_ident_matches_starburst => AdapterType::Starburst,
        quote_ident_matches_trino => AdapterType::Trino,
        quote_ident_matches_datafusion => AdapterType::Datafusion,
        quote_ident_matches_lakecompute => AdapterType::LakeCompute,
        quote_ident_matches_motherduck => AdapterType::MotherDuck,
    }

    #[test]
    #[ignore = "quote_char(ClickHouse) is a backtick but canonical_quote(ClickHouse) is a double quote"]
    fn quote_ident_matches_clickhouse() {
        for identifier in IDENTIFIERS {
            assert_equivalent(AdapterType::ClickHouse, identifier);
        }
    }

    #[test]
    #[ignore = "quote_char(Athena) is a double quote but canonical_quote(Athena) is a backtick"]
    fn quote_ident_matches_athena() {
        for identifier in IDENTIFIERS {
            assert_equivalent(AdapterType::Athena, identifier);
        }
    }

    #[test]
    fn quote_ident_wraps_in_the_adapter_quote_char() {
        for &adapter_type in COMPARABLE_ADAPTERS
            .iter()
            .chain(&[AdapterType::ClickHouse, AdapterType::Athena])
        {
            let quote = quote_char(adapter_type);
            let quoted = quote_ident(adapter_type, "plain");
            assert_eq!(quoted, format!("{quote}plain{quote}"));
        }
    }
}
