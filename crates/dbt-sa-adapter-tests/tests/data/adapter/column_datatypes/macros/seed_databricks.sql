{# The test's first command: creates the table the rest of the test reads. #}
{% macro seed_databricks() %}
  {% set s = target.schema %}
  {% do run_query("create schema if not exists `" ~ target.database ~ "`.`" ~ s ~ "`") %}
  {% do run_query("create or replace table " ~ s ~ ".a (val int)") %}
  {% set create_source %}
CREATE TABLE {{ s }}.source AS SELECT
  -- Integer types
  CAST(1 AS TINYINT) as tinyint_col,
  CAST(1 AS SMALLINT) as smallint_col,
  CAST(1 AS INT) as int_col,
  CAST(1 AS BIGINT) as bigint_col,

  -- Decimal/Numeric types
  CAST(123.45 AS DECIMAL(10,2)) as decimal_col,

  -- Floating point types
  CAST(1.23 AS FLOAT) as float_col,
  CAST(1.23 AS DOUBLE) as double_col,

  -- String types
  CAST('test_string' AS STRING) as string_col,
  CAST('test_char' AS CHAR(10)) as char_col,
  CAST('test_varchar' AS VARCHAR(100)) as varchar_col,

  -- Binary type
  CAST('test_binary' AS BINARY) as binary_col,

  -- Boolean type
  CAST(TRUE AS BOOLEAN) as boolean_col,

  -- Date and time types
  CAST('2023-01-01' AS DATE) as date_col,
  CAST('2023-01-01 12:30:00' AS TIMESTAMP) as timestamp_col,
  CAST('2023-01-01 12:30:00' AS TIMESTAMP_NTZ) as timestamp_ntz_col,

  -- Interval types
  INTERVAL '1' YEAR as interval_year_col,
  INTERVAL '1' MONTH as interval_month_col,
  INTERVAL '1' DAY as interval_day_col,
  INTERVAL '1' HOUR as interval_hour_col,
  INTERVAL '1' MINUTE as interval_minute_col,
  INTERVAL '1' SECOND as interval_second_col,

  -- Array types
  ARRAY(1, 2, 3) as array_int_col,
  ARRAY('a', 'b', 'c') as array_string_col,
  ARRAY(CAST('2023-01-01' AS DATE), CAST('2023-01-02' AS DATE)) as array_date_col,

  -- Map type
  MAP('key1', 'value1', 'key2', 'value2') as map_col,
  MAP('num1', 1, 'num2', 2) as map_int_col,

  -- Struct types
  STRUCT('John' as name, 30 as age, TRUE as active) as struct_col,

  -- Complex nested types
  STRUCT(
    'address' as type,
    STRUCT(
      '123 Main St' as street,
      'Anytown' as city,
      'CA' as state,
      '12345' as zip
    ) as details,
    MAP('home', '555-1234', 'work', '555-5678') as phones
  ) as nested_struct_col,

  -- Array of structs
  ARRAY(
    STRUCT('Alice' as name, 25 as age),
    STRUCT('Bob' as name, 30 as age)
  ) as array_struct_col,

  -- Struct with arrays and maps
  STRUCT(
    'user_data' as type,
    ARRAY(95, 87, 92) as scores,
    ARRAY('tag1', 'tag2', 'tag3') as tags,
    MAP('level', 'advanced', 'status', 'active') as metadata
  ) as struct_with_collections_col,

  -- Map with complex values
  MAP(
    'user1', STRUCT('Alice' as name, 25 as age),
    'user2', STRUCT('Bob' as name, 30 as age)
  ) as map_struct_col,

  -- Array of maps
  ARRAY(
    MAP('name', 'Alice', 'role', 'admin'),
    MAP('name', 'Bob', 'role', 'user')
  ) as array_map_col
  {% endset %}
  {% do run_query(create_source) %}
{% endmacro %}
