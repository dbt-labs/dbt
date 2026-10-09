{% macro singlestore__get_catalog(information_schema, schemas) -%}
  {%- call statement('catalog', fetch_result=True) -%}
    {{ singlestore__get_catalog_results_sql(singlestore__get_catalog_schemas_where_clause_sql(schemas)) }}
  {%- endcall -%}
  {{ return(load_result('catalog').table) }}
{%- endmacro %}

{% macro singlestore__get_catalog_relations(information_schema, relations) -%}
  {%- call statement('catalog', fetch_result=True) -%}
    {{ singlestore__get_catalog_results_sql(singlestore__get_catalog_relations_where_clause_sql(relations)) }}
  {%- endcall -%}
  {{ return(load_result('catalog').table) }}
{%- endmacro %}

{% macro singlestore__get_catalog_results_sql(where_clause) -%}
    select
      c.table_schema as table_database,
      c.table_schema as table_schema,
      c.table_name as table_name,
      case when t.table_type = 'VIEW' then 'view' else 'table' end as table_type,
      nullif(t.table_comment, '') as table_comment,
      c.column_name as column_name,
      c.ordinal_position as column_index,
      c.column_type as column_type,
      nullif(c.column_comment, '') as column_comment,
      null as table_owner
    from information_schema.columns c
    join information_schema.tables t
      on c.table_schema = t.table_schema and c.table_name = t.table_name
    {{ where_clause }}
    order by c.table_schema, c.table_name, c.ordinal_position
{%- endmacro %}

{% macro singlestore__get_catalog_schemas_where_clause_sql(schemas) -%}
  {% if schemas | length == 0 %}
    where 1 = 0
  {% else %}
    where c.table_schema not in ('information_schema')
      and (
      {%- for schema in schemas -%}
        lower(c.table_schema) = lower('{{ schema }}')
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
      )
  {% endif %}
{%- endmacro %}

{% macro singlestore__get_catalog_relations_where_clause_sql(relations) -%}
  {% if relations | length == 0 %}
    where 1 = 0
  {% else %}
    where c.table_schema not in ('information_schema')
      and (
      {%- for relation in relations -%}
        {% if not relation.schema %}
          {% do exceptions.raise_compiler_error(
            '`get_catalog_relations` requires a list of relations, each with a schema'
          ) %}
        {% endif %}

        (
          lower(c.table_schema) = lower('{{ relation.schema }}')
          and lower(c.table_name) = lower('{{ relation.identifier }}')
        )
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
      )
  {% endif %}
{%- endmacro %}
