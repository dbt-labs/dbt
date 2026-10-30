use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

/// Results of `DESCRIBE TABLE EXTENDED {database}.{schema}.{identifier} AS JSON;`
/// https://docs.databricks.com/aws/en/sql/language-manual/sql-ref-syntax-aux-describe-table
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksDescribeTableExtended {
    pub table_name: String,
    pub catalog_name: String,
    pub schema_name: String,
    pub namespace: Vec<String>,
    #[serde(rename = "type")]
    pub type_: String,
    pub provider: Option<String>,
    pub columns: Vec<DatabricksColumnInfo>,
    pub partition_values: BTreeMap<String, String>,
    pub partition_columns: Vec<String>,
    pub location: String,
    pub view_text: Option<String>,
    pub view_original_text: Option<String>,
    pub view_schema_mode: Option<String>,
    pub view_catalog_and_namespace: Option<String>,
    pub view_query_output_columns: Option<Vec<String>>,
    pub comment: String,
    pub table_properties: BTreeMap<String, String>,
    pub statistics: Option<DatabricksTableStatistics>,
    pub storage_properties: BTreeMap<String, String>,
    pub serde_library: String,
    pub input_format: String,
    pub output_format: String,
    pub num_buckets: Option<i32>,
    pub bucket_columns: Vec<String>,
    pub sort_columns: Vec<String>,
    pub created_time: String,
    pub created_by: String,
    pub last_access: String,
    pub partition_provider: Option<String>,
    pub collation: Option<String>,
    pub language: Option<String>,
    pub row_filter: Option<DatabricksRowFilter>,
    pub column_masks: Vec<DatabricksColumnMask>,
    pub owner: String, // this field isn't documented but is returned
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DatabricksColumnInfo {
    pub name: String,
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_databricks_column_type"
    )]
    pub type_: DatabricksColumnTypeInfo,
    pub comment: Option<String>,
    pub nullable: bool,
    pub default: Option<String>,
    pub is_measure: Option<bool>, // for measure columns
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksTableStatistics {
    pub num_rows: Option<i64>,
    pub size_in_bytes: Option<i64>,
    pub table_change_stats: Option<DatabricksTableChangeStats>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksTableChangeStats {
    pub inserted: Option<i64>,
    pub deleted: Option<i64>,
    pub updated: Option<i64>,
    pub change_percent: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksRowFilter {
    pub filter_function: DatabricksFunctionReference,
    pub arguments: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksColumnMask {
    pub column_name: String,
    pub mask_function: DatabricksFunctionReference,
    pub arguments: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
#[serde(default)]
pub struct DatabricksFunctionReference {
    pub catalog_name: String,
    pub schema_name: String,
    pub function_name: String,
    pub specific_name: String,
}

fn default_element_nullable() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "name")]
#[serde(rename_all = "lowercase")]
pub enum DatabricksColumnTypeInfo {
    TinyInt,
    SmallInt,
    Int,
    BigInt,
    Float,
    Double,
    Decimal {
        precision: u8,
        scale: u8,
    },
    String {
        collation: Option<String>,
    },
    VarChar {
        length: u32,
    },
    Char {
        length: u32,
    },
    Binary,
    Boolean,
    Date,
    #[serde(rename = "timestamp_ltz")]
    Timestamp,
    #[serde(rename = "timestamp_ntz")]
    TimestampNtz,
    Interval {
        start_unit: String,
        end_unit: Option<String>,
    },
    Array {
        #[serde(deserialize_with = "deserialize_boxed_databricks_column_type")]
        element_type: Box<DatabricksColumnTypeInfo>,
        #[serde(default = "default_element_nullable")]
        element_nullable: bool,
    },
    Map {
        #[serde(deserialize_with = "deserialize_boxed_databricks_column_type")]
        key_type: Box<DatabricksColumnTypeInfo>,
        #[serde(deserialize_with = "deserialize_boxed_databricks_column_type")]
        value_type: Box<DatabricksColumnTypeInfo>,
        #[serde(default = "default_element_nullable")]
        element_nullable: bool,
    },
    Struct {
        fields: Vec<DatabricksStructFieldInfo>,
    },
    Variant,
    Void,
    Geography {
        srid: u32,
    },
    Geometry {
        srid: u32,
    },
}

/// Deserialize spatial datatypes from Databricks 'DESCRIBE TABLE <table> AS JSON output.
///
/// Currently (2026-10-18) the documented <type_json> schema does not include
/// GEOGRAPHY or GEOMETRY
/// https://docs.databricks.com/aws/en/sql/language-manual/sql-ref-syntax-aux-describe-table
///
/// Inspecting the schema defition for the spatial types directly in Databricks,
/// the JSON representation looks like:
/// { "name" : "geography(<srid>)"} and { "name" : "geometry(<srid>)"}
///
/// This shows that Databricks is currently embedding the srid directly in the datatype name,
/// unlike e.g. CHAR(n), whose JSON representation is { "name" : "char", "length": n }.
///
/// If Databricks changes the JSON representation for spatial types to:
/// { "name" : "geography", "srid" : n} and { "name" : "geometry", "srid" : n}
/// this special handling of spatial data types can be removed.
fn deserialize_databricks_column_type<'de, D>(
    deserializer: D,
) -> Result<DatabricksColumnTypeInfo, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Read incoming JSON object of the column type
    let value = serde_json::Value::deserialize(deserializer)?;

    // Get the name of the column type
    let type_name = value["name"].as_str().unwrap_or("").to_ascii_lowercase();

    // Deserialize column type only if it is geography or geometry
    if type_name.starts_with("geography(") || type_name.starts_with("geometry(") {
        let (name, parameters) = type_name.split_once('(').ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(format!(
                "Invalid spatial type name: {type_name}"
            ))
        })?;

        let srid_text = parameters.strip_suffix(')').ok_or_else(|| {
            <D::Error as serde::de::Error>::custom(format!(
                "Invalid spatial type name: {type_name}"
            ))
        })?;

        let srid = srid_text.trim().parse::<u32>().map_err(|err| {
            <D::Error as serde::de::Error>::custom(format!("Invalid SRID in '{type_name}': {err}"))
        })?;

        if name.trim() == "geography" {
            return Ok(DatabricksColumnTypeInfo::Geography { srid });
        }

        if name.trim() == "geometry" {
            return Ok(DatabricksColumnTypeInfo::Geometry { srid });
        }
    }

    serde_json::from_value(value).map_err(<D::Error as serde::de::Error>::custom)
}

