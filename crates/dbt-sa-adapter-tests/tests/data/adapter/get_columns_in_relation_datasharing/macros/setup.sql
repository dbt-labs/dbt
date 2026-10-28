--- test set_up methods

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_name = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_name ~ ' (number_col int, text_col varchar(100), text_col2 varchar(101));') %}
{% endmacro %}
