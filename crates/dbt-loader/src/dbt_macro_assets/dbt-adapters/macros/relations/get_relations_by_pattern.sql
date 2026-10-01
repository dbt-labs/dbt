-- This is ported from the override version of get_relations_by_pattern in tests/data/internal-analytics
-- with var('single-tenant-exclusions') usage being extracted to a parameter - excluded_schemas
-- funcsign: (string, string, string, string, boolean, list[ANY]) -> list[ANY]
{% macro get_relations_by_pattern_internal(schema_pattern, table_pattern, exclude='', database=target.database, quote_table=False, excluded_schemas=[]) %}
    {{ return(adapter.dispatch('get_relations_by_pattern_internal')(schema_pattern, table_pattern, exclude, database, quote_table, excluded_schemas)) }}
{% endmacro %}

-- funcsign: (string, string, string, string, boolean, list[ANY]) -> list[ANY]
{% macro default__get_relations_by_pattern_internal(schema_pattern, table_pattern, exclude='', database=target.database, quote_table=False, excluded_schemas=[]) %}

    {%- call statement('get_tables', fetch_result=True) %}

        {{ adapter.dispatch('get_tables_by_pattern_sql')(schema_pattern, table_pattern, exclude, database) }}

    {%- endcall -%}

    {%- set table_list = load_result('get_tables') -%}

    {%- if table_list and table_list['table'] -%}
        {%- set tbl_relations = [] -%}
        {%- for row in table_list['table'] -%}
            {% if row.table_schema not in excluded_schemas %}
                {% if quote_table %}
                {% set table_name = '"' ~ row.table_name ~ '"' %}
                {% else %}
                {% set table_name = row.table_name %}
                {% endif %}
                {%- set tbl_relation = api.Relation.create(
                    database=database,
                    schema=row.table_schema,
                    identifier=table_name,
                    type=row.table_type
                ) -%}
                {%- do tbl_relations.append(tbl_relation) -%}
            {% endif %}
        {%- endfor -%}

        {{ return(tbl_relations) }}
    {%- else -%}
        {{ return([]) }}
    {%- endif -%}

{% endmacro %}