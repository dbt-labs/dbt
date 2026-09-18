use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::AdapterResult;
use crate::errors::{AdapterError, AdapterErrorKind};
use crate::metadata::snowflake::ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY;
use crate::metadata::*;
use crate::need_quotes::need_quotes;
use arrow_schema::{DataType, Field, Schema, TimeUnit};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::types::{SqlType, metadata_sql_type_key};

// TODO: Add keys here as necessary
pub const REDSHIFT_METADATA_SQL_TYPE_KEY: &str = "Type";
pub const BIGQUERY_METADATA_SQL_TYPE_KEY: &str = "Type";
// XXX: Snowflake does DATA_TYPE for GetTableSchema and SNOWFLAKE_TYPE for other queries...
pub const SNOWFLAKE_METADATA_SQL_TYPE_KEY: &str = "DATA_TYPE";
pub const FABRIC_METADATA_SQL_TYPE_KEY: &str = "DATA_TYPE";
pub const CLICKHOUSE_METADATA_SQL_TYPE_KEY: &str = "data_type";

/// An Arrow schema containing SDF types
#[derive(Clone)]
pub struct SdfSchema {
    original: Option<Arc<Schema>>,
    schema: Arc<Schema>,
}

impl SdfSchema {
    /// Creates a new SdfSchema from a transformed Arrow schema.
    ///
    /// PRE-CONDITION: the schema must have been transformed to use SDF types.
    /// All types have been converted to types that static analysis expects
    /// and all the canonicalization steps have been applied (e.g. the
    /// `FixedSizeList` hack for Snowflake timestamps)
    pub fn from_sdf_arrow_schema(original: Option<Arc<Schema>>, schema: Arc<Schema>) -> Self {
        SdfSchema { original, schema }
    }

    pub fn inner(&self) -> &Arc<Schema> {
        &self.schema
    }

    pub fn into_inner(self) -> Arc<Schema> {
        self.schema
    }

    pub fn original(&self) -> Option<&Arc<Schema>> {
        self.original.as_ref()
    }
}

pub trait TypeOps: Send + Sync {
    fn adapter_type(&self) -> AdapterType;

    /// Picks a SQL type for a given Arrow DataType and renders it as SQL.
    ///
    /// The implementation is dialect-specific.
    fn format_arrow_type_as_sql(
        &self,
        data_type: &DataType,
        nullable: bool,
        out: &mut String,
    ) -> AdapterResult<()>;

    /// Renders a given SqlType as SQL.
    ///
    /// The implementation is dialect-specific.
    fn format_sql_type(&self, sql_type: SqlType, out: &mut String) -> AdapterResult<()>;

    fn parse_into_nullable_arrow_type(&self, s: &str) -> AdapterResult<(DataType, bool)>;

    fn parse_into_arrow_type(&self, s: &str) -> AdapterResult<DataType> {
        self.parse_into_nullable_arrow_type(s).map(|(dt, _)| dt)
    }

    fn get_original_sql_type_from_field<'f>(
        &self,
        field: &'f Field,
    ) -> AdapterResult<Cow<'f, str>> {
        original_type_string(self.adapter_type(), field)
            .map(Ok)
            .unwrap_or_else(|| {
                let mut out = String::new();
                self.format_arrow_type_as_sql(field.data_type(), field.is_nullable(), &mut out)
                    .map(|_| Cow::Owned(out))
            })
    }

    /// Return internal type representation for the given source type inferred from seed data.
    /// The correct mapping is not necessarily the same, or logically the same, as the source type.
    /// The mapping is defined for compatibility with DBT seed files and is determined as follows:
    ///
    /// 1. Create a seed CSV file for which DataFusion listing table provider would return
    ///    the source type.
    /// 2. Run pure-Python `dbt seed` on that file on that file and checking what gets created in
    ///    the remote data warehouse.
    /// 3. Return corresponding internal type for given warehouse's type for given dialect.
    ///
    /// Note that in particular DataFusion listing table provider may return same Arrow DataType
    /// for data that's treated differently by pure-Python `dbt seed`. In such case, the mapping is
    /// not well defined and we have a problem.
    fn adapt_seed_type(&self, _data_type: &DataType) -> Option<DataType>;

    /// A parser for a column description, which is "everything in a column definition, except the
    /// column's name", where column definition is as in CREATE TABLE.
    fn parse_column_description(&self, s: &str) -> AdapterResult<Field>;

    /// Normalize two SQL type strings and compare them for equality.
    fn normalize_and_compare_sql_types(&self, lhs: &str, rhs: &str) -> AdapterResult<bool>;

    /// Check whether a SQL literal is compatible with the given Arrow DataType.
    ///
    /// Returns `Ok(false)` when the literal's precision/scale would overflow the column type
    /// (currently only relevant for Decimal on Snowflake and DuckDB). All other adapters and
    /// all non-Decimal types return `Ok(true)`.
    fn can_cast_literal_to_type(
        &self,
        _literal: &str,
        _data_type: &DataType,
    ) -> AdapterResult<bool> {
        Ok(true)
    }

    fn cast_from_quoted_string_literal_unsupported_for(&self, field: &Field) -> bool {
        match self.adapter_type() {
            AdapterType::Bigquery => {
                matches!(field.data_type(), DataType::Struct(_))
                    || bigquery::is_type(field, "GEOGRAPHY")
            }
            _ => false,
        }
    }

    /// Format a SQL identifier, quoting it if necessary for this dialect.
    fn format_ident(&self, id: &str) -> String {
        crate::format_ident::format_ident(id, self.adapter_type())
    }

    fn need_quotes_for_ident(&self, id: &str) -> bool {
        need_quotes(self.adapter_type(), id)
    }
}

pub trait TypeOpsFactory: Send + Sync {
    fn create(&self, adapter_type: AdapterType) -> Arc<dyn TypeOps>;
}

pub struct DefaultTypeOpsFactory;

impl TypeOpsFactory for DefaultTypeOpsFactory {
    fn create(&self, adapter_type: AdapterType) -> Arc<dyn TypeOps> {
        Arc::new(DefaultTypeOps::new(adapter_type))
    }
}

/// Source-available [TypeOps] implementation.
pub struct DefaultTypeOps(AdapterType);

impl DefaultTypeOps {
    pub fn new(adapter_type: AdapterType) -> Self {
        Self(adapter_type)
    }
}

impl TypeOps for DefaultTypeOps {
    fn adapter_type(&self) -> AdapterType {
        self.0
    }

    fn format_arrow_type_as_sql(
        &self,
        data_type: &DataType,
        nullable: bool,
        out: &mut String,
    ) -> AdapterResult<()> {
        use AdapterType::*;
        let adapter_type = self.0;
        match adapter_type {
            Postgres | Salesforce => postgres::try_format_type(data_type, nullable, out),
            Fabric => fabric::try_format_type(data_type, nullable, out),
            ClickHouse => clickhouse::try_format_type(data_type, nullable, out),
            _ => {
                // sdf-specific distinct types are encoded as FixedSizeList(field, 1).
                // Render them as the uppercased field name (e.g. "variant" → "VARIANT").
                if let DataType::FixedSizeList(field, 1) = data_type {
                    out.push_str(&field.name().to_ascii_uppercase());
                    return Ok(());
                }
                // List types: Snowflake uses unparameterized ARRAY; other dialects use ARRAY<T>.
                if let DataType::List(field) = data_type {
                    if adapter_type == Snowflake {
                        out.push_str("ARRAY");
                    } else {
                        out.push_str("ARRAY<");
                        self.format_arrow_type_as_sql(field.data_type(), field.is_nullable(), out)?;
                        out.push('>');
                    }
                    return Ok(());
                }
                // Struct types: render as STRUCT<name TYPE, ...>.
                if let DataType::Struct(fields) = data_type {
                    out.push_str("STRUCT<");
                    for (i, field) in fields.iter().enumerate() {
                        if i > 0 {
                            out.push_str(", ");
                        }
                        out.push_str(field.name());
                        out.push(' ');
                        self.format_arrow_type_as_sql(field.data_type(), field.is_nullable(), out)?;
                    }
                    out.push('>');
                    return Ok(());
                }
                self.write_sql_type_for_dbt_convert_functions(data_type, out)
            }
        }
    }

