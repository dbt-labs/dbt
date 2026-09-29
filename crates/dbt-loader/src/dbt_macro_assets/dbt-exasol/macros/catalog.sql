{#- Catalog queries for `docs generate`. Aliases are quoted lowercase because
    ExasolMetadataAdapter::build_schemas_from_stats_sql /
    build_columns_from_get_columns look up lowercase column names in the
    returned RecordBatch. column_index is cast to DECIMAL(36,0) (Exasol's max
    precision) so it arrives as a Decimal128 Arrow column. -#}

{% macro exasol__get_catalog_tables_sql(information_schema) -%}
    select
        '{{ information_schema.database }}' as "table_database",
        root_name as "table_schema",
        object_name as "table_name",
        root_name as "table_owner",
        object_type as "table_type",
        object_comment as "table_comment"
    from sys.exa_user_objects
    where object_type in ('TABLE', 'VIEW')
{%- endmacro %}

{% macro exasol__get_catalog_columns_sql(information_schema) -%}
    select
        '{{ information_schema.database }}' as "table_database",
        column_schema as "table_schema",
        column_table as "table_name",
        column_name as "column_name",
        cast(column_ordinal_position as decimal(36,0)) as "column_index",
        column_type as "column_type",
        column_comment as "column_comment"
    from sys.exa_user_columns
{%- endmacro %}

{% macro exasol__get_catalog_results_sql() -%}
    select
        tables."table_database",
        tables."table_schema",
        tables."table_name",
        tables."table_owner",
        tables."table_type",
        tables."table_comment",
        columns."column_name",
        columns."column_index",
        columns."column_type",
        columns."column_comment"
    from tables
    join columns
      on tables."table_schema" = columns."table_schema"
     and tables."table_name" = columns."table_name"
{%- endmacro %}

{% macro exasol__get_catalog(information_schema, schemas) -%}
    {% set query %}
        with tables as (
            {{ exasol__get_catalog_tables_sql(information_schema) }}
        ),
        columns as (
            {{ exasol__get_catalog_columns_sql(information_schema) }}
        )
        {{ exasol__get_catalog_results_sql() }}
        where (
        {%- for schema in schemas -%}
            upper(tables."table_schema") = upper('{{ schema }}'){%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
        )
        order by tables."table_schema", tables."table_name", columns."column_index"
    {%- endset -%}

    {{ return(run_query(query)) }}
{%- endmacro %}

{% macro exasol__get_catalog_relations(information_schema, relations) -%}
    {% set query %}
        with tables as (
            {{ exasol__get_catalog_tables_sql(information_schema) }}
        ),
        columns as (
            {{ exasol__get_catalog_columns_sql(information_schema) }}
        )
        {{ exasol__get_catalog_results_sql() }}
        where (
        {%- for relation in relations -%}
            {%- if relation.schema and relation.identifier -%}
                (upper(tables."table_schema") = upper('{{ relation.schema }}')
                 and upper(tables."table_name") = upper('{{ relation.identifier }}'))
            {%- elif relation.schema -%}
                (upper(tables."table_schema") = upper('{{ relation.schema }}'))
            {%- else -%}
                {% do exceptions.raise_compiler_error(
                    '`get_catalog_relations` requires a list of relations, each with a schema'
                ) %}
            {%- endif -%}
            {%- if not loop.last %} or {% endif -%}
        {%- endfor -%}
        )
        order by tables."table_schema", tables."table_name", columns."column_index"
    {%- endset -%}

    {{ return(run_query(query)) }}
{%- endmacro %}
