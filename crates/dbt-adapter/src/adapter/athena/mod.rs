//! Athena-only `adapter.*` methods.
//!
//! dbt-athena implements these on the Python `AthenaAdapter`
//! (`dbt-athena/src/dbt/adapters/athena/impl.py`) and the vendored macro package
//! calls them from every materialization. This module holds the pure helpers;
//! [`aws`] holds the Glue / S3 / Athena / STS calls; [`dispatch`] wires both
//! into `adapter.call_method_impl`.

pub mod aws;
mod dispatch;

use minijinja::Value;
use regex::Regex;
use std::sync::LazyLock;

/// Glue federated catalog prefix under which S3 Tables buckets appear
/// (`s3tablescatalog/<bucket>`).
pub const S3_TABLES_GLUE_CATALOG_PREFIX: &str = "s3tablescatalog";

/// `AthenaAdapter.INTEGER_MAX_VALUE_32_BIT_SIGNED`
const INTEGER_MAX_VALUE_32_BIT_SIGNED: i64 = 0x7FFF_FFFF;

/// `dbt.adapters.athena.s3.S3DataNaming`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum S3DataNaming {
    Unique,
    Table,
    TableUnique,
    SchemaTable,
    SchemaTableUnique,
}

impl S3DataNaming {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "unique" => Self::Unique,
            "table" => Self::Table,
            "table_unique" => Self::TableUnique,
            "schema_table" => Self::SchemaTable,
            "schema_table_unique" => Self::SchemaTableUnique,
            _ => return None,
        })
    }
}

/// `dbt.adapters.athena.relation.TableType`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableType {
    Table,
    View,
    Cte,
    MaterializedView,
    Iceberg,
}

impl TableType {
    /// The Python enum's `.value`, which the macros compare against (`'iceberg_table'`).
    pub fn value(self) -> &'static str {
        match self {
            Self::Table => "table",
            Self::View => "view",
            Self::Cte => "cte",
            Self::MaterializedView => "materializedview",
            Self::Iceberg => "iceberg_table",
        }
    }

    pub fn is_physical(self) -> bool {
        matches!(self, Self::Table | Self::Iceberg)
    }

    /// `dbt.adapters.athena.relation.get_table_type`: the Iceberg `table_type`
    /// parameter wins over Glue's `TableType`, which S3 Tables report as "customer".
    pub fn from_glue(
        glue_table_type: Option<&str>,
        parameter_table_type: Option<&str>,
        table_full_name: &str,
    ) -> Result<Self, String> {
        if parameter_table_type.is_some_and(|t| t.eq_ignore_ascii_case("iceberg")) {
            return Ok(Self::Iceberg);
        }
        match glue_table_type {
            Some("EXTERNAL_TABLE" | "EXTERNAL" | "GOVERNED" | "MANAGED_TABLE" | "table") => {
                Ok(Self::Table)
            }
            Some("VIRTUAL_VIEW" | "view") => Ok(Self::View),
            Some("cte") => Ok(Self::Cte),
            Some("materializedview") => Ok(Self::MaterializedView),
            Some(other) => Err(format!(
                "Table type {other} is not supported for table {table_full_name}"
            )),
            None => Err(format!(
                "Table type cannot be None for table {table_full_name}"
            )),
        }
    }

    /// Jinja representation: a map with a `value` key, so `x.value` and `x == none`
    /// behave as they do on the Python enum.
    pub fn to_jinja(self) -> Value {
        Value::from_iter([("value", Value::from(self.value()))])
    }
}

pub fn is_s3_tables_database(database: Option<&str>) -> bool {
    database.is_some_and(|d| {
        d.to_ascii_lowercase().starts_with(&format!(
            "{}/",
            S3_TABLES_GLUE_CATALOG_PREFIX.to_ascii_lowercase()
        ))
    })
}

/// `os.path.join` for S3 URIs: one separator between the parts.
fn join_s3(base: &str, part: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), part)
}

/// `AthenaAdapter._s3_table_prefix`
pub fn s3_table_prefix(
    s3_staging_dir: &str,
    profile_s3_tmp_table_dir: Option<&str>,
    s3_data_dir: Option<&str>,
    s3_tmp_table_dir: Option<&str>,
    is_temporary_table: bool,
) -> String {
    let s3_tmp_table_dir = s3_tmp_table_dir
        .filter(|d| !d.is_empty())
        .or_else(|| profile_s3_tmp_table_dir.filter(|d| !d.is_empty()));
    if let Some(tmp) = s3_tmp_table_dir
        && is_temporary_table
    {
        return tmp.to_string();
    }
    if let Some(dir) = s3_data_dir {
        return dir.to_string();
    }
    join_s3(s3_staging_dir, "tables")
}

