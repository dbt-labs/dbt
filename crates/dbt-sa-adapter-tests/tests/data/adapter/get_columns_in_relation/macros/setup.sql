--- test set_up methods

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro postgres__set_up(schema_name, table_name) %}
  {{ print("postgres") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101));') %}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101), array_col array(object(key varchar, value varchar, nested object(nested_key varchar, nested_value varchar))));') %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  -- BigQuery supports parameterized data type for String but `dbt` doesn't populate this to the char_size field in BigQueryColumn
  -- https://cloud.google.com/bigquery/docs/reference/standard-sql/data-types#parameterized_data_types
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (number_col int, text_col string, text_col2 string(10));') %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101));') %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101));') %}
{% endmacro %}

{% macro duckdb__set_up(schema_name, table_name) %}
  {{ print("duckdb") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101));') %}
{% endmacro %}

{% macro clickhouse__set_up(schema_name, table_name) %}
  {{ print("clickhouse") }}
  {# Not DROP DATABASE IF EXISTS followed by CREATE DATABASE: this schema is the session's default database, so dropping it makes the next statement fail with UNKNOWN_DATABASE (Code 81) and the test cannot be recorded. #}
  {% do adapter.execute('CREATE DATABASE IF NOT EXISTS `' ~ schema_name ~ '`') %}
  {% set full_name = '`' ~ schema_name ~ '`' ~ '.`' ~ table_name ~ '`' %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col Int32, text_col String, text_col2 String) ENGINE = MergeTree() ORDER BY tuple()') %}
  {% do adapter.execute('CREATE TABLE `' ~ schema_name ~ '`.`' ~ table_name ~ '_2` (val Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
{% endmacro %}
