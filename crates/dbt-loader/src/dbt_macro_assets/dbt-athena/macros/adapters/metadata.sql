{% macro athena__get_catalog(information_schema, schemas) -%}
    {{ return(adapter.get_catalog()) }}
{%- endmacro %}


{% macro athena__list_schemas(database) -%}
  {{ return(adapter.list_schemas(database)) }}
{% endmacro %}


{% macro athena__list_relations_without_caching(schema_relation) %}
  {{ return(adapter.list_relations_without_caching(schema_relation)) }}
{% endmacro %}

{% macro athena__get_catalog_relations(information_schema, relations) %}
  {{ return(adapter.get_catalog_by_relations(information_schema, relations)) }}
{% endmacro %}
