{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  
  -- Create source_non_empty table with a single column
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.source_non_empty (id int64)') %}
  {% do adapter.execute('INSERT INTO ' ~ schema_name ~ '.source_non_empty VALUES (1)') %}

  -- Create existing_dest table with the same column as source_non_empty
  {% do adapter.execute('CREATE OR REPLACE TABLE ' ~ schema_name ~ '.existing_dest (id int64)') %}
  {% do adapter.execute('INSERT INTO ' ~ schema_name ~ '.existing_dest VALUES (2)') %}
{% endmacro %}