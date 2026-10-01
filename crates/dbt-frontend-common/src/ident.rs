use crate::dialect::Dialect;
use crate::error::InternalError;
use crate::utils::{get_version_hash, strip_version_hash};
use crate::{internal_err, make_internal_err};
use datafusion_common::{ResolvedTableReference, TableReference};
use itertools::Itertools;
use serde::{Deserialize, Deserializer, Serialize, de};
use std::path::PathBuf;

pub use dbt_ident::{Ident, Identifier};
/// Owned version of [Qualified].
pub type QualifiedName = Qualified<'static>;

pub trait IdentJoin<Separator> {
    type Output;

    fn join(&self, separator: Separator) -> Self::Output
    where
        Separator: AsRef<str>;
}

impl<Separator, V> IdentJoin<Separator> for V
where
    V: std::borrow::Borrow<[Ident<'static>]>,
{
    type Output = String;

    fn join(&self, separator: Separator) -> Self::Output
    where
        Separator: AsRef<str>,
    {
        self.borrow()
            .iter()
            .map(|ident| ident.name())
            .join(separator.as_ref())
    }
}

/// A wrapper type around a [TableReference] that implements case-insensitive
/// semantics.
#[derive(Clone, PartialEq, Eq, Hash)]
pub enum Qualified<'a> {
    Bare {
        table: Ident<'a>,
    },
    Partial {
        schema: Ident<'a>,
        table: Ident<'a>,
    },
    Full {
        catalog: Ident<'a>,
        schema: Ident<'a>,
        table: Ident<'a>,
    },
}

impl std::fmt::Display for Qualified<'_> {
    /// NOTE: this is a naive dot-separated format for display purposes *only*!
    /// This form is **NOT** round-trippable. For a fully information-preserving
    /// string format of a [Qualified], you must use [Self::format_as] with a
    /// specific [Dialect].
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Qualified::Bare { table } => write!(f, "{table}"),
            Qualified::Partial { schema, table } => write!(f, "{schema}.{table}"),
            Qualified::Full {
                catalog,
                schema,
                table,
            } => write!(f, "{catalog}.{schema}.{table}"),
        }
    }
}

impl std::fmt::Debug for Qualified<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Qualified::Bare { table } => write!(f, "{table:?}"),
            Qualified::Partial { schema, table } => write!(f, "{schema:?}.{table:?}"),
            Qualified::Full {
                catalog,
                schema,
                table,
            } => write!(f, "{catalog:?}.{schema:?}.{table:?}"),
        }
    }
}

impl Default for Qualified<'static> {
    fn default() -> Self {
        Qualified::Bare {
            table: Ident::default(),
        }
    }
}

impl PartialOrd for Qualified<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Qualified<'_> {
    fn cmp<'a>(&'a self, other: &'a Self) -> std::cmp::Ordering {
        // Compare them as if tuples of (catalog, schema, table)
        let cmp_value =
            |qualified: &'a Self| -> (Option<&'a Ident<'_>>, Option<&'a Ident<'_>>, &'a Ident<'_>) {
                match qualified {
                    Qualified::Full {
                        catalog,
                        schema,
                        table,
                    } => (Some(catalog), Some(schema), table),
                    Qualified::Partial { schema, table } => (None, Some(schema), table),
                    Qualified::Bare { table } => (None, None, table),
                }
            };

        let self_tuple = cmp_value(self);
        let other_tuple = cmp_value(other);
        self_tuple.cmp(&other_tuple)
    }
}

impl<'de> Deserialize<'de> for Qualified<'static> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let dialect = infer_dialect_for_deserialize(&value);
        dialect
            .parse_qualified_name(&value)
            .map_err(|e| de::Error::custom(e.to_string()))
    }
}

impl<'a> Qualified<'a> {
    pub fn bare(table: impl Into<Ident<'a>>) -> Self {
        Qualified::Bare {
            table: table.into(),
        }
    }

    pub fn partial(schema: impl Into<Ident<'a>>, table: impl Into<Ident<'a>>) -> Self {
        Qualified::Partial {
            schema: schema.into(),
            table: table.into(),
        }
    }

    pub fn full(
        catalog: impl Into<Ident<'a>>,
        schema: impl Into<Ident<'a>>,
        table: impl Into<Ident<'a>>,
    ) -> Self {
        Qualified::Full {
            catalog: catalog.into(),
            schema: schema.into(),
            table: table.into(),
        }
    }

