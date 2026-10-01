{% macro clickhouse__get_catalog(information_schema, schemas) -%}
  {%- call statement('catalog', fetch_result=True) -%}
    {{ clickhouse__get_catalog_results_sql(clickhouse__get_catalog_schemas_where_clause_sql(schemas)) }}
  {%- endcall -%}
  {{ return(load_result('catalog').table) }}
{%- endmacro %}

{% macro clickhouse__get_catalog_relations(information_schema, relations) -%}
  {%- call statement('catalog', fetch_result=True) -%}
    {{ clickhouse__get_catalog_results_sql(clickhouse__get_catalog_relations_where_clause_sql(relations)) }}
  {%- endcall -%}
  {{ return(load_result('catalog').table) }}
{%- endmacro %}

{% macro clickhouse__get_catalog_results_sql(where_clause) -%}
    select
      '' as table_database,
      columns.database as table_schema,
      columns.table as table_name,
      if(tables.engine not in ('MaterializedView', 'View'), 'table', 'view') as table_type,
      nullIf(tables.comment, '') as table_comment,
      columns.name as column_name,
      columns.position as column_index,
      columns.type as column_type,
      nullIf(columns.comment, '') as column_comment,
      cast(null as Nullable(String)) as table_owner
    from system.columns as columns
    join system.tables as tables on tables.database = columns.database and tables.name = columns.table
    {{ where_clause }}
    order by columns.database, columns.table, columns.position
{%- endmacro %}

{% macro clickhouse__get_catalog_schemas_where_clause_sql(schemas) -%}
  {% if schemas | length == 0 %}
    where 1 = 0
  {% else %}
    where columns.database != 'system'
      and (
      {%- for schema in schemas -%}
        columns.database = '{{ schema }}'
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
      )
  {% endif %}
{%- endmacro %}

{% macro clickhouse__get_catalog_relations_where_clause_sql(relations) -%}
  {% if relations | length == 0 %}
    where 1 = 0
  {% else %}
    where columns.database != 'system'
      and (
      {%- for relation in relations -%}
        {% if not relation.schema %}
          {% do exceptions.raise_compiler_error(
            '`get_catalog_relations` requires a list of relations, each with a schema'
          ) %}
        {% endif %}

        (
          columns.database = '{{ relation.schema }}'
          {%- if relation.identifier %}
          and columns.table = '{{ relation.identifier }}'
          {%- endif %}
        )
        {%- if not loop.last %} or {% endif -%}
      {%- endfor -%}
      )
  {% endif %}
{%- endmacro %}
