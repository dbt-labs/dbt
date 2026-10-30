--- test related macros
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
  --- additional tables to test get_relation helper macro list_relations_without_caching
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ '_2 (val int);') %}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (val int);') %}
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (val int);') %}
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro duckdb__set_up(schema_name, table_name) %}
  {{ print("duckdb") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val int);') %}
  --- additional tables to test get_relation helper macro list_relations_without_caching
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ '_2 (val int);') %}
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro clickhouse__set_up(schema_name, table_name) %}
  {{ print("clickhouse") }}
  {# Not DROP DATABASE IF EXISTS followed by CREATE DATABASE: this schema is the session's default database, so dropping it makes the next statement fail with UNKNOWN_DATABASE (Code 81) and the test cannot be recorded. #}
  {% do adapter.execute('CREATE DATABASE IF NOT EXISTS `' ~ schema_name ~ '`') %}
  {% set full_name = '`' ~ schema_name ~ '`' ~ '.`' ~ table_name ~ '`' %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (val Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
  {% do adapter.execute('CREATE TABLE `' ~ schema_name ~ '`.`' ~ table_name ~ '_2` (val Int32) ENGINE = MergeTree() ORDER BY tuple()') %}
{% endmacro %}