    pub fn catalog(&self) -> Option<&Ident<'a>> {
        match &self {
            Qualified::Full { catalog, .. } => Some(catalog),
            _ => None,
        }
    }

    pub fn schema(&self) -> Option<&Ident<'a>> {
        match &self {
            Qualified::Full { schema, .. } | Qualified::Partial { schema, .. } => Some(schema),
            _ => None,
        }
    }

    pub fn table(&self) -> &Ident<'a> {
        match &self {
            Qualified::Full { table, .. }
            | Qualified::Partial { table, .. }
            | Qualified::Bare { table } => table,
        }
    }

    pub fn resolve<'b, 'c>(
        &self,
        default_catalog: impl Into<Ident<'b>>,
        default_schema: impl Into<Ident<'c>>,
    ) -> FullyQualifiedName {
        FullyQualifiedName {
            catalog: self
                .catalog()
                .map_or_else(|| default_catalog.into().to_owned(), |c| c.to_owned()),
            schema: self
                .schema()
                .map_or_else(|| default_schema.into().to_owned(), |s| s.to_owned()),
            table: self.table().to_owned(),
        }
    }

    pub fn to_owned(&self) -> Qualified<'static> {
        match self {
            Qualified::Bare { table } => Qualified::Bare {
                table: table.to_owned(),
            },
            Qualified::Partial { schema, table } => Qualified::Partial {
                schema: schema.to_owned(),
                table: table.to_owned(),
            },
            Qualified::Full {
                catalog,
                schema,
                table,
            } => Qualified::Full {
                catalog: catalog.to_owned(),
                schema: schema.to_owned(),
                table: table.to_owned(),
            },
        }
    }

    pub fn into_table_ref(self) -> TableReference {
        match self {
            Qualified::Bare { table } => TableReference::Bare {
                table: table.into_inner(),
            },
            Qualified::Partial { schema, table } => TableReference::Partial {
                schema: schema.into_inner(),
                table: table.into_inner(),
            },
            Qualified::Full {
                catalog,
                schema,
                table,
            } => TableReference::Full {
                catalog: catalog.into_inner(),
                schema: schema.into_inner(),
                table: table.into_inner(),
            },
        }
    }

    pub fn matches(&self, target: &Qualified) -> bool {
        self.table() == target.table()
            && self
                .schema()
                .is_none_or(|s| target.schema().is_some_and(|t| s == t))
            && self
                .catalog()
                .is_none_or(|c| target.catalog().is_some_and(|t| c == t))
    }

    pub fn matches_fqn(&self, target: &FullyQualifiedName) -> bool {
        self.table() == target.table()
            && self.schema().is_none_or(|s| s == target.schema())
            && self.catalog().is_none_or(|c| c == target.catalog())
    }

    /// Case sensitive comparison
    pub fn matches_exact(&self, target: &Qualified) -> bool {
        self.table().matches_exact(target.table())
            && self
                .schema()
                .is_none_or(|s| target.schema().is_some_and(|t| s.matches_exact(t)))
            && self
                .catalog()
                .is_none_or(|c| target.catalog().is_some_and(|t| c.matches_exact(t)))
    }

    pub fn parse(sql: &str, dialect: impl Into<Dialect>) -> Result<Self, Box<InternalError>> {
        dialect.into().parse_qualified_name(sql)
    }

    /// Transform this name by applying a function to each component of the
    /// qualified name, returning the result as a new qualified name.
    pub fn map<F>(self, mut f: F) -> Qualified<'a>
    where
        F: FnMut(Ident<'a>) -> Ident<'a>,
    {
        match self {
            Qualified::Bare { table } => Qualified::Bare { table: f(table) },
            Qualified::Partial { schema, table } => Qualified::Partial {
                schema: f(schema),
                table: f(table),
            },
            Qualified::Full {
                catalog,
                schema,
                table,
            } => Qualified::Full {
                catalog: f(catalog),
                schema: f(schema),
                table: f(table),
            },
        }
    }
}

