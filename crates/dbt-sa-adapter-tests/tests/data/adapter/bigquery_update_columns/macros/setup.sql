{% macro set_up(schema_name) %}
  {% do adapter.dispatch("set_up")(schema_name) %}
{% endmacro %}

{% macro default__set_up(schema_name) %}
  {{ print("default") }}
{% endmacro %}

{% macro bigquery__set_up(schema_name) %}
  {{ print("bigquery") }}
  {% do adapter.execute('CREATE SCHEMA IF NOT EXISTS ' ~ schema_name) %}
  {{ print(schema_name ~ '.a') }}
  {% do adapter.execute('
    CREATE OR REPLACE TABLE ' ~ schema_name ~ '.a (
      a STRUCT<b STRING>,
      b STRING
    )'
  ) %}
  {# Add the created relation to cache #}
  {% set table_relation = api.Relation.create(database=target.database, schema=schema_name, identifier='a', type='table') %}
  {% do adapter.cache_added(table_relation) %}
{% endmacro %}