    fn format_sql_type(&self, sql_type: SqlType, out: &mut String) -> AdapterResult<()> {
        let adapter_type = self.0;
        sql_type.write(adapter_type, out).map_err(|e| {
            AdapterError::new(
                AdapterErrorKind::NotSupported,
                format!("Failed to convert SQL type {sql_type:?}. Error: {e}"),
            )
        })
    }

    fn parse_into_nullable_arrow_type(&self, s: &str) -> AdapterResult<(DataType, bool)> {
        let adapter_type = self.0;
        let (sql_type, nullable) = SqlType::parse(adapter_type, s)
            .map_err(|e| AdapterError::new(AdapterErrorKind::UnexpectedResult, e))?;
        let data_type = sql_type.pick_best_arrow_type(adapter_type);
        Ok((data_type, nullable))
    }

    fn adapt_seed_type(&self, _data_type: &DataType) -> Option<DataType> {
        None
    }

    fn parse_column_description(&self, s: &str) -> AdapterResult<Field> {
        let backend = self.adapter_type();
        let col = SqlType::parse_column_description(backend, s, true)
            .map_err(|err| AdapterError::new(AdapterErrorKind::NotSupported, err))?;
        let name = col
            .name
            .as_ref()
            .map(|id| id.as_ref().to_string())
            .unwrap_or_default();
        let mut field = col.sql_type.to_field(backend, name, col.nullable);
        if let Some(comment) = col.comment {
            let mut metadata = field.metadata().clone();
            metadata.insert("comment".to_string(), comment);
            field = field.with_metadata(metadata);
        }
        Ok(field)
    }

    fn normalize_and_compare_sql_types(&self, lhs: &str, rhs: &str) -> AdapterResult<bool> {
        let adapter_type = self.adapter_type();

        let lhs = SqlType::parse(adapter_type, lhs)
            .map(|(ty, _)| ty.display(adapter_type).to_string())
            .unwrap_or_else(|_| lhs.to_string());

        let rhs = SqlType::parse(self.adapter_type(), rhs)
            .map(|(ty, _)| ty.display(adapter_type).to_string())
            .unwrap_or_else(|_| rhs.to_string());

        Ok(lhs == rhs)
    }
}

impl DefaultTypeOps {
    /// Implements the dbt-adapters `convert_{type}_type` family of functions [1].
    ///
    /// Converts an Arrow [DataType] into backend-specific SQL text by first obtaining
    /// a [SqlType] via [SqlType::from_arrow_type], then applying the overrides that
    /// match the upstream Python adapter behaviour.  Arms that can be handled by the
    /// standard [SqlType::write] path are delegated; the rest are written directly.
    ///
    /// The functions are:
    /// - `convert_integer_type`
    /// - `convert_number_type` (floating and decimal types)
    /// - `convert_boolean_type`
    /// - `convert_datetime_type`
    /// - `convert_date_type`
    /// - `convert_time_type`
    /// - `convert_text_type`
    ///
    /// Databricks uses the conversion rules from Spark [3].
    ///
    /// [1]: https://github.com/dbt-labs/dbt-adapters/blob/b0223a88d67012bcc4c6cce5449c4fe10c6ed198/dbt-adapters/src/dbt/adapters/sql/impl.py
    /// [2]: https://github.com/dbt-labs/dbt-adapters/blob/b0223a88d67012bcc4c6cce5449c4fe10c6ed198/dbt-bigquery/src/dbt/adapters/bigquery/impl.py
    /// [3]: https://github.com/dbt-labs/dbt-adapters/blob/b0223a88d67012bcc4c6cce5449c4fe10c6ed198/dbt-spark/src/dbt/adapters/spark/impl.py
    /// [4]: https://github.com/microsoft/dbt-fabric/blob/81d9764e24b00e7c923a2235ba68fa6bd6b90ea9/dbt/adapters/fabric/fabric_adapter.py
    fn write_sql_type_for_dbt_convert_functions(
        &self,
        data_type: &DataType,
        out: &mut String,
    ) -> AdapterResult<()> {
        use dbt_adapter_core::AdapterType::*;

        let adapter_type = self.0;

        // ## convert_integer_type() – Null
        // Null-typed columns (all values null, no real type) are rendered as "text"
        // before entering the SqlType pipeline.
        if data_type.is_null() {
            out.push_str("text");
            return Ok(());
        }

        let sql_type = SqlType::from_arrow_type(adapter_type, data_type);

        let type_str = match (&sql_type, adapter_type) {
            // ## convert_integer_type()
            // Upstream collapses all integer widths to a single type per backend.
            (
                SqlType::TinyInt
                | SqlType::SmallInt
                | SqlType::Integer
                | SqlType::BigInt
                | SqlType::UTinyInt
                | SqlType::USmallInt
                | SqlType::UInteger
                | SqlType::UBigInt,
                _,
            ) => match adapter_type {
                Bigquery => "int64",
                Databricks => "bigint",
                _ => "integer",
            },

            // ## convert_number_type() - Float32
            (SqlType::Real | SqlType::HalfFloat, _) => match adapter_type {
                Bigquery => "float64",
                Databricks => "float",
                Fabric => "real",
                // Exasol float type is DOUBLE PRECISION (no float8 alias).
                Exasol => "DOUBLE PRECISION",
                _ => "float8",
            },

            // ## convert_number_type() - Float64
            (SqlType::Double | SqlType::Float(_), _) => match adapter_type {
                Bigquery => "float64",
                Databricks => "double",
                // Divergence: upstream has an implicit narrowing bug we fix
                // see https://github.com/microsoft/dbt-fabric/blob/0de219082282724a789b0d1b18509d39899da8e1/dbt/adapters/fabric/fabric_adapter.py#L117
                // https://learn.microsoft.com/en-us/sql/t-sql/data-types/float-and-real-transact-sql?view=fabric&preserve-view=true
                Fabric => "float",
                // Exasol float type is DOUBLE PRECISION (no float8 alias).
                Exasol => "DOUBLE PRECISION",
                _ => "float8",
            },

            // ## convert_number_type() - Decimal
            // Upstream coerces decimals to float (fractional) or integer (zero/negative scale).
            // TODO(versusfacit): stick to upstream and integer coercion, or map decimal types?
            // https://docs.cloud.google.com/bigquery/docs/reference/standard-sql/data-types#numeric_types
            // https://learn.microsoft.com/en-us/sql/t-sql/data-types/decimal-and-numeric-transact-sql?view=fabric&preserve-view=true
            (
                SqlType::Numeric(Some((_, Some(scale))))
                | SqlType::BigNumeric(Some((_, Some(scale)))),
                _,
            ) => match (adapter_type, scale) {
                (Bigquery, 1..) => "float64",
                (Bigquery, ..=0) => "int64",
                (Fabric, _) => "float",
                (Databricks, 1..) => "double",
                (Databricks, ..=0) => "bigint",
                // Exasol: fractional -> DOUBLE PRECISION; zero/negative scale
                // falls through to "integer" (a valid DECIMAL(18,0) alias).
                (Exasol, 1..) => "DOUBLE PRECISION",
                (_, 1..) => "float8",
                (_, ..=0) => "integer",
            },

            (SqlType::Numeric(Some((_, None))) | SqlType::BigNumeric(Some((_, None))), _) => {
                unreachable!("from_arrow_type always produces an explicit scale")
            }

            // ## convert_boolean_type()
            (SqlType::Boolean, _) => match adapter_type {
                Bigquery => "bool",
                Fabric => "bit",
                _ => "boolean",
            },

            // ## convert_datetime_type()
            (SqlType::Timestamp { .. }, _) => match adapter_type {
                Bigquery => "datetime",
                Databricks => "timestamp",
                Fabric => "datetime2(6)",
                Exasol => "timestamp",
                _ => "timestamp without time zone",
            },

            // ## convert_date_type()
            (SqlType::Date(_), _) => "date",

            // ## convert_time_type()
            // Upstream maps Duration and Interval Arrow types to time.
            (SqlType::Interval(_) | SqlType::Time { .. }, _) => match adapter_type {
                Fabric => "time(6)",
                // Exasol stores a time-of-day value as TIMESTAMP.
                Exasol => "timestamp",
                _ => "time",
            },

            // ## convert_text_type()
            (SqlType::Varchar(..) | SqlType::Text | SqlType::Clob | SqlType::Char(_), _) => {
                match adapter_type {
                    Bigquery | Databricks => "string",
                    // technically should be `varchar(N)`
                    // where `N` is based on the max length of the strings in the column
                    // but that information isn't available here
                    // - N = 64 if column is empty
                    // - N = max(16, max_length) if column is not empty
                    Fabric => "varchar",
                    Exasol => "VARCHAR(2000000)",
                    _ => "text",
                }
            }

            // Delegate everything else to SqlType::write
            _ => {
                return sql_type.write(adapter_type, out).map_err(|e| {
                    AdapterError::new(
                        AdapterErrorKind::NotSupported,
                        format!(
                            "Failed to convert SQL type {sql_type:?} for {adapter_type:?}. Error: {e}"
                        ),
                    )
                });
            }
        };

        out.push_str(type_str);
        Ok(())
    }
}

