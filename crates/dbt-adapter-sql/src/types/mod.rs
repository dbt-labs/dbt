use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::num::ParseIntError;
use std::sync::Arc;

use arrow_schema::{DataType, Field, Fields, IntervalUnit, TimeUnit};
use dbt_adapter_core::AdapterType;

use super::ident::Ident;
use super::tokenizer::{Token, Tokenizer};

#[cfg(test)]
mod tests;

#[derive(Debug, Copy, Clone)]
pub enum DateTimeField {
    Year,
    Month,
    Day,
    Hour,
    Minute,
    Second,
    Millisecond,
    Microsecond,
    Nanosecond,
}

impl fmt::Display for DateTimeField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use DateTimeField::*;
        match self {
            Year => write!(f, "YEAR"),
            Month => write!(f, "MONTH"),
            Day => write!(f, "DAY"),
            Hour => write!(f, "HOUR"),
            Minute => write!(f, "MINUTE"),
            Second => write!(f, "SECOND"),
            Millisecond => write!(f, "MILLISECOND"),
            Microsecond => write!(f, "MICROSECOND"),
            Nanosecond => write!(f, "NANOSECOND"),
        }
    }
}

impl DateTimeField {
    fn write(&self, backend: AdapterType, out: &mut String) -> fmt::Result {
        use AdapterType::*;
        use DateTimeField::*;
        use fmt::Write as _;
        // In PostgreSQL, the sub-second fields are expressed as
        // `SECOND` followed by a precision, e.g. `SECOND(3)`.
        if matches!(backend, Postgres | Redshift) {
            match self {
                Millisecond => {
                    out.push_str("SECOND(3)");
                    Ok(())
                }
                Microsecond => {
                    out.push_str("SECOND(6)");
                    Ok(())
                }
                Nanosecond => {
                    out.push_str("SECOND(9)");
                    Ok(())
                }
                _ => write!(out, "{self}"),
            }
        } else {
            write!(out, "{self}")
        }
    }

