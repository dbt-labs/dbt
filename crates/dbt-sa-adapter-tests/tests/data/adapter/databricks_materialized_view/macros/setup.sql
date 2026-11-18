{# Set up materialized views for testing #}

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro default__set_up(schema_name, table_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro databricks__set_up(schema_name, table_name) %}
  {{ print("databricks") }}
  
  {# Create dataset (schema) #}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}

  {# Create base table #}
  {% set full_table = schema_name ~ '.' ~ table_name %}
  {% do adapter.execute('CREATE TABLE IF NOT EXISTS ' ~ full_table ~ ' (id INT, event_name STRING, user_id INT, timestamp TIMESTAMP)') %}

  {# Create materialized views #}
  {% set mv_name_alter = 'test_alter' %}
  {% set full_mv_alter = schema_name ~ '.' ~ mv_name %}
  {% do adapter.execute('CREATE MATERIALIZED VIEW IF NOT EXISTS ' ~ full_mv_alter ~ ' AS SELECT id, event_name, user_id, timestamp FROM ' ~ full_table) %}
  
  {% set mv_name_fail = 'test_fail' %}
  {% set full_mv_fail = schema_name ~ '.' ~ mv_name_fail %}
  {% do adapter.execute('CREATE MATERIALIZED VIEW IF NOT EXISTS ' ~ full_mv_fail ~ ' AS SELECT id, event_name, user_id, timestamp FROM ' ~ full_table) %}
  
  {% set mv_name_full_refresh = 'test_full_refresh' %}
  {% set full_mv_full_refresh = schema_name ~ '.' ~ mv_name_full_refresh %}
  {% do adapter.execute('CREATE MATERIALIZED VIEW IF NOT EXISTS ' ~ mv_name_full_refresh ~ ' AS SELECT id, event_name, user_id, timestamp FROM ' ~ full_table) %}

  {# Add the created relation to cache #}
  {% set table_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(table_relation) %}
  {% set mv_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=mv_name_alter, type='materialized_view') %}
  {% do adapter.cache_added(mv_relation) %}
  {% set mv_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=mv_name_fail, type='materialized_view') %}
  {% do adapter.cache_added(mv_relation) %}
  {% set mv_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=mv_name_full_refresh, type='materialized_view') %}
  {% do adapter.cache_added(mv_relation) %}
{% endmacro %}