/// Replacement for `new_arrow_field_with_metadata` that fuses parsing and `Field` creation.
#[inline]
pub fn make_arrow_field(
    type_ops: &dyn TypeOps,
    col_name: String,
    sql_type_str: &str,
    nullable_override: Option<bool>,
    comment: Option<String>,
) -> Result<Field, AdapterError> {
    make_arrow_field_v1(type_ops, col_name, sql_type_str, nullable_override, comment)
}

/// Implementation of [make_arrow_field] that is currently in use.
pub fn make_arrow_field_v1(
    type_ops: &dyn TypeOps,
    col_name: String,
    sql_type_str: &str,
    nullable_override: Option<bool>,
    comment: Option<String>,
) -> Result<Field, AdapterError> {
    use SqlType::*;

    let (data_type, nullable) = type_ops.parse_into_nullable_arrow_type(sql_type_str)?;
    let field = Field::new(col_name, data_type, nullable_override.unwrap_or(nullable));

    let adapter_type = type_ops.adapter_type();
    let mut metadata = HashMap::new();
    metadata.insert(
        ARROW_FIELD_ORIGINAL_TYPE_METADATA_KEY.to_string(),
        sql_type_str.to_string(),
    );
    if let Some(comment) = comment {
        metadata.insert(ARROW_FIELD_COMMENT_METADATA_KEY.to_string(), comment);
    }

    // HACK: Insert the width of the field as its own value
    // Special handling for Snowflake char width fields
    // because these are given to the user as separate types
    if adapter_type == AdapterType::Snowflake {
        let sql_type_res = SqlType::parse(adapter_type, sql_type_str).map(|(ty, _)| ty);
        match sql_type_res {
            Ok(Binary(Some(max_len))) | Ok(Varchar(Some(max_len), _)) => {
                metadata.insert(
                    ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY.to_string(),
                    max_len.to_string(),
                );
            }
            _ => (),
        }
    }

    let field = field.with_metadata(metadata);

    Ok(field)
}

/// The version we want to standardize on as we move away from using Arrow types
/// with SDF-isms encoded in them (e.g. FixedSizeList for timestamps).
pub fn make_arrow_field_v2(
    type_ops: &dyn TypeOps,
    col_name: String,
    sql_type_str: &str,
    nullable_override: Option<bool>,
    comment: Option<String>,
) -> Result<Field, AdapterError> {
    use AdapterType::*;
    use SqlType::*;
    let adapter_type = type_ops.adapter_type();
    let (sql_type, nullable) = SqlType::parse(adapter_type, sql_type_str)
        .map_err(|e| AdapterError::new(AdapterErrorKind::UnexpectedResult, e))?;
    let data_type = sql_type.pick_best_arrow_type(adapter_type);

    let field = Field::new(col_name, data_type, nullable_override.unwrap_or(nullable));

    let mut metadata = HashMap::new();
    metadata.insert(
        metadata_sql_type_key(adapter_type).to_string(),
        sql_type_str.to_string(),
    );
    if let Some(comment) = comment {
        metadata.insert(ARROW_FIELD_COMMENT_METADATA_KEY.to_string(), comment);
    }

    match (adapter_type, &sql_type) {
        (Snowflake, Varchar(Some(max_len), _)) | (Snowflake, Binary(Some(max_len))) => {
            metadata.insert(
                ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY.to_string(),
                max_len.to_string(),
            );
        }
        _ => {}
    }

    let field = field.with_metadata(metadata);

    Ok(field)
}

pub const fn get_field_sql_type_metadata_key(adapter_type: AdapterType) -> &'static str {
    match adapter_type {
        AdapterType::Bigquery => BIGQUERY_METADATA_SQL_TYPE_KEY,
        AdapterType::Redshift => REDSHIFT_METADATA_SQL_TYPE_KEY,
        AdapterType::Snowflake => SNOWFLAKE_METADATA_SQL_TYPE_KEY,
        AdapterType::Databricks => todo!(),
        AdapterType::Postgres => todo!(),
        AdapterType::Salesforce => todo!(),
        AdapterType::Spark => todo!(),
        AdapterType::DuckDB | AdapterType::LakeCompute => todo!(),
        AdapterType::Fabric => FABRIC_METADATA_SQL_TYPE_KEY,
        AdapterType::ClickHouse => CLICKHOUSE_METADATA_SQL_TYPE_KEY,
        AdapterType::Exasol => "DATA_TYPE",
        AdapterType::Starburst => todo!(),
        AdapterType::Athena => todo!(),
        AdapterType::Trino => todo!(),
        AdapterType::Dremio => todo!(),
        AdapterType::Oracle => todo!(),
        AdapterType::Datafusion => todo!(),
    }
}