impl From<TableReference> for Qualified<'static> {
    fn from(value: TableReference) -> Self {
        match value {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => Qualified::Full {
                catalog: catalog.into(),
                schema: schema.into(),
                table: table.into(),
            },
            TableReference::Partial { schema, table } => Qualified::Partial {
                schema: schema.into(),
                table: table.into(),
            },
            TableReference::Bare { table } => Qualified::Bare {
                table: table.into(),
            },
        }
    }
}

impl<'a> From<&'a TableReference> for Qualified<'a> {
    fn from(value: &'a TableReference) -> Self {
        match value {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => Qualified::Full {
                catalog: catalog.as_ref().into(),
                schema: schema.as_ref().into(),
                table: table.as_ref().into(),
            },
            TableReference::Partial { schema, table } => Qualified::Partial {
                schema: schema.as_ref().into(),
                table: table.as_ref().into(),
            },
            TableReference::Bare { table } => Qualified::Bare {
                table: table.as_ref().into(),
            },
        }
    }
}

impl<'a> From<Ident<'a>> for Qualified<'a> {
    fn from(value: Ident<'a>) -> Self {
        Qualified::Bare { table: value }
    }
}

impl<'a> From<(Ident<'a>, Ident<'a>)> for Qualified<'a> {
    fn from(value: (Ident<'a>, Ident<'a>)) -> Self {
        Qualified::Partial {
            schema: value.0,
            table: value.1,
        }
    }
}

impl<'a> From<(Ident<'a>, Ident<'a>, Ident<'a>)> for Qualified<'a> {
    fn from(value: (Ident<'a>, Ident<'a>, Ident<'a>)) -> Self {
        Qualified::Full {
            catalog: value.0,
            schema: value.1,
            table: value.2,
        }
    }
}

impl<'a> TryFrom<Vec<Ident<'a>>> for Qualified<'a> {
    type Error = Box<InternalError>;

    fn try_from(value: Vec<Ident<'a>>) -> Result<Self, Self::Error> {
        match value.len() {
            1 => Ok(Qualified::Bare {
                table: value.into_iter().next().unwrap(),
            }),
            2 => {
                let mut iter = value.into_iter();
                Ok(Qualified::Partial {
                    schema: iter.next().unwrap(),
                    table: iter.next().unwrap(),
                })
            }
            3 => {
                let mut iter = value.into_iter();
                Ok(Qualified::Full {
                    catalog: iter.next().unwrap(),
                    schema: iter.next().unwrap(),
                    table: iter.next().unwrap(),
                })
            }
            _ => {
                internal_err!("Invalid number of identifiers: {:?}", value)
            }
        }
    }
}

impl<'a> From<Qualified<'a>> for TableReference {
    fn from(value: Qualified<'a>) -> Self {
        value.into_table_ref()
    }
}

#[derive(Clone, Default, Eq, Serialize)]
pub struct FullyQualifiedName {
    pub catalog: Identifier,
    pub schema: Identifier,
    pub table: Identifier,
}

impl Ord for FullyQualifiedName {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.catalog.cmp(&other.catalog).then_with(|| {
            self.schema
                .cmp(&other.schema)
                .then_with(|| self.table.cmp(&other.table))
        })
    }
}

impl PartialOrd for FullyQualifiedName {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for FullyQualifiedName {
    fn eq(&self, other: &Self) -> bool {
        self.catalog == other.catalog && self.schema == other.schema && self.table == other.table
    }
}

impl std::hash::Hash for FullyQualifiedName {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.catalog.hash(state);
        self.schema.hash(state);
        self.table.hash(state);
    }
}

impl std::fmt::Display for FullyQualifiedName {
    /// NOTE: this is a naive dot-separated format for display purposes *only*!
    /// This form is **NOT** round-trippable. For a fully information-preserving
    /// string format of a [Qualified], you must either use [Self::format_as]
    /// with a specific [Dialect], or use serde serialization (which calls
    /// [Self::format] under the hood)
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.catalog, self.schema, self.table)
    }
}

impl std::fmt::Debug for FullyQualifiedName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}.{:?}.{:?}", self.catalog, self.schema, self.table)
    }
}

impl<'de> Deserialize<'de> for FullyQualifiedName {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        let dialect = infer_dialect_for_deserialize(&value);
        dialect
            .parse_fqn(&value)
            .map_err(|e| de::Error::custom(e.to_string()))
    }
}

