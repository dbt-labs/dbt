use dbt_adapter_sql::types::{SqlType, StructField};
use dbt_common::current_function_name;
use once_cell::sync::Lazy;
use regex::Regex;
use std::sync::Arc;

use dbt_adapter_core::*;
use dbt_schemas::schemas::dbt_column::DbtCoreBaseColumn;
use dbt_schemas::schemas::serde::minijinja_value_to_typed_struct;
use minijinja;
use minijinja::{
    Value,
    arg_utils::{ArgParser, ArgsIter, check_num_args},
    value::{Enumerator, Object},
};

use dbt_schemas::schemas::dbt_column::DbtColumn;

static LOG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"([^(]+)(\([^)]+\))?").expect("A valid regex"));

/// Matches a COLLATE clause such as `COLLATE 'en-ci'` (case-insensitive).
static COLLATION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\s+COLLATE\s+'([^']+)'").expect("A valid regex"));

/// A struct representing a column type for use with static methods
#[derive(Clone, Copy, Debug)]
pub struct ColumnStatic(AdapterType);

impl Object for ColumnStatic {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "create" => self.jinja_create(args),
            "translate_type" => {
                let iter = ArgsIter::new("ColumnStatic.translate_type", &["dtype"], args);
                let dtype = iter.next_arg::<&str>()?;
                iter.finish()?;

                Ok(Value::from(self.translate_type(dtype)))
            }
            "numeric_type" => {
                let iter = ArgsIter::new(
                    "ColumnStatic.numeric_type",
                    &["dtype", "precision", "scale"],
                    args,
                );
                let dtype = iter.next_arg::<&str>()?;
                let precision: Option<u64> = iter.next_arg::<Option<u64>>()?;
                let scale: Option<u64> = iter.next_arg::<Option<u64>>()?;
                iter.finish()?;

                Ok(Value::from(self.numeric_type(dtype, precision, scale)))
            }
            "string_type" => {
                let iter = ArgsIter::new("ColumnStatic.string_type", &["size"], args);
                let size = iter.next_arg::<Option<usize>>()?;
                iter.finish()?;

                Ok(Value::from(self.string_type(size)))
            }
            "from_description" => {
                let iter = ArgsIter::new(
                    "ColumnStatic.from_description",
                    &["name", "raw_data_type"],
                    args,
                );
                let name = iter.next_arg::<&str>()?;
                let raw_data_type = iter.next_arg::<&str>()?;
                iter.finish()?;

                self.from_description(name, raw_data_type)
                    .map(Value::from_object)
            }

            // Below are DatabricksColumn-only
            "format_add_column_list" => {
                // TODO: ArgsIter
                let mut args = ArgParser::new(args, None);
                let columns = args.get::<Value>("columns")?;
                let columns = Column::vec_from_jinja_value(AdapterType::Databricks, columns)?;

                Ok(Value::from(self.dbx_format_add_column_list(&columns)?))
            }
            "format_remove_column_list" => {
                // TODO: ArgsIter
                let mut args = ArgParser::new(args, None);
                let columns = args.get::<Value>("columns")?;
                let columns = Column::vec_from_jinja_value(AdapterType::Databricks, columns)?;

                Ok(Value::from(self.dbx_format_remove_column_list(&columns)?))
            }
            "get_name" => {
                let mut args: ArgParser = ArgParser::new(args, None);
                let column = args.get::<Value>("column")?;
                // FIXME: why is this DbtColumn and not Column?
                let column = minijinja_value_to_typed_struct::<DbtColumn>(column).map_err(|e| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::SerdeDeserializeError,
                        e.to_string(),
                    )
                })?;

                Ok(Value::from(self.dbx_get_name(&column)))
            }
            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on ColumnStatic: '{name}'"),
            )),
        }
    }

    fn call(
        self: &Arc<Self>,
        _state: &minijinja::State,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        self.jinja_create(args)
    }
}

impl ColumnStatic {
    pub fn new(adapter_type: AdapterType) -> Self {
        Self(adapter_type)
    }

    fn jinja_create(&self, args: &[Value]) -> Result<Value, minijinja::Error> {
        let iter = ArgsIter::new(
            "ColumnStatic.create",
            &[
                "name",
                "label_or_dtype",
                "char_size",
                "numeric_precision",
                "numeric_scale",
            ],
            args,
        );

        let name = iter.next_pos_arg_aliased::<&str>(&["column"])?;
        let dtype = iter.next_pos_arg_aliased::<&str>(&["dtype"])?;

        let char_size = iter.next_arg::<Option<u32>>().unwrap_or(None);
        let numeric_precision = iter.next_arg::<Option<u64>>().unwrap_or(None);
        let numeric_scale = iter.next_arg::<Option<u64>>().unwrap_or(None);
        iter.finish()?;

        Ok(Value::from_object(self.new_instance(
            name.to_string(),
            dtype.to_string(),
            char_size,
            numeric_precision,
            numeric_scale,
        )))
    }

    /// Create a new column from the given arguments
    /// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/column.py#L28-L29
    pub fn new_instance(
        &self,
        name: String,
        dtype: String,
        char_size: Option<u32>,
        numeric_precision: Option<u64>,
        numeric_scale: Option<u64>,
    ) -> Column {
        Column::new(
            self.0,
            name,
            dtype,
            char_size,
            numeric_precision,
            numeric_scale,
        )
    }

    /// TODO: reconcile with [quote_char] for ClickHouse and remove the special handling
    /// For ClickHouse, this function returns "{s}" (double quotes), while [quote_char]
    /// returns `{s}` (backticks). This was done deliberately to avoid changing goldie
    /// recordings and avoid CI job failures
    fn quote(&self, s: &str) -> String {
        match self.0 {
            AdapterType::ClickHouse => format!("\"{s}\""),
            _ => {
                let q = quote_char(self.0);
                format!("{q}{s}{q}")
            }
        }
    }

