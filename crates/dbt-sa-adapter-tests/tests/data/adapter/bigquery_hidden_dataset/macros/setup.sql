--- test related macros
{% macro set_up() %}
  {% do adapter.dispatch("set_up")() %}
{% endmacro %}

{% macro bigquery__set_up() %}
  {% set fqn = '`' ~ target.database ~ '`.`' ~ target.schema ~ '`' %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ fqn ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ fqn) %}
{% endmacro %}