// unfolded constants.rs from sdf.cli
pub const UPPERCASE_DRAFT_SUFFIX: &str = "___DRAFT";
pub const LOWERCASE_DRAFT_SUFFIX: &str = "___draft";
pub const QUOTED_UPPERCASE_DRAFT_SUFFIX: &str = "___DRAFT\"";
pub const QUOTED_LOWERCASE_DRAFT_SUFFIX: &str = "___draft\"";
pub const DRAFT_SUFFIX_LEN: usize = LOWERCASE_DRAFT_SUFFIX.len();
pub const QUOTED_DRAFT_SUFFIX_LEN: usize = QUOTED_LOWERCASE_DRAFT_SUFFIX.len();

impl FullyQualifiedName {
    pub fn new(catalog: impl AsRef<str>, schema: impl AsRef<str>, id: impl AsRef<str>) -> Self {
        FullyQualifiedName {
            catalog: Ident::new(catalog),
            schema: Ident::new(schema),
            table: Ident::new(id),
        }
    }

    pub fn catalog(&self) -> &Identifier {
        &self.catalog
    }

    pub fn schema(&self) -> &Identifier {
        &self.schema
    }

    pub fn table(&self) -> &Identifier {
        &self.table
    }

    pub fn with_catalog(&self, catalog: impl Into<Identifier>) -> Self {
        Self {
            catalog: catalog.into(),
            ..self.clone()
        }
    }

    pub fn with_schema(&self, schema: impl Into<Identifier>) -> Self {
        Self {
            schema: schema.into(),
            ..self.clone()
        }
    }

    pub fn with_table(&self, table: impl Into<Identifier>) -> Self {
        Self {
            table: table.into(),
            ..self.clone()
        }
    }

    pub fn to_path(&self) -> PathBuf {
        [&self.catalog, &self.schema, &self.table].iter().collect()
    }

    pub fn to_draft_path(&self) -> PathBuf {
        let draft_table = format!("{}_draft", self.table());
        FullyQualifiedName::new(&self.catalog, &self.schema, draft_table).to_path()
    }

    pub fn add_draft_suffix(&self, dialect: Dialect) -> FullyQualifiedName {
        let suffix = dialect.draft_suffix();
        let draft_table = format!("{}{}", self.table(), suffix);
        FullyQualifiedName::new(&self.catalog, &self.schema, draft_table)
    }

    pub fn drop_draft_suffix(&self) -> FullyQualifiedName {
        let t = self.table().as_str();
        let base_table =
            if t.ends_with(LOWERCASE_DRAFT_SUFFIX) || t.ends_with(UPPERCASE_DRAFT_SUFFIX) {
                &t[..t.len() - DRAFT_SUFFIX_LEN]
            } else if t.ends_with(QUOTED_LOWERCASE_DRAFT_SUFFIX)
                || t.ends_with(QUOTED_UPPERCASE_DRAFT_SUFFIX)
            {
                &t[..t.len() - QUOTED_DRAFT_SUFFIX_LEN]
            } else {
                t
            };
        FullyQualifiedName::new(&self.catalog, &self.schema, base_table)
    }

    /// Case-sensitive comparison
    pub fn matches_exact(&self, other: &FullyQualifiedName) -> bool {
        self.catalog.matches_exact(&other.catalog)
            && self.schema.matches_exact(&other.schema)
            && self.table.matches_exact(&other.table)
    }

    pub fn parse(sql: &str, dialect: impl Into<Dialect>) -> Result<Self, Box<InternalError>> {
        dialect.into().parse_fqn(sql)
    }

    /// Transform this name by applying a function to each component of the
    /// fully qualified name, returning the result as a new fully qualified name.
    pub fn map<F>(self, mut f: F) -> FullyQualifiedName
    where
        F: FnMut(Ident<'static>) -> Ident<'static>,
    {
        FullyQualifiedName {
            catalog: f(self.catalog),
            schema: f(self.schema),
            table: f(self.table),
        }
    }
}

// LEGACY: Table name versioning support
impl FullyQualifiedName {
    pub const HASH_SIZE: usize = 16;
    pub const VERSION_SIZE: usize = 17;

