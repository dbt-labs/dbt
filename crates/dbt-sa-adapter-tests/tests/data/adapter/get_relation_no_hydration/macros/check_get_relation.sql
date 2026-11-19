{% macro check_get_relation() %}
  {% set table_name = 'a' %}
  {% set db_schema = target.schema %}
  {% do set_up(db_schema, table_name) %}

  {% set relation = adapter.get_relation(
      database=target.database,
      schema=db_schema,
      identifier=table_name) %}

  {{ print("relation") }}
  {{ print(relation) }}
{% endmacro %}