/// Same as [deserialize_databricks_column_type], for the boxed types nested in arrays and maps.
fn deserialize_boxed_databricks_column_type<'de, D>(
    deserializer: D,
) -> Result<Box<DatabricksColumnTypeInfo>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    deserialize_databricks_column_type(deserializer).map(Box::new)
}

impl DatabricksColumnTypeInfo {
    /// Converts [DatabricksColumnTypeInfo] into Raw Databricks SQL Type
    /// https://docs.databricks.com/aws/en/sql/language-manual/sql-ref-datatypes
    pub fn sql_type(&self) -> String {
        let mut out = String::new();
        self.render_sql_type(&mut out).unwrap();
        out
    }

    pub fn render_sql_type(&self, out: &mut String) -> Result<(), fmt::Error> {
        use fmt::Write as _;
        match &self {
            DatabricksColumnTypeInfo::TinyInt => out.push_str("tinyint"),
            DatabricksColumnTypeInfo::SmallInt => out.push_str("smallint"),
            DatabricksColumnTypeInfo::Int => out.push_str("int"),
            DatabricksColumnTypeInfo::BigInt => out.push_str("bigint"),
            DatabricksColumnTypeInfo::Float => out.push_str("float"),
            DatabricksColumnTypeInfo::Double => out.push_str("double"),
            DatabricksColumnTypeInfo::Binary => out.push_str("binary"),
            DatabricksColumnTypeInfo::Boolean => out.push_str("boolean"),
            DatabricksColumnTypeInfo::Date => out.push_str("date"),
            DatabricksColumnTypeInfo::Timestamp => out.push_str("timestamp_ltz"),
            DatabricksColumnTypeInfo::TimestampNtz => out.push_str("timestamp_ntz"),
            DatabricksColumnTypeInfo::Variant => out.push_str("variant"),
            DatabricksColumnTypeInfo::Void => out.push_str("void"),
            DatabricksColumnTypeInfo::Decimal { precision, scale } => {
                write!(out, "DECIMAL({precision},{scale})")?;
            }
            DatabricksColumnTypeInfo::String { collation: _ } => {
                // todo: collation syntax is undocumented, figure out how to apply it
                out.push_str("STRING")
            }
            DatabricksColumnTypeInfo::VarChar { length } => {
                // not documented - under the hood it's a String with length constraint
                write!(out, "VARCHAR({length})")?;
            }
            DatabricksColumnTypeInfo::Char { length } => {
                write!(out, "CHAR({length})")?;
            }
            DatabricksColumnTypeInfo::Interval {
                start_unit,
                end_unit,
            } => match end_unit {
                Some(end) if start_unit != end => write!(out, "INTERVAL {start_unit} TO {end}")?,
                _ => write!(out, "INTERVAL {start_unit}")?,
            },
            DatabricksColumnTypeInfo::Array {
                element_type,
                element_nullable: _,
            } => {
                // todo: element_nullable isn't possible to set here, find workaround
                write!(out, "ARRAY<")?;
                element_type.render_sql_type(out)?;
                write!(out, ">")?;
            }
            DatabricksColumnTypeInfo::Map {
                key_type,
                value_type,
                element_nullable: _,
            } => {
                // todo: element_nullable isn't possible to set here
                write!(out, "MAP<")?;
                key_type.render_sql_type(out)?;
                write!(out, ",")?;
                value_type.render_sql_type(out)?;
                write!(out, ">")?;
            }
            DatabricksColumnTypeInfo::Struct { fields } => {
                write!(out, "STRUCT<")?;
                for (i, field) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(out, ",")?;
                    }
                    write!(out, "`{}`:", field.name)?;
                    field.type_.render_sql_type(out)?;
                    if !field.nullable {
                        write!(out, " NOT NULL")?;
                    }
                    if let Some(comment) = &field.comment {
                        // escape the comment string literal
                        write!(out, " COMMENT \"{}\"", comment.replace("\"", "\\\""))?;
                    }
                }
                write!(out, ">")?;
            }
            DatabricksColumnTypeInfo::Geography { srid } => {
                write!(out, "GEOGRAPHY({srid})")?;
            }
            DatabricksColumnTypeInfo::Geometry { srid } => {
                write!(out, "GEOMETRY({srid})")?;
            }
        };
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatabricksStructFieldInfo {
    pub name: String,
    #[serde(
        rename = "type",
        deserialize_with = "deserialize_databricks_column_type"
    )]
    pub type_: DatabricksColumnTypeInfo,
    #[serde(default = "default_element_nullable")]
    pub nullable: bool,
    pub comment: Option<String>,
    pub default: Option<String>,
}

