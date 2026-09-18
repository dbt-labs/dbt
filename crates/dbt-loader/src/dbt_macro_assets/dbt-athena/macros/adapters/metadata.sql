{% macro athena__get_catalog(information_schema, schemas) -%}
    {{ return(adapter.get_catalog()) }}
{%- endmacro %}


{#- Fusion runs this macro from AdapterImpl::list_schemas, so delegating to
    adapter.list_schemas would recurse. Query information_schema instead. The
    database arrives rendered (quoted); strip the quotes for the literal. -#}
{% macro athena__list_schemas(database) -%}
  {% call statement('list_schemas', fetch_result=True, auto_begin=False) %}
    select schema_name from information_schema.schemata
    {% if database %} where lower(catalog_name) = '{{ database | replace('"', '') | lower }}' {% endif %}
  {% endcall %}
  {{ return(load_result('list_schemas').table) }}
{% endmacro %}


{% macro athena__list_relations_without_caching(schema_relation) %}
  {{ return(adapter.list_relations_without_caching(schema_relation)) }}
{% endmacro %}

{% macro athena__get_catalog_relations(information_schema, relations) %}
  {{ return(adapter.get_catalog_by_relations(information_schema, relations)) }}
{% endmacro %}
