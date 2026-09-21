use std::borrow::Cow;
use std::sync::LazyLock;

use crate::AdapterResult;
use crate::column::{BigqueryColumnMode, Column};
use crate::errors::{AdapterError, AdapterErrorKind};
use crate::metadata;
use crate::sql_types::{self, TypeOps, original_type_string};
use arrow_schema::{DataType, FieldRef};
use dbt_adapter_core::AdapterType;
use dbt_adapter_sql::types::SqlType;
use regex::Regex;

pub struct ColumnBuilder {
    adapter_type: AdapterType,
}

impl ColumnBuilder {
    pub fn new(adapter_type: AdapterType) -> Self {
        Self { adapter_type }
    }

    pub fn build(&self, field: &FieldRef, type_ops: &dyn TypeOps) -> AdapterResult<Column> {
        use AdapterType::*;
        match self.adapter_type {
            Snowflake => Ok(Self::build_snowflake(field, type_ops)),
            Bigquery => Ok(Self::build_bigquery(field, type_ops)),
            Databricks | Spark => Ok(Self::build_databricks(field, type_ops)),
            Redshift => Ok(Self::build_redshift(field, type_ops)),
            // These adapters have always been tagged as Postgres columns here;
            // keep that as-is for them.
            Postgres | Salesforce | DuckDB | LakeCompute => {
                Ok(Self::build_postgres_like(Postgres, field, type_ops))
            }
            // Same rendering rules, but tag the column with its own adapter type so
            // downstream `Column` logic and `build_from_parts` agree.
            GizmoSQL => Ok(Self::build_postgres_like(GizmoSQL, field, type_ops)),
            Fabric => Ok(Self::build_fabric(field, type_ops)),
            ClickHouse => Self::build_clickhouse(field, type_ops),
            Exasol => Ok(Self::build_exasol(field, type_ops)),
            Starburst => todo!("Starburst"),
            Athena => todo!("Athena"),
            Trino => todo!("Trino"),
            Dremio => todo!("Dremio"),
            Oracle => todo!("Oracle"),
            Datafusion => todo!("Datafusion"),
        }
    }

