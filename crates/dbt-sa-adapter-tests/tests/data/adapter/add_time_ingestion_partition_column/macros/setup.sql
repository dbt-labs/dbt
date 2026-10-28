--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_name ~ ' (id INT64, msg STRING);') %}
{% endmacro %}
