
{% macro duckdb__get_catalog(information_schema, schemas) -%}
  {% set query %}
    with tables as (
      {{ duckdb__get_catalog_tables_sql(information_schema) }}
      {{ duckdb__get_catalog_schemas_where_clause_sql(schemas) }}
    ),
    columns as (
      {{ duckdb__get_catalog_columns_sql(information_schema) }}
      {{ duckdb__get_catalog_schemas_where_clause_sql(schemas) }}
    )
    {{ duckdb__get_catalog_results_sql() }}
  {% endset %}

  {{ return(run_query(query)) }}
{%- endmacro %}


{% macro duckdb__get_catalog_relations(information_schema, relations) -%}
  {% set query %}
    with tables as (
      {{ duckdb__get_catalog_tables_sql(information_schema) }}
      {{ duckdb__get_catalog_relations_where_clause_sql(relations) }}
    ),
    columns as (
      {{ duckdb__get_catalog_columns_sql(information_schema) }}
      {{ duckdb__get_catalog_relations_where_clause_sql(relations) }}
    )
    {{ duckdb__get_catalog_results_sql() }}
  {% endset %}

  {{ return(run_query(query)) }}
{%- endmacro %}


{% macro duckdb__get_catalog_tables_sql(information_schema) -%}
    select *
    from (
        select
            t.database_name as table_database,
            t.schema_name as table_schema,
            t.table_name as table_name,
            'BASE TABLE' as table_type,
            t.comment as table_comment
        from duckdb_tables() t
        where upper(t.database_name) = upper('{{ information_schema.database }}')

        union all

        select
            v.database_name as table_database,
            v.schema_name as table_schema,
            v.view_name as table_name,
            'VIEW' as table_type,
            v.comment as table_comment
        from duckdb_views() v
        where upper(v.database_name) = upper('{{ information_schema.database }}')
    ) catalog_tables
{%- endmacro %}


{% macro duckdb__get_catalog_columns_sql(information_schema) -%}
    select *
    from (
        select
            c.database_name as table_database,
            c.schema_name as table_schema,
            c.table_name as table_name,
            c.column_name as column_name,
            c.column_index as column_index,
            c.data_type as column_type,
            c.comment as column_comment
        from duckdb_columns() c
        where upper(c.database_name) = upper('{{ information_schema.database }}')
    ) catalog_columns
{%- endmacro %}


{% macro duckdb__get_catalog_results_sql() -%}
    select
        tables.table_database,
        tables.table_schema,
        tables.table_name,
        tables.table_type,
        tables.table_comment,
        columns.column_name,
        columns.column_index,
        columns.column_type,
        columns.column_comment,
        cast(null as varchar) as table_owner
    from tables
    join columns using (table_database, table_schema, table_name)
    order by table_schema, table_name, column_index
{%- endmacro %}


{% macro duckdb__get_catalog_schemas_where_clause_sql(schemas) -%}
    {% if schemas | length == 0 %}
        where 1 = 0
    {% else %}
        where (
            {%- for schema in schemas -%}
                upper(table_schema) = upper('{{ schema }}'){% if not loop.last %} or {% endif %}
            {%- endfor -%}
        )
    {% endif %}
{%- endmacro %}


{% macro duckdb__get_catalog_relations_where_clause_sql(relations) -%}
    {% if relations | length == 0 %}
        where 1 = 0
    {% else %}
        where (
            {%- for relation in relations -%}
                {% if not relation.schema %}
                    {% do exceptions.raise_compiler_error(
                        '`get_catalog_relations` requires a list of relations, each with a schema'
                    ) %}
                {% endif %}

                (
                    upper(table_schema) = upper('{{ relation.schema }}')
                    {%- if relation.identifier %}
                    and upper(table_name) = upper('{{ relation.identifier }}')
                    {%- endif %}
                ){% if not loop.last %} or {% endif %}
            {%- endfor -%}
        )
    {% endif %}
{%- endmacro %}