pub fn original_type_string<'a>(
    adapter_type: AdapterType,
    field: &'a Field,
) -> Option<Cow<'a, str>> {
    // The first step is trying to the original SQL type from the `Field` by
    // probing the `<VENDOR>::type` metadata key as proposed in [1].
    //
    // [1]: https://github.com/apache/arrow-adbc/issues/3449
    let sql_type_string = dbt_adapter_sql::types::original_type_string(adapter_type, field)
        .map(|s| Cow::Borrowed(s.as_str()));

    match (adapter_type, sql_type_string) {
        (_, s @ Some(_)) => s,
        (AdapterType::Bigquery, None) => {
            // this can build a SQL type from the "Type" metadata key
            bigquery::field_to_string(field)
        }
        (_, s) => s,
    }
}

struct SdfSchemaBuilder {
    adapter_type: AdapterType,
    original: Arc<Schema>,
}

impl SdfSchemaBuilder {
    pub fn new(adapter_type: AdapterType, original: Arc<Schema>) -> Self {
        Self {
            adapter_type,
            original,
        }
    }

    fn field_comment<'a>(&self, field: &'a Field) -> Option<&'a String> {
        use AdapterType::*;
        let metadata = field.metadata();
        let comment = match self.adapter_type {
            Bigquery => metadata.get("Description"),
            Redshift | Databricks | Spark | DuckDB | LakeCompute => {
                metadata.get(ARROW_FIELD_COMMENT_METADATA_KEY)
            }
            // no evidence that these drivers store comments in metadata, but just in case
            Postgres | Snowflake | Salesforce | Fabric | ClickHouse | Exasol | Starburst
            | Athena | Trino | Dremio | Oracle | Datafusion => {
                metadata.get(ARROW_FIELD_COMMENT_METADATA_KEY)
            }
        };
        comment
    }

    fn convert_field(&self, type_ops: &dyn TypeOps, field: &Field) -> AdapterResult<Arc<Field>> {
        let current_type = field.data_type();
        let nullable = field.is_nullable();
        let comment = self.field_comment(field);
        let original_type_text = match self.adapter_type {
            AdapterType::Bigquery => bigquery::field_to_string(field),
            _ => original_type_string(self.adapter_type, field),
        };
        // XXX: We should probably error here rather than approximate
        let resolved_type = if let Some(original_type_text) = &original_type_text {
            type_ops
                .parse_into_arrow_type(original_type_text.as_ref())
                .unwrap_or_else(|_| current_type.clone())
        } else {
            current_type.clone()
        };
        let field = new_arrow_field_with_metadata(
            field.name(),
            resolved_type,
            nullable,
            original_type_text.map(|s| s.to_string()),
            comment.cloned(),
        );
        Ok(Arc::new(field))
    }

    pub fn build_sdf_schema(self, type_ops: &dyn TypeOps) -> AdapterResult<SdfSchema> {
        use AdapterType::*;
        match self.adapter_type {
            Bigquery | Redshift | Databricks | Spark | DuckDB | LakeCompute | Fabric
            | ClickHouse | Exasol | Starburst | Athena | Trino | Dremio | Oracle | Datafusion => {
                let original_fields = self.original.fields();
                let mut sdf_fields = Vec::with_capacity(original_fields.len());
                for field in original_fields {
                    let sdf_field = self.convert_field(type_ops, field)?;
                    sdf_fields.push(sdf_field);
                }
                let sdf_arrow_schema = Arc::new(Schema::new(sdf_fields));
                let sdf_schema =
                    SdfSchema::from_sdf_arrow_schema(Some(self.original), sdf_arrow_schema);
                Ok(sdf_schema)
            }
            Postgres | Snowflake | Salesforce => {
                // NOTE(felipecrv): this is not correct, but it's a temporary fallback
                // that allows us to call [to_sdf_arrow_schema] from anywhere.
                //
                // TODO: move conversion logic for other adapters here
                let sdf_arrow_schema = Arc::clone(&self.original);
                let sdf_schema =
                    SdfSchema::from_sdf_arrow_schema(Some(self.original), sdf_arrow_schema);
                Ok(sdf_schema)
            }
        }
    }
}

/// Converts a regular Arrow Schema into an SDF Arrow Schema.
///
/// A regular Arrow Schema is one that may come from drivers or internal adapter
/// logic. It's free of any SDF-specific type encoding rules (e.g. `FixedSizeList`
/// hack for timestamps) which we can't expect to be present in these contexts.
///
/// Applies SDF-specific type encoding rules (e.g. `FixedSizeList` hack for timestamps).
pub fn arrow_schema_to_sdf_schema(
    src_schema: Arc<Schema>,
    type_ops: &dyn TypeOps,
) -> AdapterResult<SdfSchema> {
    let builder = SdfSchemaBuilder::new(type_ops.adapter_type(), src_schema);
    builder.build_sdf_schema(type_ops)
}

pub mod bigquery {
    use std::borrow::Cow;

    // XXX: make private once all tests are moved to here
    use arrow_schema::{DataType, Field};
    use dbt_adapter_core::AdapterType;

    use crate::sql_types::{get_field_sql_type_metadata_key, original_type_string};

    pub fn is_type(field: &Field, sql_type: &str) -> bool {
        original_type_string(AdapterType::Bigquery, field)
            .is_some_and(|declared| declared.eq_ignore_ascii_case(sql_type))
    }

    pub fn field_to_string<'a>(field: &'a Field) -> Option<Cow<'a, str>> {
        let type_key = get_field_sql_type_metadata_key(AdapterType::Bigquery);

        if let Some(original_ctype) = field.metadata().get(type_key) {
            // REPEATED - this is an Array type
            let is_array = field
                .metadata()
                .get("Repeated")
                .is_some_and(|v| v == "true");

            let inner_sql_type = match original_ctype.as_str() {
                "RECORD" => {
                    // STRUCT/RECORD type, recurse and build original type.
                    let struct_fields = match field.data_type() {
                        DataType::Struct(fields) => Some(fields),
                        DataType::List(element) if is_array => match element.data_type() {
                            DataType::Struct(fields) => Some(fields),
                            _ => None,
                        },
                        _ => None,
                    };

                    match struct_fields {
                        Some(fields) => {
                            let field_strings: Vec<String> = fields
                                .iter()
                                .map(|nested_field| {
                                    let field_name = format!("`{}`", nested_field.name());
                                    let field_type = field_to_string(nested_field)?;
                                    Some(format!("{field_name} {field_type}"))
                                })
                                .collect::<Option<Vec<_>>>()?;
                            Cow::Owned(format!("STRUCT<{}>", field_strings.join(", ")))
                        }
                        None => Cow::Borrowed(original_ctype.as_str()),
                    }
                }
                "INTEGER" => Cow::Borrowed("INT64"),
                "FLOAT" => Cow::Borrowed("FLOAT64"),
                "BOOLEAN" => Cow::Borrowed("BOOL"),
                // Pass through other types as-is
                other => Cow::Borrowed(other),
            };

            let sql_type = if is_array {
                Cow::Owned(format!("ARRAY<{}>", inner_sql_type))
            } else {
                inner_sql_type
            };
            Some(sql_type)
        } else {
            None
        }
    }
}

pub mod postgres {
    use arrow_schema::{DataType, TimeUnit};

    use crate::AdapterResult;
    use crate::errors::{AdapterError, AdapterErrorKind};

