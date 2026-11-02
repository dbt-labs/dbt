{#- dbt-athena builds the catalog Python-side from Glue (adapter.get_catalog).
    Fusion runs this macro from `compile --write-catalog` / docs and expects the
    Postgres-shaped result (one row per column, table columns repeated), so it is
    a query over Trino's information_schema instead. Glue exposes no table owner
    or comment there; both are returned empty. -#}
{% macro athena__get_catalog(information_schema, schemas) -%}
  {%- set relations = [] -%}
  {%- for schema in schemas -%}
    {%- do relations.append({'schema': schema}) -%}
  {%- endfor -%}
  {{ return(athena__get_catalog_relations(information_schema, relations)) }}
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

{% macro athena__get_catalog_relations(information_schema, relations) -%}
  {%- call statement('catalog', fetch_result=True) -%}
    select
        '{{ information_schema.database }}' as table_database,
        t.table_schema as table_schema,
        t.table_name as table_name,
        t.table_type as table_type,
        cast(null as varchar) as table_comment,
        c.column_name as column_name,
        c.ordinal_position as column_index,
        c.data_type as column_type,
        c.comment as column_comment,
        cast(null as varchar) as table_owner
    from information_schema.tables t
    join information_schema.columns c
      on c.table_catalog = t.table_catalog
     and c.table_schema = t.table_schema
     and c.table_name = t.table_name
    where (
      {%- for relation in relations -%}
        {%- if relation.identifier -%}
          (lower(t.table_schema) = lower('{{ relation.schema }}') and
           lower(t.table_name) = lower('{{ relation.identifier }}'))
        {%- else -%}
          lower(t.table_schema) = lower('{{ relation.schema }}')
        {%- endif -%}
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
    )
    order by t.table_schema, t.table_name, c.ordinal_position
  {%- endcall -%}
  {{ return(load_result('catalog').table) }}
{%- endmacro %}

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
