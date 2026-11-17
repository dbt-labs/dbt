--- test related macros
{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro snowflake__set_up(schema_name, table_name) %}
  {{ print("snowflake") }}
  
  {# Create or replace schema #}
  {% do adapter.execute('USE DATABASE ' ~ target.database ~ '; CREATE OR REPLACE SCHEMA ' ~ schema_name) %}

  {# Create table #}
  {% set full_table = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ full_table ~ ' (one int)') %}

  {# grant permissions, ad_hoc is the role used by the test user #}
  {% do adapter.execute('GRANT SELECT ON ' ~ full_table ~ ' TO ROLE tester') %}
{% endmacro %}