    pub fn try_format_type(
        datatype: &DataType,
        nullable: bool,
        out: &mut String,
    ) -> AdapterResult<()> {
        use std::fmt::Write as _;
        match datatype {
            DataType::Null => out.push_str("null"),
            DataType::Boolean => out.push_str("boolean"),
            DataType::Int8 => out.push_str("tinyint"),
            DataType::Int16 => out.push_str("smallint"),
            DataType::Int32 => out.push_str("integer"),
            DataType::Int64 => out.push_str("bigint"),
            DataType::UInt8 => out.push_str("tinyint"),
            DataType::UInt16 => out.push_str("smallint"),
            DataType::UInt32 => out.push_str("integer"),
            DataType::UInt64 => out.push_str("bigint"),
            DataType::Float32 => out.push_str("real"),
            DataType::Float64 => out.push_str("double"),
            DataType::Timestamp(TimeUnit::Second, _) => out.push_str("timestamp without time zone"),
            DataType::Timestamp(TimeUnit::Millisecond, _) => {
                out.push_str("timestamp without time zone")
            }
            DataType::Timestamp(TimeUnit::Microsecond, _) => {
                out.push_str("timestamp without time zone")
            }
            DataType::Timestamp(TimeUnit::Nanosecond, _) => {
                out.push_str("timestamp without time zone")
            }
            DataType::Date32 => out.push_str("date"),
            DataType::Time32(TimeUnit::Second) => out.push_str("time without time zone"),
            DataType::Time32(TimeUnit::Millisecond) => out.push_str("time without time zone"),
            DataType::Time64(TimeUnit::Microsecond) => out.push_str("time without time zone"),
            DataType::Time64(TimeUnit::Nanosecond) => out.push_str("time without time zone"),
            DataType::Interval(_) => out.push_str("interval"),
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => {
                out.push_str("binary")
            }
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
                out.push_str("character varying")
            }
            DataType::List(_) => out.push_str("array"),
            DataType::Dictionary(key, value)
                if key.as_ref() == &DataType::UInt16 && value.as_ref() == &DataType::Utf8 =>
            {
                out.push_str("text")
            }
            DataType::Decimal128(precision, scale) => {
                write!(out, "decimal({precision}, {scale})").unwrap()
            }
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnsupportedType,
                    format!("{datatype} is not convertible to postgres type"),
                ));
            }
        };
        if !nullable {
            out.push_str(" not null");
        }
        Ok(())
    }
}

pub mod clickhouse {
    use super::*;

    /// TODO: long-term, ClickHouse column SQL types should be sourced from the
    /// driver's schema metadata rather than reconstructed from Arrow types here.
    pub fn try_format_type(
        datatype: &DataType,
        nullable: bool,
        out: &mut String,
    ) -> AdapterResult<()> {
        let mut datatype = datatype;
        let mut array_layers = Vec::new();
        while let DataType::List(item) | DataType::LargeList(item) = datatype {
            let item_type = item.data_type();
            array_layers.push((
                item.is_nullable(),
                matches!(item_type, DataType::List(_) | DataType::LargeList(_)),
            ));
            datatype = item_type;
        }

        let mut rendered = String::new();
        match datatype {
            DataType::Null => rendered.push_str("String"),
            DataType::Boolean => rendered.push_str("Bool"),
            DataType::Int8 => rendered.push_str("Int8"),
            DataType::Int16 => rendered.push_str("Int16"),
            DataType::Int32 => rendered.push_str("Int32"),
            DataType::Int64 => rendered.push_str("Int64"),
            DataType::UInt8 => rendered.push_str("UInt8"),
            DataType::UInt16 => rendered.push_str("UInt16"),
            DataType::UInt32 => rendered.push_str("UInt32"),
            DataType::UInt64 => rendered.push_str("UInt64"),
            DataType::Float16 | DataType::Float32 => rendered.push_str("Float32"),
            DataType::Float64 => rendered.push_str("Float64"),
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
                rendered.push_str("String")
            }
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => {
                rendered.push_str("String")
            }
            DataType::Date32 | DataType::Date64 => rendered.push_str("Date32"),
            DataType::Timestamp(TimeUnit::Second, _) => rendered.push_str("DateTime"),
            DataType::Timestamp(TimeUnit::Millisecond, _) => rendered.push_str("DateTime64(3)"),
            DataType::Timestamp(TimeUnit::Microsecond, _) => rendered.push_str("DateTime64(6)"),
            DataType::Timestamp(TimeUnit::Nanosecond, _) => rendered.push_str("DateTime64(9)"),
            DataType::Time32(_) | DataType::Time64(_) => rendered.push_str("String"),
            DataType::Decimal128(precision, scale) | DataType::Decimal256(precision, scale) => {
                rendered = format!("Decimal({precision}, {scale})");
            }
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnsupportedType,
                    format!("{datatype} is not convertible to clickhouse sql type"),
                ));
            }
        }

        if array_layers.is_empty() {
            if nullable {
                out.push_str(&format!("Nullable({rendered})"));
            } else {
                out.push_str(&rendered);
            }
        } else {
            for (item_nullable, item_is_array) in array_layers.into_iter().rev() {
                if item_nullable && !item_is_array {
                    rendered = format!("Nullable({rendered})");
                }
                rendered = format!("Array({rendered})");
            }
            out.push_str(&rendered);
        }
        Ok(())
    }
}

pub mod fabric {

    use arrow_schema::DataType;
    use std::fmt::Write;

    use crate::AdapterResult;
    use crate::errors::{AdapterError, AdapterErrorKind};

    // Microsoft Fabric row size limits allow VARCHAR up to 8000 bytes.
    // We default to VARCHAR(8000) for string-like Arrow types because VARCHAR
    // only stores the actual string length and does not allocate the full
    // declared size. Therefore using the maximum allowed size does not add
    // storage overhead but avoids truncation for longer values.
    const FABRIC_MAX_VARCHAR_TYPE: &str = "VARCHAR(8000)";

    pub fn try_format_type(
        datatype: &DataType,
        nullable: bool,
        out: &mut String,
    ) -> AdapterResult<()> {
        match datatype {
            DataType::Null => out.push_str("INT"),
            DataType::Boolean => out.push_str("BIT"),
            DataType::Int8 => out.push_str("SMALLINT"),
            DataType::Int16 => out.push_str("SMALLINT"),
            DataType::Int32 => out.push_str("INT"),
            DataType::Int64 => out.push_str("BIGINT"),

            DataType::UInt8 => out.push_str("SMALLINT"),
            DataType::UInt16 => out.push_str("INT"),
            DataType::UInt32 => out.push_str("BIGINT"),
            DataType::UInt64 => out.push_str("DECIMAL(20,0)"),
            DataType::Float32 => out.push_str("REAL"),
            DataType::Float64 => out.push_str("FLOAT"),

            DataType::Timestamp(_, _) => out.push_str("DATETIME2(6)"),

            DataType::Date32 => out.push_str("DATE"),

            DataType::Time32(_) | DataType::Time64(_) => out.push_str("TIME(6)"),

            DataType::Interval(_) => {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnsupportedType,
                    "INTERVAL is not supported in Microsoft Fabric",
                ));
            }
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => {
                out.push_str("VARBINARY(MAX)")
            }
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View => {
                out.push_str(FABRIC_MAX_VARCHAR_TYPE)
            }

