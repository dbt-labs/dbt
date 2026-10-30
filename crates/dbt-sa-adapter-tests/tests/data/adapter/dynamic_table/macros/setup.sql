{% macro set_up() %}
  {% do adapter.dispatch("set_up")() %}
{% endmacro %}

{% macro default__set_up() %}
{% endmacro %}

{% macro snowflake__set_up() %}
  {% do adapter.execute('USE DATABASE ' ~ target.database) %}
  {% do adapter.execute('CREATE OR ALTER SCHEMA ' ~ target.database ~ '.' ~ target.schema) %}
  {% set full_name = target.database ~ '.' ~ target.schema ~ '.source_table' %}
  {% do adapter.execute('CREATE OR ALTER TABLE ' ~ full_name ~ ' (id int, name varchar, id2 int) CHANGE_TRACKING = TRUE') %}
  {% do adapter.execute('INSERT INTO ' ~ full_name ~ ' VALUES (1, \'a\', 2), (5, \'b\', 6)') %}
{% endmacro %}