    fn from_precision(p: u8) -> Self {
        use DateTimeField::*;
        match p {
            0..=2 => Second,
            3..=5 => Millisecond,
            6..=8 => Microsecond,
            9 => Nanosecond,
            _ => Nanosecond,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TimeZone {
    Named(String),
}

impl TimeZone {
    pub fn display(&self, backend: AdapterType) -> TimeZoneDisplay<'_> {
        TimeZoneDisplay(self, backend)
    }
}

pub struct TimeZoneDisplay<'a>(&'a TimeZone, AdapterType);

impl fmt::Display for TimeZoneDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use AdapterType::*;
        use TimeZone::*;
        match (self.1, self.0) {
            // https://clickhouse.com/docs/use-cases/time-series/date-time-data-types#time-series-timezones
            (ClickHouse, Named(name)) => write!(f, "'{name}'")?,
            (Exasol, Named(name)) => write!(f, "'{name}'")?,

            (_, Named(name)) => write!(f, "{name}")?,
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum TimeZoneSpec {
    /// WITH LOCAL TIME ZONE, TIMESTAMP_LTZ
    Local,
    // WITH TIME ZONE, TIMESTAMPTZ, TIMESTAMP_TZ
    With,
    // WITHOUT TIME ZONE, TIMESTAMP_NTZ
    Without,
    /// no specification (e.g. TIMESTAMP)
    Unspecified,
    /// Fixed timezone associated with the type instead of individual values.
    ///
    /// (e.g. DateTime('Europe/Berlin') on ClickHouse)
    Fixed(TimeZone),
}

impl TimeZoneSpec {
    fn write_with_leading_space(&self, backend: AdapterType, out: &mut String) -> fmt::Result {
        use AdapterType::*;
        use TimeZoneSpec::*;
        use fmt::Write as _;
        match (backend, self) {
            // Bigquery TIMESTAMP is always stored without a time zone so the type name never says
            // anything about time zones.
            //
            // NOTE: literals can contain time zone information, so *there are more types of
            // literals than types that end up stored in the database.* So in case a type were to
            // be instantiated with a time zone spec and we have to render it, we will produce the
            // "WITH TIME ZONE" form which can be useful for debugging.
            (Bigquery, Without | Unspecified) => Ok(()),

            // PostgreSQL TIMESTAMP WITHOUT TIME ZONE can be rendered as TIMESTAMP
            (Postgres | Redshift, Without) => Ok(()),

            (_, Local) => write!(out, " WITH LOCAL TIME ZONE"),
            (_, With) => write!(out, " WITH TIME ZONE"),
            (_, Without) => write!(out, " WITHOUT TIME ZONE"),

            (_, Fixed(tz)) => write!(out, "('{}')", tz.display(backend)),

            (_, Unspecified) => Ok(()),
        }
    }

    /// Render the time zone specification as a suffix on the type name if the backend supports it.
    ///
    /// Example: -TZ in PostgreSQL, -_LTZ in Databricks.
    fn write_single_token_suffix(&self, backend: AdapterType, out: &mut String) -> fmt::Result {
        use AdapterType::*;
        use TimeZoneSpec::*;
        use fmt::Write as _;
        match (backend, self) {
            // See [TimeZoneSpec::write_with_leading_space] for explanation about Bigquery.
            (Bigquery | ClickHouse | Exasol, _) => {
                debug_assert!(
                    matches!(self, Without | Unspecified),
                    "{backend} does not support time zone suffixes in their type names",
                );
                Ok(())
            }

            // TIMETZ and TIMESTAMPTZ in PostgreSQL which doesn't have
            // a type that is specifically for local time zone.
            (Postgres | Redshift | Salesforce, Local | With) => {
                debug_assert!(
                    !matches!(self, Local),
                    "PostgreSQL does not have a TIMESTAMP WITH LOCAL TIME ZONE type"
                );
                write!(out, "TZ")
            }
            // In PostgreSQL, TIMESTAMP WITHOUT TIME ZONE is just TIMESTAMP
            (Postgres | Redshift | Salesforce, Without | Unspecified) => Ok(()),

            // Databricks doesn't have a TIMESTAMP WITH TIME ZONE type, only WITH LOCAL TIME ZONE
            // (TIMESTAMP or TIMESTAMP_LTZ) and WITHOUT TIME ZONE (TIMESTAMP_NTZ).
            (Databricks, Unspecified) => Ok(()),
            (Databricks, Without) => write!(out, "_NTZ"),
            (Databricks, With) => Ok(()),

            (_, Local) => write!(out, "_LTZ"),
            (_, With) => write!(out, "_TZ"),
            (_, Without) => write!(out, "_NTZ"),

            // No suffix for unspecified time zone spec.
            //
            // In Snowflake, TIMESTAMP without a time zone spec is ambiguous,
            // we forward the ambiguity to the rendered SQL instead of picking
            // a default.
            //
            // In Databricks, TIMESTAMP has TIMESTAMP_LTZ semantics by default,
            // but we don't render it as TIMESTAMP_LTZ unless explicitly specified
            // even though TIMESTAMP_LTZ is an alias to TIMESTAMP in Databricks.
            (_, Unspecified) => Ok(()),

            (_, Fixed(_)) => {
                debug_assert!(
                    false,
                    "Fixed time zone specifications cannot be rendered as single-token suffixes"
                );
                Ok(())
            }
        }
    }

    pub fn is_with_time_zone(&self, backend: AdapterType) -> bool {
        use AdapterType::*;
        use TimeZoneSpec::*;
        match (backend, self) {
            // Databricks TIMESTAMP has WITH LOCAL TIME ZONE semantics by default
            (Databricks, Unspecified | With | Local) => true,

            (Snowflake, Unspecified) => {
                // Users can run `ALTER SESSION SET TIMESTAMP_TYPE_MAPPING = TIMESTAMP_TZ;`
                // so the meaning of `TIMESTAMP` is dependent on the session state.
                debug_assert!(
                    false,
                    "Snowflake TIMESTAMP without a time zone spec is ambiguous. \
Avoid constructing Snowflake TIME/TIMESTAMP types without an explicit time zone specification."
                );
                false
            }

            (_, With | Local) => true,
            (_, Without | Unspecified) => false,
            (_, Fixed(_)) => {
                debug_assert!(
                    false,
                    "Fixed time zone specifications cannot be categorized as simply with or without time zone"
                );
                true
            }
        }
    }
}

pub fn default_time_unit(backend: AdapterType) -> TimeUnit {
    use AdapterType::*;
    use TimeUnit::*;
    match backend {
        Snowflake | Databricks | Spark => Nanosecond,
        Bigquery | Redshift => Microsecond,
        Postgres | Salesforce | DuckDB | GizmoSQL => Microsecond,
        Fabric => Microsecond,
        ClickHouse => Second,
        // Athena (Presto/Trino-based) uses millisecond precision for timestamps.
        // https://docs.aws.amazon.com/athena/latest/ug/data-types.html
        // The adbc_driver_athena is being updated to emit Timestamp_ms to match;
        // see https://github.com/dbt-labs/athena/issues/6
        Athena => Millisecond,
        Exasol => Millisecond,
        _ => Microsecond, // a reasonable default
    }
}

pub const fn time_unit_to_precision(time_unit: TimeUnit) -> u8 {
    use TimeUnit::*;
    match time_unit {
        Second => 0,
        Millisecond => 3,
        Microsecond => 6,
        Nanosecond => 9,
    }
}

/// Returns the [TimeUnit] that can represent the given precision.
///
/// The unit should allow the representation of `n**(-precision)` seconds
/// for any `n <= 9` without being unnecessarily more precise than that.
pub fn suitable_time_unit(precision: u8) -> TimeUnit {
    use TimeUnit::*;
    match precision {
        0 => Second,
        1..=3 => Millisecond,
        4..=6 => Microsecond,
        7..=9 => Nanosecond,
        _ => {
            debug_assert!(false, "invalid time precision: {precision}");
            Nanosecond
        }
    }
}

/// Additional attributes for string types.
#[derive(Debug, Clone, Default)]
pub struct StringAttrs {
    pub collate_spec: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StructField {
    pub name: Ident,
    pub sql_type: SqlType,
    pub nullable: bool,
    /// The raw token found after the COMMENT keyword.
    pub comment_tok: Option<String>,
}

impl StructField {
    pub fn new(name: Ident, sql_type: SqlType, nullable: bool) -> Self {
        Self {
            name,
            sql_type,
            nullable,
            comment_tok: None,
        }
    }

    pub fn with_comment(mut self, comment: String) -> Self {
        self.comment_tok = Some(comment);
        self
    }
}

/// A parsed column description from a CREATE TABLE statement.
///
/// Example: `id INTEGER NOT NULL COMMENT 'primary key'`
#[derive(Debug, Clone)]
pub struct ColumnDescription {
    pub name: Option<Ident>,
    pub sql_type: SqlType,
    pub nullable: bool,
    pub comment: Option<String>,
}

/// Encodes an entry in an enum type.
///
/// The enum label and an optional integer value.
///
/// Example of entries: `'small' = 1`, `'medium'`, `'large' = 3`
#[derive(Debug, Clone)]
pub struct EnumEntry(pub Ident, pub Option<i64>);

#[derive(Debug, Clone, Default)]
pub struct EnumAttrs {
    /// Useful for encoding Enum, Enum8, Enum16 on ClickHouse.
    pub bit_width: Option<u8>,
}

/// Syntactic representation of SQL types.
///
/// The string representation and semantics of each SQL type can only be
/// realized in the context of a specific [SQL backend](`AdapterType`).
/// But this enum aims to be a common representation that can be used
/// across different backends with slight tweaks in the behavior.
#[derive(Debug, Clone)] // DO NOT derive PartialEq or Eq, use `to_string(backend)` for comparisons!
pub enum SqlType {
    /// BOOLEAN
    Boolean,
    /// TINYINT
    TinyInt,
    /// SMALLINT
    SmallInt,
    /// INTEGER / INT
    Integer,
    /// BIGINT
    BigInt,
    /// HUGEINT (128-bit signed integer, DuckDB)
    HugeInt,
    /// INT256 (256-bit signed integer, ClickHouse)
    Int256,
    /// UTINYINT (unsigned 8-bit integer, DuckDB)
    UTinyInt,
    /// USMALLINT (unsigned 16-bit integer, DuckDB)
    USmallInt,
    /// UINTEGER (unsigned 32-bit integer, DuckDB)
    UInteger,
    /// UBIGINT (unsigned 64-bit integer, DuckDB)
    UBigInt,
    /// UHUGEINT (unsigned 128-bit integer, DuckDB)
    UHugeInt,
    /// UINT256 (unsigned 256-bit integer, ClickHouse)
    UInt256,
    /// Half-precision float (BFloat16 on ClickHouse)
    HalfFloat,
    /// REAL
    Real,
    /// FLOAT [ '(' precision ')' ]
    Float(Option<u8>),
    /// DOUBLE PRECISION
    Double,
    /// (DECIMAL | NUMERIC) [ '(' precision [ ',' scale ] ')' ]
    Numeric(Option<(u8, Option<i8>)>),
    /// (BIGDECIMAL | BIGNUMERIC) [ '(' precision [ ',' scale ] ')' ]
    BigNumeric(Option<(u8, Option<i8>)>),
    /// (CHAR | CHARACTER | NCHAR | NATIONAL CHAR) [ '(' length ')' ]
    Char(Option<usize>),
    /// ((VARCHAR | CHARACTER VARYING) [ '(' length ')' ] |
    ///  (NVARCHAR | NATIONAL CHAR VARYING) [ '(' length ')' ])
    /// [ COLLATE collation ]
    Varchar(Option<usize>, StringAttrs),
    /// TEXT
    Text,
    /// CLOB / CHARACTER LARGE OBJECT
    Clob,
    /// BLOB / BINARY LARGE OBJECT
    Blob,
    /// BINARY / VARBINARY [ '(' length ')' ]
    Binary(Option<usize>),
    /// DATE, Date, Date32
    Date(Option<u8>),
    /// TIME [ '(' precision ')' ] [ WITH TIME ZONE | WITH LOCAL | WITHOUT TIME ZONE ]
    Time {
        precision: Option<u8>,
        time_zone_spec: TimeZoneSpec,
    },
    /// TIMESTAMP
    Timestamp {
        precision: Option<u8>,
        time_zone_spec: TimeZoneSpec,
    },
    /// DATETIME is different from timestamps in Bigquery.
    DateTime,
    /// INTERVAL [
    ///        <start field> TO <end field>
    ///      | <single datetime field>
    /// ]
    Interval(Option<(DateTimeField, Option<DateTimeField>)>),
    /// JSON
    Json,
    /// JSONB
    Jsonb,
    /// UUID (universally unique identifier)
    Uuid,
    /// IPv4 (ClickHouse)
    IPv4,
    /// IPv6 (ClickHouse)
    IPv6,
    /// HASHTYPE [ '(' n BYTE | n BIT ')' ] (Exasol; byte size, default 16)
    HashType(Option<u32>),
    /// GEOMETRY [ '(' srid | 'ANY' ')' ]
    Geometry(Option<String>),
    /// GEOGRAPHY [ '(' srid | 'ANY' ')' ]
    Geography(Option<String>),
    /// ARRAY
    Array(Option<Box<SqlType>>),
    /// STRUCT, STRUCT<>, STRUCT<...>
    Struct(Option<Vec<StructField>>),
    /// MAP <key type, value type>
    Map(Option<(Box<SqlType>, Box<SqlType>)>),
    /// Enum (ClickHouse, MySQL...)
    Enum(Option<Vec<EnumEntry>>, EnumAttrs),
    /// VARIANT
    Variant,
    /// VOID
    Void,
    /// Other SQL types that are not explicitly defined.
    ///
    /// This is useful in situations where we can treat the SQL type as an
    /// opaque name without needing to deal with it in a specific way. If
    /// we need more awareness about a specific type, we should expand
    /// the enum with a new variant.
    Other(String),
}

impl SqlType {
    pub fn varchar(max_len: Option<usize>) -> Self {
        SqlType::Varchar(max_len, Default::default())
    }

    /// Extract the SQL type and nullability from an Arrow `Field`.
    ///
    /// This is a lossless conversion if the SQL type is stored in the
    /// Arrow field metadata. If the SQL type is not present, it will try
    /// to come up with a best-effort conversion from the Arrow DataType.
    pub fn from_field(backend: AdapterType, field: &Field) -> Result<(Self, bool), String> {
        let type_string = original_type_string(backend, field);
        match type_string {
            Some(type_str) => {
                let (sql_type, nullable) = Self::parse(backend, type_str)?;
                let nullable = nullable || field.is_nullable();
                Ok((sql_type, nullable))
            }
            None => {
                let sql_type = Self::from_arrow_type(backend, field.data_type());
                Ok((sql_type, field.is_nullable()))
            }
        }
    }

    /// Convert the SQL type to an Arrow `Field`.
    ///
    /// It encodes the SQL type as metadata in the Arrow field and picks the best
    /// Arrow `DataType` that matches for the SQL type.
    pub fn to_field(&self, backend: AdapterType, name: String, nullable: bool) -> Field {
        let data_type = self.pick_best_arrow_type(backend);
        let mut metadata = HashMap::new();
        metadata.insert(
            metadata_sql_type_key(backend).to_string(),
            self.to_string(backend),
        );
        Field::new(name, data_type, nullable).with_metadata(metadata)
    }

    /// Parse the SQL type and return it along with a boolean indicating if its nullable.
    pub fn parse(backend: AdapterType, input: &str) -> Result<(SqlType, bool), String> {
        let mut parser = Parser::new(input);
        parser
            .parse(backend)
            .map_err(|err| format!("Failed to parse SQL type '{input}': {err}"))
    }

    /// Parse a column description (identifier followed by type and optional constraints).
    ///
    /// When `identifier_optional` is true, the parser will attempt to parse a type directly
    /// if no identifier is found (for inputs like `INTEGER NOT NULL`).
    ///
    /// Example inputs: `id INTEGER NOT NULL`, `name VARCHAR(50) COMMENT 'full name'`
    pub fn parse_column_description(
        backend: AdapterType,
        input: &str,
        identifier_optional: bool,
    ) -> Result<ColumnDescription, String> {
        let mut parser = Parser::new(input);
        parser
            .parse_column_description(backend, identifier_optional)
            .map_err(|err| format!("Failed to parse column description '{input}': {err}"))
    }

    pub fn to_string(&self, backend: AdapterType) -> String {
        let mut out = String::new();
        self.write(backend, &mut out).unwrap();
        out
    }

    /// Render a SQL type string in the preferred syntax for a given backend.
    pub fn write(&self, backend: AdapterType, out: &mut String) -> fmt::Result {
        use AdapterType::*;
        use SqlType::*;
        use fmt::Write as _;
        match (backend, self) {
            // Bigquery {{{
            (Bigquery, Boolean) => write!(out, "BOOL"),
            (Bigquery, TinyInt | SmallInt | Integer | BigInt) => write!(out, "INT64"),
            (Bigquery, Real | Float(_) | Double) => {
                write!(out, "FLOAT64")
            }
            (Bigquery, Char(_) | Varchar(..) | Text | Clob) => {
                write!(out, "STRING")
            }
            (Bigquery, Blob | Binary(_)) => write!(out, "BYTES"),
            (Bigquery, Time { time_zone_spec, .. }) => {
                write!(out, "TIME")?;
                // Bigquery does not use precision for time and timestamp types
                time_zone_spec.write_with_leading_space(backend, out)
            }
            (Bigquery, Timestamp { time_zone_spec, .. }) => {
                write!(out, "TIMESTAMP",)?;
                // Bigquery does not use precision for timestamps
                time_zone_spec.write_with_leading_space(backend, out)
            }
            // }}}

            // Snowflake {{{
            (Snowflake, Float(_)) => write!(out, "FLOAT"),
            (Snowflake, Numeric(None) | BigNumeric(None)) => {
                write!(out, "NUMBER")
            }
            (Snowflake, Numeric(Some((p, None))) | BigNumeric(Some((p, None)))) => {
                write!(out, "NUMBER({p})")
            }
            (Snowflake, Numeric(Some((p, Some(s)))) | BigNumeric(Some((p, Some(s))))) => {
                write!(out, "NUMBER({p}, {s})")
            }
            (Snowflake, Clob) => write!(out, "TEXT"),
            (Snowflake, Blob) => write!(out, "BINARY"),
            (
                Snowflake,
                Time {
                    precision,
                    time_zone_spec,
                },
            ) => {
                write!(out, "TIME")?;
                if let Some(p) = precision {
                    write!(out, "({p})")?;
                }
                // Snowflake does not have a TIME WITH TIME ZONE type
                match time_zone_spec {
                    TimeZoneSpec::Unspecified | TimeZoneSpec::Without => Ok(()),
                    TimeZoneSpec::Local | TimeZoneSpec::With => {
                        // for debugging purposes, we still render these invalid specs
                        time_zone_spec.write_with_leading_space(backend, out)
                    }
                    TimeZoneSpec::Fixed(_) => {
                        debug_assert!(
                            false,
                            "Snowflake does not support fixed time zone specifications"
                        );
                        Ok(())
                    }
                }
            }
            (
                Snowflake,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => {
                write!(out, "TIMESTAMP")?;
                time_zone_spec.write_single_token_suffix(backend, out)?;
                match precision {
                    Some(p) => write!(out, "({p})"),
                    None => Ok(()),
                }
            }
            (Snowflake, DateTime) => write!(out, "TIMESTAMP_NTZ"),
            // }}}

            // PostgreSQL {{{
            (Postgres | Redshift, TinyInt) => write!(out, "SMALLINT"),
            (Postgres | Redshift, Binary(_) | Blob) => write!(out, "BYTEA"),
            (Postgres | Redshift, DateTime) => write!(out, "TIMESTAMP"),
            (
                Postgres | Redshift,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => match precision {
                Some(p) => {
                    // if there is a precision, we use the (..) WITH TIME ZONE form
                    write!(out, "TIMESTAMP({p})")?;
                    time_zone_spec.write_with_leading_space(backend, out)
                }
                None => {
                    // if there is no precision, we use the TIMESTAMPTZ / TIMESTAMP form
                    write!(out, "TIMESTAMP")?;
                    time_zone_spec.write_single_token_suffix(backend, out)
                }
            },
            (Postgres | Redshift, Float(_)) => write!(out, "REAL"),
            (Postgres | Redshift, Clob) => write!(out, "TEXT"),
            (Postgres | Redshift | Salesforce, Array(Some(inner))) => {
                inner.write(backend, out)?;
                write!(out, "[]")
            }
            // }}}

            // Databricks {{{
            (Databricks, Binary(_) | Blob) => {
                // max_len for BINARY is ignored because Databricks doesn't support it
                write!(out, "BINARY")
            }
            (Databricks, Clob | Text | Varchar(..)) => {
                write!(out, "STRING")?;
                if let Varchar(_, attrs) = self {
                    if let Some(collate_spec) = &attrs.collate_spec {
                        write!(out, " COLLATE {collate_spec}")?;
                    }
                }
                Ok(())
            }
            (Databricks, Numeric(None) | BigNumeric(None)) => {
                write!(out, "DECIMAL")
            }
            (Databricks, Numeric(Some((p, None))) | BigNumeric(Some((p, None)))) => {
                write!(out, "DECIMAL({p})")
            }
            (Databricks, Numeric(Some((p, Some(s)))) | BigNumeric(Some((p, Some(s))))) => {
                write!(out, "DECIMAL({p}, {s})")
            }
            (Databricks, Real | Float(_)) => write!(out, "FLOAT"),
            (Databricks, Double) => write!(out, "DOUBLE"),
            (Databricks, DateTime) => write!(out, "TIMESTAMP_NTZ"),
            (Databricks, Timestamp { time_zone_spec, .. }) => {
                write!(out, "TIMESTAMP")?;
                time_zone_spec.write_single_token_suffix(backend, out)
            }
            // }}}

            // ClickHouse {{{
            (ClickHouse, Boolean) => write!(out, "Boolean"),
            (ClickHouse, TinyInt) => write!(out, "Int8"),
            (ClickHouse, SmallInt) => write!(out, "Int16"),
            (ClickHouse, Integer) => write!(out, "Int32"),
            (ClickHouse, BigInt) => write!(out, "Int64"),
            (ClickHouse, HugeInt) => write!(out, "Int128"),
            (ClickHouse, Int256) => write!(out, "Int256"),
            (ClickHouse, UTinyInt) => write!(out, "UInt8"),
            (ClickHouse, USmallInt) => write!(out, "UInt16"),
            (ClickHouse, UInteger) => write!(out, "UInt32"),
            (ClickHouse, UBigInt) => write!(out, "UInt64"),
            (ClickHouse, UHugeInt) => write!(out, "UInt128"),
            (ClickHouse, UInt256) => write!(out, "UInt256"),
            (ClickHouse, HalfFloat) => write!(out, "BFloat16"),
            (ClickHouse, Real) => write!(out, "Float32"),
            (ClickHouse, Float(_)) => write!(out, "Float32"),
            (ClickHouse, Double) => write!(out, "Float64"),
            (ClickHouse, Char(None)) => write!(out, "String"),
            (ClickHouse, Char(Some(n))) => write!(out, "FixedString({n})"),
            (ClickHouse, Varchar(..) | Text | Clob) => write!(out, "String"),
            (ClickHouse, Date(Some(32))) => write!(out, "Date32"),
            (ClickHouse, Date(_)) => write!(out, "Date"),
            (
                ClickHouse,
                Time {
                    precision: None, ..
                },
            ) => write!(out, "Time"),
            (
                ClickHouse,
                Time {
                    precision: Some(p), ..
                },
            ) => write!(out, "Time64({p})"),
            (
                ClickHouse,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => match time_zone_spec {
                TimeZoneSpec::Fixed(tz) => {
                    if let Some(p) = precision {
                        write!(out, "DateTime64({}, {})", p, tz.display(backend))
                    } else {
                        write!(out, "DateTime({})", tz.display(backend))
                    }
                }
                _ => {
                    if let Some(p) = precision {
                        write!(out, "DateTime64({p})")
                    } else {
                        write!(out, "DateTime")
                    }
                }
            },
            (ClickHouse, DateTime) => write!(out, "DateTime"),
            (ClickHouse, Binary(_) | Blob) => write!(out, "String"),
            (ClickHouse, Numeric(None) | BigNumeric(None)) => write!(out, "Decimal"),
            (ClickHouse, Numeric(Some((p, None))) | BigNumeric(Some((p, None)))) => {
                write!(out, "Decimal({p})")
            }
            (ClickHouse, Numeric(Some((p, Some(s)))) | BigNumeric(Some((p, Some(s))))) => {
                write!(out, "Decimal({p}, {s})")
            }
            (ClickHouse, Map(Some((key, value)))) => {
                write!(out, "Map(")?;
                key.write(backend, out)?;
                write!(out, ", ")?;
                value.write(backend, out)?;
                write!(out, ")")
            }
            (ClickHouse, IPv4) => write!(out, "IPv4"),
            (ClickHouse, IPv6) => write!(out, "IPv6"),
            // }}}

            // Exasol {{{
            // Strings and arbitrary bytes use VARCHAR, up to Exasol's
            // 2,000,000-char maximum (CLOB / LONG VARCHAR are aliases of it).
            (Exasol, Text | Clob | Blob | Binary(_)) => write!(out, "VARCHAR(2000000)"),
            (Exasol, Varchar(max_len, attrs)) => {
                // VARCHAR needs an explicit length; default to the maximum.
                let len = max_len.unwrap_or(0);
                let len = if len > 0 { len } else { 2_000_000 };
                write!(out, "VARCHAR({len})")?;
                if let Some(collate_spec) = &attrs.collate_spec {
                    write!(out, " COLLATE {collate_spec}")?;
                }
                Ok(())
            }
            (Exasol, Real | HalfFloat | Float(_) | Double) => write!(out, "DOUBLE PRECISION"),
            (
                Exasol,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => {
                write!(out, "TIMESTAMP")?;
                if let Some(p) = precision {
                    write!(out, "({p})")?;
                }
                // Exasol's only tz-aware timestamp is WITH LOCAL TIME ZONE.
                if matches!(
                    time_zone_spec,
                    TimeZoneSpec::Local | TimeZoneSpec::With | TimeZoneSpec::Fixed(_)
                ) {
                    write!(out, " WITH LOCAL TIME ZONE")?;
                }
                Ok(())
            }
            // Time-of-day and datetime values both render as TIMESTAMP.
            (Exasol, Time { precision, .. }) => {
                write!(out, "TIMESTAMP")?;
                if let Some(p) = precision {
                    write!(out, "({p})")?;
                }
                Ok(())
            }
            (Exasol, DateTime) => write!(out, "TIMESTAMP"),
            // Normalize to Exasol's two interval types.
            (Exasol, Interval(qualifier)) => {
                if matches!(
                    qualifier,
                    Some((DateTimeField::Year | DateTimeField::Month, _))
                ) {
                    write!(out, "INTERVAL YEAR TO MONTH")
                } else {
                    write!(out, "INTERVAL DAY TO SECOND")
                }
            }
            // }}}

            // Generic SQL / Fallback logic {{{
            (_, Boolean) => write!(out, "BOOLEAN"),
            (_, TinyInt) => write!(out, "TINYINT"),
            (_, SmallInt) => write!(out, "SMALLINT"),
            (_, Integer) => write!(out, "INT"),
            (_, BigInt) => write!(out, "BIGINT"),
            // DuckDB-specific integer types
            (_, HugeInt) => write!(out, "HUGEINT"),
            (_, Int256) => write!(out, "INT256"),
            (_, UTinyInt) => write!(out, "UTINYINT"),
            (_, USmallInt) => write!(out, "USMALLINT"),
            (_, UInteger) => write!(out, "UINTEGER"),
            (_, UBigInt) => write!(out, "UBIGINT"),
            (_, UHugeInt) => write!(out, "UHUGEINT"),
            (_, UInt256) => write!(out, "UINT256"),

            (_, HalfFloat) => write!(out, "FLOAT16"),
            (_, Real) => write!(out, "REAL"),
            (_, Float(Some(p))) => write!(out, "FLOAT({p})"),
            (_, Float(None)) => write!(out, "FLOAT"),
            (_, Double) => write!(out, "DOUBLE PRECISION"),

            (_, Numeric(None)) => write!(out, "NUMERIC"),
            (_, Numeric(Some((p, None)))) => write!(out, "NUMERIC({p})"),
            (_, Numeric(Some((p, Some(s))))) => write!(out, "NUMERIC({p}, {s})"),
            (_, BigNumeric(None)) => write!(out, "BIGNUMERIC"),
            (_, BigNumeric(Some((p, None)))) => write!(out, "BIGNUMERIC({p})"),
            (_, BigNumeric(Some((p, Some(s))))) => write!(out, "BIGNUMERIC({p}, {s})"),

            (_, Char(None)) => write!(out, "CHAR"),
            (_, Char(Some(len))) => {
                write!(out, "CHAR")?;
                if *len > 0 {
                    write!(out, "({len})")?;
                }
                Ok(())
            }
            (_, Varchar(max_len, attrs)) => {
                write!(out, "VARCHAR")?;
                let max_len = max_len.unwrap_or(0);
                if max_len > 0 {
                    write!(out, "({max_len})")?;
                }
                if let Some(collate_spec) = &attrs.collate_spec {
                    write!(out, " COLLATE {collate_spec}")?;
                }
                Ok(())
            }
            (_, Text) => write!(out, "TEXT"),
            (_, Clob) => write!(out, "CLOB"),
            (_, Blob) => write!(out, "BLOB"),
            (_, Binary(None)) => write!(out, "BINARY"),
            (_, Binary(Some(len))) => {
                write!(out, "BINARY")?;
                if *len > 0 {
                    write!(out, "({len})")?;
                }
                Ok(())
            }

            (_, Date(_)) => write!(out, "DATE"),
            (
                _,
                Time {
                    precision,
                    time_zone_spec,
                },
            ) => {
                match precision {
                    Some(p) => write!(out, "TIME({p})"),
                    None => write!(out, "TIME"),
                }?;
                time_zone_spec.write_with_leading_space(backend, out)
            }
            (_, DateTime) => write!(out, "DATETIME"),
            (
                _,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => {
                match precision {
                    Some(p) => write!(out, "TIMESTAMP({p})"),
                    None => write!(out, "TIMESTAMP"),
                }?;
                time_zone_spec.write_with_leading_space(backend, out)
            }

            (_, Interval(qualifier)) => match qualifier {
                None => write!(out, "INTERVAL"),
                Some((start, end)) => {
                    write!(out, "INTERVAL ")?;
                    match end {
                        Some(end) => {
                            write!(out, "{start} TO ")?;
                            end.write(backend, out)
                        }
                        None => start.write(backend, out),
                    }
                }
            },

            // Snowflake uses VARIANT for JSON data
            (Snowflake, Json | Jsonb) => write!(out, "VARIANT"),
            (_, Json) => write!(out, "JSON"),
            (_, Jsonb) => write!(out, "JSONB"),
            (_, Uuid) => write!(out, "UUID"),
            (_, IPv4) => write!(out, "IPV4"),
            (_, IPv6) => write!(out, "IPV6"),
            (_, HashType(byte_size)) => {
                write!(out, "HASHTYPE")?;
                if let Some(n) = byte_size {
                    write!(out, "({n} BYTE)")?;
                }
                Ok(())
            }
            (_, Geometry(srid)) => {
                write!(out, "GEOMETRY")?;
                if let Some(srid) = srid {
                    write!(out, "({})", srid)?;
                }
                Ok(())
            }
            (_, Geography(srid)) => {
                write!(out, "GEOGRAPHY")?;
                if let Some(srid) = srid {
                    write!(out, "({})", srid)?;
                }
                Ok(())
            }
            (_, Array(None)) => write!(out, "ARRAY"),
            (backend, Array(Some(inner))) => {
                match backend {
                    Snowflake => write!(out, "ARRAY(")?,
                    ClickHouse => write!(out, "Array(")?,
                    _ => write!(out, "ARRAY<")?,
                }
                inner.write(backend, out)?;
                match backend {
                    Snowflake | ClickHouse => write!(out, ")"),
                    _ => write!(out, ">"),
                }
            }
            (_, Struct(None)) => write!(out, "STRUCT"),
            (_, Struct(Some(fields))) => {
                match backend {
                    Snowflake => write!(out, "OBJECT(")?,
                    Bigquery | Databricks | Spark | Athena => write!(out, "STRUCT<")?,
                    Postgres | Salesforce | DuckDB | GizmoSQL | ClickHouse | Exasol => {
                        write!(out, "(")?
                    }
                    // Redshift doesn't support object/struct types
                    Redshift => write!(out, "(")?,
                    Fabric => unimplemented!("SQL Server does't have a struct type"),
                    _ => write!(out, "STRUCT<")?,
                }
                for (i, field) in fields.iter().enumerate() {
                    let StructField {
                        name,
                        sql_type,
                        nullable,
                        comment_tok,
                    } = field;

                    if i > 0 {
                        write!(out, ", ")?;
                    }
                    write!(
                        out,
                        "{}{}",
                        name.display(backend),
                        // Databricks allows a `:` between the name and type
                        if matches!(backend, Databricks) {
                            ": "
                        } else {
                            " "
                        }
                    )?;
                    sql_type.write(backend, out)?;
                    if !nullable {
                        write!(out, " NOT NULL")?;
                    }
                    if let Some(tok) = comment_tok {
                        write!(out, " COMMENT {tok}")?;
                    }
                }
                match backend {
                    Snowflake => write!(out, ")"),
                    Bigquery | Databricks | Spark | Athena => {
                        write!(out, ">")
                    }
                    Postgres | Salesforce | DuckDB | GizmoSQL | ClickHouse | Exasol => {
                        write!(out, ")")
                    }
                    Redshift => write!(out, ")"),
                    Fabric => unimplemented!("SQL Server does't have a struct type"),
                    _ => write!(out, ">"),
                }
            }
            (_, Map(None)) => write!(out, "MAP"),
            (_, Map(Some((key, value)))) => {
                write!(out, "MAP<")?;
                key.write(backend, out)?;
                write!(out, ", ")?;
                value.write(backend, out)?;
                write!(out, ">")
            }
            (_, Enum(entries, attrs)) => {
                match (backend, attrs.bit_width) {
                    (ClickHouse, Some(8)) => write!(out, "Enum8")?,
                    (ClickHouse, Some(16)) => write!(out, "Enum16")?,
                    _ => write!(out, "ENUM")?,
                }
                if let Some(entries) = entries {
                    write!(out, "(")?;
                    for (i, EnumEntry(label, value)) in entries.iter().enumerate() {
                        if i > 0 {
                            write!(out, ", ")?;
                        }
                        write!(out, "{}", label.display(backend))?;
                        if let Some(v) = value {
                            write!(out, " = {v}")?;
                        }
                    }
                    write!(out, ")")?;
                }
                Ok(())
            }
            (Redshift, Variant) => write!(out, "SUPER"),
            (ClickHouse, Variant) => write!(out, "Dynamic"),
            (_, Variant) => write!(out, "VARIANT"),
            (_, Void) => write!(out, "VOID"),
            (_, Other(s)) => write!(out, "{s}"),
            // }}}
        }
    }

    pub fn display(&self, backend: AdapterType) -> SqlTypeDisplay<'_> {
        SqlTypeDisplay(self, backend)
    }

    /// Best-effort conversion from an Arrow `DataType` to a `SqlType`.
    ///
    /// Arrow types are less expressive than SQL types, so this function
    /// will return a `SqlType` that is the closest match. This is only
    /// used in situations where the field metadata in an Arrow schema
    /// doesn't contain the SQL type string.
    pub fn from_arrow_type(backend: AdapterType, data_type: &DataType) -> SqlType {
        match data_type {
            DataType::Null => SqlType::Varchar(None, Default::default()),
            DataType::Boolean => SqlType::Boolean,
            DataType::Int8 | DataType::UInt8 | DataType::Int16 => SqlType::SmallInt,
            DataType::UInt16 | DataType::Int32 => SqlType::Integer,
            DataType::UInt32 | DataType::Int64 | DataType::UInt64 => SqlType::BigInt,
            DataType::Float16 | DataType::Float32 => SqlType::Real,
            DataType::Float64 => SqlType::Double,
            DataType::Decimal32(p, s)
            | DataType::Decimal64(p, s)
            | DataType::Decimal128(p, s)
            | DataType::Decimal256(p, s) => {
                // XXX: make these more succinct by looking up the defaults
                // for each different backend.
                SqlType::Numeric(Some((*p, Some(*s))))
            }
            DataType::Utf8View | DataType::Utf8 => SqlType::Varchar(None, Default::default()),
            DataType::LargeUtf8 => SqlType::Text,
            DataType::Binary
            | DataType::LargeBinary
            | DataType::BinaryView
            | DataType::FixedSizeBinary(_) => SqlType::Binary(None),
            DataType::Date32 => SqlType::Date(Some(32)),
            DataType::Date64 => SqlType::Date(Some(64)),
            DataType::Time32(TimeUnit::Second) => SqlType::Time {
                precision: None,
                time_zone_spec: TimeZoneSpec::Without,
            },
            DataType::Time32(TimeUnit::Millisecond) => SqlType::Time {
                precision: Some(3),
                time_zone_spec: TimeZoneSpec::Without,
            },
            DataType::Time64(TimeUnit::Microsecond) => SqlType::Time {
                precision: Some(6),
                time_zone_spec: TimeZoneSpec::Without,
            },
            DataType::Time64(TimeUnit::Nanosecond) => SqlType::Time {
                precision: Some(9),
                time_zone_spec: TimeZoneSpec::Without,
            },
            DataType::Time32(_) | DataType::Time64(_) => {
                unreachable!("unexpected time unit in Arrow data type: {data_type:?}")
            }
            DataType::Timestamp(TimeUnit::Second, tz) => SqlType::Timestamp {
                precision: None,
                time_zone_spec: if tz.is_some() {
                    TimeZoneSpec::With
                } else {
                    TimeZoneSpec::Without
                },
            },
            DataType::Timestamp(TimeUnit::Millisecond, tz) => SqlType::Timestamp {
                precision: Some(3),
                time_zone_spec: if tz.is_some() {
                    TimeZoneSpec::With
                } else {
                    TimeZoneSpec::Without
                },
            },
            DataType::Timestamp(TimeUnit::Microsecond, tz) => SqlType::Timestamp {
                precision: Some(6),
                time_zone_spec: if tz.is_some() {
                    TimeZoneSpec::With
                } else {
                    TimeZoneSpec::Without
                },
            },
            DataType::Timestamp(TimeUnit::Nanosecond, tz) => SqlType::Timestamp {
                precision: Some(9),
                time_zone_spec: if tz.is_some() {
                    TimeZoneSpec::With
                } else {
                    TimeZoneSpec::Without
                },
            },
            // TODO: think more carefully about this one
            DataType::Duration(unit) => SqlType::Time {
                precision: Some(time_unit_to_precision(*unit)),
                time_zone_spec: TimeZoneSpec::Without,
            },
            // Proposal for extending Arrow to support more SQL interval types:
            // https://docs.google.com/document/d/12ghQxWxyAhSQeZyy0IWiwJ02gTqFOgfYm8x851HZFLk/edit
            DataType::Interval(interval_unit) => match interval_unit {
                IntervalUnit::YearMonth => {
                    SqlType::Interval(Some((DateTimeField::Year, Some(DateTimeField::Month))))
                }
                IntervalUnit::DayTime => {
                    SqlType::Interval(Some((DateTimeField::Day, Some(DateTimeField::Millisecond))))
                }
                // MonthDayNano was added to Arrow because it is closest to how Postgress
                // and Bigquery model intervals.  Each field is independent (e.g. there is
                // no constraint that nanoseconds have the same sign as days or that the
                // quantity of nanoseconds represents less than a day's worth of time).
                // One limitation that might be problematic is that the number of
                // nanoseconds alone can't represent a full +/- 10K years range as
                // required by the SQL spec. But a literal representing
                //
                //     "9999 years, 10 months, 8 days, and 100 milliseconds"
                //
                // can be represented by turning the number of year into a number of
                // months so the nanoseconds part does not overflow the 64 bits
                //
                //     MonthDayNano {
                //        months: 9999 * 12 + 10,
                //        days: 8,
                //        nanos: 100 * 10^-6 * 10^9,
                //     }
                //
                //
                // Internal PostgreSQL representation of intervals
                // ===============================================
                //
                // ```c
                // typedef struct {
                //     int64 time;   // microseconds
                //     int32 day;    // days
                //     int32 month;  // months
                // } Interval;
                //
                // The 64-bit field together with the two 32-bit fields create a struct
                // that needs no padding. So these can be stored contiguously in memory
                // without wasting space.
                // ```
                IntervalUnit::MonthDayNano => SqlType::Interval(Some((
                    DateTimeField::Month,
                    Some(DateTimeField::Nanosecond),
                ))),
            },

            // XXX: things get tricky here and conversions don't really work well yet
            DataType::List(_)
            | DataType::LargeList(_)
            | DataType::ListView(_)
            | DataType::LargeListView(_) => SqlType::Array(None), // XXX
            DataType::FixedSizeList(_, _) => SqlType::Other("ARRAY".to_string()),
            DataType::Struct(fields) => {
                let mut sql_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    let sql_type = Self::from_arrow_type(backend, field.data_type());
                    let nullable = field.is_nullable();
                    // XXX: this is not necessarily correct, field names might contain
                    // quote characters that need to be escaped (meaning they should exist
                    // in a Ident::Unquoted). But we don't have that information here.
                    let name = Ident::Plain(field.name().clone());
                    sql_fields.push(StructField::new(name, sql_type, nullable));
                }
                SqlType::Struct(Some(sql_fields))
            }
            DataType::Union(..) => SqlType::Other("UNION".to_string()),
            DataType::Map(..) => SqlType::Map(None), // TODO: handle key/value types
            DataType::Dictionary(_, value_type) => Self::from_arrow_type(backend, value_type),
            DataType::RunEndEncoded(_, values) => {
                Self::from_arrow_type(backend, values.as_ref().data_type())
            }
        }
    }

    /// DON'T USE THIS. Try to approximate the best Arrow [DataType] for this SQL type.
    ///
    /// Going from SQL types to Arrow types is lossy because SQL types are more expressive
    /// than Arrow types. This function tries to pick the best matching Arrow type for a
    /// given SQL type. But there are many SQL types that don't have an isomorphic mapping
    /// in Arrow.
    ///
    /// # SQL Timestamps and Apache Arrow types
    ///
    /// ## TIMESTAMP WITH LOCAL TIME ZONE
    ///
    /// *Meaning:* a point in time that is *displayed* and *processed* in the local time zone of
    /// the database client's session.
    ///
    /// *Example:* the scheduled time of a meeting in a calendar app. Each user sees the time
    /// of the meeting in their own time zone, but the meeting happens at a single point in
    /// time.
    ///
    /// *Also known as:* `TIMESTAMP_LTZ`.
    ///
    /// ### Storage
    ///
    /// `Timestamp(Second | Millisecond | Microsecond | Nanosecond, "UTC")` is sufficient to
    /// store a local-tz timestamp because the actual point in time is stored in UTC. But the
    /// type does not capture the fact that the timestamp is supposed to be displayed in the
    /// local time zone of the user. This information has to be conveyed separately, e.g. in
    /// the metadata of an Arrow `Field`.
    ///
    /// ## TIMESTAMP WITH TIME ZONE
    ///
    /// *Meaning:* a point in time that is stored along with a time zone offset. The time zone
    /// offset is used to convert the timestamp to UTC for storage, and to convert it back
    /// to the original time zone when displaying it.
    ///
    /// *Example:* a flight departure and arrival time in an airline reservation system. Both
    /// timestamps are stored along with the local time zone of the airport, so they can be
    /// displayed correctly to users.
    ///
    /// *Also known as:* `TIMESTAMP_TZ`.
    ///
    /// ### Storage
    ///
    /// Most systems store the timestamp in UTC and the time zone offset (in minutes) as
    /// a signed integer. To render the timestamp in the local time zone, the offset is
    /// added to the UTC timestamp. If the system chooses to store the timestamp pre-added
    /// to the offset, it needs to subtract the offset to get the UTC timestamp back.
    ///
    /// When inserting data into the database, we can push a `Timestamp(_, UTC)` along
    /// and the database will store the offset=0 for us. If we push a `Timestamp(_, Some(tz))`
    /// with a non-UTC time zone, the database should convert the timestamp to UTC and resolve
    /// the offset for us. Storing the same offset for all the values we are inserting.
    ///
    /// The situation is more complicated when reading data from the database [1]. Since each
    /// value of the column can have a different offset, we can't represent the data as a simple
    /// `Timestamp(_, Some(tz))` because that assumes a single time zone for the whole column.
    ///
    /// For these situations, we are going to propose and Arrow Extension type [2] that dbt
    /// Fusion and ADBC drivers can adopt:
    ///
    ///     name: "arrow.timestamp_with_offsets"
    ///     storage_type:
    ///         struct<
    ///             timestamp: timestamp[s | ms | us | ns, tz=UTC],
    ///             offset_minutes: int16
    ///         >
    ///
    /// The `timestamp` field stores the point in time in UTC. We recommend always setting the
    /// timezone explicitly to UTC to avoid ambiguity. If not provided, UTC is assumed. Anything
    /// else should be considered an error.
    ///
    /// The `offset_minutes` field stores the time zone offset in minutes that was used to
    /// convert the original timestamp to UTC. This allows reconstructing the original
    /// timestamp in the local time zone by adding the offset to the UTC timestamp.
    ///
    /// ## TIMESTAMP WITHOUT TIME ZONE
    ///
    /// *Meaning:* a data and time literal that is not associated with any time zone.
    /// It represents a calendar date and time that is the same for all users, regardless
    /// of their time zone.
    ///
    /// *Example:* a birth date and time in a user profile. The timestamp is stored
    /// as-is without any conversion or time zone information.
    ///
    /// *Also known as:* `TIMESTAMP_NTZ`, `DATETIME`.
    ///
    /// ### Storage
    ///
    /// `Timestamp(Second | Millisecond | Microsecond | Nanosecond, None)` is sufficient.
    /// The lack of a time zone indicates that the timestamp is to be interpreted as a literal.
    /// So to render it in YYYY-MM-DD HH:MM:SS format, the system just needs to format the
    /// timestamp assuming it is in the UTC time zone, but the resulting string does not
    /// mean the point in time represented by that UTC timestamp.
    ///
    /// [1] Trying to insert a column with different offsets for each value would also not work
    ///     with a `Timestamp(_, Some(tz))` because that assumes a single time zone for the
    ///     whole column.
    /// [2] https://arrow.apache.org/docs/format/Columnar.html#format-metadata-extension-types
    pub fn pick_best_arrow_type(&self, backend: AdapterType) -> DataType {
        use AdapterType::*;
        use SqlType::*;
        use TimeZoneSpec::*;

        let arrow_timestamp = |precision: Option<u8>, arrow_tz: Option<Arc<str>>| {
            let time_unit = precision
                .map(suitable_time_unit)
                .unwrap_or_else(|| default_time_unit(backend));
            DataType::Timestamp(time_unit, arrow_tz)
        };
        let arrow_timestamp_with_local_tz =
            |precision: Option<u8>| arrow_timestamp(precision, Some("UTC".into()));
        // TODO: use an extension type for `TIMESTAMP WITH TIME ZONE` becaues SQL
        // timestamps with time zone can have one offset per row, while Arrow's
        // `Timestamp(_, Some(tz))` assumes a single time zone for the whole column.
        let arrow_timestamp_tz = |precision: Option<u8>| {
            let time_unit = precision
                .map(suitable_time_unit)
                .unwrap_or_else(|| default_time_unit(backend));
            let fields = Fields::from(vec![
                Field::new(
                    "timestamp",
                    DataType::Timestamp(time_unit, Some("UTC".into())),
                    false,
                ),
                Field::new("offset_minutes", DataType::Int16, false),
            ]);
            DataType::Struct(fields)
        };

        // Generate a debug_assert!(false, ...) call for a SQL type string that is
        // not explicitly handled yet. Meaning that a conscious decision should be
        // made about what Arrow type to pick for that SQL type on the given backend
        // if we ever encounter it.
        macro_rules! not_explicitly_handled {
            ($sql_type:expr) => {
                debug_assert!(
                    false,
                    "SQL type '{}' is not explicitly handled in pick_best_arrow_type() for backend '{:?}'. This is a debug-only assertion.",
                    $sql_type.to_string(backend),
                    backend
                );
            };
        }

        let data_type: DataType = match (backend, self) {
            (_, Boolean) => DataType::Boolean,

            // Snowflake {{{
            (Snowflake, TinyInt | SmallInt | Integer | BigInt) => DataType::Decimal128(38, 0),
            (Snowflake, Real | Float(_) | Double) => DataType::Float64,
            // "NUMBER" in Snowflake is an alias for "DECIMAL(38,0)"
            (Snowflake, Numeric(None) | BigNumeric(None)) => DataType::Decimal128(38, 0),

            (Snowflake, DateTime) => arrow_timestamp(Some(9), None),
            // }}}

            // Bigquery {{{
            (Bigquery, TinyInt | SmallInt | Integer | BigInt) => DataType::Int64,
            (Bigquery, Numeric(None)) => DataType::Decimal128(38, 9),
            (Bigquery, BigNumeric(None)) => DataType::Decimal256(76, 38),

            // Bigquery's DATETIME has microsecond precision
            // https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#datetime_type
            (Bigquery, DateTime) => arrow_timestamp(Some(6), None),

            // Bigquery's TIME always has microsecond precision and no time zone
            // https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#time_type
            (
                Bigquery,
                Time {
                    precision: _,
                    time_zone_spec: _,
                },
            ) => DataType::Time64(TimeUnit::Microsecond),

            // Bigquery floats are 64-bit
            // https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#floating_point_types
            (Bigquery, Real | Float(_) | Double) => DataType::Float64,
            // }}}

            // Databricks {{{
            // https://docs.databricks.com/aws/en/sql/language-manual/data-types/decimal-type
            (Databricks | Spark, Numeric(None) | BigNumeric(None)) => DataType::Decimal128(10, 0),
            // }}}

            // Athena {{{
            // Athena (Presto/Trino-based) DECIMAL without precision/scale defaults to DECIMAL(38, 0)
            // https://docs.aws.amazon.com/athena/latest/ug/data-types.html
            (Athena, Numeric(None) | BigNumeric(None)) => DataType::Decimal128(38, 0),
            // }}}

            // Redshift {{{
            (Redshift, Numeric(None) | BigNumeric(None)) => {
                // The default precision, if not specified, is 18. The maximum precision is 38.
                // The default scale, if not specified, is 0. The maximum scale is 37.
                // https://docs.aws.amazon.com/redshift/latest/dg/r_Numeric_types201.html#r_Numeric_types201-decimal-or-numeric-type
                DataType::Decimal128(18, 0)
            }
            // Redshift system tables have a variety of odd types that come in as Other. Most of them can be safely treated
            // as Utf8, but oid is known BIGINT.
            (Redshift, Other(name)) if name.eq_ignore_ascii_case("oid") => DataType::Int64,
            (Redshift, Other(_)) => DataType::Utf8,
            // }}}

            // DuckDB {{{
            (DuckDB | GizmoSQL, Numeric(None) | BigNumeric(None)) => {
                // DuckDB's DECIMAL type without precision/scale defaults to DECIMAL(18, 3)
                // https://duckdb.org/docs/sql/data_types/numeric
                DataType::Decimal128(18, 3)
            }
            // }}}

            // Fabric {{{
            (Fabric, Numeric(None) | BigNumeric(None)) => {
                // Fabric's DECIMAL/NUMERIC type has a default precision of 18, and default scale of 0.
                // https://learn.microsoft.com/en-us/sql/t-sql/data-types/decimal-and-numeric-transact-sql?view=fabric
                DataType::Decimal128(18, 0)
            }
            (Fabric, DateTime) => arrow_timestamp(Some(6), None),
            // }}}

            // ClickHouse {{{
            (ClickHouse, Numeric(None) | BigNumeric(None)) => {
                // https://clickhouse.com/docs/sql-reference/data-types/decimal#parameters
                DataType::Decimal128(10, 0)
            }
            // https://github.com/ClickHouse/ClickHouse/blob/3196ab525aa/src/Processors/Formats/Impl/CHColumnToArrowColumn.cpp#L93-L96
            (ClickHouse, HugeInt) => DataType::FixedSizeBinary(16),
            (ClickHouse, Int256) => DataType::FixedSizeBinary(32),
            (ClickHouse, UHugeInt) => DataType::FixedSizeBinary(16),
            (ClickHouse, UInt256) => DataType::FixedSizeBinary(32),
            // https://github.com/ClickHouse/ClickHouse/blob/3196ab525aa/src/Processors/Formats/Impl/CHColumnToArrowColumn.cpp#L86
            (ClickHouse, Date(None)) => DataType::UInt16,
            // https://github.com/ClickHouse/ClickHouse/blob/3196ab525aa/src/Processors/Formats/Impl/CHColumnToArrowColumn.cpp#L87
            (ClickHouse, DateTime) => DataType::UInt32,
            (
                ClickHouse,
                Timestamp {
                    precision: None,
                    time_zone_spec: Without,
                },
            ) => DataType::UInt32,
            // }}}

            // Exasol {{{
            (Exasol, Numeric(None) | BigNumeric(None)) => {
                // Exasol default: DECIMAL(18, 0)
                DataType::Decimal128(18, 0)
            }
            // }}}

            // PostgreSQL {{{
            (Postgres | Salesforce, Numeric(None) | BigNumeric(None)) => {
                // PostgreSQL's NUMERIC type is truly arbitrary (precision can go to a 1000!). When
                // precision and scale are not specified, it's truly unconstrained. We pick the max
                // precision that fits in Decimal128 with a scale of 9 as a reasonable default,
                // but it doesn't reflect the full range supported by PostgreSQL.
                //
                // For more portability, PostgreSQL docs recommend users to always specify
                // precision and scale explicitly.
                //
                // https://www.postgresql.org/docs/current/datatype-numeric.html#DATATYPE-NUMERIC-DECIMAL
                DataType::Decimal128(38, 0)
            }
            // }}}
            (_, Numeric(None) | BigNumeric(None)) => DataType::Decimal128(38, 0),
            (_, TinyInt) => DataType::Int8,
            (_, SmallInt) => DataType::Int16,
            (_, Integer) => DataType::Int32,
            (_, BigInt) => DataType::Int64,
            // DuckDB-specific integer types
            // Arrow doesn't have native Int128, so we use Decimal128 for HugeInt
            (_, HugeInt) => DataType::Decimal128(38, 0),
            // Arrow doesn't have native Int256, so we use Decimal256 for Int256
            (_, Int256) => DataType::Decimal256(76, 0),
            (_, UTinyInt) => DataType::UInt8,
            (_, USmallInt) => DataType::UInt16,
            (_, UInteger) => DataType::UInt32,
            (_, UBigInt) => DataType::UInt64,
            // Arrow doesn't have native UInt128, so we use Decimal256 for UHugeInt
            (_, UHugeInt) => DataType::Decimal256(39, 0),
            // Arrow doesn't have native UInt256, so we use Decimal256 for UInt256
            (_, UInt256) => DataType::Decimal256(78, 0),

            (_, HalfFloat) => DataType::Float16,
            (_, Real) => DataType::Float32,
            (_, Float(_)) => DataType::Float32,
            (_, Double) => DataType::Float64,

            (_, Numeric(Some((p, s))) | BigNumeric(Some((p, s)))) => {
                let (p, s) = (*p, s.unwrap_or(0));
                if p <= 38 {
                    DataType::Decimal128(p, s)
                } else {
                    DataType::Decimal256(p, s)
                }
            }

            (_, Char(_)) => DataType::Utf8,
            (_, Varchar(..)) => DataType::Utf8,
            (_, Text) => DataType::Utf8,
            (_, Clob) => DataType::Utf8,
            (_, Blob) => DataType::Binary,
            (_, Binary(_)) => DataType::Binary,
            (_, Date(Some(bit_width))) if *bit_width > 32 => DataType::Date64,
            (_, Date(_)) => DataType::Date32,
            (
                backend,
                Time {
                    precision,
                    time_zone_spec: _, // TODO: handle timezones in TIME type
                },
            ) => {
                let time_unit = match (backend, precision) {
                    (_, Some(precision)) => suitable_time_unit(*precision),
                    // TIME's default precision on Snowflake is 9 (nanoseconds)
                    // https://docs.snowflake.com/en/sql-reference/data-types-datetime#time
                    (Snowflake, None) => TimeUnit::Nanosecond,
                    // TIME's default precision on Bigquery is 6 (microseconds)
                    // https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#time_type
                    (Bigquery, None) => TimeUnit::Microsecond,
                    // Databricks doesn't have the TIME type, so we use the precision of its
                    // TIMESTAMP type as the default here, which is 6 (microseconds).
                    // https://docs.databricks.com/aws/en/sql/language-manual/data-types/timestamp-type
                    (Databricks | Spark, None) => TimeUnit::Microsecond,
                    // TIME's default precision on Redshift is assumed to matche PostgreSQL's but
                    // nothing is mentioned in the docs page
                    // https://docs.aws.amazon.com/redshift/latest/dg/r_Date_and_time_literals.html#r_Date_and_time_literals-times
                    (Redshift, None) => TimeUnit::Microsecond,
                    // TIME's default precision on PostgreSQL is 6 (microseconds)
                    // https://www.postgresql.org/docs/current/datatype-datetime.html
                    (Postgres | Salesforce | DuckDB | GizmoSQL, None) => TimeUnit::Microsecond,
                    (Fabric, None) => {
                        // In SQL Server, the number enclosed in parenthesis is the fractional second scale, rather than precision.
                        //
                        // The default depends on the warehouse:
                        // - Fabric has no default and must be explicitly specified. limited to the 0-6 range.
                        // - Otherwise, default is 7 (100 ns) which can't be represented with `TimeUnit` currently.
                        //
                        // https://learn.microsoft.com/en-us/sql/t-sql/data-types/time-transact-sql?view=fabric
                        //
                        // TODO: fs#8086
                        TimeUnit::Nanosecond
                    }
                    // ClickHouse's `Time` type has a precision of 1 second:
                    // https://clickhouse.com/docs/sql-reference/data-types/time
                    // `Time64` has adjustable precision:
                    // https://clickhouse.com/docs/sql-reference/data-types/time64
                    (ClickHouse, None) => TimeUnit::Second,
                    // Athena (Presto/Trino-based) TIME has millisecond precision
                    // https://docs.aws.amazon.com/athena/latest/ug/data-types.html
                    (Athena, None) => TimeUnit::Millisecond,
                    (Exasol, None) => TimeUnit::Millisecond,
                    (_, None) => {
                        // we pick microseconds as a reasonable default
                        TimeUnit::Microsecond
                    }
                };
                match time_unit {
                    TimeUnit::Second | TimeUnit::Millisecond => DataType::Time32(time_unit),
                    TimeUnit::Microsecond | TimeUnit::Nanosecond => DataType::Time64(time_unit),
                }
            }
            (
                backend,
                Timestamp {
                    precision,
                    time_zone_spec,
                },
            ) => {
                match (backend, time_zone_spec) {
                    (_, Local) => arrow_timestamp_with_local_tz(*precision),
                    (_, With) => arrow_timestamp_tz(*precision),

                    // Databricks TIMESTAMP and TIMESTAMP_LTZ are both local-tz timestamps
                    (Databricks, Without | Unspecified) => {
                        arrow_timestamp_with_local_tz(*precision)
                    }
                    // This is configuration on Snowflake (!), but the default for TIMESTAMP
                    // is to be an alias for TIMESTAMP_NTZ which is what we assume here.
                    (Snowflake, Unspecified) => arrow_timestamp(*precision, None),

                    (Bigquery, Unspecified) => arrow_timestamp(*precision, Some("UTC".into())),

                    (_, Without | Unspecified) => arrow_timestamp(*precision, None),
                    (_, Fixed(TimeZone::Named(name))) => {
                        let arrow_tz = if name.eq_ignore_ascii_case("UTC") {
                            Some("UTC".into())
                        } else if name.eq_ignore_ascii_case("Europe/Berlin") {
                            Some("Europe/Berlin".into())
                        } else {
                            debug_assert!(
                                false,
                                "TODO: Figure how to lookup Arrow timezones from a table provided by arrow-rs"
                            );
                            None
                        };
                        arrow_timestamp(*precision, arrow_tz)
                    }
                }
            }
            // A DATETIME is a timestamp without time zone information
            (backend, DateTime) => DataType::Timestamp(default_time_unit(backend), None),

            (backend, Interval(fields)) => {
                use DateTimeField::*;
                use IntervalUnit::*;
                let interval_unit = match backend {
                    Snowflake => MonthDayNano, // XXX: intervals types are not supported on Snowflake, only value literals
                    Databricks | Spark | Redshift => {
                        // ## Databricks
                        //
                        //     INTERVAL { yearMonthIntervalQualifier | dayTimeIntervalQualifier }
                        //
                        // ## Redshift
                        //
                        //       INTERVAL year_to_month_qualifier
                        //     | INTERVAL day_to_second_qualifier [ (fractional_precision) ]
                        //
                        //  The maximum value of `fractional_precision` is 6 and it only appears
                        //  after SECOND in day_to_second_qualifier. This parser turns `SECOND(3)`
                        //  into `DateTimeField::Millisecond` simplifying the processing below.
                        //
                        // https://docs.databricks.com/aws/en/sql/language-manual/data-types/interval-type
                        // https://docs.aws.amazon.com/redshift/latest/dg/r_interval_data_types.html
                        fields
                            .map(|range| match range {
                                // yearMonthIntervalQualifier: { YEAR [TO MONTH] | MONTH }
                                //
                                // YEAR
                                // MONTH
                                // YEAR TO MONTH
                                (Year, None) | (Year, Some(Month)) | (Month, None) => YearMonth,

                                // ## Databricks
                                //
                                //     dayTimeIntervalQualifier:
                                //       { DAY    [TO { HOUR | MINUTE | SECOND } ] |
                                //         HOUR   [TO {        MINUTE | SECOND } ] |
                                //         MINUTE [TO                   SECOND] |
                                //         SECOND }
                                //
                                // ## Redshift (`INTERVAL day_to_second_qualifier [ (fractional_precision) ]`)
                                //
                                //       DAY
                                //     | HOUR
                                //     | MINUTE
                                //     | SECOND [ (fractional_precision) ]
                                //     | DAY TO HOUR
                                //     | DAY TO MINUTE
                                //     | DAY TO SECOND [ (fractional_precision) ]
                                //     | HOUR TO MINUTE
                                //     | HOUR TO SECOND [ (fractional_precision) ]
                                //     | MINUTE TO SECOND [ (fractional_precision) ]
                                (Day, None | Some(Hour | Minute | Second))
                                | (Hour, None | Some(Minute | Second))
                                | (Minute, None | Some(Second))
                                | (Second, None) => DayTime,
                                // -- Redshift-specific
                                (Day | Hour | Minute | Second, Some(Millisecond)) => DayTime,
                                (Day | Hour | Minute | Second, Some(Microsecond)) => MonthDayNano,

                                // not supported directly, but we map it in some reasonable way
                                (Year, Some(Year)) | (Month, Some(Month)) => {
                                    YearMonth
                                }
                                (Day, Some(Day))
                                | (Hour, Some(Hour))
                                | (Minute, Some(Minute))
                                | (Second, Some(Second))
                                | (Millisecond, None | Some(Millisecond)) => DayTime,
                                // -- anything more precise than millis, requires MonthDayNano
                                (Microsecond, _)
                                | (Nanosecond, _)
                                | (_, Some(Microsecond))
                                | (_, Some(Nanosecond))
                                // -- anything outside of Year-Month or Day-Time ranges
                                | (
                                    Year | Month,
                                    Some(Day | Hour | Minute | Second | Millisecond),
                                )
                                // -- inverted patterns (here so we can rely on exhaustiveness checking)
                                | (
                                    Month | Day | Hour | Minute | Second | Millisecond,
                                    Some(Year),
                                )
                                | (Day | Hour | Minute | Second | Millisecond, Some(Month))
                                | (Hour | Minute | Second | Millisecond, Some(Day))
                                | (Minute | Second | Millisecond, Some(Hour))
                                | (Second | Millisecond, Some(Minute))
                                | (Millisecond, Some(Second)) => MonthDayNano,
                            })
                            .unwrap_or(
                                // INTERVAL in Databricks must have fields specified, but if not,
                                // we pick `MonthDayNano` as a reasonable default that can represent
                                // all possible values.
                                MonthDayNano,
                            )
                    }
                    Bigquery | Postgres | DuckDB | GizmoSQL => MonthDayNano, // MonthDayNano is exactly what BQ and PG use internally
                    // FIXME: ClickHouse doesn't actually seem to support Arrow's Interval
                    ClickHouse => MonthDayNano,
                    Exasol => MonthDayNano,
                    Salesforce => MonthDayNano, // Salesforce seems to follow PostgreSQL
                    Fabric => MonthDayNano, // SQL Server doesn't appear to have an INTERVAL type
                    // Athena (Presto/Trino-based) uses MonthDayNano as a reasonable default
                    Athena => MonthDayNano,
                    _ => MonthDayNano, // Reasonable default
                };
                DataType::Interval(interval_unit)
            }

            (_, Json) => DataType::Utf8,
            (_, Jsonb) => {
                not_explicitly_handled!(self);
                DataType::Utf8
            }
            // UUID is 16 bytes, we represent it as FixedSizeBinary(16)
            (_, Uuid) => DataType::FixedSizeBinary(16),
            (_, IPv4) => DataType::UInt32,
            (_, IPv6) => DataType::FixedSizeBinary(16),
            (_, Enum(_, EnumAttrs { bit_width: Some(8) })) => DataType::Int8,
            (
                _,
                Enum(
                    _,
                    EnumAttrs {
                        bit_width: Some(16),
                    },
                ),
            ) => DataType::Int16,
            (_, Enum(..)) => DataType::Utf8,
            (Snowflake, Variant) => DataType::Binary,
            (_, Variant) => DataType::Utf8,
            // Exasol's ADBC driver (exarrow-rs) maps HASHTYPE values to Binary
            (_, HashType(_)) => DataType::Binary,
            (_, Geometry(_)) => DataType::Utf8,
            (_, Geography(_)) => DataType::Utf8,
            (_, Array(Some(inner_sql_type))) => {
                let inner_sql_type_string = inner_sql_type.to_string(backend);
                let inner_ty = inner_sql_type.pick_best_arrow_type(backend);
                let inner_metadata = {
                    let mut metadata = HashMap::new();
                    metadata.insert(
                        metadata_sql_type_key(backend).to_string(),
                        inner_sql_type_string,
                    );
                    metadata
                };
                let inner_field = Field::new("item", inner_ty, true).with_metadata(inner_metadata);
                DataType::List(Arc::new(inner_field))
            }
            (_, Array(None)) => DataType::List(Arc::new(Field::new("item", DataType::Utf8, true))),
            (_, Struct(fields)) => {
                let arrow_fields = match fields {
                    Some(struct_fields) => {
                        let arrow_fields_vec = struct_fields
                            .iter()
                            .map(
                                |StructField {
                                     name,
                                     sql_type,
                                     nullable,
                                     comment_tok,
                                 }| {
                                    let inner_ty = sql_type.pick_best_arrow_type(backend);

                                    // XXX: can't preserve the original quotes here because an
                                    // Arrow `Field` is meant to keep a name w/o quotes.
                                    //
                                    // TODO(felipecrv): figure out how to preserve quoting status of
                                    // identifiers through Arrow types (e.g. using metadata)
                                    //
                                    // `name.display(backend)` can't be used because it might
                                    // render quotes which are not desired in Arrow field names.
                                    let name = name.to_string_lossy();
                                    // render the SQL type according to the backend-specific syntax
                                    let sql_type_string = sql_type.to_string(backend);

                                    let metadata = {
                                        let mut metadata = HashMap::new();
                                        metadata.insert(
                                            metadata_sql_type_key(backend).to_string(),
                                            sql_type_string,
                                        );
                                        if let Some(tok) = comment_tok {
                                            metadata.insert("comment".to_string(), tok.clone());
                                        }
                                        metadata
                                    };
                                    Field::new(name, inner_ty, *nullable).with_metadata(metadata)
                                },
                            )
                            .collect::<Vec<_>>();
                        Fields::from(arrow_fields_vec)
                    }
                    None => Fields::empty(),
                };
                DataType::Struct(arrow_fields)
            }
            (backend, Map(inner)) => {
                let fallback = Varchar(None, Default::default());
                let (key_type, value_type) = inner
                    .as_ref()
                    .map(|(k, v)| (k.as_ref(), v.as_ref()))
                    .unwrap_or_else(|| (&fallback, &fallback));
                let entries_struct = Struct(Some(vec![
                    StructField::new(
                        Ident::Plain("key".to_string()),
                        key_type.clone(),
                        false, // keys must be non-nullable in Arrow
                    ),
                    StructField::new(
                        Ident::Plain("value".to_string()),
                        value_type.clone(),
                        true, // values are nullable
                    ),
                ]));
                let entries = Field::new(
                    "entries",
                    entries_struct.pick_best_arrow_type(backend),
                    false,
                );
                DataType::Map(Arc::new(entries), false)
            }
            (_, Void) => DataType::Null,
            (_, Other(_)) => {
                not_explicitly_handled!(self);
                DataType::Utf8
            }
        };
        data_type
    }
}

pub struct SqlTypeDisplay<'a>(&'a SqlType, AdapterType);

impl fmt::Display for SqlTypeDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // TODO(felipecrv): change SqlType::write to take a [fmt::Write] instead of
        // [String] so I can pass `f` directly here.
        let mut buffer = String::new();
        self.0.write(self.1, &mut buffer)?;
        write!(f, "{}", buffer)
    }
}

// We want drivers, CSV parsers and anything producing Arrow schemas
// that interact with SQL data warehouses to inform us about the SQL type
// they intend to use for each field. These are the metadata keys we
// are supporting for each backend.
//
// The first one is the one we use when writing the Arrow schema, but when
// reading we check all of them in order to be compatible with existing
// schemas that might have been written using different metadata keys.
const POSTGRES_KEYS: [&str; 2] = ["POSTGRES:type", "type_text"];
const SNOWFLAKE_KEYS: [&str; 2] = ["SNOWFLAKE:type", "type_text"];
const BIGQUERY_KEYS: [&str; 2] = ["BIGQUERY:type", "type_text"];
const DATABRICKS_KEYS: [&str; 2] = ["DBX:type", "type_text"];
const REDSHIFT_KEYS: [&str; 2] = ["REDSHIFT:type", "type_text"];
const DUCKDB_KEYS: [&str; 2] = ["DUCKDB:type", "type_text"];
const CLICKHOUSE_KEYS: [&str; 2] = ["CLICKHOUSE:type", "type_text"];
const EXASOL_KEYS: [&str; 2] = ["EXASOL:type", "type_text"];
const SPARK_KEYS: [&str; 2] = ["SPARK:type", "type_text"];
const SQLSERVER_KEYS: [&str; 2] = ["SQLSERVER:type", "type_text"];
const GENERIC_KEYS: [&str; 2] = ["SQL:type", "type_text"];
// The "ATHENA:type" key is emitted by adbc_driver_athena in Arrow field metadata
// (see https://github.com/dbt-labs/athena/issues/6).
const ATHENA_KEYS: [&str; 1] = ["ATHENA:type"];

fn metadata_type_candidate_keys(backend: AdapterType) -> &'static [&'static str] {
    match backend {
        AdapterType::Postgres | AdapterType::Salesforce => &POSTGRES_KEYS,
        AdapterType::Snowflake => &SNOWFLAKE_KEYS,
        AdapterType::Bigquery => &BIGQUERY_KEYS,
        AdapterType::Databricks => &DATABRICKS_KEYS,
        AdapterType::Spark => &SPARK_KEYS,
        AdapterType::Redshift => &REDSHIFT_KEYS,
        AdapterType::DuckDB | AdapterType::LakeCompute | AdapterType::GizmoSQL => &DUCKDB_KEYS,
        AdapterType::Fabric => &SQLSERVER_KEYS,
        AdapterType::ClickHouse => &CLICKHOUSE_KEYS,
        AdapterType::Athena => &ATHENA_KEYS,
        AdapterType::Exasol => &EXASOL_KEYS,
        _ => &GENERIC_KEYS,
    }
}

pub fn metadata_sql_type_key(backend: AdapterType) -> &'static str {
    metadata_type_candidate_keys(backend)[0]
}

/// Get the type string metadata from an Arrow `Field` for a given backend.
pub fn original_type_string(backend: AdapterType, field: &Field) -> Option<&String> {
    metadata_type_candidate_keys(backend)
        .iter()
        .find_map(|&k| field.metadata().get(k))
}

fn eqi(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[derive(Debug)]
enum ParseError<'source> {
    UnexpectedEndOfInput,
    Unexpected(Token<'source>),
    ParseIntError(ParseIntError),
    UnclosedQuote(char),
    ExpectedDateTimeField,
}

impl Error for ParseError<'_> {}

impl fmt::Display for ParseError<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::UnexpectedEndOfInput => write!(f, "unexpected end of input"),
            ParseError::Unexpected(token) => write!(f, "unexpected token: {token:}"),
            ParseError::ParseIntError(err) => write!(f, "{err}"),
            ParseError::UnclosedQuote(quote) => write!(f, "'{}' is not closed", *quote),
            ParseError::ExpectedDateTimeField => {
                write!(
                    f,
                    "expected a date/time field (e.g. YEAR, DAY, SECOND, etc.)"
                )
            }
        }
    }
}

