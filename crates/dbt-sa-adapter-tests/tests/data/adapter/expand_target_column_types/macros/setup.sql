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
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.a (id VARCHAR(2))') %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.b (id VARCHAR(2))') %}
{% endmacro %}

{% macro redshift__set_up(schema_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.a (id VARCHAR(2))') %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.b (id VARCHAR(1))') %}
{% endmacro %}

{% macro databricks__set_up(schema_name) %}
  {{ print("databricks") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.a (id VARCHAR(2))') %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ schema_name ~ '.b (id VARCHAR(1))') %}
{% endmacro %}