            DataType::List(_) => {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnsupportedType,
                    "ARRAY is not supported in Microsoft Fabric",
                ));
            }

            DataType::Dictionary(_, value) if value.as_ref() == &DataType::Utf8 => {
                out.push_str(FABRIC_MAX_VARCHAR_TYPE)
            }
            DataType::Decimal128(precision, scale) => {
                write!(out, "DECIMAL({precision}, {scale})").unwrap()
            }
            _ => {
                return Err(AdapterError::new(
                    AdapterErrorKind::UnsupportedType,
                    format!("{datatype} is not convertible to fabric sql type"),
                ));
            }
        };
        if !nullable {
            out.push_str(" NOT NULL");
        }
        Ok(())
    }
}

pub const fn max_varchar_size(adapter_type: AdapterType) -> Option<usize> {
    use AdapterType::*;
    match adapter_type {
        // FIXME: Actual MAX is 134_217_728 - 16_777_216 is the default value
        Snowflake => Some(16_777_216),
        Redshift => Some(256),
        Postgres | Bigquery | Databricks | Salesforce | Spark | DuckDB | LakeCompute | Fabric
        | ClickHouse | Exasol | Starburst | Athena | Trino | Dremio | Oracle | Datafusion => None,
    }
}

pub const fn max_varbinary_size(adapter_type: AdapterType) -> Option<usize> {
    use AdapterType::*;
    match adapter_type {
        Snowflake => Some(16_777_216),
        Redshift => Some(65_535),
        // TODO: define limits for more systems
        Postgres | Bigquery | Databricks | Salesforce | Spark | DuckDB | LakeCompute | Fabric
        | ClickHouse | Exasol | Starburst | Athena | Trino | Dremio | Oracle | Datafusion => None,
    }
}

pub mod snowflake {
    use arrow_schema::DataType;

    // TODO: move away from this when we move away from the FixedSizeList hack
    // Additionally, it's a completely wrong assumption that drivers return types
    // like this. Drivers can't return these types. We should be using proper
    // SQL types and parsing them with [dbt_adapter_sql::types] instead.

    #[derive(Clone, Copy)]
    pub struct TimePrecision(u8);

    impl From<TimePrecision> for u8 {
        fn from(val: TimePrecision) -> Self {
            val.0
        }
    }

    impl TimePrecision {
        /// PRE-CONDITION: valid_precision <= 9
        pub const fn new(valid_precision: u8) -> Self {
            TimePrecision(valid_precision)
        }
    }

    #[derive(Clone, Copy)]
    pub enum IsTimestamp {
        No,
        Yes(TimePrecision),
    }

    impl IsTimestamp {
        pub const fn is_yes(&self) -> bool {
            matches!(self, IsTimestamp::Yes(_))
        }

        pub const fn precision(&self) -> Option<TimePrecision> {
            match self {
                IsTimestamp::No => None,
                IsTimestamp::Yes(precision) => Some(*precision),
            }
        }

        pub fn unwrap(self) -> TimePrecision {
            match self {
                IsTimestamp::No => panic!("Cannot unwrap IsTimestamp::No"),
                IsTimestamp::Yes(precision) => precision,
            }
        }
    }

    pub fn is_time(data_type: &DataType) -> IsTimestamp {
        match data_type {
            DataType::FixedSizeList(field, 1) if field.name().starts_with("time:") => {
                IsTimestamp::Yes(TimePrecision::new(
                    field
                        .name()
                        .strip_prefix("time:")
                        .expect("string prefix checked")
                        .parse::<u8>()
                        .expect("invalid serialized time precision"),
                ))
            }
            _ => IsTimestamp::No,
        }
    }

    pub fn is_timestamp_ntz(data_type: &DataType) -> IsTimestamp {
        match data_type {
            DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_ntz:") => {
                IsTimestamp::Yes(TimePrecision::new(
                    field
                        .name()
                        .strip_prefix("timestamp_ntz:")
                        .expect("string prefix checked")
                        .parse::<u8>()
                        .expect("invalid serialized timestamp precision"),
                ))
            }
            _ => IsTimestamp::No,
        }
    }

    pub fn is_timestamp_ltz(data_type: &DataType) -> IsTimestamp {
        match data_type {
            DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_ltz:") => {
                IsTimestamp::Yes(TimePrecision::new(
                    field
                        .name()
                        .strip_prefix("timestamp_ltz:")
                        .expect("string prefix checked")
                        .parse::<u8>()
                        .expect("invalid serialized timestamp precision"),
                ))
            }
            _ => IsTimestamp::No,
        }
    }

    pub fn is_timestamp_tz(data_type: &DataType) -> IsTimestamp {
        match data_type {
            DataType::FixedSizeList(field, 1) if field.name().starts_with("timestamp_tz:") => {
                IsTimestamp::Yes(TimePrecision::new(
                    field
                        .name()
                        .strip_prefix("timestamp_tz:")
                        .expect("string prefix checked")
                        .parse::<u8>()
                        .expect("invalid serialized timestamp precision"),
                ))
            }
            _ => IsTimestamp::No,
        }
    }
}

/// Returns the number of fractional digits for a given Arrow time unit.
fn time_precision(unit: TimeUnit) -> u8 {
    match unit {
        TimeUnit::Second => 0,
        TimeUnit::Millisecond => 3,
        TimeUnit::Microsecond => 6,
        TimeUnit::Nanosecond => 9,
    }
}

/// The size constraint for variable-size types (e.g. VARCHAR, VARBINARY).
pub fn var_size(adapter_type: AdapterType, data_type: &DataType) -> Option<usize> {
    use AdapterType::*;
    match (adapter_type, data_type) {
        // Strings: Redshift wants a length; persist it in char_size
        // TODO(jason): We need to report the correct size and not just a default
        (Redshift, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View) => {
            max_varchar_size(Redshift)
        }
        // For VARCHAR types, no explicit size in Snowflake unless specified
        (Snowflake, DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View) => None,
        // XXX: need to think about the defaults for these adapters
        (
            Postgres | Bigquery | Databricks | Salesforce,
            DataType::Utf8 | DataType::LargeUtf8 | DataType::Utf8View,
        ) => None,

        // Bytes
        // TODO(jason): We need to report the correct size and not just a default
        (Redshift, DataType::Binary | DataType::LargeBinary | DataType::BinaryView) => {
            max_varbinary_size(Redshift)
        }
        // XXX: need to think about the defaults for these adapters
        (
            Snowflake | Postgres | Bigquery | Databricks | Salesforce,
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView,
        ) => None,

        // Snowflake: For timestamp/date/time types, extract precision if available
        (Snowflake, dt) if snowflake::is_time(dt).is_yes() => {
            let char_size: u8 = snowflake::is_time(dt).unwrap().into();
            Some(char_size as usize)
        }
        (Snowflake, dt)
            if snowflake::is_timestamp_ntz(dt).is_yes()
                || snowflake::is_timestamp_ltz(dt).is_yes()
                || snowflake::is_timestamp_tz(dt).is_yes() =>
        {
            // For timestamp types, the precision is the fractional seconds precision
            // For compatibility with dbt core column type rendering code, precision is stored as char_size
            let time_precision = if snowflake::is_timestamp_ntz(dt).is_yes() {
                snowflake::is_timestamp_ntz(dt).unwrap()
            } else if snowflake::is_timestamp_ltz(dt).is_yes() {
                snowflake::is_timestamp_ltz(dt).unwrap()
            } else if snowflake::is_timestamp_tz(dt).is_yes() {
                snowflake::is_timestamp_tz(dt).unwrap()
            } else {
                return None;
            };
            let char_size: u8 = time_precision.into();
            Some(char_size as usize)
        }

        // Recurse for dictionary-encoded types
        // XXX: the key type is irrelevant and should probably be removed from the match pattern
        (_, DataType::Dictionary(key_ty, value_ty))
            if key_ty.as_ref() == &DataType::UInt16 && value_ty.as_ref() == &DataType::Utf8 =>
        {
            var_size(adapter_type, value_ty)
        }

        _ => None,
    }
}

