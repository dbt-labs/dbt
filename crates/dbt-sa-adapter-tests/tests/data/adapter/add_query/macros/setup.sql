--- test related macros

{% macro supports_explicit_transactions() %}
  {% do return(adapter.dispatch("supports_explicit_transactions")()) %}
{% endmacro %}

{% macro default__supports_explicit_transactions() %}
  {% do return(true) %}
{% endmacro %}

{% macro databricks__supports_explicit_transactions() %}
  {% do return(false) %}
{% endmacro %}

{% macro duckdb__supports_explicit_transactions() %}
  {% do return(false) %}
{% endmacro %}

{% macro clickhouse__supports_explicit_transactions() %}
  {% do return(false) %}
{% endmacro %}

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
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (val int);') %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery: does not support add_query") }}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
{% endmacro %}

{% macro clickhouse__set_up(schema_name, table_name) %}
  {{ print("clickhouse") }}
  {# Not DROP DATABASE IF EXISTS followed by CREATE DATABASE: this schema is the session's default database, so dropping it makes the next statement fail with UNKNOWN_DATABASE (Code 81) and the test cannot be recorded. #}
  {% do adapter.execute('CREATE DATABASE IF NOT EXISTS `' ~ schema_name ~ '`') %}
  {% set full_name = '`' ~ schema_name ~ '`' ~ '.`' ~ table_name ~ '`' %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
{% endmacro %}
