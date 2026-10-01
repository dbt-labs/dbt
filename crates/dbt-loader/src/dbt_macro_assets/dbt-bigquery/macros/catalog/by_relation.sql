{# DIVERGENCE: parameter `information_schema` is now `dbschema` #}
{% macro bigquery__get_catalog_relations(db_schema, relations) -%}

    {%- if (relations | length) == 0 -%}
        {# Hopefully nothing cares about the columns we return when there are no rows #}
        {%- set query = "select 1 as id limit 0" -%}

    {%- else -%}
        {%- set query -%}
            with
                table_shards_stage as ({{ _bigquery__get_table_shards_sql(db_schema) }}),
                table_shards as (
                    select * from table_shards_stage
                    where (
                        {%- for relation in relations -%}
                            (
                                upper(table_schema) = upper('{{ relation.schema }}')
                            and upper(table_name) = upper('{{ relation.identifier }}')
                            )
                            {%- if not loop.last %} or {% endif -%}
                        {%- endfor -%}
                    )
                ),
                tables as ({{ _bigquery__get_tables_sql() }}),
                table_stats as ({{ _bigquery__get_table_stats_sql() }}),

                columns as ({{ _bigquery__get_columns_sql(db_schema) }}),
                column_stats as ({{ _bigquery__get_column_stats_sql() }})

            {{ _bigquery__get_extended_catalog_sql() }}
        {%- endset -%}

    {%- endif -%}

    {{ return(run_query(query)) }}

{%- endmacro %}
