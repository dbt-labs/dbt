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
  {# Add the created relation to cache #}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(relation) %}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (val int);') %}
{% endmacro %}
