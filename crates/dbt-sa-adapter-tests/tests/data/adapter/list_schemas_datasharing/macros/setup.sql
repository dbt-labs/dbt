--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro postgres__set_up(schema_name, table_name) %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_1 CASCADE') %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ '_2 CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_1') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name ~ '_2') %}
{% endmacro %}