pub fn numeric_precision_scale(
    adapter_type: AdapterType,
    data_type: &DataType,
) -> AdapterResult<Option<(u8, Option<i8>)>> {
    use AdapterType::*;
    let precision_scale = match (adapter_type, data_type) {
        (_, DataType::Decimal128(precision, scale) | DataType::Decimal256(precision, scale)) => {
            // cap precision at 38 for Redshift
            if adapter_type == Redshift && *precision > 38 {
                return Err(AdapterError::new(
                    AdapterErrorKind::NotSupported,
                    format!("Decimal precision '{}' exceed 38 place limit", *precision),
                ));
            }
            Some((*precision, Some(*scale)))
        }

        // For integer types (i.e. non-scaled numbers)
        (Postgres, DataType::Int8) => Some((3, Some(0))),
        (Postgres, DataType::Int16) => Some((5, Some(0))),
        (Postgres, DataType::Int32) => Some((10, Some(0))),
        (Postgres, DataType::Int64) => Some((19, Some(0))),
        (Postgres, DataType::UInt8) => Some((3, Some(0))),
        (Postgres, DataType::UInt16) => Some((5, Some(0))),
        (Postgres, DataType::UInt32) => Some((10, Some(0))),
        (Postgres, DataType::UInt64) => Some((20, Some(0))),
        (_, DataType::Int8) => Some((3, None)),
        (_, DataType::Int16) => Some((5, None)),
        (_, DataType::Int32) => Some((10, None)),
        (_, DataType::Int64) => Some((19, None)),
        (_, DataType::UInt8) => Some((3, None)),
        (_, DataType::UInt16) => Some((5, None)),
        (_, DataType::UInt32) => Some((10, None)),
        (_, DataType::UInt64) => Some((20, None)),

        // For floating point types (i.e. arbitrarily scaled numbers)
        (_, DataType::Float32) => Some((24, None)),
        (_, DataType::Float64) => Some((53, None)),

        // For timestamp/date/time types, extract precision if available
        (Snowflake, dt) if snowflake::is_time(dt).is_yes() => {
            let precision = snowflake::is_time(dt).unwrap();
            Some((precision.into(), None))
        }
        // XXX: maybe numeric_precision must be extract in this case too?

        // Handle general timestamp types
        (Snowflake, DataType::Timestamp(unit, _)) => {
            let precision = time_precision(*unit);
            Some((precision, None))
        }

        (_, DataType::Time64(_) | DataType::Time32(_)) => {
            // Redshift stores microseconds (6 fractional digits)
            Some((6, None))
        }
        // Timestamps (with or without tz) – clamp to microseconds
        // TODO: handle more complex timestamp/date/time types not in sdk front end
        (_, DataType::Timestamp(_, _)) => Some((6, None)),

        // Other types don't have specific precision/scale
        _ => None,
    };

    Ok(precision_scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use dbt_adapter_core::AdapterType::*;

    fn convert_type(data_type: &DataType, adapter_type: AdapterType) -> String {
        let ops = DefaultTypeOps::new(adapter_type);
        let mut out = String::new();
        ops.write_sql_type_for_dbt_convert_functions(data_type, &mut out)
            .unwrap();
        out
    }

    #[test]
    fn test_convert_integer_type() {
        let convert_integer_type = |adapter_type| convert_type(&DataType::Int64, adapter_type);
        assert_eq!(convert_integer_type(Bigquery), "int64");
        assert_eq!(convert_integer_type(Databricks), "bigint");
        assert_eq!(convert_integer_type(Postgres), "integer");
        assert_eq!(convert_integer_type(Snowflake), "integer");
        assert_eq!(convert_integer_type(Redshift), "integer");
    }

    #[test]
    fn clickhouse_try_format_type_formats_supported_arrow_types() {
        let mut out = String::new();
        clickhouse::try_format_type(&DataType::Int32, false, &mut out).unwrap();
        assert_eq!(out, "Int32");

        // ClickHouse expresses nullability inline: `Nullable(T)`, not as a
        // separate column attribute.
        out.clear();
        clickhouse::try_format_type(&DataType::Int32, true, &mut out).unwrap();
        assert_eq!(out, "Nullable(Int32)");

        // Arrays themselves are never nullable in ClickHouse — the wrapper
        // sits around the element type when the inner Field is nullable.
        out.clear();
        let nullable_item = Arc::new(Field::new("item", DataType::Utf8, true));
        clickhouse::try_format_type(&DataType::List(nullable_item), false, &mut out).unwrap();
        assert_eq!(out, "Array(Nullable(String))");

        // Non-nullable items round-trip as bare `Array(T)`.
        out.clear();
        let non_null_item = Arc::new(Field::new("item", DataType::Utf8, false));
        clickhouse::try_format_type(&DataType::List(non_null_item), false, &mut out).unwrap();
        assert_eq!(out, "Array(String)");

        out.clear();
        let nested_item = Arc::new(Field::new("item", DataType::Int32, true));
        let nested_list = Arc::new(Field::new("item", DataType::List(nested_item), false));
        clickhouse::try_format_type(&DataType::List(nested_list), false, &mut out).unwrap();
        assert_eq!(out, "Array(Array(Nullable(Int32)))");

        // Arrow `Date32` must map to ClickHouse `Date32` (4-byte Int32), not
        // plain `Date` (2-byte UInt16), to avoid silent truncation past ~2149.
        out.clear();
        clickhouse::try_format_type(&DataType::Date32, false, &mut out).unwrap();
        assert_eq!(out, "Date32");

        out.clear();
        clickhouse::try_format_type(&DataType::Date64, true, &mut out).unwrap();
        assert_eq!(out, "Nullable(Date32)");
    }

    #[test]
    fn clickhouse_try_format_type_rejects_unsupported_arrow_types() {
        let mut out = String::new();
        let fields = arrow_schema::Fields::from(Vec::<Field>::new());
        let err = clickhouse::try_format_type(&DataType::Struct(fields), true, &mut out)
            .expect_err("structs must not silently format as String");
        assert_eq!(err.kind(), AdapterErrorKind::UnsupportedType);
    }

    #[test]
    fn test_convert_number_type() {
        let convert_floating_type = |adapter_type| convert_type(&DataType::Float64, adapter_type);
        assert_eq!(convert_floating_type(Bigquery), "float64");
        assert_eq!(convert_floating_type(Databricks), "double");
        assert_eq!(convert_floating_type(Postgres), "float8");
        assert_eq!(convert_floating_type(Snowflake), "float8");
        assert_eq!(convert_floating_type(Redshift), "float8");
        let convert_decimal_type =
            |adapter_type| convert_type(&DataType::Decimal32(10, 0), adapter_type);
        assert_eq!(convert_decimal_type(Bigquery), "int64");
        assert_eq!(convert_decimal_type(Databricks), "bigint");
        assert_eq!(convert_decimal_type(Postgres), "integer");
        assert_eq!(convert_decimal_type(Snowflake), "integer");
        assert_eq!(convert_decimal_type(Redshift), "integer");
        let convert_decimal_type =
            |adapter_type| convert_type(&DataType::Decimal128(10, 2), adapter_type);
        assert_eq!(convert_decimal_type(Bigquery), "float64");
        assert_eq!(convert_decimal_type(Databricks), "double");
        assert_eq!(convert_decimal_type(Postgres), "float8");
        assert_eq!(convert_decimal_type(Snowflake), "float8");
        assert_eq!(convert_decimal_type(Redshift), "float8");
    }

    #[test]
    fn test_convert_boolean_type() {
        let convert_boolean_type = |adapter_type| convert_type(&DataType::Boolean, adapter_type);
        assert_eq!(convert_boolean_type(Bigquery), "bool");
        assert_eq!(convert_boolean_type(Databricks), "boolean");
        assert_eq!(convert_boolean_type(Postgres), "boolean");
        assert_eq!(convert_boolean_type(Snowflake), "boolean");
        assert_eq!(convert_boolean_type(Redshift), "boolean");
    }

    #[test]
    fn test_convert_datetime_type() {
        let convert_datetime_type = |adapter_type| {
            convert_type(
                &DataType::Timestamp(TimeUnit::Millisecond, None),
                adapter_type,
            )
        };
        assert_eq!(convert_datetime_type(Bigquery), "datetime");
        assert_eq!(convert_datetime_type(Databricks), "timestamp");
        assert_eq!(
            convert_datetime_type(Postgres),
            "timestamp without time zone"
        );
        assert_eq!(
            convert_datetime_type(Snowflake),
            "timestamp without time zone"
        );
        assert_eq!(
            convert_datetime_type(Redshift),
            "timestamp without time zone"
        );
    }
    const ALL_ADAPTERS: [AdapterType; 5] = [Bigquery, Databricks, Postgres, Snowflake, Redshift];

    #[test]
    fn test_convert_date_type() {
        let convert_date_type = |adapter_type| convert_type(&DataType::Date64, adapter_type);
        // Test all adapters return "date"
        for adapter_type in ALL_ADAPTERS {
            assert_eq!(convert_date_type(adapter_type), "date");
        }
    }

    #[test]
    fn test_convert_time_type() {
        let convert_time_type =
            |adapter_type| convert_type(&DataType::Duration(TimeUnit::Nanosecond), adapter_type);
        // Test all adapters return "time"
        for adapter_type in ALL_ADAPTERS {
            assert_eq!(convert_time_type(adapter_type), "time");
        }
    }

    #[test]
    fn test_convert_text_type() {
        let convert_text_type = |adapter_type| convert_type(&DataType::Utf8, adapter_type);
        assert_eq!(convert_text_type(Bigquery), "string");
        assert_eq!(convert_text_type(Databricks), "string");
        assert_eq!(convert_text_type(Postgres), "text");
        assert_eq!(convert_text_type(Snowflake), "text");
        assert_eq!(convert_text_type(Redshift), "text");
    }

    fn bq(ty: &str, repeated: bool) -> HashMap<String, String> {
        HashMap::from([
            ("Type".to_string(), ty.to_string()),
            ("Repeated".to_string(), repeated.to_string()),
        ])
    }

    #[test]
    fn bigquery_field_to_string_recovers_nested_types_of_repeated_records() {
        // pois ARRAY<STRUCT<poi_name STRING, location STRUCT<poi_type STRING, point_geom GEOGRAPHY>>>
        // A REPEATED RECORD arrives as a list wrapping the struct.
        let poi_name =
            Field::new("poi_name", DataType::Utf8, true).with_metadata(bq("STRING", false));
        let poi_type =
            Field::new("poi_type", DataType::Utf8, true).with_metadata(bq("STRING", false));
        let point_geom =
            Field::new("point_geom", DataType::Utf8, true).with_metadata(bq("GEOGRAPHY", false));

        let location = Field::new(
            "location",
            DataType::Struct(vec![poi_type, point_geom].into()),
            true,
        )
        .with_metadata(bq("RECORD", false));
        let element = Field::new(
            "item",
            DataType::Struct(vec![poi_name, location].into()),
            true,
        );
        let pois = Field::new("pois", DataType::List(Arc::new(element)), true)
            .with_metadata(bq("RECORD", true));

        assert_eq!(
            bigquery::field_to_string(&pois).unwrap(),
            "ARRAY<STRUCT<`poi_name` STRING, `location` STRUCT<`poi_type` STRING, `point_geom` GEOGRAPHY>>>"
        );
    }

    #[test]
    fn bigquery_field_to_string_recurses_through_repeated_records_at_every_level() {
        // outer ARRAY<STRUCT<inner ARRAY<STRUCT<point_geom GEOGRAPHY>>>>
        let point_geom =
            Field::new("point_geom", DataType::Utf8, true).with_metadata(bq("GEOGRAPHY", false));

        let inner_element = Field::new("item", DataType::Struct(vec![point_geom].into()), true);
        let inner = Field::new("inner", DataType::List(Arc::new(inner_element)), true)
            .with_metadata(bq("RECORD", true));

        let outer_element = Field::new("item", DataType::Struct(vec![inner].into()), true);
        let outer = Field::new("outer", DataType::List(Arc::new(outer_element)), true)
            .with_metadata(bq("RECORD", true));

        assert_eq!(
            bigquery::field_to_string(&outer).unwrap(),
            "ARRAY<STRUCT<`inner` ARRAY<STRUCT<`point_geom` GEOGRAPHY>>>>"
        );
    }

    // Regression tests for the Exasol seed/agate `convert_*` type mappings.
    // Text-like types map to Exasol's maximum VARCHAR, and floating types to
    // DOUBLE PRECISION.

    #[test]
    fn test_exasol_convert_text_type() {
        // text/varchar/clob/char all collapse to the maximum VARCHAR.
        assert_eq!(convert_type(&DataType::Utf8, Exasol), "VARCHAR(2000000)");
        assert_eq!(
            convert_type(&DataType::Utf8View, Exasol),
            "VARCHAR(2000000)"
        );
        assert_eq!(
            convert_type(&DataType::LargeUtf8, Exasol),
            "VARCHAR(2000000)"
        );
    }

    #[test]
    fn test_exasol_convert_number_type() {
        // Real/Float/Double all map to DOUBLE PRECISION on Exasol.
        assert_eq!(convert_type(&DataType::Float32, Exasol), "DOUBLE PRECISION");
        assert_eq!(convert_type(&DataType::Float64, Exasol), "DOUBLE PRECISION");
        // Fractional decimals also map to DOUBLE PRECISION.
        assert_eq!(
            convert_type(&DataType::Decimal128(10, 2), Exasol),
            "DOUBLE PRECISION"
        );
    }

    #[test]
    fn test_exasol_convert_datetime_type() {
        // Exasol uses "timestamp" for datetime values.
        assert_eq!(
            convert_type(&DataType::Timestamp(TimeUnit::Microsecond, None), Exasol),
            "timestamp"
        );
    }

    #[test]
    fn test_exasol_convert_integer_type() {
        // Integers stay "integer" on Exasol (a valid DECIMAL(18,0) alias).
        assert_eq!(convert_type(&DataType::Int64, Exasol), "integer");
    }

    #[test]
    fn test_exasol_convert_time_type() {
        // A time-of-day seed column becomes TIMESTAMP on Exasol.
        assert_eq!(
            convert_type(&DataType::Time64(TimeUnit::Microsecond), Exasol),
            "timestamp"
        );
    }
}