    pub fn translate_type(&self, column_type: &str) -> String {
        let translated = match self.0 {
            // https://github.com/dbt-labs/dbt-adapters/blob/6f2aae13e39c5df1c93e5d514678914142d71768/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L16
            AdapterType::Bigquery => match column_type.to_uppercase().as_str() {
                "TEXT" => "STRING",
                "FLOAT" => "FLOAT64",
                "INTEGER" => "INT64",
                _ => column_type,
            },
            AdapterType::Databricks | AdapterType::Spark => {
                match column_type.to_uppercase().as_str() {
                    "LONG" => "BIGINT",
                    _ => column_type,
                }
            }
            // https://github.com/microsoft/dbt-fabric/blob/81d9764e24b00e7c923a2235ba68fa6bd6b90ea9/dbt/adapters/fabric/fabric_column.py#L8
            AdapterType::Fabric => match column_type.to_uppercase().as_str() {
                "STRING" => "VARCHAR(8000)",
                "VARCHAR" => "VARCHAR(8000)",
                "CHAR" => "CHAR(1)",
                "NCHAR" => "CHAR(1)",
                "NVARCHAR" => "VARCHAR(8000)",
                "TIMESTAMP" => "DATETIME2(6)",
                "DATETIME2" => "DATETIME2(6)",
                "DATETIME2(6)" => "DATETIME2(6)",
                "DATE" => "DATE",
                "TIME" => "TIME(6)",
                "FLOAT" => "FLOAT",
                "REAL" => "REAL",
                "INT" => "INT",
                "INTEGER" => "INT",
                "BIGINT" => "BIGINT",
                "SMALLINT" => "SMALLINT",
                "TINYINT" => "SMALLINT",
                "BIT" => "BIT",
                "BOOLEAN" => "BIT",
                "DECIMAL" => "DECIMAL",
                "NUMERIC" => "NUMERIC",
                "MONEY" => "DECIMAL",
                "SMALLMONEY" => "DECIMAL",
                "UNIQUEIDENTIFIER" => "UNIQUEIDENTIFIER",
                "VARBINARY" => "VARBINARY(MAX)",
                "BINARY" => "BINARY(1)",
                _ => column_type,
            },
            // ClickHouseColumn.TYPE_LABELS; unmatched types pass through verbatim.
            // https://github.com/ClickHouse/dbt-clickhouse/blob/main/dbt/adapters/clickhouse/column.py#L13-L18
            AdapterType::ClickHouse => match column_type.to_uppercase().as_str() {
                "STRING" => "String",
                "TIMESTAMP" => "DateTime",
                "FLOAT" => "Float32",
                "INTEGER" => "Int32",
                _ => column_type,
            },
            // https://github.com/dbt-labs/dbt-adapters/blob/fed0e2e7a2e252175dcc9caccbdd91d354ac6a9d/dbt-adapters/src/dbt/adapters/base/column.py#L24
            _ => match column_type.to_uppercase().as_str() {
                "STRING" => "TEXT",
                _ => column_type,
            },
        };
        match self.0 {
            // https://github.com/databricks/dbt-databricks/blob/45351e11517d3f37c5ac7a736b5fcba453d3f368/dbt/adapters/databricks/column.py#L24
            AdapterType::Databricks => translated.to_lowercase(),
            _ => translated.to_string(),
        }
    }

    pub fn numeric_type(&self, dtype: &str, precision: Option<u64>, scale: Option<u64>) -> String {
        match self.0 {
            // https://github.com/dbt-labs/dbt-adapters/blob/6f2aae13e39c5df1c93e5d514678914142d71768/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L97
            AdapterType::Bigquery => dtype.to_string(),
            _ => match (precision, scale) {
                (Some(p), Some(s)) => format!("{dtype}({p},{s})"),
                _ => dtype.to_string(),
            },
        }
    }

