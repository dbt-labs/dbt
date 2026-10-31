{# The test's first command: creates the table the rest of the test reads. #}
{% macro seed_bigquery() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ s ~ "`") %}
  {% do run_query("create or replace table " ~ s ~ ".a (val int)") %}
  {% set create_source %}
CREATE TABLE {{ s }}.source AS
WITH source AS (
  SELECT
    -- Numeric types
    CAST(1 AS INT64) as int64_col,
    CAST(1.5 AS FLOAT64) as float64_col,
    CAST(99999999999999999999999999999.999999999 AS NUMERIC) as numeric_col,
    CAST(99999999999999999999999999999.999999999 AS BIGNUMERIC) as bignumeric_col,

    -- String types
    CAST('test_string' AS STRING) as string_col,

    -- Bytes type
    CAST('test_bytes' AS BYTES) as bytes_col,

    -- Boolean type
    CAST(true AS BOOL) as bool_col,

    -- Date and time types
    CAST('2023-01-01' AS DATE) as date_col,
    CAST('12:30:00' AS TIME) as time_col,
    CAST('2023-01-01 12:30:00' AS DATETIME) as datetime_col,
    CAST('2023-01-01 12:30:00 UTC' AS TIMESTAMP) as timestamp_col,

    -- Geography type
    ST_GEOGPOINT(-122.35, 37.55) as geography_col,

    -- JSON type
    PARSE_JSON('{"key": "value", "number": 123}') as json_col,

    -- Array types
    [1, 2, 3] as array_int64_col,
    ['a', 'b', 'c'] as array_string_col,
    [CAST('2023-01-01' AS DATE), CAST('2023-01-02' AS DATE)] as array_date_col,

    -- Struct types
    STRUCT(
      'John' as name,
      30 as age,
      true as active
    ) as struct_col,

    STRUCT(
        200 AS code,
        JSON """{"a": 1}""" AS `json`
    ) AS response,

    -- Complex nested types
    STRUCT(
      'address' as type,
      STRUCT(
        '123 Main St' as street,
        'Anytown' as city,
        'CA' as state,
        '12345' as zip
      ) as details
    ) as nested_struct_col,

    -- Array of structs
    [
      STRUCT('Alice' as name, 25 as age),
      STRUCT('Bob' as name, 30 as age)
    ] as array_struct_col,

    -- Struct with arrays
    STRUCT(
      'user_data' as type,
      [1, 2, 3] as scores,
      ['tag1', 'tag2'] as tags
    ) as struct_with_arrays_col,
)

SELECT * FROM source
  {% endset %}
  {% do run_query(create_source) %}
{% endmacro %}
