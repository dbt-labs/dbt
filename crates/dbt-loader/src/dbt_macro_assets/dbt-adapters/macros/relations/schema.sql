-- funcsign: (string) -> string
{% macro drop_schema_named(schema_name) %}
    {{ return(adapter.dispatch('drop_schema_named', 'dbt') (schema_name)) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro default__drop_schema_named(schema_name) %}
  {% set schema_relation = api.Relation.create(schema=schema_name) %}
  {{ adapter.drop_schema(schema_relation) }}
{% endmacro %}