    pub fn string_type(&self, size: Option<usize>) -> String {
        match self.0 {
            // https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#string_type
            AdapterType::Bigquery => match size {
                Some(size) => format!("STRING({size})"),
                _ => "STRING".to_string(),
            },
            // ClickHouseColumn.string_type ignores the size: always plain String,
            // never FixedString (would break contract comparisons and ALTERs).
            AdapterType::ClickHouse => "String".to_string(),
            _ => match size {
                Some(size) => format!("character varying({size})"),
                _ => "character varying".to_string(),
            },
        }
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/column.py#L127-L128
    #[expect(clippy::wrong_self_convention)]
    fn from_description(
        &self,
        name: &str,
        raw_data_type: &str,
    ) -> Result<Column, minijinja::Error> {
        // TODO(serramatutu): why is this Snowflake specific in non-Snowflake specific trait?
        // It seems like it is used by other adapters as well... (tested with BigQuery)
        let mut col = Column::try_from_snowflake_raw_data_type(name, raw_data_type)
            .map_err(|msg| minijinja::Error::new(minijinja::ErrorKind::InvalidArgument, msg))?;
        col._adapter_type = self.0;
        Ok(col)
    }

    /// https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/column.py#L66
    fn dbx_format_add_column_list(
        self: &Arc<Self>,
        columns: &[Column],
    ) -> Result<String, minijinja::Error> {
        if self.0 != AdapterType::Databricks {
            unimplemented!("Only available for Databricks")
        };

        Ok(columns
            .iter()
            .map(|c| format!("{} {}", c.quoted(), c.core_dtype))
            .collect::<Vec<String>>()
            .join(", "))
    }

    /// https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/column.py#L62
    fn dbx_format_remove_column_list(
        self: &Arc<Self>,
        columns: &[Column],
    ) -> Result<String, minijinja::Error> {
        if self.0 != AdapterType::Databricks {
            unimplemented!("Only available for Databricks")
        };

        Ok(columns
            .iter()
            .map(|c| c.quoted().as_str().to_owned())
            .collect::<Vec<String>>()
            .join(", "))
    }

    /// https://github.com/databricks/dbt-databricks/blob/5e20eeaef43e671913f995d8079d4ec2b8a1da6d/dbt/adapters/databricks/column.py#L34
    fn dbx_get_name(self: &Arc<Self>, column: &DbtColumn) -> String {
        if self.0 != AdapterType::Databricks {
            unimplemented!("Only available for Databricks")
        };

        if column.quote.unwrap_or(false) {
            self.quote(&column.name)
        } else {
            column.name.to_string()
        }
    }
}

/// NULLABLE, REQUIRED, REPEATED
#[derive(Default)]
pub enum BigqueryColumnMode {
    /// NULLABLE
    #[default]
    Nullable,
    /// REQUIRED
    Required,
    /// REPEATED
    Repeated,
}

impl AsRef<str> for BigqueryColumnMode {
    fn as_ref(&self) -> &str {
        match self {
            Self::Nullable => "NULLABLE",
            Self::Required => "REQUIRED",
            Self::Repeated => "REPEATED",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Column {
    /// The adapter this column is associated with.
    ///
    /// Instead of using sub-typing and virtual-dispatch as in dbt-adapters, we
    /// pattern-match against the adapter type for adapter-specific behavior.
    ///
    /// NOTE: Fields starting with _ are not exposed via the Jinja API of the object.
    #[allow(clippy::used_underscore_binding)]
    _adapter_type: AdapterType,

    /// Whether this column is `NULLABLE` or `NOT NULL` (optional).
    #[allow(clippy::used_underscore_binding)]
    _nullable: Option<bool>,

    /// Whether this column is an array/repeated field (optional).
    ///
    /// This is important for BigQuery, where a column can be `REPEATED`.
    /// [Column::mode] provides a unified way to derive the BigQuery mode
    /// of a column. BigQuery adopts the Protobuf model of nullability where
    /// repeated fields are always non-nullable because the null state is
    /// represented by an empty array.
    #[allow(clippy::used_underscore_binding)]
    _repeated: Option<bool>,

    /// The fields of this column in case it's a nested struct (on BigQuery).
    ///
    /// BigQuery allows columns to be nested structs such through the `STRUCT<NUMBER, BOOL>`
    /// syntax. These subfields can be infinitely nested to compose complex objects.
    #[allow(clippy::used_underscore_binding)]
    _fields: Vec<Self>,

    /// The original data type string as used during instantiation.
    original_sql_str: Option<String>,

    /// Name of the column. Confusingly named `column` in dbt-adapters.
    name: String,

    /// Optional comment / description fetched from the warehouse.
    ///
    /// This is needed to support upstream-style persist-docs behavior, including clearing existing
    /// comments by setting them to the empty string when the model column has no description.
    comment: Option<String>,

    /// dbt Core's degenerate representation of dtype, derived from _original_sql_str
    core_dtype: String,

    /// dbt Core's slightly less degenerate representation of data type, derived from _original_sql_str
    core_data_type: String,

    /// The size of the column in characters (u32 is enough to hold) var char of max length
    /// Postgres is 65536 (2^16 - 1)
    /// Snowflake is 16777216 (2^24)
    char_size: Option<u32>,
    // TODO no need for u64; this should use 32 as char size (for consistency) or less; in some database scale can be negative
    numeric_precision: Option<u64>,
    numeric_scale: Option<u64>,

    /// Collation specifier for string columns (e.g. `"en-ci"` from `VARCHAR(10) COLLATE 'en-ci'`).
    /// Populated when parsing Snowflake raw data types that include a COLLATE clause.
    collation: Option<String>,
}

impl Column {
    #[cfg(test)]
    fn cmp_column(&self, other: &Self) -> bool {
        self._adapter_type == other._adapter_type
            && self._nullable == other._nullable
            && self._repeated == other._repeated
            && self
                ._fields
                .iter()
                .zip(other._fields.iter())
                .all(|(a, b)| a.cmp_column(b))
            && self.name == other.name
            && self.comment == other.comment
            && self.core_dtype == other.core_dtype
            && self.core_data_type == other.core_data_type
            && self.char_size == other.char_size
            && self.numeric_precision == other.numeric_precision
            && self.numeric_scale == other.numeric_scale
            && self.collation == other.collation
    }

    fn make_degenerate_data_type_from_parsed_struct_fields(
        adapter_type: AdapterType,
        fields: &[StructField],
    ) -> String {
        let fields_str = fields
            .iter()
            .map(|f| {
                let (_, inner_data_type) =
                    Self::make_degenerate_types_from_parsed_sqltype(adapter_type, &f.sql_type);
                format!("{} {inner_data_type}", f.name.display(adapter_type))
            })
            .collect::<Vec<_>>()
            .join(", ");

        format!("STRUCT<{}>", fields_str)
    }

    fn make_degenerate_types_from_parsed_sqltype(
        adapter_type: AdapterType,
        sql_type: &SqlType,
    ) -> (String, String) {
        match (adapter_type, sql_type) {
            (AdapterType::Bigquery, SqlType::Boolean) => {
                ("BOOLEAN".to_string(), "BOOLEAN".to_string())
            }
            (AdapterType::Bigquery, sql_type) => {
                match sql_type {
                    SqlType::Array(Some(inner)) => match inner.as_ref() {
                        SqlType::Struct(Some(fields)) => (
                            "RECORD".to_string(),
                            format!(
                                "ARRAY<{}>",
                                Self::make_degenerate_data_type_from_parsed_struct_fields(
                                    adapter_type,
                                    fields.as_slice()
                                )
                            ),
                        ),
                        SqlType::Array(_) => unreachable!(
                            "ARRAY of ARRAY is not allowed in BigQuery. This is a bug."
                        ),
                        _ => {
                            let (dtype, data_type) =
                                Self::make_degenerate_types_from_parsed_sqltype(
                                    adapter_type,
                                    inner.as_ref(),
                                );

                            (dtype, format!("ARRAY<{}>", data_type))
                        }
                    },
                    SqlType::Struct(Some(fields)) => (
                        "RECORD".to_string(),
                        Self::make_degenerate_data_type_from_parsed_struct_fields(
                            adapter_type,
                            fields.as_slice(),
                        ),
                    ),
                    // TODO: is Array(None) possible? How does Core represent that?
                    _ => {
                        let s = sql_type.to_string(adapter_type);
                        (s.clone(), s)
                    }
                }
            }
            _ => {
                let dtype = sql_type.to_string(adapter_type);

                // FIXME: the implementation of data_type() is wrong anyways
                (dtype.clone(), dtype)
            }
        }
    }

    /// Normalize the original_sql_str to dbt Core's canonical representations, which might lose some
    /// information.
    ///
    /// Some types can be represented in different ways. For example, a boolean in BigQuery
    /// can be `BOOL` or `BOOLEAN`. However, dbt Core always represents them consistently with
    /// an opinionated choice. This method normalizes dtype names to this consistent
    /// representation.
    fn make_degenerate_types(
        adapter_type: AdapterType,
        original_sql_str: &str,
    ) -> (String, String) {
        match adapter_type {
            AdapterType::Bigquery => {
                let Ok((sql_type, _nullable)) =
                    SqlType::parse(AdapterType::Bigquery, original_sql_str)
                else {
                    return (original_sql_str.to_string(), original_sql_str.to_string());
                };

                Self::make_degenerate_types_from_parsed_sqltype(adapter_type, &sql_type)
            }
            _ => {
                let stripped = original_sql_str
                    .strip_suffix(" not null")
                    .or_else(|| original_sql_str.strip_suffix(" NOT NULL"))
                    .unwrap_or(original_sql_str);
                let was_nullable = stripped == original_sql_str;

                debug_assert!(
                    {
                        let parsed = SqlType::parse(adapter_type, original_sql_str);
                        match parsed {
                            Ok((_, nullable)) => was_nullable == nullable,
                            Err(_) => true, // TODO(felipecrv): assert here so we discover bad inputs
                        }
                    },
                    "not null specified in original_sql_str ('{}') but not as 'not null' or 'NOT NULL' suffix",
                    original_sql_str
                );

                (stripped.to_string(), stripped.to_string())
            }
        }
    }

    pub fn new(
        adapter_type: AdapterType,
        name: String,
        original_sql_str: String,
        char_size: Option<u32>,
        numeric_precision: Option<u64>,
        numeric_scale: Option<u64>,
    ) -> Self {
        let (core_dtype, core_data_type) =
            Self::make_degenerate_types(adapter_type, &original_sql_str);

        Self {
            _adapter_type: adapter_type,
            _nullable: None,
            _repeated: None,
            _fields: Vec::new(),
            original_sql_str: Some(original_sql_str),
            name,
            comment: None,
            core_dtype,
            core_data_type,
            char_size,
            numeric_precision,
            numeric_scale,
            collation: None,
        }
    }

    /// Construct based on a value parsed from dbt Core Jinja
    fn from_dbt_core(adapter_type: AdapterType, col: DbtCoreBaseColumn) -> Self {
        Self {
            _adapter_type: adapter_type,
            _nullable: None,
            _repeated: None,
            _fields: Vec::new(),
            // TODO(serramatutu): figure out a way to not lose the original dtype information
            // while roundtripping from Jinja
            original_sql_str: None,
            name: col.name,
            comment: None,
            core_dtype: col.dtype.clone(),
            core_data_type: col.dtype,
            char_size: col.char_size,
            numeric_precision: col.numeric_precision,
            numeric_scale: col.numeric_scale,
            collation: None,
        }
    }

    pub fn from_jinja_value(
        adapter_type: AdapterType,
        value: Value,
    ) -> Result<Self, minijinja::Error> {
        let core_col =
            minijinja_value_to_typed_struct::<DbtCoreBaseColumn>(value).map_err(|e| {
                minijinja::Error::new(minijinja::ErrorKind::SerdeDeserializeError, e.to_string())
            })?;

        Ok(Self::from_dbt_core(adapter_type, core_col))
    }

    pub fn vec_from_jinja_value(
        _adapter_type: AdapterType,
        value: Value,
    ) -> Result<Vec<Self>, minijinja::Error> {
        // Iterate over the jinja value which should be a sequence
        value
            .try_iter()?
            .map(|item| {
                item.downcast_object_ref::<Self>().cloned().ok_or_else(|| {
                    minijinja::Error::new(
                        minijinja::ErrorKind::InvalidOperation,
                        "Failed to downcast jinja value to Column; expected Column object",
                    )
                })
            })
            .collect()
    }

    /// Create a new BigQuery column
    ///
    /// `mode` ias a field is seen in BQ (https://cloud.google.com/bigquery/docs/schemas#modes)
    pub fn new_bigquery(
        name: String,
        original_sql_str: String,
        fields: impl Into<Vec<Self>>,
        mode: BigqueryColumnMode,
    ) -> Self {
        use BigqueryColumnMode::*;
        let (nullable, repeated) = match mode {
            Nullable => (Some(true), None),
            Required => (Some(false), None),
            Repeated => (None, Some(true)),
        };
        let (core_dtype, core_data_type) =
            Self::make_degenerate_types(AdapterType::Bigquery, &original_sql_str);
        Self {
            _adapter_type: AdapterType::Bigquery,
            _nullable: nullable,
            _repeated: repeated,
            _fields: fields.into(),
            original_sql_str: Some(original_sql_str),
            name,
            comment: None,
            core_dtype,
            core_data_type,
            char_size: None,
            numeric_precision: None,
            numeric_scale: None,
            collation: None,
        }
    }

    pub fn as_static(&self) -> ColumnStatic {
        ColumnStatic::new(self._adapter_type)
    }

    /// Parse a Snowflake raw data type into a tuple of (data_type, char_size, numeric_precision, numeric_scale)
    fn try_from_snowflake_raw_data_type(name: &str, raw_data_type: &str) -> Result<Column, String> {
        // Extract and strip any COLLATE clause before further parsing.
        // e.g. "VARCHAR(10) COLLATE 'en-ci'" → collation = Some("en-ci"), type_without_collation = "VARCHAR(10)"
        let collation = COLLATION_RE.captures(raw_data_type).map(|c| {
            c.get(1)
                .expect("capture group 1 exists")
                .as_str()
                .to_string()
        });
        let type_without_collation = COLLATION_RE.replace(raw_data_type, "");
        let raw_data_type_for_parse: &str = &type_without_collation;

        // We want to pass through numeric parsing for composite types
        let raw_data_type_trimmed = raw_data_type_for_parse.trim().to_lowercase();
        if raw_data_type_trimmed.starts_with("array")
            || raw_data_type_trimmed.starts_with("object")
            || raw_data_type_trimmed.starts_with("map")
            || raw_data_type_trimmed.starts_with("vector")
        {
            let (core_dtype, core_data_type) =
                Self::make_degenerate_types(AdapterType::Snowflake, raw_data_type);

            return Ok(Column {
                _adapter_type: AdapterType::Snowflake,
                _nullable: None,
                _repeated: None,
                _fields: Vec::new(),
                original_sql_str: Some(raw_data_type.to_string()),
                name: name.to_string(),
                comment: None,
                core_dtype,
                core_data_type,
                char_size: None,
                numeric_precision: None,
                numeric_scale: None,
                collation: None,
            });
        }
        // Parse data type using regex pattern ([^(]+)(\([^)]+\))?
        // Use the collation-stripped version for parsing, but keep the original for original_sql_str.

        let captures = LOG_RE
            .captures(raw_data_type_for_parse)
            .ok_or_else(|| format!("Could not interpret raw_data_type \"{raw_data_type}\""))?;

        let data_type = captures
            .get(1)
            .expect("First match group exists")
            .as_str()
            .to_string();
        let mut char_size = None;
        let mut numeric_precision = None;
        let mut numeric_scale = None;

        // If we have size info (the second capture group)
        let err_msg = |raw_data_type: &str, name: &str| {
            format!(
                "Could not interpret data_type \"{raw_data_type}\": could not convert \"{name}\" to an integer"
            )
        };
        if let Some(size_match) = captures.get(2) {
            let size_info = &size_match.as_str()[1..size_match.as_str().len() - 1];
            let parts: Vec<&str> = size_info.split(',').collect();

            match parts.len() {
                1 => {
                    // parse as char_size
                    char_size = Some(
                        parts[0]
                            .parse::<u32>()
                            .map_err(|_| err_msg(raw_data_type, parts[0]))?,
                    );
                }
                2 => {
                    // parse as numeric precision and scale
                    numeric_precision = Some(
                        parts[0]
                            .parse::<u64>()
                            .map_err(|_| err_msg(raw_data_type, parts[0]))?,
                    );
                    numeric_scale = Some(
                        parts[1]
                            .parse::<u64>()
                            .map_err(|_| err_msg(raw_data_type, parts[0]))?,
                    );
                }
                _ => {}
            }
        }
        Ok(Self {
            _adapter_type: AdapterType::Snowflake,
            _nullable: None,
            _repeated: None,
            _fields: Vec::new(),
            original_sql_str: Some(raw_data_type.to_string()),
            name: name.to_string(),
            comment: None,
            core_dtype: data_type.clone(),
            core_data_type: data_type,
            char_size,
            numeric_precision,
            numeric_scale,
            collation,
        })
    }

    pub fn mode(&self) -> BigqueryColumnMode {
        match (self._nullable, self._repeated) {
            (_, Some(true)) => BigqueryColumnMode::Repeated,
            (Some(true), _) => BigqueryColumnMode::Nullable,
            (Some(false), _) => BigqueryColumnMode::Required,
            (_, _) => BigqueryColumnMode::Nullable,
        }
    }

    pub fn comment(&self) -> Option<&str> {
        self.comment.as_deref()
    }

    pub fn with_comment(mut self, comment: Option<String>) -> Self {
        self.comment = comment;
        self
    }

    /// Enrich column with model config (Databricks/Spark).
    ///
    /// Returns a new Column with data_type, comment, and nullability from model config.
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/column.py#L153-L166
    pub fn enrich_for_create(&self, model_column: Option<&DbtColumn>, not_null: bool) -> Self {
        let mut col = self.clone();
        if let Some(mc) = model_column {
            if let Some(ref dt) = mc.data_type {
                col.core_dtype = dt.clone();
                col.core_data_type = dt.clone();
            }
            if let Some(ref desc) = mc.description {
                col.comment = Some(desc.clone());
            }
        }
        col._nullable = Some(!not_null);
        col
    }

    #[inline]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn into_name(self) -> String {
        self.name
    }

    pub fn original_sql_str(&self) -> Option<&str> {
        self.original_sql_str.as_deref()
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/column.py#L92-L93
    pub fn string_size(&self) -> Result<u32, String> {
        if !self.is_string() {
            return Err("Called string_size() on non-string field".to_string());
        }

        // FIXME: why self.core_dtype == "text" instead of is_string()? This is probably a bug...
        if self.core_dtype == "text" || self.char_size.is_none() {
            let size = match self._adapter_type {
                AdapterType::Snowflake => 16777216,
                _ => 256,
            };
            Ok(size)
        } else {
            // TODO: this is probably unsafe. But in `dbt-adapters`
            // char_size seems to be unset unless initialized from `from_description` class method
            Ok(self
                .char_size
                .ok_or_else(|| format!("char_size is not set for column: {}", self.name))?)
        }
    }

    fn is_numeric(&self) -> bool {
        match self._adapter_type {
            AdapterType::Bigquery => {
                matches!(self.core_dtype.to_lowercase().as_str(), "numeric")
            }
            AdapterType::Snowflake => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "int"
                        | "integer"
                        | "bigint"
                        | "smallint"
                        | "tinyint"
                        | "byteint"
                        | "numeric"
                        | "decimal"
                        | "number"
                )
            }
            _ => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "numeric" | "decimal"
                )
            }
        }
    }