// based on samples from the docs
#[cfg(test)]
mod tests {
    use super::*;
    use DatabricksColumnTypeInfo::*;

    #[test]
    fn test_basic_type_sql_type() {
        // Test basic primitive types
        assert_eq!(TinyInt.sql_type(), "tinyint");
        assert_eq!(SmallInt.sql_type(), "smallint");
        assert_eq!(Int.sql_type(), "int");
        assert_eq!(BigInt.sql_type(), "bigint");
        assert_eq!(Float.sql_type(), "float");
        assert_eq!(Double.sql_type(), "double");
        assert_eq!(Binary.sql_type(), "binary");
        assert_eq!(Boolean.sql_type(), "boolean");
        assert_eq!(Date.sql_type(), "date");
        assert_eq!(Timestamp.sql_type(), "timestamp_ltz");
        assert_eq!(TimestampNtz.sql_type(), "timestamp_ntz");
        assert_eq!(Variant.sql_type(), "variant");
    }

    #[test]
    fn test_decimal_sql_type() {
        let decimal = Decimal {
            precision: 10,
            scale: 2,
        };
        assert_eq!(decimal.sql_type(), "DECIMAL(10,2)");
    }

    #[test]
    fn test_string_types_sql_type() {
        let string = String {
            collation: Some("UTF8_BINARY".to_string()),
        };
        assert_eq!(string.sql_type(), "STRING");

        let varchar = VarChar { length: 255 };
        assert_eq!(varchar.sql_type(), "VARCHAR(255)");

        let char = Char { length: 10 };
        assert_eq!(char.sql_type(), "CHAR(10)");
    }