    pub fn to_unhashed(&self) -> Self {
        let table = self.table();
        let (maybe_version, hash) = self.get_version_hash();
        let unhashed = strip_version_hash(table.as_ref(), &maybe_version, &hash);
        if let Some(version) = maybe_version {
            Self {
                table: format!("{unhashed}_{version}").into(),
                ..self.clone()
            }
        } else {
            Self {
                table: unhashed.into(),
                ..self.clone()
            }
        }
    }

    pub fn get_unversioned(&self) -> Self {
        let (version, hash) = self.get_version_hash();
        let unversioned = strip_version_hash(self.table.as_ref(), &version, &hash);
        Self {
            table: unversioned.into(),
            ..self.clone()
        }
    }

    pub fn get_version_hash(&self) -> (Option<String>, Option<String>) {
        get_version_hash(self.table.as_ref())
    }

    pub fn is_versioned(&self) -> bool {
        let (v, h) = self.get_version_hash();
        v.is_some() || h.is_some()
    }
}

// LEGACY: adapter
impl FullyQualifiedName {
    pub fn try_parse(
        value: &str,
        default_catalog: impl AsRef<str>,
        default_schema: impl AsRef<str>,
        dialect: impl Into<Dialect>,
    ) -> Result<Self, Box<InternalError>> {
        Ok(dialect
            .into()
            .parse_qualified_name(value)?
            .resolve(default_catalog.as_ref(), default_schema.as_ref()))
    }

    pub fn catalog_schema_table(&self) -> (&str, &str, &str) {
        (
            self.catalog.as_ref(),
            self.schema.as_ref(),
            self.table.as_ref(),
        )
    }
}

impl From<(Identifier, Identifier, Identifier)> for FullyQualifiedName {
    fn from(value: (Identifier, Identifier, Identifier)) -> Self {
        FullyQualifiedName {
            catalog: value.0,
            schema: value.1,
            table: value.2,
        }
    }
}

impl From<&FullyQualifiedName> for FullyQualifiedName {
    fn from(value: &FullyQualifiedName) -> Self {
        value.clone()
    }
}

impl From<ResolvedTableReference> for FullyQualifiedName {
    fn from(value: ResolvedTableReference) -> Self {
        FullyQualifiedName::new(value.catalog, value.schema, value.table)
    }
}

impl TryFrom<TableReference> for FullyQualifiedName {
    type Error = Box<InternalError>;

    fn try_from(value: TableReference) -> Result<Self, Self::Error> {
        match value {
            TableReference::Full {
                catalog,
                schema,
                table,
            } => Ok(Self::new(catalog, schema, table)),
            _ => internal_err!(
                "Partial table reference cannot be converted to fully qualified name: {}",
                value
            ),
        }
    }
}

impl TryFrom<&TableReference> for FullyQualifiedName {
    type Error = Box<InternalError>;

    fn try_from(value: &TableReference) -> Result<Self, Self::Error> {
        value.clone().try_into()
    }
}

impl From<FullyQualifiedName> for ResolvedTableReference {
    fn from(value: FullyQualifiedName) -> Self {
        ResolvedTableReference {
            catalog: value.catalog.into_inner(),
            schema: value.schema.into_inner(),
            table: value.table.into_inner(),
        }
    }
}

impl From<FullyQualifiedName> for Qualified<'static> {
    fn from(value: FullyQualifiedName) -> Self {
        Qualified::Full {
            catalog: value.catalog,
            schema: value.schema,
            table: value.table,
        }
    }
}

impl From<FullyQualifiedName> for TableReference {
    fn from(value: FullyQualifiedName) -> Self {
        TableReference::Full {
            catalog: value.catalog.into_inner(),
            schema: value.schema.into_inner(),
            table: value.table.into_inner(),
        }
    }
}

impl TryFrom<Qualified<'_>> for FullyQualifiedName {
    type Error = Box<InternalError>;

    fn try_from(value: Qualified) -> Result<Self, Self::Error> {
        match value {
            Qualified::Full {
                catalog,
                schema,
                table,
            } => Ok(FullyQualifiedName::new(
                catalog.to_owned(),
                schema.to_owned(),
                table.to_owned(),
            )),
            _ => internal_err!(
                "Partial qualifier cannot be converted to fully qualified name: {:?}",
                value
            ),
        }
    }
}

#[derive(Clone, Eq, PartialEq, Hash, PartialOrd, Ord)]
pub struct ColumnRef {
    table_name: FullyQualifiedName,
    column: Identifier,
}

