--- test set_up methods

{% macro set_up(schema_name, table_names=[]) %}
  {% do adapter.dispatch("set_up")(schema_name, table_names) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_names=[]) %}
  {{ print("default") }}
{% endmacro %}

{% macro postgres__set_up(schema_name, table_names=[]) %}
  {{ print("postgres") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100));') %}
  {% endfor %}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_names=[]) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100));') %}
  {% endfor %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_names=[]) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (number_col int, text_col string);') %}
  {% endfor %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_names=[]) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100));') %}
  {% endfor %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_names=[]) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100));') %}
  {% endfor %}
{% endmacro %}

{% macro duckdb__set_up(schema_name, table_names=[]) %}
  {{ print("duckdb") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS dbtdb.' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA dbtdb.' ~ schema_name) %}
  {% for table_name in table_names %}
    {% set full_name = 'dbtdb.' ~ schema_name ~ '.' ~ table_name %}
    {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100));') %}
  {% endfor %}
{% endmacro %}
