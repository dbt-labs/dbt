{% macro drop_dev_schema() %}
  {% set schema_relation = api.Relation.create(database=target.database, schema=target.schema) %}
  {{ adapter.drop_schema(schema_relation) }}
{% endmacro %}