    #[test]
    fn test_interval_sql_type() {
        // Single unit intervals
        let year_interval = Interval {
            start_unit: "year".to_string(),
            end_unit: None,
        };
        assert_eq!(year_interval.sql_type(), "INTERVAL year");

        // Sometimes end and start are the same
        let year_interval = Interval {
            start_unit: "year".to_string(),
            end_unit: Some("year".to_string()),
        };
        assert_eq!(year_interval.sql_type(), "INTERVAL year");

        let month_interval = Interval {
            start_unit: "month".to_string(),
            end_unit: None,
        };
        assert_eq!(month_interval.sql_type(), "INTERVAL month");

        // Range intervals
        let year_to_month = Interval {
            start_unit: "year".to_string(),
            end_unit: Some("month".to_string()),
        };
        assert_eq!(year_to_month.sql_type(), "INTERVAL year TO month");

        let day_to_hour = Interval {
            start_unit: "day".to_string(),
            end_unit: Some("hour".to_string()),
        };
        assert_eq!(day_to_hour.sql_type(), "INTERVAL day TO hour");
    }

    #[test]
    fn test_simple_array_sql_type() {
        // Array of primitive types
        let int_array = Array {
            element_type: Box::new(Int),
            element_nullable: true,
        };
        assert_eq!(int_array.sql_type(), "ARRAY<int>");

        let string_array = Array {
            element_type: Box::new(String { collation: None }),
            element_nullable: false,
        };
        assert_eq!(string_array.sql_type(), "ARRAY<STRING>");

        let decimal_array = Array {
            element_type: Box::new(Decimal {
                precision: 15,
                scale: 3,
            }),
            element_nullable: true,
        };
        assert_eq!(decimal_array.sql_type(), "ARRAY<DECIMAL(15,3)>");
    }

    #[test]
    fn test_nested_array_sql_type() {
        let nested_array = Array {
            element_type: Box::new(Array {
                element_type: Box::new(Int),
                element_nullable: true,
            }),
            element_nullable: false,
        };
        assert_eq!(nested_array.sql_type(), "ARRAY<ARRAY<int>>");

        let triple_nested = Array {
            element_type: Box::new(Array {
                element_type: Box::new(Array {
                    element_type: Box::new(String { collation: None }),
                    element_nullable: true,
                }),
                element_nullable: false,
            }),
            element_nullable: true,
        };
        assert_eq!(triple_nested.sql_type(), "ARRAY<ARRAY<ARRAY<STRING>>>");
    }

    #[test]
    fn test_array_with_complex_elements_sql_type() {
        // Array of structs
        let struct_field = DatabricksStructFieldInfo {
            name: "id".to_string(),
            type_: Int,
            nullable: false,
            comment: None,
            default: None,
        };

        let struct_type = Struct {
            fields: vec![struct_field],
        };

        let struct_array = Array {
            element_type: Box::new(struct_type),
            element_nullable: true,
        };
        assert_eq!(struct_array.sql_type(), "ARRAY<STRUCT<`id`:int NOT NULL>>");

        // Array of maps
        let map_type = Map {
            key_type: Box::new(String { collation: None }),
            value_type: Box::new(Int),
            element_nullable: true,
        };

        let map_array = Array {
            element_type: Box::new(map_type),
            element_nullable: false,
        };
        assert_eq!(map_array.sql_type(), "ARRAY<MAP<STRING,int>>");
    }

