{% macro redis_adbc__set_up(schema_name, table_name) %}
  {% set schema_relation = api.Relation.create(database=target.database, schema=schema_name) %}
  {% do adapter.create_schema(schema_relation) %}
  {% set relation = api.Relation.create(database=target.database, schema=schema_name, identifier=table_name, type='table') %}
  {% do adapter.execute('create table ' ~ relation ~ ' (number_col numeric(10,2), text_col varchar(10), text_col2 varchar(20))') %}
{% endmacro %}
