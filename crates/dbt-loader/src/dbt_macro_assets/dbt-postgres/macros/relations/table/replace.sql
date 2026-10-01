{% macro postgres__get_replace_table_sql(relation, sql) -%}

    {%- set sql_header = config.get('sql_header', none) -%}
    {{ sql_header if sql_header is not none }}

    create or replace table {{ relation }}
        {% set contract_config = config.get('contract') %}
        {% if contract_config.enforced %}
            {{ get_assert_columns_equivalent(sql) }}
            {{ get_table_columns_and_constraints() }}
            {%- set sql = get_select_subquery(sql) %}
        {% endif %}
    as (
        {{ sql }}
    );

{%- endmacro %}