    #[test]
    fn test_simple_map_sql_type() {
        // Map with primitive types
        let string_int_map = Map {
            key_type: Box::new(String { collation: None }),
            value_type: Box::new(Int),
            element_nullable: true,
        };
        assert_eq!(string_int_map.sql_type(), "MAP<STRING,int>");

        let int_double_map = Map {
            key_type: Box::new(Int),
            value_type: Box::new(Double),
            element_nullable: false,
        };
        assert_eq!(int_double_map.sql_type(), "MAP<int,double>");
    }

    #[test]
    fn test_complex_map_sql_type() {
        // Map with complex key/value types
        let map_with_decimal_key = Map {
            key_type: Box::new(Decimal {
                precision: 10,
                scale: 2,
            }),
            value_type: Box::new(String { collation: None }),
            element_nullable: true,
        };
        assert_eq!(map_with_decimal_key.sql_type(), "MAP<DECIMAL(10,2),STRING>");

        let map_with_array_values = Map {
            key_type: Box::new(Int),
            value_type: Box::new(Array {
                element_type: Box::new(String { collation: None }),
                element_nullable: true,
            }),
            element_nullable: false,
        };
        assert_eq!(map_with_array_values.sql_type(), "MAP<int,ARRAY<STRING>>");
    }

    #[test]
    fn test_nested_map_sql_type() {
        let nested_map = Map {
            key_type: Box::new(String { collation: None }),
            value_type: Box::new(Map {
                key_type: Box::new(Int),
                value_type: Box::new(Double),
                element_nullable: true,
            }),
            element_nullable: false,
        };
        assert_eq!(nested_map.sql_type(), "MAP<STRING,MAP<int,double>>");

        // Map with array of maps as values
        let map_with_array_of_maps = Map {
            key_type: Box::new(Int),
            value_type: Box::new(Array {
                element_type: Box::new(Map {
                    key_type: Box::new(String { collation: None }),
                    value_type: Box::new(Boolean),
                    element_nullable: true,
                }),
                element_nullable: false,
            }),
            element_nullable: true,
        };
        assert_eq!(
            map_with_array_of_maps.sql_type(),
            "MAP<int,ARRAY<MAP<STRING,boolean>>>"
        );
    }