impl From<ParseIntError> for ParseError<'_> {
    fn from(err: ParseIntError) -> Self {
        ParseError::ParseIntError(err)
    }
}

/// Converts a [Token::Word] to an identifier by removing quotes and resolving escape sequences.
///
/// NOTE: uppercasing IS NOT performed, callers should use [eqi] if case-insensitive comparison is
/// needed. PostgreSQL docs, for instance, say "Quoting an identifier also makes it case-sensitive" [1].
///
/// [1] https://www.postgresql.org/docs/current/sql-syntax-lexical.html#SQL-SYNTAX-IDENTIFIERS
fn word2ident<'source>(word: String, backend: AdapterType) -> Result<Ident, ParseError<'source>> {
    let mut bytes = word.bytes();
    let first_byte = bytes.next().ok_or(ParseError::UnexpectedEndOfInput)?;
    let is_quoted = [b'\'', b'"', b'`'].contains(&first_byte);

    // TODO: handle the U&"..." quoted identifiers in PostgreSQL

    if is_quoted {
        // If the first byte is a quote, the last byte must be the same quote.
        // This is not enough to validate the quoted identifier, but it's a necessary
        // condition. More thorough validation is done in _unescape_quoted_ident.
        //
        // For instance, "abc"" passes this check, but is not a valid quoted identifier
        // because the last "" are a escaped "" and the closing quote is missing.
        let open_ended = match bytes.next_back() {
            Some(b) => b != first_byte,
            None => true,
        };
        if open_ended {
            let err = ParseError::UnclosedQuote(first_byte as char);
            return Err(err);
        }
        let s = _unescape_quoted_ident(word.as_ref(), first_byte, backend)?;
        Ok(Ident::Unquoted(first_byte.try_into().unwrap(), s))
    } else {
        // Plain identifier, return as is
        Ok(Ident::Plain(word))
    }
}

