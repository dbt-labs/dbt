--- test related macros
{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {{ print("redshift") }}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set full_table = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE ' ~ full_table ~ ' (one int)') %}

  {# Seed a plain grantee and a special-character grantee. The special-char
     user must be quoted here (dot in the name). apply_grants later revokes
     it, and that revoke is the behavior under test. #}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO dbt_test_user_1') %}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO "test.user"') %}
{% endmacro %}
