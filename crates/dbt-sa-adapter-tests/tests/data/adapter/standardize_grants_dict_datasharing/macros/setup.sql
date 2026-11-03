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

  {# Seed grants spanning the identity types SHOW GRANTS returns. Only the user
     grant survives `standardize_grants_dict`; the role and PUBLIC grants are
     filtered to match the legacy pg_user + has_table_privilege() behavior. #}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO dbt_test_user_1') %}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO ROLE tester') %}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO PUBLIC') %}
{% endmacro %}
