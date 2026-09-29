-- funcsign: (relation, list[string]) -> string
{% test aggregated_not_null(model, column_names) %}
    {% set macro = adapter.dispatch('test_aggregated_not_null', 'dbt') %}
    {{ macro(model, column_names) }}
{% endtest %}

-- funcsign: (relation, list[string]) -> string
{% macro default__test_aggregated_not_null(model, column_names) %}
{% set skip_column_names = aggregated_test_skip_column_names | default([]) %}
{% set filtered_columns = [] %}

{% for column_name in column_names %}
    {% if column_name not in skip_column_names %}
        {% do filtered_columns.append(column_name) %}
    {% endif %}
{% endfor %}
{#- One scan of the model produces every column's null count; the cross join then
    pivots those counts into one row per tested column. #}
{%- if filtered_columns -%}
select
    column_name,
    failures
from (
    select
        test_columns.column_name as column_name,
        case test_columns.column_name
        {%- for column_name in filtered_columns %}
            when {{ dbt.string_literal(column_name) }} then null_counts.failures_{{ loop.index0 }}
        {%- endfor %}
        end as failures
    from (
        select
        {%- for column_name in filtered_columns %}
            count(*) - count({{ column_name }}) as failures_{{ loop.index0 }}{{ "," if not loop.last }}
        {%- endfor %}
        from {{ model }}
    ) null_counts
    cross join (
        {%- for column_name in filtered_columns %}
        select {{ dbt.string_literal(column_name) }} as column_name
        {%- if not loop.last %}
        union all
        {%- endif %}
        {%- endfor %}
    ) test_columns
) results
where failures > 0
order by column_name
{%- else -%}
select
    cast(null as {{ dbt.type_string() }}) as column_name,
    cast(null as {{ dbt.type_int() }}) as failures
where 1=0
{%- endif %}
{% endmacro %}