/// Unescape a quoted identifier based on the backend rules.
///
/// PRE-CONDITIONS:
/// - `quote` is one of: `"`, `'`, or `` ` ``
/// - `word` is quoted with the same quote character at the start and end.
fn _unescape_quoted_ident<'source>(
    word: &str,
    quote: u8,
    backend: AdapterType,
) -> Result<String, ParseError<'source>> {
    use AdapterType::*;

    debug_assert!(word.len() >= 2);
    debug_assert!(word.as_bytes()[0] == quote);
    debug_assert!(word.as_bytes()[word.len() - 1] == quote);

    let inner = &word[1..word.len() - 1];
    // TODO: review all the ident escaping rules for different backends here
    let unescaped_string = match (backend, quote) {
        (Postgres | Redshift, b'"') => {
            // In PostgreSQL, double quotes are escaped by doubling them
            inner.replace("\"\"", "\"")
        }
        (_, b'\'') => {
            // In SQL, single quotes are escaped by doubling them
            inner.replace("''", "'")
        }
        _ => inner.to_string(),
    };
    Ok(unescaped_string)
}

struct Parser<'source> {
    tokenizer: Tokenizer<'source>,
}

impl<'source> Parser<'source> {
    pub fn new(input: &'source str) -> Self {
        Parser {
            tokenizer: Tokenizer::new(input),
        }
    }