    fn is_integer(&self) -> bool {
        match self._adapter_type {
            AdapterType::Bigquery => {
                matches!(self.core_dtype.to_lowercase().as_str(), "int64")
            }
            AdapterType::Snowflake => false,
            _ => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "smallint"
                        | "integer"
                        | "bigint"
                        | "smallserial"
                        | "serial"
                        | "bigserial"
                        | "int2"
                        | "int4"
                        | "int8"
                        | "serial2"
                        | "serial4"
                        | "serial8"
                )
            }
        }
    }

    fn is_float(&self) -> bool {
        match self._adapter_type {
            AdapterType::Bigquery => {
                matches!(self.core_dtype.to_lowercase().as_str(), "float64")
            }
            AdapterType::Snowflake => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "float" | "float4" | "float8" | "double" | "double precision" | "real"
                )
            }
            _ => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "real" | "float4" | "float" | "double precision" | "float8" | "double"
                )
            }
        }
    }

    fn is_number(&self) -> bool {
        self.is_float() || self.is_integer() || self.is_numeric()
    }

    fn is_string(&self) -> bool {
        match self._adapter_type {
            AdapterType::Bigquery | AdapterType::ClickHouse => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "string" | "fixedstring"
                )
            }
            _ => {
                matches!(
                    self.core_dtype.to_lowercase().as_str(),
                    "text" | "character varying" | "character" | "varchar"
                )
            }
        }
    }

    fn quoted(&self) -> String {
        self.as_static().quote(&self.name)
    }

    pub fn dtype(&self) -> &str {
        &self.core_dtype
    }

    // TODO: impl data_type - need to handle nested types
    // https://github.com/dbt-labs/dbt-adapters/blob/6f2aae13e39c5df1c93e5d514678914142d71768/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L80
    pub fn data_type(&self) -> String {
        // FIXME: replace all implementations with core_data_type
        match self._adapter_type {
            AdapterType::Bigquery => {
                fn bigquery_data_type_inner(col: &Column) -> String {
                    let base = if col._fields.is_empty() {
                        // `core_data_type` may already include an ARRAY wrapper (e.g. "ARRAY<INT64>")
                        // for repeated columns due to how BigQuery models arrays via `mode=REPEATED`.
                        // Render the element type as the base and then re-apply `mode` uniformly.
                        if matches!(col.mode(), BigqueryColumnMode::Repeated) {
                            let inner =
                                SqlType::parse(AdapterType::Bigquery, col.core_data_type.as_str())
                                    .ok()
                                    .and_then(|(sql_type, _nullable)| match sql_type {
                                        SqlType::Array(Some(inner)) => Some(inner),
                                        _ => None,
                                    });

                            if let Some(inner) = inner {
                                Column::make_degenerate_types_from_parsed_sqltype(
                                    AdapterType::Bigquery,
                                    inner.as_ref(),
                                )
                                .1
                            } else {
                                col.core_data_type.clone()
                            }
                        } else {
                            col.core_data_type.clone()
                        }
                    } else {
                        let fields_str = col
                            ._fields
                            .iter()
                            .map(|f| {
                                let field_type = bigquery_data_type_inner(f);
                                format!("{} {field_type}", col.as_static().quote(f.name()))
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        format!("STRUCT<{fields_str}>")
                    };

                    if matches!(col.mode(), BigqueryColumnMode::Repeated) {
                        format!("ARRAY<{base}>")
                    } else {
                        base
                    }
                }
                bigquery_data_type_inner(self)
            }
            _ => {
                if self.is_string() {
                    self.as_static().string_type(Some(
                        self.string_size().expect("string should have a size") as usize,
                    ))
                } else if self.is_numeric() {
                    self.as_static().numeric_type(
                        &self.core_dtype,
                        self.numeric_precision,
                        self.numeric_scale,
                    )
                } else {
                    // TODO for types such as Snowflake TIMESTAMP_LTZ(6), we should return ``format!("{}({})", dtype, precision)``.
                    //  Note that this would not be dbt core compatible behavior, but a more correct one.
                    //  Otherwise we may create/alter a table to a wrong type.
                    //  See also https://github.com/dbt-labs/fs/pull/3585#discussion_r2112390711
                    self.core_dtype.to_string()
                }
            }
        }
    }

    /// Render column DDL for CREATE TABLE (Databricks/Spark).
    ///
    /// Returns e.g. `` `col` BIGINT NOT NULL COMMENT 'comment' ``
    ///
    /// Reference: https://github.com/databricks/dbt-databricks/blob/822b105b15e644676d9e1f47cbfd765cd4c1541f/dbt/adapters/databricks/column.py#L167-L179
    pub fn render_for_create(&self) -> String {
        match self._adapter_type {
            AdapterType::Databricks | AdapterType::Spark => {
                let mut s = format!("{} {}", self.quoted(), self.data_type());
                if self._nullable == Some(false) {
                    s.push_str(" NOT NULL");
                }
                if let Some(comment) = &self.comment
                    && !comment.is_empty()
                {
                    let escaped = comment.replace('\\', "\\\\").replace('\'', "\\'");
                    s.push_str(&format!(" COMMENT '{escaped}'"));
                }
                s
            }
            _ => unimplemented!("render_for_create is only available for Databricks/Spark"),
        }
    }

    pub fn char_size(&self) -> Option<u32> {
        self.char_size
    }

    pub fn numeric_precision(&self) -> Option<u64> {
        self.numeric_precision
    }

    pub fn numeric_scale(&self) -> Option<u64> {
        self.numeric_scale
    }

    pub fn collation(&self) -> Option<&str> {
        self.collation.as_deref()
    }

    /// Returns `data_type()` with collation appended for string columns that have one.
    /// e.g. `VARCHAR(10) collate 'en-ci'` instead of just `VARCHAR(10)`.
    pub fn expanded_data_type(&self) -> String {
        let base = self.data_type();
        match &self.collation {
            Some(c) if self.is_string() => format!("{base} collate '{c}'"),
            _ => base,
        }
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/main/dbt-adapters/src/dbt/adapters/base/column.py#L102-L103
    pub fn can_expand_to(&self, other: &Column) -> Result<bool, minijinja::Error> {
        Ok(self.is_string()
            && other.is_string()
            && self
                .string_size()
                .map_err(|msg| minijinja::Error::new(minijinja::ErrorKind::MissingArgument, msg))?
                < other.string_size().map_err(|msg| {
                    minijinja::Error::new(minijinja::ErrorKind::MissingArgument, msg)
                })?)
    }

    fn _bq_flatten_inner(&self, prefix: &str) -> Vec<Self> {
        let new_prefix = if prefix.is_empty() {
            self.name.clone()
        } else {
            format!("{prefix}.{}", self.name)
        };

        let original_sql_str = if let Some(s) = self.original_sql_str.as_ref() {
            s.clone()
        } else {
            self.core_data_type.clone()
        };

        if self._fields.is_empty() {
            Vec::from([Self::new_bigquery(
                new_prefix,
                original_sql_str,
                &[],
                self.mode(),
            )])
        } else {
            let mut new_fields = Vec::new();
            for f in &self._fields {
                let mut flatten_f = f._bq_flatten_inner(&new_prefix);
                new_fields.append(&mut flatten_f);
            }
            new_fields
        }
    }

    /// https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-bigquery/src/dbt/adapters/bigquery/column.py#L69
    pub fn flatten(&self) -> Vec<Self> {
        if !matches!(self._adapter_type, AdapterType::Bigquery) {
            unimplemented!("'flatten' is only implemented for Bigquery")
        }

        self._bq_flatten_inner("")
    }

    pub fn fields(&self) -> &[Self] {
        &self._fields
    }
}

impl Object for Column {
    fn call_method(
        self: &Arc<Self>,
        _state: &minijinja::State,
        name: &str,
        args: &[Value],
        _listeners: &[std::rc::Rc<dyn minijinja::listener::RenderingEventListener>],
    ) -> Result<Value, minijinja::Error> {
        match name {
            "is_string" => Ok(Value::from(self.is_string())),
            "string_size" => Ok(Value::from(self.string_size().map_err(|msg| {
                minijinja::Error::new(minijinja::ErrorKind::InvalidArgument, msg)
            })?)),
            "is_number" => Ok(Value::from(self.is_number())),
            "is_float" => Ok(Value::from(self.is_float())),
            "is_integer" => Ok(Value::from(self.is_integer())),
            "is_numeric" => Ok(Value::from(self.is_numeric())),
            "can_expand_to" => {
                // TODO(serramatutu): use ArgsIter
                let mut parser = ArgParser::new(args, None);
                check_num_args(current_function_name!(), &parser, 1, 1)?;
                let other_raw = parser.get::<Value>("other_column")?;
                let other = Column::from_jinja_value(self._adapter_type, other_raw)?;
                Ok(Value::from(self.can_expand_to(&other)?))
            }
            // Bigquery only
            "flatten" => Ok(Value::from(self.flatten())),
            // Databricks/Spark only - render column DDL for CREATE TABLE
            "render_for_create" => Ok(Value::from(self.render_for_create())),

            _ => Err(minijinja::Error::new(
                minijinja::ErrorKind::UnknownMethod,
                format!("Unknown method on Column: '{name}'"),
            )),
        }
    }

    fn get_value(self: &Arc<Self>, key: &Value) -> Option<Value> {
        match key.as_str() {
            // @property methods
            Some("name") | Some("column") => Some(Value::from(&self.name)),
            Some("quoted") => Some(Value::from(self.quoted())),
            Some("data_type") => Some(Value::from(self.data_type())),
            // direct fields
            Some("dtype") => Some(Value::from(&self.core_dtype)),
            Some("char_size") => Some(Value::from(self.char_size)),
            Some("numeric_precision") => Some(Value::from(self.numeric_precision)),
            Some("numeric_scale") => Some(Value::from(self.numeric_scale)),
            Some("collation") => Some(Value::from(self.collation.as_deref())),
            Some("expanded_data_type") => Some(Value::from(self.expanded_data_type())),
            Some("mode") => Some(Value::from(self.mode().as_ref())),
            Some("fields") => Some(Value::from(
                self._fields
                    .iter()
                    .map(|f| Value::from_object(f.clone()))
                    .collect::<Vec<_>>(),
            )),
            _ => None,
        }
    }

    fn enumerate(self: &Arc<Self>) -> Enumerator {
        let mut keys = vec![
            "name",
            "dtype",
            "char_size",
            "column",
            "quoted",
            "numeric_precision",
            "numeric_scale",
        ];

        if matches!(self._adapter_type, AdapterType::Bigquery) {
            keys.push("fields");
            keys.push("mode");
        }

        Enumerator::Iter(Box::new(keys.into_iter().map(Value::from)))
    }
}

#[expect(clippy::from_over_into)]
impl Into<Value> for Column {
    fn into(self) -> Value {
        Value::from_object(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ClickHouseColumn.TYPE_LABELS mapping (the array macros build
    /// `emptyArray{type}` from it); unmatched types pass through.
    #[test]
    fn test_translate_type_clickhouse_type_labels() {
        let col = ColumnStatic(AdapterType::ClickHouse);
        assert_eq!(col.translate_type("string"), "String");
        assert_eq!(col.translate_type("timestamp"), "DateTime");
        assert_eq!(col.translate_type("float"), "Float32");
        assert_eq!(col.translate_type("integer"), "Int32");
        assert_eq!(col.translate_type("UInt64"), "UInt64");
    }

    /// string_type must never manufacture FixedString for ClickHouse.
    #[test]
    fn test_string_type_clickhouse_ignores_size() {
        let col = ColumnStatic(AdapterType::ClickHouse);
        assert_eq!(col.string_type(Some(256)), "String");
        assert_eq!(col.string_type(None), "String");
    }

    #[test]
    fn test_stripping_of_not_null_constraint() {
        let (dtype, _) =
            Column::make_degenerate_types(AdapterType::Snowflake, "VARCHAR(255) NOT NULL");
        assert_eq!(dtype, "VARCHAR(255)");

        let (dtype, _) =
            Column::make_degenerate_types(AdapterType::Snowflake, "NUMBER(10,2) not null");
        assert_eq!(dtype, "NUMBER(10,2)");

        let (dtype, _) = Column::make_degenerate_types(AdapterType::Snowflake, "BLAH NOT NULL");
        assert_eq!(dtype, "BLAH");
    }

    #[test]
    fn test_translate_type_lowercases_for_databricks_only() {
        // Databricks lowercases translated types; the input type is preserved
        // otherwise (DECIMAL(10, 2) does not match any rewrite rule).
        assert_eq!(
            ColumnStatic::new(AdapterType::Databricks).translate_type("DECIMAL(10, 2)"),
            "decimal(10, 2)"
        );

        // Snowflake (and other adapters) preserve the original casing.
        assert_eq!(
            ColumnStatic::new(AdapterType::Snowflake).translate_type("DECIMAL(10, 2)"),
            "DECIMAL(10, 2)"
        );

        // The rewrite still applies before lowercasing on Databricks.
        assert_eq!(
            ColumnStatic::new(AdapterType::Databricks).translate_type("LONG"),
            "bigint"
        );
        // Spark shares the same rewrite but is not lowercased.
        assert_eq!(
            ColumnStatic::new(AdapterType::Spark).translate_type("LONG"),
            "BIGINT"
        );
    }

    #[test]
    fn test_identifier_quoting_per_adapter() {
        // Spark, like Databricks and BigQuery, quotes identifiers with backticks.
        // Double quotes would make Spark parse them as string literals (not columns).
        for adapter in [
            AdapterType::Spark,
            AdapterType::Databricks,
            AdapterType::Bigquery,
        ] {
            let col = Column::new(
                adapter,
                "id".to_string(),
                "int".to_string(),
                None,
                None,
                None,
            );
            assert_eq!(
                col.quoted(),
                "`id`",
                "{adapter:?} should backtick-quote identifiers"
            );
        }

        for adapter in [AdapterType::Snowflake, AdapterType::Postgres] {
            let col = Column::new(
                adapter,
                "id".to_string(),
                "int".to_string(),
                None,
                None,
                None,
            );
            assert_eq!(
                col.quoted(),
                "\"id\"",
                "{adapter:?} should double-quote identifiers"
            );
        }
    }

    #[test]
    fn databricks_create_column_omits_empty_comment() {
        let column = Column::new(
            AdapterType::Databricks,
            "id".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .with_comment(Some(String::new()));

        assert_eq!(column.render_for_create(), "`id` int");
    }

    #[test]
    fn databricks_create_column_preserves_non_empty_comment() {
        let column = Column::new(
            AdapterType::Databricks,
            "id".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .with_comment(Some("documented".to_string()));

        assert_eq!(column.render_for_create(), "`id` int COMMENT 'documented'");
    }

    #[test]
    fn test_try_from_snowflake_raw_data_type_object() {
        let result = Column::try_from_snowflake_raw_data_type(
            "test_col",
            "OBJECT(name VARCHAR, age NUMBER)",
        );
        assert!(result.is_ok());

        let column = result.unwrap();
        assert_eq!(column.name, "test_col");
        assert_eq!(column.dtype(), "OBJECT(name VARCHAR, age NUMBER)");
        assert_eq!(column._adapter_type, AdapterType::Snowflake);
        assert_eq!(column.char_size, None);
        assert_eq!(column.numeric_precision, None);
        assert_eq!(column.numeric_scale, None);
    }

    #[test]
    fn test_try_from_snowflake_raw_data_type_numeric() {
        let result = Column::try_from_snowflake_raw_data_type("test_col", "NUMERIC(10,2)");
        assert!(result.is_ok());

        let column = result.unwrap();
        assert_eq!(column.name, "test_col");
        assert_eq!(column.dtype(), "NUMERIC");
        assert_eq!(column._adapter_type, AdapterType::Snowflake);
        assert_eq!(column.char_size, None);
        assert_eq!(column.numeric_precision, Some(10));
        assert_eq!(column.numeric_scale, Some(2));
    }

    #[test]
    fn test_try_from_snowflake_raw_data_type_numeric_precision_only() {
        let result = Column::try_from_snowflake_raw_data_type("test_col", "NUMERIC(18)");
        assert!(result.is_ok());

        let column = result.unwrap();
        assert_eq!(column.name, "test_col");
        assert_eq!(column.dtype(), "NUMERIC");
        assert_eq!(column._adapter_type, AdapterType::Snowflake);
        assert_eq!(column.char_size, Some(18));
        assert_eq!(column.numeric_precision, None);
        assert_eq!(column.numeric_scale, None);
    }

    #[test]
    fn test_bq_flatten() {
        let simple = Column::new_bigquery(
            "col".to_string(),
            "NUMBER".to_string(),
            &[],
            BigqueryColumnMode::Nullable,
        );
        assert!(
            simple
                .flatten()
                .iter()
                .zip(
                    [Column::new_bigquery(
                        "col".to_string(),
                        "NUMBER".to_string(),
                        &[],
                        BigqueryColumnMode::Nullable
                    )]
                    .iter()
                )
                .all(|(a, b)| a.cmp_column(b))
        );

        let nested = Column::new_bigquery(
            "parent".to_string(),
            "STRUCT<NUMBER col>".to_string(),
            &[simple],
            BigqueryColumnMode::Nullable,
        );
        assert!(
            nested
                .flatten()
                .iter()
                .zip(
                    [Column::new_bigquery(
                        "parent.col".to_string(),
                        "NUMBER".to_string(),
                        &[],
                        BigqueryColumnMode::Nullable
                    ),]
                    .iter()
                )
                .all(|(a, b)| a.cmp_column(b))
        );

        let deeply_nested = Column::new_bigquery(
            "grandpa".to_string(),
            "STRUCT<NUMBER col>".to_string(),
            &[nested],
            BigqueryColumnMode::Nullable,
        );
        assert!(
            deeply_nested
                .flatten()
                .iter()
                .zip(
                    [Column::new_bigquery(
                        "grandpa.parent.col".to_string(),
                        "NUMBER".to_string(),
                        &[],
                        BigqueryColumnMode::Nullable
                    ),]
                    .iter()
                )
                .all(|(a, b)| a.cmp_column(b))
        );

        let nested_with_siblings = Column::new_bigquery(
            "parent".to_string(),
            "STRUCT<NUMBER a, BOOLEAN b>".to_string(),
            &[
                Column::new_bigquery(
                    "a".to_string(),
                    "NUMBER".to_string(),
                    &[],
                    BigqueryColumnMode::Nullable,
                ),
                Column::new_bigquery(
                    "b".to_string(),
                    "BOOLEAN".to_string(),
                    &[],
                    BigqueryColumnMode::Nullable,
                ),
            ],
            BigqueryColumnMode::Nullable,
        );
        assert!(
            nested_with_siblings
                .flatten()
                .iter()
                .zip(
                    [
                        Column::new_bigquery(
                            "parent.a".to_string(),
                            "NUMBER".to_string(),
                            &[],
                            BigqueryColumnMode::Nullable
                        ),
                        Column::new_bigquery(
                            "parent.b".to_string(),
                            "BOOLEAN".to_string(),
                            &[],
                            BigqueryColumnMode::Nullable
                        )
                    ]
                    .iter()
                )
                .all(|(a, b)| a.cmp_column(b))
        );
    }

    #[test]
    fn test_bigquery_data_type_should_render_struct_fields_when_present() {
        let city = Column::new_bigquery(
            "city".to_string(),
            "STRING".to_string(),
            &[],
            BigqueryColumnMode::Nullable,
        );

        let coords = Column::new_bigquery(
            "coords".to_string(),
            "STRUCT".to_string(),
            &[
                Column::new_bigquery(
                    "lat".to_string(),
                    "FLOAT64".to_string(),
                    &[],
                    BigqueryColumnMode::Nullable,
                ),
                Column::new_bigquery(
                    "lon".to_string(),
                    "FLOAT64".to_string(),
                    &[],
                    BigqueryColumnMode::Nullable,
                ),
            ],
            BigqueryColumnMode::Nullable,
        );

        let location = Column::new_bigquery(
            "location_entity".to_string(),
            "STRUCT".to_string(),
            &[city, coords],
            BigqueryColumnMode::Nullable,
        );

        assert_eq!(
            location.data_type(),
            "STRUCT<`city` STRING, `coords` STRUCT<`lat` FLOAT64, `lon` FLOAT64>>"
        );
    }

    #[test]
    fn test_bigquery_data_type_should_render_repeated_struct_fields_as_array() {
        let authors = Column::new_bigquery(
            "authors".to_string(),
            "STRING".to_string(),
            &[],
            BigqueryColumnMode::Repeated,
        );

        let content = Column::new_bigquery(
            "content_entity".to_string(),
            "STRUCT".to_string(),
            &[authors],
            BigqueryColumnMode::Nullable,
        );

        assert_eq!(content.data_type(), "STRUCT<`authors` ARRAY<STRING>>");
    }

    #[test]
    fn test_snowflake_collation_parsing() {
        let col =
            Column::try_from_snowflake_raw_data_type("c", "VARCHAR(10) COLLATE 'en-ci'").unwrap();
        assert_eq!(col.collation(), Some("en-ci"));
        assert_eq!(col.char_size, Some(10));
        assert_eq!(col.dtype(), "VARCHAR");
        // original_sql_str preserves the full string including collation
        assert_eq!(col.original_sql_str(), Some("VARCHAR(10) COLLATE 'en-ci'"));
    }

    #[test]
    fn test_snowflake_collation_case_insensitive() {
        let col =
            Column::try_from_snowflake_raw_data_type("c", "varchar(20) collate 'utf8'").unwrap();
        assert_eq!(col.collation(), Some("utf8"));
        assert_eq!(col.char_size, Some(20));
    }

    #[test]
    fn test_snowflake_no_collation() {
        let col = Column::try_from_snowflake_raw_data_type("c", "VARCHAR(10)").unwrap();
        assert_eq!(col.collation(), None);
        assert_eq!(col.char_size, Some(10));
    }

    #[test]
    fn test_expanded_data_type_with_collation() {
        let col =
            Column::try_from_snowflake_raw_data_type("c", "VARCHAR(10) COLLATE 'en-ci'").unwrap();
        assert_eq!(col.data_type(), "character varying(10)");
        assert_eq!(
            col.expanded_data_type(),
            "character varying(10) collate 'en-ci'"
        );
    }

    #[test]
    fn test_expanded_data_type_without_collation() {
        let col = Column::try_from_snowflake_raw_data_type("c", "VARCHAR(10)").unwrap();
        assert_eq!(col.expanded_data_type(), col.data_type());
    }

    #[test]
    fn test_can_expand_to_with_collation() {
        let small =
            Column::try_from_snowflake_raw_data_type("c", "VARCHAR(10) COLLATE 'en-ci'").unwrap();
        let large =
            Column::try_from_snowflake_raw_data_type("c", "VARCHAR(20) COLLATE 'en-ci'").unwrap();
        assert!(small.can_expand_to(&large).unwrap());
        assert!(!large.can_expand_to(&small).unwrap());
    }

    #[test]
    fn test_render_for_create_omits_empty_comment() {
        let no_comment = Column::new(
            AdapterType::Databricks,
            "col_a".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .with_comment(None);
        assert_eq!(no_comment.render_for_create(), "`col_a` int");

        let empty_comment = Column::new(
            AdapterType::Databricks,
            "col_a".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .with_comment(Some(String::new()));
        assert_eq!(empty_comment.render_for_create(), "`col_a` int");

        let with_comment = Column::new(
            AdapterType::Databricks,
            "col_b".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .with_comment(Some("has a description".to_string()));
        assert_eq!(
            with_comment.render_for_create(),
            "`col_b` int COMMENT 'has a description'"
        );
    }

    #[test]
    fn test_enrich_for_create_with_empty_description_renders_no_comment() {
        let model_column = DbtColumn {
            name: "col_a".to_string(),
            description: Some(String::new()),
            ..Default::default()
        };
        let enriched = Column::new(
            AdapterType::Databricks,
            "col_a".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .enrich_for_create(Some(&model_column), false);
        assert_eq!(enriched.render_for_create(), "`col_a` int");

        let described = DbtColumn {
            name: "col_b".to_string(),
            description: Some("has a description".to_string()),
            ..Default::default()
        };
        let enriched = Column::new(
            AdapterType::Databricks,
            "col_b".to_string(),
            "int".to_string(),
            None,
            None,
            None,
        )
        .enrich_for_create(Some(&described), true);
        assert_eq!(
            enriched.render_for_create(),
            "`col_b` int NOT NULL COMMENT 'has a description'"
        );
    }
}
