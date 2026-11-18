--- test related macros

{% macro set_up(schema_name, table_name) %}
  {% do adapter.dispatch("set_up")(schema_name, table_name) %}
{% endmacro %}

{% macro redshift__set_up(schema_name, table_name) %}
  {% do adapter.execute('DROP SCHEMA IF EXISTS ' ~ schema_name ~ ' CASCADE;') %}
  {% do adapter.execute('CREATE SCHEMA ' ~ schema_name) %}
  {% set table_full_name = schema_name ~ '.' ~ table_name %}
  {% set view_full_name = schema_name ~ '.' ~ table_name ~ '_v' %}
  {% do adapter.execute('CREATE TABLE ' ~ table_full_name ~ ' (val int);') %}
  {% do adapter.execute('CREATE VIEW ' ~ view_full_name ~ ' AS SELECT 1 AS val;') %}
  {# Add the created relations to cache so adapter.get_relation finds them at render time #}
  {% set table_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.cache_added(table_relation) %}
  {% set view_relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name ~ '_v', type='view') %}
  {% do adapter.cache_added(view_relation) %}
{% endmacro %}