    // Basic token operations

    fn next(&mut self) -> Result<Token<'source>, ParseError<'source>> {
        self.tokenizer
            .next()
            .ok_or(ParseError::UnexpectedEndOfInput)
    }

    fn expect(&mut self, expected: Token<'source>) -> Result<(), ParseError<'source>> {
        let tok = self.next()?;
        if tok == expected {
            Ok(())
        } else {
            Err(ParseError::Unexpected(tok))
        }
    }

    fn match_(&mut self, pat: Token<'source>) -> bool {
        self.tokenizer.match_(move |tok| tok == pat)
    }

    fn match_word(&mut self, word: &'source str) -> bool {
        self.match_(Token::Word(word))
    }

    fn next_int<T>(&mut self) -> Result<T, ParseError<'source>>
    where
        T: std::str::FromStr<Err = ParseIntError>,
    {
        let tok = self.next()?;
        if let Token::Word(w) = tok {
            let value = w.parse::<T>()?;
            Ok(value)
        } else {
            Err(ParseError::Unexpected(tok))
        }
    }

    // Grammar productions

    /// Parse optional parenthesized integer value (e.g. `(3)`).
    fn precision<T>(&mut self) -> Result<Option<T>, ParseError<'source>>
    where
        T: std::str::FromStr<Err = ParseIntError>,
    {
        if self.match_(Token::LParen) {
            let value = self.next_int::<T>()?;
            self.expect(Token::RParen)?;
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    fn precision_and_scale(&mut self) -> Result<Option<(u8, Option<i8>)>, ParseError<'source>> {
        if self.match_(Token::LParen) {
            let precision = self.next_int::<u8>()?;
            let scale = if self.match_(Token::Comma) {
                Some(self.next_int::<i8>()?)
            } else {
                None
            };
            self.expect(Token::RParen)?;
            Ok(Some((precision, scale)))
        } else {
            Ok(None)
        }
    }

    fn time_zone_spec(&mut self) -> Result<TimeZoneSpec, ParseError<'source>> {
        if self.match_word("WITH") {
            let local = self.match_word("LOCAL");
            self.expect(Token::Word("TIME"))?;
            self.expect(Token::Word("ZONE"))?;
            Ok(if local {
                TimeZoneSpec::Local
            } else {
                TimeZoneSpec::With
            })
        } else if self.match_word("WITHOUT") {
            self.expect(Token::Word("TIME"))?;
            self.expect(Token::Word("ZONE"))?;
            Ok(TimeZoneSpec::Without)
        } else {
            Ok(TimeZoneSpec::Unspecified)
        }
    }

    fn datetime_field(&mut self) -> Option<DateTimeField> {
        self.tokenizer.peek_and_then(|tok| {
            if let Token::Word(w) = tok {
                let field = if eqi(w, "YEAR") {
                    DateTimeField::Year
                } else if eqi(w, "MONTH") {
                    DateTimeField::Month
                } else if eqi(w, "DAY") {
                    DateTimeField::Day
                } else if eqi(w, "HOUR") {
                    DateTimeField::Hour
                } else if eqi(w, "MINUTE") {
                    DateTimeField::Minute
                } else if eqi(w, "SECOND") {
                    DateTimeField::Second
                } else if eqi(w, "MILLISECOND") {
                    DateTimeField::Millisecond
                } else if eqi(w, "MICROSECOND") {
                    DateTimeField::Microsecond
                } else if eqi(w, "NANOSECOND") {
                    DateTimeField::Nanosecond
                } else {
                    return None;
                };
                Some(field)
            } else {
                None
            }
        })
    }

    fn interval_qualifier(
        &mut self,
    ) -> Result<Option<(DateTimeField, Option<DateTimeField>)>, ParseError<'source>> {
        if let Some(start) = self.datetime_field() {
            if self.match_word("TO") {
                let end = self.datetime_field();
                if end.is_some() {
                    // XXX: validate end is higher resolution than start unit?
                    return Ok(Some((start, end)));
                } else {
                    return Err(ParseError::ExpectedDateTimeField);
                }
            }
            Ok(Some((start, None)))
        } else {
            Ok(None)
        }
    }