    pub fn build_from_parts(
        &self,
        name: String,
        dtype: String,
        char_size: Option<u32>,
        numeric_precision: Option<u64>,
        numeric_scale: Option<u64>,
        mode: Option<BigqueryColumnMode>,
    ) -> Column {
        use AdapterType::*;
        match self.adapter_type {
            Postgres => Column::new(
                Postgres,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            DuckDB => Column::new(
                DuckDB,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            GizmoSQL => Column::new(
                GizmoSQL,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            LakeCompute => Column::new(
                LakeCompute,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            Snowflake => Column::new(
                Snowflake,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            // TODO: BigQuery fields
            Bigquery => Column::new_bigquery(name, dtype, &[], mode.unwrap_or_default()),
            Redshift => Column::new(
                Redshift,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            Databricks | Spark => Column::new(
                self.adapter_type,
                name,
                dtype,
                char_size,
                None, // numeric_precision
                None, // numeric_scale
            ),
            Salesforce => todo!("Salesforce column creation not implemented yet"),
            ClickHouse => Column::new(
                ClickHouse,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            Exasol => Column::new(
                Exasol,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
            Starburst => todo!("Starburst"),
            Athena => todo!("Athena"),
            Trino => todo!("Trino"),
            Dremio => todo!("Dremio"),
            Oracle => todo!("Oracle"),
            Datafusion => todo!("Datafusion"),
            Fabric => Column::new(
                Fabric,
                name,
                dtype,
                char_size,
                numeric_precision,
                numeric_scale,
            ),
        }
    }

    const CLICKHOUSE_TYPE_WRAPPERS: [&'static str; 2] = ["LowCardinality", "Nullable"];

    fn strip_clickhouse_wrappers(dtype: &str) -> &str {
        let mut s = dtype.trim();
        while let Some(inner) = Self::CLICKHOUSE_TYPE_WRAPPERS
            .iter()
            .find_map(|wrapper| Self::strip_type_wrapper(s, wrapper))
        {
            s = inner;
        }
        s
    }

    fn strip_type_wrapper<'a>(s: &'a str, wrapper: &str) -> Option<&'a str> {
        let s = s.trim();
        let rest = Self::strip_prefix_ignore_ascii_case(s, wrapper)?;
        let inner = rest.strip_prefix('(')?.strip_suffix(')')?;
        Some(inner.trim())
    }

    fn strip_prefix_ignore_ascii_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
        let head = s.get(..prefix.len())?;
        if head.eq_ignore_ascii_case(prefix) {
            s.get(prefix.len()..)
        } else {
            None
        }
    }

    fn parse_fixed_string_size(dtype: &str) -> Option<u32> {
        let inner = Self::strip_prefix_ignore_ascii_case(dtype.trim(), "FixedString(")?
            .strip_suffix(')')?;
        inner.trim().parse().ok()
    }

    fn parse_decimal_precision_scale(dtype: &str) -> Option<(u64, u64)> {
        let inner =
            Self::strip_prefix_ignore_ascii_case(dtype.trim(), "Decimal(")?.strip_suffix(')')?;
        let mut parts = inner.split(',').map(str::trim);
        let precision = parts.next()?.parse().ok()?;
        let scale = parts.next()?.parse().ok()?;
        Some((precision, scale))
    }

    fn clickhouse_numeric_precision_scale(
        data_type: &DataType,
    ) -> AdapterResult<(Option<u64>, Option<u64>)> {
        match sql_types::numeric_precision_scale(AdapterType::ClickHouse, data_type)? {
            Some((precision, Some(scale))) => Ok((
                Some(u64::from(precision)),
                Some(Self::widen_numeric_scale(scale)?),
            )),
            Some((precision, None)) => Ok((Some(u64::from(precision)), None)),
            None => Ok((None, None)),
        }
    }

    fn widen_numeric_scale(scale: i8) -> AdapterResult<u64> {
        debug_assert!(scale >= 0);
        u64::try_from(scale).map_err(|_| {
            AdapterError::new(
                AdapterErrorKind::UnexpectedResult,
                format!("negative numeric scale {scale}"),
            )
        })
    }

    fn build_clickhouse(field: &FieldRef, type_ops: &dyn TypeOps) -> AdapterResult<Column> {
        use AdapterType::ClickHouse;

        let type_text = match original_type_string(ClickHouse, field) {
            Some(s) => s.into_owned(),
            None => {
                let mut out = String::new();
                type_ops.format_arrow_type_as_sql(
                    field.data_type(),
                    field.is_nullable(),
                    &mut out,
                )?;
                out
            }
        };

        let inner = Self::strip_clickhouse_wrappers(&type_text);
        let char_size = Self::parse_fixed_string_size(inner);
        let (numeric_precision, numeric_scale) =
            if let Some((precision, scale)) = Self::parse_decimal_precision_scale(inner) {
                (Some(precision), Some(scale))
            } else {
                Self::clickhouse_numeric_precision_scale(field.data_type())?
            };

        Ok(Column::new(
            ClickHouse,
            field.name().to_string(),
            type_text,
            char_size,
            numeric_precision,
            numeric_scale,
        ))
    }

    fn build_fabric(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        use AdapterType::Fabric;
        let data_type = field.data_type();
        let char_size = sql_types::var_size(Fabric, data_type);
        let (numeric_precision, numeric_scale) = {
            let precision_scale = sql_types::numeric_precision_scale(Fabric, data_type)
                .ok()
                .flatten();
            match precision_scale {
                Some((p, Some(s))) => (Some(p), Some(s)),
                Some((p, None)) => (Some(p), None),
                None => (None, None),
            }
        };

        let mut type_name_or_formatted = String::new();
        if type_ops
            .format_arrow_type_as_sql(data_type, field.is_nullable(), &mut type_name_or_formatted)
            .is_err()
        {
            type_name_or_formatted = data_type.to_string();
        }

        Column::new(
            Fabric,
            field.name().to_string(),
            type_name_or_formatted,
            char_size.map(|p| p as u32),
            numeric_precision.map(|p| p as u64),
            numeric_scale.map(|s| s as u64),
        )
    }

    fn build_snowflake(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        use AdapterType::Snowflake;
        use sql_types::snowflake::*;

        // XXX: the code here is messy because it's the result of porting logic bug by bug
        // from a previous implementation. It can be greatly simplified and it will be.
        let data_type = field.data_type();
        let mut char_size = sql_types::var_size(Snowflake, data_type);

        // XXX: errors are ignored
        let (mut numeric_precision, mut numeric_scale) = {
            let precision_scale = sql_types::numeric_precision_scale(Snowflake, data_type)
                .ok()
                .flatten();
            match precision_scale {
                Some((p, Some(s))) => (Some(p), Some(s)),
                Some((p, None)) => (Some(p), None),
                None => (None, None),
            }
        };

        let mut type_name_or_formatted = String::new();
        if type_ops
            .format_arrow_type_as_sql(data_type, field.is_nullable(), &mut type_name_or_formatted)
            .is_err()
        {
            // TODO this is for sure wrong type. We should rather propagate error here
            type_name_or_formatted = data_type.to_string();
            char_size = None;
            numeric_precision = None;
            numeric_scale = None;
        }
        let mut dtype = type_name_or_formatted.clone();

        static PRECISION_REGEX: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"\(.*?\)$").unwrap());
        match data_type {
            DataType::Int8
            | DataType::Int16
            | DataType::Int32
            | DataType::Int64
            | DataType::UInt8
            | DataType::UInt16
            | DataType::UInt32
            | DataType::UInt64
            | DataType::Decimal128(_, _) => {
                dtype.clear();
                dtype.push_str("NUMBER");
            }
            // Snowflake: For timestamp/date/time types, extract precision if available
            dt if is_time(dt).is_yes() => {
                dtype.clear();
                dtype.push_str("TIME");
            }
            dt if is_timestamp_ntz(dt).is_yes()
                || is_timestamp_ltz(dt).is_yes()
                || is_timestamp_tz(dt).is_yes()
                || matches!(dt, DataType::Timestamp(_, _)) =>
            {
                dtype.clear();
                dtype.push_str(
                    PRECISION_REGEX
                        .replace(&type_name_or_formatted, "")
                        .as_ref(),
                )
            }

            _ => {}
        }

        // HACK(jason): the frontend does not provide character size, parse it out of the type ourselves if available
        let mut resolved_char_size = char_size;
        match data_type {
            DataType::Utf8 | DataType::Utf8View | DataType::LargeUtf8 => {
                // Extract size from metadata if present
                if let Some(char_size) = field
                    .metadata()
                    .get(metadata::snowflake::ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY)
                {
                    resolved_char_size = char_size
                        .parse::<usize>()
                        .ok()
                        .or(sql_types::max_varchar_size(Snowflake));
                }
            }
            DataType::Binary | DataType::LargeBinary | DataType::BinaryView => {
                if let Some(char_size) = field
                    .metadata()
                    .get(metadata::snowflake::ARROW_FIELD_SNOWFLAKE_FIELD_WIDTH_METADATA_KEY)
                {
                    resolved_char_size = char_size
                        .parse::<usize>()
                        .ok()
                        .or(sql_types::max_varbinary_size(Snowflake));
                }
            }
            _ => {}
        }

        Column::new(
            Snowflake,
            field.name().to_string(),
            dtype,
            resolved_char_size.map(|p| p as u32),
            numeric_precision.map(|p| p as u64),
            numeric_scale.map(|s| s as u64),
        )
    }

    /// The logic from `get_column_schema_from_query` for BigQuery [1].
    ///
    /// [1] https://github.com/dbt-labs/dbt-adapters/blob/c16cc7047e8678f8bb88ae294f43da2c68e9f5cc/dbt-bigquery/src/dbt/adapters/bigquery/impl.py#L444
    fn build_bigquery(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        let original_type_str = type_ops
            .get_original_sql_type_from_field(field)
            // FIXME: whats a good fallback here? This should technically never fail unless the
            // warehouse produces a very weird arrow type.
            .unwrap_or_else(|_| Cow::Owned(field.data_type().to_string()));
        let sql_type = SqlType::parse(AdapterType::Bigquery, original_type_str.as_ref()).ok();

        // NOTE: In dbt Core, if a column is both REPEATED and NULLABLE,
        // REPEATED takes precedence.
        let non_repeated_mode = match field.is_nullable() {
            true => BigqueryColumnMode::Nullable,
            false => BigqueryColumnMode::Required,
        };

        let mode = match sql_type {
            Some((sql_type, _nullable)) => match sql_type {
                SqlType::Array(_) => BigqueryColumnMode::Repeated,
                _ => non_repeated_mode,
            },
            None => {
                // FIXME(serramatutu): desperate fallback to arrow in case SqlType fails to parse whatever comes
                // from the warehouse
                match field.data_type() {
                    DataType::List(..)
                    | DataType::ListView(..)
                    | DataType::FixedSizeList(..)
                    | DataType::LargeList(..)
                    | DataType::LargeListView(..) => BigqueryColumnMode::Repeated,
                    _ => non_repeated_mode,
                }
            }
        };

        let inner_columns = match field.data_type() {
            DataType::List(inner)
            | DataType::ListView(inner)
            | DataType::FixedSizeList(inner, _)
            | DataType::LargeList(inner)
            | DataType::LargeListView(inner) => {
                match inner.data_type() {
                    // only structs can have named fields
                    DataType::Struct(inner) => inner
                        .into_iter()
                        .map(|f| Self::build_bigquery(f, type_ops))
                        .collect(),
                    // we don't need to worry about nested lists because that is not supported by
                    // BigQuery
                    _ => Vec::new(),
                }
            }
            DataType::Struct(fields) => fields
                .into_iter()
                .map(|f| Self::build_bigquery(f, type_ops))
                .collect(),
            _ => Vec::new(),
        };

        Column::new_bigquery(
            field.name().to_string(),
            original_type_str.to_string(),
            inner_columns,
            mode,
        )
    }

    fn build_databricks(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        let name = field.name().to_string();
        let type_text = {
            let type_text = original_type_string(AdapterType::Databricks, field);
            if let Some(type_text) = type_text {
                type_text
            } else {
                let mut type_text = String::new();
                type_ops
                    .format_arrow_type_as_sql(
                        field.data_type(),
                        field.is_nullable(),
                        &mut type_text,
                    )
                    .unwrap();
                if !field.is_nullable() {
                    type_text.push_str(" not null");
                }
                Cow::Owned(type_text)
            }
        };
        Column::new(
            AdapterType::Databricks,
            name,
            type_text.to_string(),
            None, // char_size
            None, // numeric_precision
            None, // numeric_scale
        )
    }

    fn build_postgres_like(
        adapter_type: AdapterType,
        field: &FieldRef,
        type_ops: &dyn TypeOps,
    ) -> Column {
        let data_type_ref = field.data_type();
        let mut rendered_type = String::new();
        match data_type_ref {
            // Mimic broken conversion that was here before just in case
            // something depends on it.
            // TODO: remove this broken formatting behavior
            DataType::Timestamp(_, _) | DataType::Time64(_) => rendered_type.push_str("datetime"),
            _ => {
                type_ops
                    .format_arrow_type_as_sql(
                        data_type_ref,
                        field.is_nullable(),
                        &mut rendered_type,
                    )
                    .unwrap();
            }
        }
        if !field.is_nullable() {
            rendered_type.push_str(" not null");
        }

        let (numeric_precision, numeric_scale) = {
            // Precision/scale follow the Postgres rules for every adapter built
            // here (integers get scale 0); only the column's adapter tag varies.
            let precision_scale =
                sql_types::numeric_precision_scale(AdapterType::Postgres, data_type_ref)
                    .ok()
                    .flatten();
            match precision_scale {
                Some((p, Some(s))) => (Some(p as u64), Some(s as u64)),
                Some((p, None)) => (Some(p as u64), None),
                None => (None, None),
            }
        };
        Column::new(
            adapter_type,
            field.name().to_string(),
            rendered_type,
            None, // char_size
            numeric_precision,
            // If it is an integer, the scale is 0, otherwise it is the scale of the number.
            numeric_scale,
        )
    }

    fn build_exasol(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        use AdapterType::Exasol;
        let data_type = field.data_type();
        let char_size = sql_types::var_size(Exasol, data_type);
        let (numeric_precision, numeric_scale) = {
            let precision_scale = sql_types::numeric_precision_scale(Exasol, data_type)
                .ok()
                .flatten();
            match precision_scale {
                Some((p, Some(s))) => (Some(p), Some(s)),
                Some((p, None)) => (Some(p), None),
                None => (None, None),
            }
        };

        let mut rendered_type = String::new();
        if type_ops
            .format_arrow_type_as_sql(data_type, field.is_nullable(), &mut rendered_type)
            .is_err()
        {
            rendered_type = data_type.to_string();
        }

        Column::new(
            Exasol,
            field.name().to_string(),
            rendered_type,
            char_size.map(|p| p as u32),
            numeric_precision.map(|p| p as u64),
            numeric_scale.map(|s| s as u64),
        )
    }

    fn build_redshift(field: &FieldRef, type_ops: &dyn TypeOps) -> Column {
        use AdapterType::Redshift;
        let data_type = field.data_type();
        let char_size = sql_types::var_size(Redshift, data_type);
        // XXX: errors are ignored
        let (numeric_precision, numeric_scale) = {
            let precision_scale = sql_types::numeric_precision_scale(Redshift, data_type)
                .ok()
                .flatten();
            match precision_scale {
                Some((p, Some(s))) => (Some(p), Some(s)),
                Some((p, None)) => (Some(p), None),
                None => (None, None),
            }
        };

        let mut type_name_or_formatted = String::new();
        if type_ops
            .format_arrow_type_as_sql(data_type, field.is_nullable(), &mut type_name_or_formatted)
            .is_err()
        {
            // TODO: this is for sure wrong type. We should rather propagate error here
            type_name_or_formatted = data_type.to_string();
        }

        let base_type_name = if matches!(
            data_type,
            DataType::Decimal128(_, _) | DataType::Decimal256(_, _)
        ) {
            "NUMERIC".to_string()
        } else {
            type_name_or_formatted
        };

        Column::new(
            Redshift,
            field.name().to_string(),
            base_type_name, // dtype
            char_size.map(|p| p as u32),
            numeric_precision.map(|p| p as u64),
            numeric_scale.map(|s| s as u64),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sql_types::DefaultTypeOps;
    use arrow_schema::{DataType, Field};
    use dbt_adapter_sql::types::metadata_sql_type_key;
    use std::collections::HashMap;
    use std::sync::Arc;

    #[test]
    fn test_build_clickhouse_decimal_with_wrappers() {
        let type_text = "Nullable(Decimal(18, 4))";
        let mut metadata = HashMap::new();
        metadata.insert(
            metadata_sql_type_key(AdapterType::ClickHouse).to_string(),
            type_text.to_string(),
        );
        let field = Arc::new(
            Field::new("amount", DataType::Decimal128(18, 4), true).with_metadata(metadata),
        );

        let builder = ColumnBuilder::new(AdapterType::ClickHouse);
        let type_ops = DefaultTypeOps::new(AdapterType::ClickHouse);
        let column = builder.build(&field, &type_ops).unwrap();

        assert_eq!(column.name(), "amount");
        // dtype keeps the wrappers so downstream rendering preserves nullability.
        assert_eq!(column.dtype(), type_text);
        assert_eq!(column.numeric_precision(), Some(18));
        assert_eq!(column.numeric_scale(), Some(4));
        assert_eq!(column.char_size(), None);
    }

    #[test]
    fn test_strip_clickhouse_wrappers_unwrapped() {
        assert_eq!(ColumnBuilder::strip_clickhouse_wrappers("String"), "String");
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("FixedString(10)"),
            "FixedString(10)"
        );
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("Decimal(18, 4)"),
            "Decimal(18, 4)"
        );
    }

    #[test]
    fn test_strip_clickhouse_wrappers_nullable() {
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("Nullable(String)"),
            "String"
        );
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("Nullable(FixedString(10))"),
            "FixedString(10)"
        );
    }

    #[test]
    fn test_strip_clickhouse_wrappers_low_cardinality_nullable() {
        // Real-world stack: LowCardinality wraps Nullable wraps String.
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("LowCardinality(Nullable(String))"),
            "String"
        );
        // Either ordering should fully unwrap.
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("Nullable(LowCardinality(String))"),
            "String"
        );
    }

