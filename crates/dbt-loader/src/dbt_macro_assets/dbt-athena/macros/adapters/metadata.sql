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

{#- Fusion's `default__check_schema_exists` calls
    `information_schema.replace(information_schema_view=...)`, which Fusion
    relations do not implement; every adapter package overrides it. Athena's
    Glue-backed `information_schema.schemata` is case-insensitive in practice
    (everything is stored lowercased), so compare lowercased. -#}
{% macro athena__check_schema_exists(information_schema, schema) -%}
  {% call statement('check_schema_exists', fetch_result=True, auto_begin=False) -%}
    select count(*) from information_schema.schemata
    where lower(schema_name) = '{{ schema | lower }}'
    {%- if information_schema.database %}
      and lower(catalog_name) = '{{ information_schema.database | replace('"', '') | lower }}'
    {%- endif %}
  {%- endcall %}
  {{ return(load_result('check_schema_exists').table) }}
{% endmacro %}