impl std::fmt::Display for ColumnRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.table_name, self.column)
    }
}

impl std::fmt::Debug for ColumnRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}.{:?}", self.table_name, self.column)
    }
}

impl<'de> Deserialize<'de> for ColumnRef {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Dialect::default()
            .parse_column_ref(&value)
            .map_err(|e| de::Error::custom(e.to_string()))
    }
}

impl ColumnRef {
    pub fn new(table_name: impl Into<FullyQualifiedName>, column: impl AsRef<str>) -> Self {
        Self {
            table_name: table_name.into(),
            column: Identifier::new(column),
        }
    }

    pub fn table_name(&self) -> &FullyQualifiedName {
        &self.table_name
    }

    pub fn column(&self) -> &Identifier {
        &self.column
    }

    pub fn parse(sql: &str, dialect: impl Into<Dialect>) -> Result<Self, Box<InternalError>> {
        dialect.into().parse_column_ref(sql)
    }
}

impl ColumnRef {
    pub fn try_parse(
        name: &str,
        default_catalog: impl AsRef<str>,
        default_schema: impl AsRef<str>,
        default_table: impl AsRef<str>,
        dialect: impl Into<Dialect>,
    ) -> Result<Self, Box<InternalError>> {
        let idvec = dialect.into().parse_dot_separated_identifiers(name)?;
        match idvec.len() {
            1 => Ok(Self {
                table_name: FullyQualifiedName::new(default_catalog, default_schema, default_table),
                column: idvec[0].clone(),
            }),
            2 => Ok(Self {
                table_name: FullyQualifiedName::new(
                    default_catalog,
                    default_schema,
                    idvec[0].as_str(),
                ),
                column: idvec[1].clone(),
            }),
            3 => Ok(Self {
                table_name: FullyQualifiedName::new(
                    default_catalog,
                    idvec[0].as_str(),
                    idvec[1].as_str(),
                ),
                column: idvec[2].clone(),
            }),
            4 => Ok(Self {
                table_name: FullyQualifiedName::new(
                    idvec[0].as_str(),
                    idvec[1].as_str(),
                    idvec[2].as_str(),
                ),
                column: idvec[3].clone(),
            }),
            _ => internal_err!("Invalid column reference: {name}"),
        }
    }
}

impl TryFrom<datafusion_common::Column> for ColumnRef {
    type Error = Box<InternalError>;

    fn try_from(value: datafusion_common::Column) -> Result<Self, Self::Error> {
        Ok(Self {
            table_name: value
                .relation
                .ok_or_else(|| make_internal_err!("Invalid column ref: missing relation"))?
                .try_into()?,
            column: value.name.into(),
        })
    }
}

impl TryFrom<Vec<Identifier>> for ColumnRef {
    type Error = Box<InternalError>;

    fn try_from(value: Vec<Identifier>) -> Result<Self, Self::Error> {
        if value.len() != 4 {
            return internal_err!("Invalid column reference: {:?}", value);
        }
        let mut iter = value.into_iter();
        Ok(Self {
            table_name: FullyQualifiedName::from((
                iter.next().unwrap(),
                iter.next().unwrap(),
                iter.next().unwrap(),
            )),
            column: iter.next().unwrap(),
        })
    }
}

pub trait NamedItemCollection {
    type Item;

    fn find_matching<'a>(&self, name: &'a Ident<'a>) -> Option<(usize, &Self::Item)>;
}

impl NamedItemCollection for arrow_schema::Fields {
    type Item = arrow_schema::FieldRef;

    fn find_matching<'a>(&self, name: &'a Ident<'a>) -> Option<(usize, &Self::Item)> {
        self.iter()
            .enumerate()
            .find(|(_, field)| name.matches(field.name()))
    }
}

fn infer_dialect_for_deserialize(value: &str) -> Dialect {
    let dot_count = value.matches('.').count();

    // Check for 4-part identifier (BigQuery pattern)
    if dot_count == 3 {
        return Dialect::Bigquery;
    }

    // Check for region- pattern specifically in context of INFORMATION_SCHEMA
    if value.contains("region-") && value.to_uppercase().contains("INFORMATION_SCHEMA") {
        return Dialect::Bigquery;
    }

    Dialect::default()
}