    #[test]
    fn test_strip_clickhouse_wrappers_case_insensitive() {
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("nullable(String)"),
            "String"
        );
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("LOWCARDINALITY(Int32)"),
            "Int32"
        );
    }

    #[test]
    fn test_strip_clickhouse_wrappers_leaves_other_wrappers_alone() {
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("Array(Int32)"),
            "Array(Int32)"
        );
        assert_eq!(
            ColumnBuilder::strip_clickhouse_wrappers("DateTime64(6)"),
            "DateTime64(6)"
        );
    }

    #[test]
    fn test_parse_fixed_string_size() {
        assert_eq!(
            ColumnBuilder::parse_fixed_string_size("FixedString(10)"),
            Some(10)
        );
        assert_eq!(
            ColumnBuilder::parse_fixed_string_size("fixedstring(255)"),
            Some(255)
        );
        assert_eq!(ColumnBuilder::parse_fixed_string_size("String"), None);
        assert_eq!(
            ColumnBuilder::parse_fixed_string_size("FixedString()"),
            None
        );
        assert_eq!(
            ColumnBuilder::parse_fixed_string_size("FixedString(abc)"),
            None
        );
    }

    #[test]
    fn test_parse_decimal_precision_scale() {
        assert_eq!(
            ColumnBuilder::parse_decimal_precision_scale("Decimal(18, 4)"),
            Some((18, 4))
        );
        assert_eq!(
            ColumnBuilder::parse_decimal_precision_scale("Decimal(38,9)"),
            Some((38, 9))
        );
        // ClickHouse-specific shorthands carry only scale — reject them here.
        assert_eq!(
            ColumnBuilder::parse_decimal_precision_scale("Decimal128(4)"),
            None
        );
        assert_eq!(ColumnBuilder::parse_decimal_precision_scale("Int32"), None);
    }
}
