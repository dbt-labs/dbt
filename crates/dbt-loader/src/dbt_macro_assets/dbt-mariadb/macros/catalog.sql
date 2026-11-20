{#- Catalog queries for `docs generate`. Aliases are lowercase because
    GenericMetadataAdapter::build_schemas_from_stats_sql /
    build_columns_from_get_columns look up lowercase column names in the
    returned RecordBatch. column_index is cast to DECIMAL(36,0) so it arrives
    as a Decimal128 Arrow column. -#}

{% macro mariadb__get_catalog_tables_sql(information_schema) -%}
    select
        '' as table_database,
        table_schema as table_schema,
        table_name as table_name,
        '' as table_owner,
        case when table_type = 'VIEW' then 'VIEW' else 'BASE TABLE' end as table_type,
        case when table_type = 'VIEW' then '' else table_comment end as table_comment
    from information_schema.tables
    where table_type in ('BASE TABLE', 'SYSTEM VERSIONED', 'VIEW')
{%- endmacro %}

{% macro mariadb__get_catalog_columns_sql(information_schema) -%}
    select
        table_schema as table_schema,
        table_name as table_name,
        column_name as column_name,
        cast(ordinal_position as decimal(36,0)) as column_index,
        column_type as column_type,
        column_comment as column_comment
    from information_schema.columns
{%- endmacro %}

{% macro mariadb__get_catalog_results_sql() -%}
    select
        tables.table_database,
        tables.table_schema,
        tables.table_name,
        tables.table_owner,
        tables.table_type,
        tables.table_comment,
        columns.column_name,
        columns.column_index,
        columns.column_type,
        columns.column_comment
    from tables
    join columns
      on tables.table_schema = columns.table_schema
     and tables.table_name = columns.table_name
{%- endmacro %}

{% macro mariadb__get_catalog(information_schema, schemas) -%}
    {% set query %}
        with tables as (
            {{ mariadb__get_catalog_tables_sql(information_schema) }}
        ),
        columns as (
            {{ mariadb__get_catalog_columns_sql(information_schema) }}
        )
        {{ mariadb__get_catalog_results_sql() }}
        where (
        {%- for schema in schemas -%}
            tables.table_schema = '{{ schema | replace("\\", "\\\\") | replace("'", "''") }}'{%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
        )
        order by tables.table_schema, tables.table_name, columns.column_index
    {%- endset -%}

    {{ return(run_query(query)) }}
{%- endmacro %}

{% macro mariadb__get_catalog_relations(information_schema, relations) -%}
    {% set query %}
        with tables as (
            {{ mariadb__get_catalog_tables_sql(information_schema) }}
        ),
        columns as (
            {{ mariadb__get_catalog_columns_sql(information_schema) }}
        )
        {{ mariadb__get_catalog_results_sql() }}
        where (
        {%- for relation in relations -%}
            {%- if relation.schema and relation.identifier -%}
                (tables.table_schema = '{{ relation.schema | replace("\\", "\\\\") | replace("'", "''") }}'
                 and tables.table_name = '{{ relation.identifier | replace("\\", "\\\\") | replace("'", "''") }}')
            {%- elif relation.schema -%}
                (tables.table_schema = '{{ relation.schema | replace("\\", "\\\\") | replace("'", "''") }}')
            {%- else -%}
                {% do exceptions.raise_compiler_error(
                    '`get_catalog_relations` requires a list of relations, each with a schema'
                ) %}
            {%- endif -%}
            {%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
        )
        order by tables.table_schema, tables.table_name, columns.column_index
    {%- endset -%}

    {{ return(run_query(query)) }}
{%- endmacro %}
