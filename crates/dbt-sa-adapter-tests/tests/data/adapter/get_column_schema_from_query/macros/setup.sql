{% macro get_sql() %}
    {{ return(adapter.dispatch('get_sql')()) }}
{% endmacro %}

{% macro postgres__get_sql() %}
    select 42 as number_col, 'postgres text' as text_col, true as boolean_col
{% endmacro %}

{% macro snowflake__get_sql() %}
    select 42 as number_col, 'snowflake text' as text_col, true as boolean_col
{% endmacro %}

{% macro bigquery__get_sql() %}
    select 42 as number_col, 'bigquery text' as text_col, true as boolean_col
{% endmacro %}

{% macro redshift__get_sql() %}
    select 42 as number_col, 'redshift text' as text_col, true as boolean_col
{% endmacro %}

{% macro databricks__get_sql() %}
    select
        42 as number_col,
        'databricks text' as text_col,
        true as boolean_col,
        cast(123.45 as decimal(38,2)) as decimal128_col,
        cast(1.23 as float) as float_col,
        cast(4.56 as double) as double_col,
        cast('2024-01-01 12:00:00' as timestamp) as timestamp_col,
        array(1, 2, 3) as array_int_col,
        array('a', 'b', 'c') as array_string_col,
        map('a', 1, 'b', 2, 'c', 3) as map_string_int_col,
        interval '1' year as interval_year_col
{% endmacro %}

{% macro duckdb__get_sql() %}
    select
        42 as number_col,
        'duckdb text' as text_col,
        true as boolean_col,
        cast('2024-01-01 12:00:00' as timestamp) as timestamp_col,
        -- https://github.com/dbt-labs/dbt/issues/16470
        cast('2024-01-01 12:00:00+00' as timestamp with time zone) as timestamptz_col
{% endmacro %}
