{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro postgres__set_up(schema_name) %}
  {{ print("postgres") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.a (one int, three int)') %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.b (one int, two int)') %}
{% endmacro %}

{% macro snowflake__set_up(schema_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE a (one int, three int)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE b (one int, two int)') %}
{% endmacro %}

{% macro bigquery__set_up(schema_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.a (one int, three int)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.b (one int, two int)') %}
{% endmacro %}

{% macro redshift__set_up(schema_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.a (one int, three int)') %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.b (one int, two int)') %}
{% endmacro %}

{% macro databricks__set_up(schema_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.a (one int, three int)') %}
  {% do adapter.execute('CREATE TABLE ' ~ schema_name ~ '.b (one int, two int)') %}
{% endmacro %}

{% macro clickhouse__set_up(schema_name) %}
  {{ print("clickhouse") }}
  {# Not DROP DATABASE IF EXISTS followed by CREATE DATABASE: this schema is the session's default database, so dropping it makes the next statement fail with UNKNOWN_DATABASE (Code 81) and the test cannot be recorded. #}
  {% do adapter.execute('CREATE DATABASE IF NOT EXISTS `' ~ schema_name ~ '`') %}
  {% do adapter.execute('CREATE TABLE `' ~ schema_name ~ '`.`a` (one Int32, three Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
  {% do adapter.execute('CREATE TABLE `' ~ schema_name ~ '`.`b` (one Int32, two Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
{% endmacro %}
