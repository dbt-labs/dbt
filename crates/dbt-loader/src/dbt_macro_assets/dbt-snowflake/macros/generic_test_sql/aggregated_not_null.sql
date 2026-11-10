{% macro snowflake__test_aggregated_not_null(model, column_names) %}
{% set skip_column_names = aggregated_test_skip_column_names | default([]) %}
{% set filtered_columns = [] %}

{% for column_name in column_names %}
    {% if column_name not in skip_column_names %}
        {% do filtered_columns.append(column_name) %}
    {% endif %}
{% endfor %}
{#- Unpivot emits the source identifier, so alias the counts positionally and map back
    to the column text below; upper case matches whether Snowflake folds it or not. #}
{#- One scan of the model produces every column's null count; unpivot then reshapes
    that single row into one row per tested column. #}
{%- if filtered_columns -%}
select
    case upper(agg_column_name)
    {%- for column_name in filtered_columns %}
        when 'AGG_COL_{{ loop.index0 }}' then {{ dbt.string_literal(column_name) }}
    {%- endfor %}
    end as column_name,
    agg_failures as failures
from (
    select
    {%- for column_name in filtered_columns %}
        count(*) - count({{ column_name }}) as "AGG_COL_{{ loop.index0 }}"{{ "," if not loop.last }}
    {%- endfor %}
    from {{ model }}
) null_counts
{#- agg_ prefixes keep unpivot's own outputs from colliding with a tested column
    that is itself named failures or column_name. #}
unpivot (agg_failures for agg_column_name in (
    {%- for column_name in filtered_columns -%}
    "AGG_COL_{{ loop.index0 }}"{{ ", " if not loop.last }}
    {%- endfor -%}
))
where agg_failures > 0
order by column_name
{%- else -%}
select
    cast(null as {{ dbt.type_string() }}) as column_name,
    cast(null as {{ dbt.type_int() }}) as failures
where 1 = 0
{%- endif %}
{% endmacro %}