    #[test]
    fn test_simple_struct_sql_type() {
        // Struct with basic fields
        let simple_struct = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "id".to_string(),
                    type_: Int,
                    nullable: false,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "name".to_string(),
                    type_: String { collation: None },
                    nullable: true,
                    comment: None,
                    default: None,
                },
            ],
        };
        assert_eq!(
            simple_struct.sql_type(),
            "STRUCT<`id`:int NOT NULL,`name`:STRING>"
        );
    }

    #[test]
    fn test_struct_with_comments_sql_type() {
        // Struct with field comments
        let struct_with_comments = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "user_id".to_string(),
                    type_: BigInt,
                    nullable: false,
                    comment: Some("Unique user identifier".to_string()),
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "email".to_string(),
                    type_: String { collation: None },
                    nullable: true,
                    comment: Some("User email address".to_string()),
                    default: None,
                },
            ],
        };
        assert_eq!(
            struct_with_comments.sql_type(),
            "STRUCT<`user_id`:bigint NOT NULL COMMENT \"Unique user identifier\",`email`:STRING COMMENT \"User email address\">"
        );
    }

    #[test]
    fn test_struct_with_escaped_comments_sql_type() {
        // Struct with comments containing quotes that need escaping
        let struct_with_escaped_comments = Struct {
            fields: vec![DatabricksStructFieldInfo {
                name: "description".to_string(),
                type_: String { collation: None },
                nullable: true,
                comment: Some("Contains \"quoted\" text".to_string()),
                default: None,
            }],
        };
        assert_eq!(
            struct_with_escaped_comments.sql_type(),
            "STRUCT<`description`:STRING COMMENT \"Contains \\\"quoted\\\" text\">"
        );
    }

    #[test]
    fn test_struct_with_complex_fields_sql_type() {
        // Struct with array and map fields
        let complex_struct = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "tags".to_string(),
                    type_: Array {
                        element_type: Box::new(String { collation: None }),
                        element_nullable: true,
                    },
                    nullable: true,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "metadata".to_string(),
                    type_: Map {
                        key_type: Box::new(String { collation: None }),
                        value_type: Box::new(String { collation: None }),
                        element_nullable: true,
                    },
                    nullable: false,
                    comment: None,
                    default: None,
                },
            ],
        };
        assert_eq!(
            complex_struct.sql_type(),
            "STRUCT<`tags`:ARRAY<STRING>,`metadata`:MAP<STRING,STRING> NOT NULL>"
        );
    }

    #[test]
    fn test_nested_struct_sql_type() {
        // Struct containing nested structs
        let address_struct = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "street".to_string(),
                    type_: String { collation: None },
                    nullable: true,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "city".to_string(),
                    type_: String { collation: None },
                    nullable: true,
                    comment: None,
                    default: None,
                },
            ],
        };

        let person_struct = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "name".to_string(),
                    type_: String { collation: None },
                    nullable: false,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "address".to_string(),
                    type_: address_struct,
                    nullable: true,
                    comment: None,
                    default: None,
                },
            ],
        };
        assert_eq!(
            person_struct.sql_type(),
            "STRUCT<`name`:STRING NOT NULL,`address`:STRUCT<`street`:STRING,`city`:STRING>>"
        );
    }

    #[test]
    fn test_deeply_nested_complex_types_sql_type() {
        // Test cominations of arrays, maps, and structs
        let inner_struct = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "key".to_string(),
                    type_: String { collation: None },
                    nullable: false,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "value".to_string(),
                    type_: Int,
                    nullable: true,
                    comment: None,
                    default: None,
                },
            ],
        };

        let struct_array = Array {
            element_type: Box::new(inner_struct),
            element_nullable: true,
        };

        let map_of_struct_arrays = Map {
            key_type: Box::new(String { collation: None }),
            value_type: Box::new(struct_array),
            element_nullable: false,
        };

        let array_of_maps = Array {
            element_type: Box::new(map_of_struct_arrays),
            element_nullable: true,
        };

        let final_struct = Struct {
            fields: vec![DatabricksStructFieldInfo {
                name: "complex_data".to_string(),
                type_: array_of_maps,
                nullable: true,
                comment: Some("Complex nested data structure".to_string()),
                default: None,
            }],
        };

        assert_eq!(
            final_struct.sql_type(),
            "STRUCT<`complex_data`:ARRAY<MAP<STRING,ARRAY<STRUCT<`key`:STRING NOT NULL,`value`:int>>>> COMMENT \"Complex nested data structure\">"
        );
    }

    #[test]
    fn test_interval_in_complex_types_sql_type() {
        // Test intervals within complex types
        let interval_array = Array {
            element_type: Box::new(Interval {
                start_unit: "day".to_string(),
                end_unit: Some("hour".to_string()),
            }),
            element_nullable: true,
        };
        assert_eq!(interval_array.sql_type(), "ARRAY<INTERVAL day TO hour>");

        let interval_map = Map {
            key_type: Box::new(String { collation: None }),
            value_type: Box::new(Interval {
                start_unit: "year".to_string(),
                end_unit: None,
            }),
            element_nullable: false,
        };
        assert_eq!(interval_map.sql_type(), "MAP<STRING,INTERVAL year>");

        let struct_with_interval = Struct {
            fields: vec![DatabricksStructFieldInfo {
                name: "duration".to_string(),
                type_: Interval {
                    start_unit: "month".to_string(),
                    end_unit: None,
                },
                nullable: true,
                comment: None,
                default: None,
            }],
        };
        assert_eq!(
            struct_with_interval.sql_type(),
            "STRUCT<`duration`:INTERVAL month>"
        );
    }

    #[test]
    fn test_decimal_in_complex_types_sql_type() {
        let decimal_array = Array {
            element_type: Box::new(Decimal {
                precision: 38,
                scale: 18,
            }),
            element_nullable: true,
        };
        assert_eq!(decimal_array.sql_type(), "ARRAY<DECIMAL(38,18)>");

        let map_with_decimal = Map {
            key_type: Box::new(Decimal {
                precision: 10,
                scale: 2,
            }),
            value_type: Box::new(Decimal {
                precision: 20,
                scale: 10,
            }),
            element_nullable: true,
        };
        assert_eq!(
            map_with_decimal.sql_type(),
            "MAP<DECIMAL(10,2),DECIMAL(20,10)>"
        );
    }

    #[test]
    fn test_timestamp_types_in_complex_types_sql_type() {
        let timestamp_array = Array {
            element_type: Box::new(Timestamp),
            element_nullable: true,
        };
        assert_eq!(timestamp_array.sql_type(), "ARRAY<timestamp_ltz>");

        let timestamp_ntz_array = Array {
            element_type: Box::new(TimestampNtz),
            element_nullable: false,
        };
        assert_eq!(timestamp_ntz_array.sql_type(), "ARRAY<timestamp_ntz>");

        let struct_with_timestamps = Struct {
            fields: vec![
                DatabricksStructFieldInfo {
                    name: "created_at".to_string(),
                    type_: Timestamp,
                    nullable: false,
                    comment: None,
                    default: None,
                },
                DatabricksStructFieldInfo {
                    name: "updated_at".to_string(),
                    type_: TimestampNtz,
                    nullable: true,
                    comment: None,
                    default: None,
                },
            ],
        };
        assert_eq!(
            struct_with_timestamps.sql_type(),
            "STRUCT<`created_at`:timestamp_ltz NOT NULL,`updated_at`:timestamp_ntz>"
        );
    }

    #[test]
    fn test_deserialize_geography_type_with_srid() {
        let json = r#"{
            "columns": [
                {
                    "name": "coordinate",
                    "type": {
                        "name": "geography(4326)"
                    },
                    "nullable": true,
                    "comment": "This is a test"
                }
            ]
        }"#;

        let table: DatabricksDescribeTableExtended = serde_json::from_str(json).unwrap();

        assert_eq!(table.columns[0].type_.sql_type(), "GEOGRAPHY(4326)")
    }

    #[test]
    fn test_geography_sql_type() {
        let geography = Geography { srid: 4326 };
        assert_eq!(geography.sql_type(), "GEOGRAPHY(4326)");
    }

    #[test]
    fn test_deserialize_geometry_type_with_srid() {
        let json = r#"{
            "columns": [
                {
                    "name": "coordinate",
                    "type": {
                        "name": "geometry(4326)"
                    },
                    "nullable": true,
                    "comment": "This is a test"
                }
            ]
        }"#;

        let table: DatabricksDescribeTableExtended = serde_json::from_str(json).unwrap();

        assert_eq!(table.columns[0].type_.sql_type(), "GEOMETRY(4326)")
    }

    #[test]
    fn test_geometry_sql_type() {
        let geometry = Geometry { srid: 4326 };
        assert_eq!(geometry.sql_type(), "GEOMETRY(4326)");
    }

    #[test]
    fn test_deserialize_nested_spatial_types() {
        let json = r#"{
            "columns": [
                {"name": "a", "nullable": true, "type": {"name": "array", "element_type": {"name": "geometry(4326)"}, "element_nullable": true}},
                {"name": "s", "nullable": true, "type": {"name": "struct", "fields": [{"name": "p", "type": {"name": "geography(4326)"}, "nullable": true}]}},
                {"name": "m", "nullable": true, "type": {"name": "map", "key_type": {"name": "string", "collation": "UTF8_BINARY"}, "value_type": {"name": "geometry(25832)"}, "value_nullable": true}}
            ]
        }"#;

        let table: DatabricksDescribeTableExtended = serde_json::from_str(json).unwrap();

        assert_eq!(table.columns[0].type_.sql_type(), "ARRAY<GEOMETRY(4326)>");
        assert_eq!(
            table.columns[1].type_.sql_type(),
            "STRUCT<`p`:GEOGRAPHY(4326)>"
        );
        assert_eq!(
            table.columns[2].type_.sql_type(),
            "MAP<STRING,GEOMETRY(25832)>"
        );
    }
}