/// `AthenaAdapter.generate_s3_location`
#[allow(clippy::too_many_arguments)]
pub fn generate_s3_location(
    schema: &str,
    identifier: &str,
    table_prefix: &str,
    naming: S3DataNaming,
    external_location: Option<&str>,
    is_temporary_table: bool,
    unique: &str,
) -> String {
    if let Some(external) = external_location.filter(|e| !e.is_empty())
        && !is_temporary_table
    {
        return external.trim_end_matches('/').to_string();
    }
    match naming {
        S3DataNaming::Unique => join_s3(table_prefix, unique),
        S3DataNaming::Table => join_s3(table_prefix, identifier),
        S3DataNaming::TableUnique => join_s3(&join_s3(table_prefix, identifier), unique),
        S3DataNaming::SchemaTable => join_s3(&join_s3(table_prefix, schema), identifier),
        S3DataNaming::SchemaTableUnique => {
            join_s3(&join_s3(&join_s3(table_prefix, schema), identifier), unique)
        }
    }
}

/// `AthenaAdapter._parse_s3_path`: `s3://bucket/a/b` -> `("bucket", "a/b/")`.
pub fn parse_s3_path(s3_path: &str) -> (String, String) {
    let rest = s3_path
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(s3_path);
    let (bucket, path) = rest.split_once('/').unwrap_or((rest, ""));
    let prefix = format!("{}/", path.trim_start_matches('/').trim_end_matches('/'));
    (bucket.to_string(), prefix)
}

/// `AthenaAdapter.format_value_for_partition`: `(rendered value, comparison operator)`.
pub fn format_value_for_partition(
    value: &Value,
    column_type: &str,
) -> Result<(String, &'static str), String> {
    if value.is_none() || value.is_undefined() {
        return Ok(("null".to_string(), " is "));
    }
    match column_type {
        "integer" => Ok((value.to_string(), "=")),
        "string" => Ok((format!("'{}'", value.to_string().replace('\'', "''")), "=")),
        "date" => Ok((format!("DATE'{value}'"), "=")),
        "timestamp" => Ok((format!("TIMESTAMP'{value}'"), "=")),
        other => Err(format!("Unsupported column type: {other}")),
    }
}

static HIDDEN_PARTITION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(hour|day|month|year)\((.+)\)").expect("valid regex"));
static BUCKET_PARTITION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"bucket\((.+),").expect("valid regex"));

/// `AthenaAdapter.format_one_partition_key`: Iceberg hidden partitioning
/// (`day(ts)` -> `date_trunc('day', ts)`) and bucketing (`bucket(col, 8)` -> `col`).
pub fn format_one_partition_key(partition_key: &str) -> String {
    let lower = partition_key.to_ascii_lowercase();
    if let Some(caps) = HIDDEN_PARTITION_RE.captures(&lower) {
        return format!("date_trunc('{}', {})", &caps[1], &caps[2]);
    }
    if let Some(caps) = BUCKET_PARTITION_RE.captures(&lower) {
        return caps[1].to_string();
    }
    lower
}

/// `AthenaAdapter.format_partition_keys`
pub fn format_partition_keys<'a>(partition_keys: impl IntoIterator<Item = &'a str>) -> String {
    partition_keys
        .into_iter()
        .map(format_one_partition_key)
        .collect::<Vec<_>>()
        .join(", ")
}

/// MurmurHash3 x86 32-bit, as `mmh3.hash` computes it (seed 0, signed result).
pub fn murmur3_32(data: &[u8], seed: u32) -> i32 {
    const C1: u32 = 0xcc9e_2d51;
    const C2: u32 = 0x1b87_3593;
    let mut h1 = seed;
    let chunks = data.chunks_exact(4);
    let tail = chunks.remainder();
    for chunk in chunks {
        let mut k1 = u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        k1 = k1.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h1 ^= k1;
        h1 = h1.rotate_left(13).wrapping_mul(5).wrapping_add(0xe654_6b64);
    }
    if !tail.is_empty() {
        let mut k1: u32 = 0;
        for (i, byte) in tail.iter().enumerate() {
            k1 ^= (*byte as u32) << (8 * i);
        }
        k1 = k1.wrapping_mul(C1).rotate_left(15).wrapping_mul(C2);
        h1 ^= k1;
    }
    h1 ^= data.len() as u32;
    h1 ^= h1 >> 16;
    h1 = h1.wrapping_mul(0x85eb_ca6b);
    h1 ^= h1 >> 13;
    h1 = h1.wrapping_mul(0xc2b2_ae35);
    h1 ^= h1 >> 16;
    h1 as i32
}

