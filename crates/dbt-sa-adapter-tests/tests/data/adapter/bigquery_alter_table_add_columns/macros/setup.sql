{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.b (one int, two int)') %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.c (one int, two int, three int64, four string, five bool, six timestamp, seven struct<a string, b int64>, eight array<struct<a string, b int64>>, nine array<int64>)') %}
{% endmacro %}