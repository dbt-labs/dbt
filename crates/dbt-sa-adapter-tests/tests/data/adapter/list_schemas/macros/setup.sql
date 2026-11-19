--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro postgres__set_up(schema_name, table_name) %}
  {{ print("postgres") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_1 CASCADE') %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_2 CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_1') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_2') %}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name ~ '_1') %}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name ~ '_2') %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name ~ '1') %}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name ~ '2') %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_1 CASCADE;') %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_2 CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_1') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_2') %}
{% endmacro %}

{% macro duckdb__set_up(schema_name, table_name) %}
  {{ print("duckdb") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_1 CASCADE;') %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_2 CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_1') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_2') %}
{% endmacro %}


{% macro clickhouse__set_up(schema_name, table_name) %}
  {{ print("clickhouse") }}
  {% do adapter.execute('DROP DATABASE IF EXISTS `' ~ schema_name ~ '_1`') %}
  {% do adapter.execute('DROP DATABASE IF EXISTS `' ~ schema_name ~ '_2`') %}
  {% do adapter.execute('CREATE DATABASE `' ~ schema_name ~ '_1`') %}
  {% do adapter.execute('CREATE DATABASE `' ~ schema_name ~ '_2`') %}
{% endmacro %}
