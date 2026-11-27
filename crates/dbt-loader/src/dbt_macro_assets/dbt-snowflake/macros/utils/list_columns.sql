-- funcsign: (list[base_column]) -> string
{% macro snowflake__list_columns(columns) -%}
{#- A parenthesized column list with types, for DDL: `("ID" NUMBER(38,0), "NAME" VARCHAR)`. -#}
    ({%- for column in columns -%}
        {{ column.quoted }} {{ column.data_type }}{% if not loop.last %}, {% endif %}
    {%- endfor -%})
{%- endmacro %}
