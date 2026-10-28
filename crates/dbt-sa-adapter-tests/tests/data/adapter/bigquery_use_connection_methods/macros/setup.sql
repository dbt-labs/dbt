--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name, table_name) %}
  {{ print("bigquery") }}

  {# Create dataset (schema) #}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}

  {# Create base table #}
  {% set full_table = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ full_table ~ ' (id INT64)') %}

  {# Create regular view #}
  {% set view_name = full_table ~ '_view' %}
  {% do adapter.execute('CREATE VIEW IF NOT EXISTS ' ~ view_name ~ ' AS SELECT * FROM ' ~ full_table) %}

  {# Create materialized view #}
  {% set materialized_view = full_table ~ '_materialized_view' %}
  {% do adapter.execute('CREATE MATERIALIZED VIEW IF NOT EXISTS ' ~ materialized_view ~ ' AS SELECT * FROM ' ~ full_table) %}
{% endmacro %}