/// `AthenaAdapter.murmur3_hash`: Iceberg bucket transform of a partition value.
/// Integers hash as 8 little-endian bytes, strings as UTF-8.
pub fn murmur3_hash(value: &Value, num_buckets: i64) -> Result<i64, String> {
    if num_buckets <= 0 {
        return Err(format!("num_buckets must be positive, got {num_buckets}"));
    }
    let hash = if let Some(int) = value.as_i64() {
        murmur3_32(&int.to_le_bytes(), 0)
    } else if let Some(bytes) = value.as_bytes() {
        murmur3_32(bytes, 0)
    } else if let Some(s) = value.as_str() {
        murmur3_32(s.as_bytes(), 0)
    } else {
        return Err(format!(
            "Need to add support data type for hashing: {}",
            value.kind()
        ));
    };
    Ok((hash as i64 & INTEGER_MAX_VALUE_32_BIT_SIGNED) % num_buckets)
}

/// `dbt.adapters.athena.utils.clean_sql_comment`
pub fn clean_sql_comment(comment: &str) -> String {
    comment
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `dbt.adapters.athena.utils.ellipsis_comment`
pub fn ellipsis_comment(s: &str, max_len: usize) -> String {
    if s.chars().count() > max_len {
        let head: String = s.chars().take(max_len.saturating_sub(3)).collect();
        format!("{head}...")
    } else {
        s.to_string()
    }
}

/// `dbt.adapters.athena.utils.is_valid_table_parameter_key`
pub fn is_valid_table_parameter_key(key: &str) -> bool {
    key.chars().count() <= 255
        && key
            .chars()
            .all(|c| c == '\t' || matches!(c as u32, 0x0020..=0xD7FF | 0xE000..=0xFFFD))
}

/// `dbt.adapters.athena.utils.stringify_table_parameter_value`: maps and lists as
/// JSON, everything else through `str()`, capped at 512000 characters.
pub fn stringify_table_parameter_value(value: &Value) -> Option<String> {
    use minijinja::value::ValueKind;
    let rendered = match value.kind() {
        ValueKind::Map | ValueKind::Seq | ValueKind::Iterable => {
            serde_json::to_string(value).ok()?
        }
        ValueKind::Undefined | ValueKind::None => "None".to_string(),
        ValueKind::Bool => if value.is_true() { "True" } else { "False" }.to_string(),
        _ => value.to_string(),
    };
    Some(rendered.chars().take(512_000).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s3_data_naming_parses_the_python_enum_values() {
        assert_eq!(
            S3DataNaming::parse("table_unique"),
            Some(S3DataNaming::TableUnique)
        );
        assert_eq!(
            S3DataNaming::parse("schema_table"),
            Some(S3DataNaming::SchemaTable)
        );
        assert_eq!(S3DataNaming::parse("nope"), None);
    }

    #[test]
    fn table_prefix_defaults_to_staging_dir_tables() {
        assert_eq!(
            s3_table_prefix("s3://b/stage/", None, None, None, false),
            "s3://b/stage/tables"
        );
        assert_eq!(
            s3_table_prefix("s3://b/stage", None, Some("s3://d/data"), None, false),
            "s3://d/data"
        );
        assert_eq!(
            s3_table_prefix(
                "s3://b/stage",
                Some("s3://t/tmp"),
                Some("s3://d/data"),
                None,
                true
            ),
            "s3://t/tmp"
        );
        assert_eq!(
            s3_table_prefix(
                "s3://b/stage",
                Some("s3://t/tmp"),
                Some("s3://d/data"),
                None,
                false
            ),
            "s3://d/data"
        );
    }

    #[test]
    fn generate_s3_location_follows_the_naming_strategy() {
        let render = |naming, external: Option<&str>, temp| {
            generate_s3_location(
                "sch",
                "tbl",
                "s3://b/tables",
                naming,
                external,
                temp,
                "UUID",
            )
        };
        assert_eq!(
            render(S3DataNaming::Unique, None, false),
            "s3://b/tables/UUID"
        );
        assert_eq!(
            render(S3DataNaming::Table, None, false),
            "s3://b/tables/tbl"
        );
        assert_eq!(
            render(S3DataNaming::TableUnique, None, false),
            "s3://b/tables/tbl/UUID"
        );
        assert_eq!(
            render(S3DataNaming::SchemaTable, None, false),
            "s3://b/tables/sch/tbl"
        );
        assert_eq!(
            render(S3DataNaming::SchemaTableUnique, None, false),
            "s3://b/tables/sch/tbl/UUID"
        );
        assert_eq!(
            render(S3DataNaming::Table, Some("s3://x/ext/"), false),
            "s3://x/ext"
        );
        // a temporary table ignores external_location
        assert_eq!(
            render(S3DataNaming::Table, Some("s3://x/ext/"), true),
            "s3://b/tables/tbl"
        );
    }

    #[test]
    fn parse_s3_path_splits_bucket_and_prefix_with_trailing_slash() {
        assert_eq!(
            parse_s3_path("s3://bucket/a/b"),
            ("bucket".to_string(), "a/b/".to_string())
        );
        assert_eq!(
            parse_s3_path("s3://bucket/a/b/"),
            ("bucket".to_string(), "a/b/".to_string())
        );
        assert_eq!(
            parse_s3_path("s3://bucket"),
            ("bucket".to_string(), "/".to_string())
        );
    }

    #[test]
    fn format_value_for_partition_matches_python() {
        assert_eq!(
            format_value_for_partition(&Value::from(()), "integer").unwrap(),
            ("null".to_string(), " is ")
        );
        assert_eq!(
            format_value_for_partition(&Value::from(7), "integer").unwrap(),
            ("7".to_string(), "=")
        );
        assert_eq!(
            format_value_for_partition(&Value::from("it's"), "string").unwrap(),
            ("'it''s'".to_string(), "=")
        );
        assert_eq!(
            format_value_for_partition(&Value::from("2024-01-02"), "date").unwrap(),
            ("DATE'2024-01-02'".to_string(), "=")
        );
        assert_eq!(
            format_value_for_partition(&Value::from("2024-01-02 03:04:05"), "timestamp").unwrap(),
            ("TIMESTAMP'2024-01-02 03:04:05'".to_string(), "=")
        );
        assert!(format_value_for_partition(&Value::from(1.5), "double").is_err());
    }

    #[test]
    fn format_partition_keys_handles_hidden_and_bucket_partitioning() {
        assert_eq!(format_one_partition_key("DAY(ts)"), "date_trunc('day', ts)");
        assert_eq!(format_one_partition_key("bucket(user_id, 16)"), "user_id");
        assert_eq!(format_one_partition_key("Region"), "region");
        assert_eq!(
            format_partition_keys(["year(dt)", "bucket(id, 4)", "kind"]),
            "date_trunc('year', dt), id, kind"
        );
    }

    #[test]
    fn murmur3_matches_the_iceberg_reference_vectors() {
        // https://iceberg.apache.org/spec/#appendix-b-32-bit-hash-requirements
        assert_eq!(murmur3_32(&34i64.to_le_bytes(), 0), 2017239379);
        assert_eq!(murmur3_32("iceberg".as_bytes(), 0), 1210000089);
        assert_eq!(
            murmur3_hash(&Value::from(34), 100).unwrap(),
            2017239379 % 100
        );
        assert_eq!(
            murmur3_hash(&Value::from("iceberg"), 16).unwrap(),
            1210000089 % 16
        );
        assert!(murmur3_hash(&Value::from(1.5), 16).is_err());
    }

    #[test]
    fn table_type_from_glue_prefers_the_iceberg_parameter() {
        assert_eq!(
            TableType::from_glue(Some("customer"), Some("ICEBERG"), "t").unwrap(),
            TableType::Iceberg
        );
        assert_eq!(
            TableType::from_glue(Some("EXTERNAL_TABLE"), None, "t").unwrap(),
            TableType::Table
        );
        assert_eq!(
            TableType::from_glue(Some("VIRTUAL_VIEW"), None, "t").unwrap(),
            TableType::View
        );
        assert!(TableType::from_glue(Some("customer"), None, "t").is_err());
        assert!(TableType::from_glue(None, None, "t").is_err());
        assert_eq!(
            TableType::Iceberg
                .to_jinja()
                .get_attr("value")
                .unwrap()
                .as_str(),
            Some("iceberg_table")
        );
    }

    #[test]
    fn docs_string_helpers_match_python() {
        assert_eq!(clean_sql_comment("  a\n\n  b  \nc"), "a b c");
        assert_eq!(ellipsis_comment("abcdef", 5), "ab...");
        assert_eq!(ellipsis_comment("abcde", 5), "abcde");
        assert!(is_valid_table_parameter_key("dbt_project_name"));
        assert!(!is_valid_table_parameter_key("bad\nkey"));
        assert_eq!(
            stringify_table_parameter_value(&Value::from_iter([("a", 1)])).unwrap(),
            r#"{"a":1}"#
        );
        assert_eq!(
            stringify_table_parameter_value(&Value::from(true)).unwrap(),
            "True"
        );
        assert_eq!(
            stringify_table_parameter_value(&Value::from("x")).unwrap(),
            "x"
        );
    }

    #[test]
    fn s3_tables_databases_are_recognised_by_prefix() {
        assert!(is_s3_tables_database(Some("s3tablescatalog/my-bucket")));
        assert!(is_s3_tables_database(Some("S3TablesCatalog/my-bucket")));
        assert!(!is_s3_tables_database(Some("awsdatacatalog")));
        assert!(!is_s3_tables_database(None));
    }
}
