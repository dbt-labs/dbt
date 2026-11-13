--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
{% endmacro %}