    /// SRID for Geography type for some backends (e.g. Databricks [1]).
    ///
    ///     [ '(' \d+ | 'ANY' ')' ]
    ///
    /// [1] https://docs.databricks.com/aws/en/sql/language-manual/data-types/geography-type
    fn srid(&mut self) -> Result<Option<&str>, ParseError<'source>> {
        if self.match_(Token::LParen) {
            let srid = self.tokenizer.peek_and_then(move |tok| match tok {
                Token::Word(any) if eqi(any, "ANY") => Some("ANY"),
                Token::Word(word) => {
                    if word.chars().all(|c| c.is_ascii_digit()) {
                        Some(word)
                    } else {
                        None
                    }
                }
                _ => None,
            });
            if srid.is_none() {
                return Err(ParseError::Unexpected(self.next()?));
            }
            self.expect(Token::RParen)?;
            Ok(srid)
        } else {
            Ok(None)
        }
    }

    fn nullable(&mut self) -> Result<Option<bool>, ParseError<'source>> {
        if self.match_word("NOT") {
            self.expect(Token::Word("NULL"))?;
            Ok(Some(false))
        } else if self.match_word("NULLABLE") {
            Ok(Some(true))
        } else {
            Ok(None)
        }
    }

    /// Parse an identifier, which can be a quoted or unquoted word.
    #[allow(dead_code)]
    fn identifier(&mut self, backend: AdapterType) -> Result<Ident, ParseError<'source>> {
        let tok = self.next()?;
        match tok {
            Token::Word(w) => word2ident(w.to_string(), backend),
            _ => Err(ParseError::Unexpected(tok)),
        }
    }

    /// Parse a string literal.
    ///
    /// Currently used only on Databricks struct field comments (after COMMENT).
    fn string_literal(&mut self) -> Result<Token<'source>, ParseError<'source>> {
        let tok = self.next()?;
        // TODO: check that token starts with a quote for the given backend
        Ok(tok)
    }

    /// Parse the token that comes after COLLATE in a type definition.
    ///
    /// The token is returned *AS IT IS* in the input string, without removing
    /// the quotes or unescaping anything.
    ///
    /// On Snowflake it seems to be a string literal since it uses single quotes
    /// and the characterr for quoting identifiers in Snowflake is the double quote.
    ///
    /// On Databricks it seems to be an identifier. I haven't found examples where this
    /// identifier is quoted because no collation name contains special characters.
    fn collation_spec(&mut self, _backend: AdapterType) -> Result<String, ParseError<'source>> {
        let tok = self.next()?;
        Ok(tok.to_string())
    }

    fn string_attrs(&mut self, backend: AdapterType) -> Result<StringAttrs, ParseError<'source>> {
        let mut collate_spec = None;
        loop {
            if collate_spec.is_none() && self.match_word("COLLATE") {
                collate_spec = Some(self.collation_spec(backend)?);
                continue;
            }
            break;
        }
        Ok(StringAttrs { collate_spec })
    }

    /// Parse the inner fields of a struct type after `(` or after `STRUCT<`.
    ///
    /// `terminator` is either `Token::RParen` or `Token::RAndle`.
    fn struct_fields(
        &mut self,
        backend: AdapterType,
        terminator: Token<'source>,
    ) -> Result<Vec<StructField>, ParseError<'source>> {
        let mut fields = Vec::new();
        loop {
            let tok = self.next()?;
            let name = match tok {
                tok if tok == terminator => break,
                Token::Word(w) => word2ident(w.to_string(), backend)?,
                _ => {
                    let e = ParseError::Unexpected(tok);
                    return Err(e);
                }
            };

            // Databricks supports an optional `:` after the field name,
            // so we consume it if present.
            let _ = self.match_(Token::Colon);

            let (ty, nullable) = self.parse_constrained_type(backend)?;
            // Assume nullable if NOT NULL or NULLABLE is not specified for the field
            let nullable = nullable.unwrap_or(true);

            // Databricks struct fields can have additional attributes:
            //
            //     [COLLATE <ident>] [COMMENT <string>]
            //
            // We also allow COLLATE to happen after COMMENT, but if we find a COLLATE
            // like that, we have to place it in the attributes of the string type itself.
            let mut collate: Option<String> = None;
            let mut comment: Option<Token> = None;
            loop {
                if collate.is_none() && self.match_word("COLLATE") {
                    collate = Some(self.collation_spec(backend)?);
                    continue;
                }
                if comment.is_none() && self.match_word("COMMENT") {
                    comment = Some(self.string_literal()?);
                    continue;
                }
                break;
            }
            let sql_type = match (ty, collate) {
                (SqlType::Varchar(max_len, attrs), Some(collate_spec)) => {
                    let mut attrs = attrs;
                    attrs.collate_spec = Some(collate_spec);
                    SqlType::Varchar(max_len, attrs)
                }
                (ty, Some(_)) => {
                    debug_assert!(false, "COLLATE specified for a non-string type");
                    ty
                }
                (ty, _) => ty,
            };

            let mut field = StructField::new(name, sql_type, nullable);
            if let Some(tok) = comment {
                field.comment_tok = Some(tok.to_string());
            }
            fields.push(field);

            let tok = self.next()?;
            match tok {
                Token::Comma => continue,
                tok if tok == terminator => break,
                _ => {
                    let e = ParseError::Unexpected(tok);
                    return Err(e);
                }
            }
        }
        Ok(fields)
    }

    /// Parse the (optional) entries of an enum type including the `(` and `)` delimiters.
    fn enum_entries(&mut self) -> Result<Option<Vec<EnumEntry>>, ParseError<'source>> {
        let entries = if self.match_(Token::LParen) {
            let mut entries = Vec::new();
            if !self.match_(Token::RParen) {
                loop {
                    let label_tok = self.next()?;
                    let Token::Word(label) = label_tok else {
                        return Err(ParseError::Unexpected(label_tok));
                    };
                    let label = Ident::Plain(label.to_string());
                    // Parse optional `= <value>`. The tokenizer may produce
                    // `Word("=1")` (no space) or `Word("=")` + `Word("1")`
                    // (with space).
                    let eq_rest = self.tokenizer.peek_and_then(|tok| {
                        if let Token::Word(w) = tok {
                            if let Some(rest) = w.strip_prefix('=') {
                                return Some(rest.to_string());
                            }
                        }
                        None
                    });
                    let value = match eq_rest {
                        Some(rest) if rest.is_empty() => Some(self.next_int::<i64>()?),
                        Some(rest) => Some(rest.trim().parse::<i64>().map_err(ParseError::from)?),
                        None => None,
                    };
                    entries.push(EnumEntry(label, value));
                    if !self.match_(Token::Comma) {
                        break;
                    }
                }
                self.expect(Token::RParen)?;
            }
            Some(entries)
        } else {
            None
        };
        Ok(entries)
    }

    /// Parse a SQL type that might have a NOT NULL constraint.
    fn parse_constrained_type(
        &mut self,
        backend: AdapterType,
    ) -> Result<(SqlType, Option<bool>), ParseError<'source>> {
        let sql_type = self.parse_unconstrained_type(backend)?;
        let nullable = self.nullable()?;
        Ok((sql_type, nullable))
    }

    // External API

    /// Parse the SQL type and return it along with a boolean indicating if its nullable.
    fn parse(&mut self, backend: AdapterType) -> Result<(SqlType, bool), ParseError<'source>> {
        let (ty, nullable) = self.parse_constrained_type(backend)?;
        Ok((ty, nullable.unwrap_or(true)))
    }

    /// Parse a column description: identifier, optional colon, type, optional NOT NULL,
    /// optional COLLATE, optional COMMENT.
    fn parse_column_description(
        &mut self,
        backend: AdapterType,
        identifier_optional: bool,
    ) -> Result<ColumnDescription, ParseError<'source>> {
        let name = if identifier_optional {
            // Try to parse a leading identifier. The first word is an identifier if:
            // - It's followed by `:` (Databricks `name: TYPE` syntax), or
            // - It's followed by another word that is not a constraint keyword
            //   (i.e. followed by a type keyword)
            //
            // If neither condition holds, try_() resets position and we parse as type-only.
            self.tokenizer
                .try_(|tokenizer| {
                    let Token::Word(w) = tokenizer.next()? else {
                        return None;
                    };
                    // A `:` after the word means it's definitely an identifier
                    if tokenizer.match_(|t| t == Token::Colon) {
                        return Some(w.to_string());
                    }
                    // Peek at what follows: if it's a type-starting word, the first word
                    // was an identifier. peek_and_then resets on None, advances on Some.
                    // We DON'T want to advance past the type keyword, so we always return
                    // None from the peek (just using it to inspect) and decide outside.
                    let mut next_is_type_start = false;
                    tokenizer.peek_and_then(|next| {
                        if let Token::Word(next_w) = next {
                            if !eqi(next_w, "NOT")
                                && !eqi(next_w, "NULL")
                                && !eqi(next_w, "NULLABLE")
                                && !eqi(next_w, "COLLATE")
                                && !eqi(next_w, "COMMENT")
                            {
                                next_is_type_start = true;
                            }
                        }
                        None::<()>
                    });
                    next_is_type_start.then_some(w.to_string())
                })
                .map(|w| word2ident(w, backend))
                .transpose()?
        } else {
            let tok = self.next()?;
            let ident = match tok {
                Token::Word(w) => word2ident(w.to_string(), backend)?,
                _ => return Err(ParseError::Unexpected(tok)),
            };
            // Databricks supports `name: TYPE` syntax
            let _ = self.match_(Token::Colon);
            Some(ident)
        };

        self.parse_column_description_body(backend, name)
    }

    fn parse_column_description_body(
        &mut self,
        backend: AdapterType,
        name: Option<Ident>,
    ) -> Result<ColumnDescription, ParseError<'source>> {
        let (sql_type, nullable) = self.parse_constrained_type(backend)?;
        let nullable = nullable.unwrap_or(true);

        // Parse optional trailing attributes: COLLATE and COMMENT
        let mut collate: Option<String> = None;
        let mut comment: Option<String> = None;
        loop {
            if collate.is_none() && self.match_word("COLLATE") {
                collate = Some(self.collation_spec(backend)?);
                continue;
            }
            if comment.is_none() && self.match_word("COMMENT") {
                comment = Some(self.string_literal()?.to_string());
                continue;
            }
            break;
        }

        // Fold COLLATE into the type if it's a string type
        let sql_type = match (sql_type, collate) {
            (SqlType::Varchar(max_len, mut attrs), Some(collate_spec)) => {
                attrs.collate_spec = Some(collate_spec);
                SqlType::Varchar(max_len, attrs)
            }
            (ty, _) => ty,
        };

        Ok(ColumnDescription {
            name,
            sql_type,
            nullable,
            comment,
        })
    }

    fn parse_unconstrained_type(
        &mut self,
        backend: AdapterType,
    ) -> Result<SqlType, ParseError<'source>> {
        use AdapterType::*;
        let mut sql_type = self.parse_inner(backend)?;
        // postfix-[] syntax for arrays in Postgres and Generic SQL
        if matches!(backend, Postgres | Redshift) {
            while self.match_(Token::LBracket) {
                self.expect(Token::RBracket)?;
                sql_type = SqlType::Array(Some(Box::new(sql_type)));
            }
        }
        Ok(sql_type)
    }

    /// Parse the SQL type string without consuming the entire string.
    ///
    /// The goal of this function is to create the `SqlType` instance that better represents
    /// the syntax of the SQL type string. For instance, the fact that Snowflake's FLOAT is
    /// actually a DOUBLE PRECISION under the hood, only becomes relevant when picking storage
    /// data structures for the values of this type.
    ///
    /// This parser, parses, it does not validate. If Bigquery accepts BOOL as a synonym for
    /// BOOLEAN, then this parser will accept it too no matter the backend passed to it.
    /// Weirder types like FLOAT4 and FLOAT8 are guarded by a backend check, just in case
    /// some other system in the future decided they mean 4 or 8 bits instead of 4 or 8
    /// bytes like Snowflake.
    ///
    /// https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types
    /// https://docs.snowflake.com/en/sql-reference/intro-summary-data-types
    fn parse_inner(&mut self, backend: AdapterType) -> Result<SqlType, ParseError<'source>> {
        use AdapterType::*;
        let tok = self.next()?;
        let sql_type = match tok {
            Token::LParen => {
                // Structs in Postgres and ClickHouse are defined by enclosing the fields in parentheses:
                //
                //     CREATE TYPE item_info AS (
                //         product_id INT,
                //         qty        INT
                //     );
                if matches!(backend, Postgres | Redshift | ClickHouse) {
                    let fields = self.struct_fields(backend, Token::RParen)?;
                    SqlType::Struct(Some(fields))
                } else {
                    return Err(ParseError::Unexpected(tok));
                }
            }
            Token::RParen
            | Token::LBracket
            | Token::RBracket
            | Token::LAngle
            | Token::RAngle
            | Token::Comma
            | Token::Colon => {
                return Err(ParseError::Unexpected(tok));
            }
            Token::Word(w) => {
                if eqi(w, "BOOLEAN") || eqi(w, "BOOL") {
                    SqlType::Boolean
                } else if eqi(w, "TINYINT") || eqi(w, "BYTEINT") {
                    SqlType::TinyInt
                } else if eqi(w, "SMALLINT")
                    || (eqi(w, "INT2") || eqi(w, "SMALLSERIAL") || eqi(w, "SERIAL2"))
                {
                    SqlType::SmallInt
                } else if eqi(w, "INTEGER")
                    || eqi(w, "INT")
                    || eqi(w, "INT4")
                    || eqi(w, "SERIAL")
                    || eqi(w, "SERIAL4")
                {
                    SqlType::Integer
                } else if eqi(w, "INT8") {
                    if backend == ClickHouse {
                        SqlType::TinyInt // ClickHouse: Int8 = 8-bits
                    } else {
                        // In standard SQL, INT8 = 8 bytes (64 bits)
                        SqlType::BigInt
                    }
                } else if eqi(w, "BIGINT")
                    || eqi(w, "INT64") // DuckDB, ClickHouse...
                    || eqi(w, "BIGSERIAL")
                    || eqi(w, "SERIAL8")
                {
                    SqlType::BigInt // 64 bits
                } else if eqi(w, "HUGEINT") || eqi(w, "INT128") {
                    SqlType::HugeInt // DuckDB: 128-bit signed integer
                } else if eqi(w, "UINT8") {
                    if backend == ClickHouse {
                        SqlType::UTinyInt // ClickHouse: UInt8 = 8-bits
                    } else {
                        SqlType::UBigInt // DuckDB: UINT8 = 8 bytes (64 bits)
                    }
                } else if eqi(w, "UINT16") && backend == ClickHouse {
                    SqlType::USmallInt // ClickHouse: UInt16 (16-bit unsigned integer)
                } else if eqi(w, "UINT32") && backend == ClickHouse {
                    SqlType::UInteger // ClickHouse: UInt32 (32-bit unsigned integer)
                } else if eqi(w, "UINT64") && backend == ClickHouse {
                    SqlType::UBigInt // ClickHouse: UInt64 (64-bit unsigned integer)
                } else if eqi(w, "UINT128") && backend == ClickHouse {
                    SqlType::UHugeInt // ClickHouse: UInt128 (128-bit unsigned integer)
                } else if eqi(w, "UINT256") {
                    SqlType::UInt256
                } else if eqi(w, "UTINYINT") || eqi(w, "UINT1") {
                    SqlType::UTinyInt // DuckDB: unsigned 8-bit integer
                } else if eqi(w, "USMALLINT") || eqi(w, "UINT2") {
                    SqlType::USmallInt // DuckDB: unsigned 16-bit integer
                } else if eqi(w, "UINTEGER") || eqi(w, "UINT4") || eqi(w, "UINT") {
                    SqlType::UInteger // DuckDB: unsigned 32-bit integer
                } else if eqi(w, "UBIGINT") || eqi(w, "UINT64") {
                    // UINT8 handled before
                    SqlType::UBigInt // DuckDB: unsigned 64-bit integer
                } else if eqi(w, "UHUGEINT") || eqi(w, "UINT128") {
                    // DuckDB: unsigned 128-bit integer
                    SqlType::UHugeInt
                } else if eqi(w, "INT16") && backend == ClickHouse {
                    // ClickHouse: Int16 (16-bit signed integer)
                    SqlType::SmallInt
                } else if eqi(w, "INT32") && backend == ClickHouse {
                    // ClickHouse: Int32 (32-bit signed integer)
                    SqlType::Integer
                } else if eqi(w, "INT256") {
                    SqlType::Int256
                } else if eqi(w, "REAL") {
                    SqlType::Real
                } else if eqi(w, "FLOAT") {
                    let precision = self.precision()?;
                    SqlType::Float(precision)
                } else if eqi(w, "FLOAT4") {
                    // Snowflake also has FLOAT4, and FLOAT8. The names FLOAT, FLOAT4, and FLOAT8
                    // are for compatibility with other systems. Snowflake treats all three as
                    // 64-bit floating-point numbers.
                    //
                    // Postgres has FLOAT4 as an alias for REAL.
                    if matches!(backend, Postgres | Redshift) {
                        SqlType::Real
                    } else {
                        SqlType::Float(None)
                    }
                } else if eqi(w, "FLOAT32") {
                    // ClickHouse: Float32 (32-bit IEEE 754 floating-point)
                    SqlType::Real
                } else if eqi(w, "FLOAT64") && backend == ClickHouse {
                    // ClickHouse: Float64 (64-bit IEEE 754 floating-point)
                    SqlType::Double
                } else if eqi(w, "BFLOAT16") || eqi(w, "FLOAT16") {
                    SqlType::HalfFloat
                } else if eqi(w, "FLOAT8") || eqi(w, "FLOAT64") {
                    // Postgres has FLOAT8 as an alias for DOUBLE PRECISION.
                    // Bigquery uses FLOAT64 as an alias for DOUBLE PRECISION.
                    SqlType::Double
                } else if eqi(w, "DOUBLE") {
                    let _ = self.match_word("PRECISION");
                    SqlType::Double
                } else if eqi(w, "DECIMAL")
                    || eqi(w, "NUMERIC")
                    // Snowflake uses NUMBER as an alias for DECIMAL and NUMERIC
                    || eqi(w, "NUMBER")
                    // Snowflake and Databricks support DEC
                    || eqi(w, "DEC")
                {
                    let precision_and_scale = self.precision_and_scale()?;
                    SqlType::Numeric(precision_and_scale)
                } else if eqi(w, "BIGDECIMAL") || eqi(w, "BIGNUMERIC") {
                    // Bigquery has BIGNUMERIC and BIGDECIMAL
                    let precision_and_scale = self.precision_and_scale()?;
                    SqlType::BigNumeric(precision_and_scale)
                } else if eqi(w, "CHAR") || eqi(w, "CHARACTER") || eqi(w, "NCHAR") {
                    if self.match_word("LARGE") {
                        self.expect(Token::Word("OBJECT"))?;
                        SqlType::Clob // CHARACTER LARGE OBJECT
                    } else {
                        let varying = self.match_word("VARYING");
                        let len = self.precision()?;
                        if varying {
                            let attrs = self.string_attrs(backend)?;
                            SqlType::Varchar(len, attrs)
                        } else {
                            SqlType::Char(len)
                        }
                    }
                } else if eqi(w, "VARCHAR") || eqi(w, "NVARCHAR") {
                    let len = self.precision()?;
                    let attrs = self.string_attrs(backend)?;
                    SqlType::Varchar(len, attrs)
                } else if eqi(w, "NATIONAL") {
                    self.expect(Token::Word("CHAR"))?;
                    let varying = self.match_word("VARYING");
                    let len = self.precision()?;
                    if varying {
                        let attrs = self.string_attrs(backend)?;
                        SqlType::Varchar(len, attrs)
                    } else {
                        SqlType::Char(len)
                    }
                } else if eqi(w, "STRING") {
                    // Bigquery uses STRING as an alias for VARCHAR
                    let attrs = self.string_attrs(backend)?;
                    SqlType::Varchar(None, attrs)
                } else if eqi(w, "FIXEDSTRING") {
                    // ClickHouse: FixedString(N) - fixed-length string
                    let len = self.precision()?;
                    SqlType::Char(len)
                } else if eqi(w, "TEXT") {
                    SqlType::Text
                } else if eqi(w, "CLOB") {
                    SqlType::Clob
                } else if eqi(w, "BLOB") {
                    SqlType::Blob
                } else if eqi(w, "BINARY") {
                    if self.match_word("LARGE") {
                        self.expect(Token::Word("OBJECT"))?;
                        SqlType::Blob // BINARY LARGE OBJECT
                    } else if self.match_word("VARYING") {
                        // BINARY VARYING (Redshift)
                        let len = self.precision()?;
                        SqlType::Binary(len)
                    } else {
                        let len = self.precision()?;
                        SqlType::Binary(len)
                    }
                } else if eqi(w, "VARBINARY")
                    // Bigquery uses BYTES
                    || eqi(w, "BYTES")
                    // PostgreSQL uses BYTEA
                    || eqi(w, "BYTEA")
                    // Redshift also uses VARBYTE and VARBINARY
                    || eqi(w, "VARBYTE")
                {
                    let len = self.precision()?;
                    SqlType::Binary(len)
                } else if eqi(w, "DATE") {
                    let bit_width = match backend {
                        ClickHouse => Some(16),
                        _ => None,
                    };
                    SqlType::Date(bit_width)
                } else if eqi(w, "DATE32") {
                    SqlType::Date(Some(32)) // ClickHouse: Date32
                } else if eqi(w, "TIME") {
                    let precision = self.precision()?;
                    let time_zone_spec = self.time_zone_spec()?;
                    SqlType::Time {
                        precision,
                        time_zone_spec:
                            // For the TIME type, it's fair to assume that if the time zone
                            // is not specified, then it is WITHOUT time zone. TIMESTAMP is
                            // more complicated because of the different defaults in different
                            // SQL dialects.
                            if let TimeZoneSpec::Unspecified = time_zone_spec {
                                TimeZoneSpec::Without
                            } else {
                                time_zone_spec
                            }
                    }
                } else if eqi(w, "TIMETZ") {
                    SqlType::Time {
                        precision: None,
                        time_zone_spec: TimeZoneSpec::With,
                    }
                } else if eqi(w, "TIMESTAMP") {
                    let precision = self.precision()?;
                    let time_zone_spec = self.time_zone_spec()?;
                    SqlType::Timestamp {
                        precision,
                        time_zone_spec,
                    }
                } else if eqi(w, "TIMESTAMPTZ") {
                    SqlType::Timestamp {
                        precision: None,
                        time_zone_spec: TimeZoneSpec::With,
                    }
                } else if eqi(w, "TIMESTAMP_LTZ") {
                    let precision = self.precision()?;
                    SqlType::Timestamp {
                        precision,
                        time_zone_spec: TimeZoneSpec::Local,
                    }
                } else if eqi(w, "TIMESTAMP_NTZ") {
                    let precision = self.precision()?;
                    SqlType::Timestamp {
                        precision,
                        time_zone_spec: TimeZoneSpec::Without,
                    }
                } else if eqi(w, "DATETIME") {
                    // In Snowflake DATETIME is an alias for TIMESTAMP_NTZ,
                    // but in Bigquery it's not the same as the TIMESTAMP type.
                    // In ClickHouse, DateTime can have a timezone parameter: DateTime('Europe/Berlin')
                    let (precision, time_zone_spec) =
                        if backend == ClickHouse && self.match_(Token::LParen) {
                            let tz_tok = self.next()?;
                            if let Token::Word(tz_str) = tz_tok {
                                let tz_name = tz_str.trim_matches('\'').to_string();
                                self.expect(Token::RParen)?;
                                (None, TimeZoneSpec::Fixed(TimeZone::Named(tz_name)))
                            } else {
                                return Err(ParseError::Unexpected(tz_tok));
                            }
                        } else {
                            (self.precision()?, TimeZoneSpec::Without)
                        };

                    if backend == Snowflake {
                        SqlType::Timestamp {
                            precision,
                            time_zone_spec: TimeZoneSpec::Without,
                        }
                    } else if backend == ClickHouse {
                        SqlType::Timestamp {
                            precision,
                            time_zone_spec,
                        }
                    } else {
                        SqlType::DateTime
                    }
                } else if eqi(w, "DATETIME2") {
                    SqlType::DateTime
                } else if eqi(w, "DATETIME64") {
                    // ClickHouse: DateTime64(precision) or DateTime64(precision, 'timezone')
                    // Note: We need to manually parse precision and timezone because
                    // the timezone comes after a comma inside the same parentheses
                    if !self.match_(Token::LParen) {
                        // DATETIME64 without precision or timezone is invalid,
                        // but we'll allow it and return no precision
                        SqlType::Timestamp {
                            precision: None,
                            time_zone_spec: TimeZoneSpec::Without,
                        }
                    } else {
                        // Parse precision
                        let precision = Some(self.next_int::<u8>()?);
                        // Check for timezone after comma
                        let time_zone_spec = if self.match_(Token::Comma) {
                            let tz_tok = self.next()?;
                            if let Token::Word(tz_str) = tz_tok {
                                let tz_name = tz_str.trim_matches('\'').to_string();
                                self.expect(Token::RParen)?;
                                TimeZoneSpec::Fixed(TimeZone::Named(tz_name))
                            } else {
                                return Err(ParseError::Unexpected(tz_tok));
                            }
                        } else {
                            self.expect(Token::RParen)?;
                            TimeZoneSpec::Without
                        };
                        SqlType::Timestamp {
                            precision,
                            time_zone_spec,
                        }
                    }
                } else if eqi(w, "TIME64") {
                    // ClickHouse: Time64(precision) - high-precision time
                    let precision = self.precision()?;
                    SqlType::Time {
                        precision,
                        time_zone_spec: TimeZoneSpec::Without,
                    }
                } else if eqi(w, "BIT") {
                    SqlType::TinyInt
                } else if eqi(w, "TIMESTAMP_TZ") {
                    let precision = self.precision()?;
                    SqlType::Timestamp {
                        precision,
                        time_zone_spec: TimeZoneSpec::With,
                    }
                } else if eqi(w, "INTERVAL") {
                    // Some backends (like PostgreSQL) support a precision for the sub-second part
                    // instead of having the time unit spelled out. Examples of equivalents:
                    //
                    //     INTERVAL / INTERVAL SECOND
                    //     INTERVAL (3) / INTERVAL MILLISECOND
                    //     INTERVAL MINUTE
                    //     INTERVAL SECOND(3) / INTERVAL MILLISECOND
                    //     INTERVAL DAY TO SECOND(6) / INTERVAL DAY TO MICROSECOND
                    //
                    // PostgreSQL treats the YEAR, MONTH TO DAY, etc., in an interval type
                    // declaration as decorative metadata, not an actual constraint. But this
                    // parser will preserve that metadata so that it can be used when rendering
                    // the type as a string.
                    let qualifier = match self.interval_qualifier()? {
                        Some((start, None)) => {
                            if matches!(start, DateTimeField::Second) {
                                self.precision()?
                                    .map(DateTimeField::from_precision)
                                    .map(|unit| (unit, None))
                                    .or(Some((start, None)))
                            } else {
                                Some((start, None))
                            }
                        }
                        Some((start, Some(end))) => {
                            if matches!(end, DateTimeField::Second) {
                                self.precision()?
                                    .map(DateTimeField::from_precision)
                                    .map(|unit| (start, Some(unit)))
                                    .or(Some((start, Some(end))))
                            } else {
                                Some((start, Some(end)))
                            }
                        }
                        None => self
                            .precision()?
                            .map(DateTimeField::from_precision)
                            .map(|unit| (unit, None)),
                    };
                    SqlType::Interval(qualifier)
                } else if eqi(w, "JSON") {
                    SqlType::Json
                } else if eqi(w, "JSONB") {
                    SqlType::Jsonb
                } else if eqi(w, "UUID") {
                    SqlType::Uuid
                } else if eqi(w, "HASHTYPE") {
                    // Exasol: HASHTYPE [(n BYTE | n BIT)], default 16 bytes
                    let byte_size = if self.match_(Token::LParen) {
                        let n = self.next_int::<u32>()?;
                        let bits = self.match_word("BIT");
                        if !bits {
                            let _ = self.match_word("BYTE");
                        }
                        self.expect(Token::RParen)?;
                        // Exasol only accepts BIT sizes that are multiples of 8,
                        // so integer division is exact for server-emitted strings.
                        Some(if bits { n / 8 } else { n })
                    } else {
                        None
                    };
                    SqlType::HashType(byte_size)
                } else if eqi(w, "GEOMETRY") {
                    let srid = self.srid()?;
                    SqlType::Geometry(srid.map(str::to_string))
                } else if eqi(w, "GEOGRAPHY") {
                    let srid = self.srid()?;
                    SqlType::Geography(srid.map(str::to_string))
                } else if eqi(w, "ARRAY") {
                    let (left, right) = match backend {
                        Snowflake | ClickHouse => (Token::LParen, Token::RParen),
                        _ => (Token::LAngle, Token::RAngle),
                    };
                    if self.match_(left) {
                        let inner_type = self.parse_unconstrained_type(backend)?;
                        self.expect(right)?;
                        SqlType::Array(Some(Box::new(inner_type)))
                    } else {
                        SqlType::Array(None)
                    }
                } else if eqi(w, "RECORD") {
                    // In some scenarios, we get "RECORD" as a type from Bigquery.
                    // That just means a generic struct.
                    SqlType::Struct(None)
                } else if eqi(w, "OBJECT") || eqi(w, "STRUCT") {
                    let (left, right) = match backend {
                        Snowflake => (Token::LParen, Token::RParen),
                        _ => (Token::LAngle, Token::RAngle),
                    };
                    let inner_fields = if self.match_(left) {
                        let fields = self.struct_fields(backend, right)?;
                        Some(fields)
                    } else {
                        None
                    };
                    SqlType::Struct(inner_fields)
                } else if eqi(w, "MAP") {
                    let (left, right) = if backend == ClickHouse {
                        (Token::LParen, Token::RParen)
                    } else {
                        (Token::LAngle, Token::RAngle)
                    };
                    let kv = if self.match_(left) {
                        let key_type = self.parse_unconstrained_type(backend)?;
                        self.expect(Token::Comma)?;
                        let value_type = self.parse_unconstrained_type(backend)?;
                        self.expect(right)?;
                        Some((Box::new(key_type), Box::new(value_type)))
                    } else {
                        None
                    };
                    SqlType::Map(kv)
                } else if eqi(w, "VARIANT") || eqi(w, "SUPER") {
                    // Redshift uses "SUPER"
                    SqlType::Variant
                } else if eqi(w, "NULLABLE") {
                    // ClickHouse: Nullable(T) - allows NULL values
                    // The nullable flag is handled by parse(), so we just return the inner type
                    self.expect(Token::LParen)?;
                    let inner = self.parse_unconstrained_type(backend)?;
                    self.expect(Token::RParen)?;
                    inner
                } else if eqi(w, "LOWCARDINALITY") {
                    // ClickHouse: LowCardinality(T) - compression wrapper
                    // This is a storage optimization hint, we parse the inner type
                    self.expect(Token::LParen)?;
                    let inner = self.parse_unconstrained_type(backend)?;
                    self.expect(Token::RParen)?;
                    inner
                } else if eqi(w, "ENUM") {
                    let entries = self.enum_entries()?;
                    SqlType::Enum(entries, EnumAttrs { bit_width: None })
                } else if eqi(w, "ENUM8") {
                    let entries = self.enum_entries()?;
                    let bit_width = Some(8);
                    SqlType::Enum(entries, EnumAttrs { bit_width })
                } else if eqi(w, "ENUM16") {
                    let entries = self.enum_entries()?;
                    let bit_width = Some(16);
                    SqlType::Enum(entries, EnumAttrs { bit_width })
                } else if eqi(w, "IPV4") {
                    SqlType::IPv4
                } else if eqi(w, "IPV6") {
                    SqlType::IPv6
                } else if eqi(w, "DYNAMIC") {
                    SqlType::Variant
                } else if eqi(w, "VOID") {
                    SqlType::Void
                } else {
                    // gather all tokens before "[NOT] NULL" and return Other(..)
                    let mut other = w.to_string();
                    while let Some(tok) = {
                        self.tokenizer.peek_and_then(|t| {
                            if t == Token::Word("NOT") || t == Token::Word("NULL") {
                                None
                            } else {
                                Some(t)
                            }
                        })
                    } {
                        use fmt::Write as _;
                        write!(&mut other, " {tok}").unwrap();
                    }
                    SqlType::Other(other)
                }
            }
        };
        // Other PostgreSQL types that we don't explicitly support yet:
        //
        //     money       currency amount
        //
        //     bit [ (n) ]                             fixed-length bit string
        //     bit varying [ (n) ] / varbit [ (n) ]    variable-length bit string
        //
        //     cidr        IPv4 or IPv6 network address
        //     inet        IPv4 or IPv6 host address
        //     macaddr     MAC (Media Access Control) address
        //     macaddr8    MAC (Media Access Control) address (EUI-64 format)
        //
        //     point       geometric point on a plane
        //     polygon     closed geometric path on a plane
        //     box         rectangular box on a plane
        //     circle      circle on a plane
        //     line        infinite line on a plane
        //     lseg        line segment on a plane
        //     path        geometric path on a plane
        //
        //     tsquery     text search query
        //     tsvector    text search document
        //     uuid        universally unique identifier
        //     xml
        //
        //     pg_lsn         PostgreSQL Log Sequence Number
        //     pg_snapshot    user-level transaction ID snapshot
        //     txid_snapshot  user-level transaction ID snapshot (deprecated; see pg_snapshot)
        Ok(sql_type)
    }
}
