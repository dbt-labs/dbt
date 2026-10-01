{% macro postgres__get_replace_view_sql(relation, sql) -%}

    {%- set sql_header = config.get('sql_header', none) -%}
    {{ sql_header if sql_header is not none }}

    create or replace view {{ relation }}
        {% set contract_config = config.get('contract') %}
        {% if contract_config.enforced %}
            {{ get_assert_columns_equivalent(sql) }}
        {%- endif %}
    as (
        {{ sql }}
    );

{%- endmacro %}
