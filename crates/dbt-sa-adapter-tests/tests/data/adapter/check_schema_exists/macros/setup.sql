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
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
{% endmacro %}

{% macro duckdb__set_up(schema_name, table_name) %}
  {{ print("duckdb") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
{% endmacro %}

{% macro clickhouse__set_up(schema_name, table_name) %}
  {{ print("clickhouse") }}
  {# Not DROP DATABASE IF EXISTS followed by CREATE DATABASE: this schema is the session's default database, so dropping it makes the next statement fail with UNKNOWN_DATABASE (Code 81) and the test cannot be recorded. #}
  {% do adapter.execute('CREATE DATABASE IF NOT EXISTS `' ~ schema_name ~ '`') %}
{% endmacro %}
