-- funcsign: (relation, list[string]) -> string
{% test aggregated_unique(model, column_names) %}
    {% set macro = adapter.dispatch('test_aggregated_unique', 'dbt') %}
    {{ macro(model, column_names) }}
{% endtest %}

-- funcsign: (relation, list[string]) -> string
{% macro default__test_aggregated_unique(model, column_names) %}

{% set skip_column_names = aggregated_test_skip_column_names | default([]) %}

{% set filtered_columns = [] %}

{% for column_name in column_names %}
    {% if column_name not in skip_column_names %}
        {% do filtered_columns.append(column_name) %}
    {% endif %}
{% endfor %}

{% set union_queries = [] %}

{#- Each duplicated value is one failure. #}
{% for column_name in filtered_columns %}
    {% set query %}
    select
        {{ dbt.string_literal(column_name) }} as column_name,
        {{ dbt.aggregated_unique_field(column_name) }} as unique_field,
        count(*) as n_records,
        1 as failures
    from {{ model }}
    where {{ column_name }} is not null
    group by {{ column_name }}
    having count(*) > 1
    {% endset %}

    {% do union_queries.append(query) %}
{% endfor %}

{% if union_queries %}
    {{ union_queries | join('\nunion all\n') }}
    order by column_name, n_records desc
{% else %}
    select
        cast(null as {{ dbt.type_string() }}) as column_name,
        cast(null as {{ dbt.type_string() }}) as unique_field,
        cast(null as {{ dbt.type_int() }}) as n_records,
        cast(null as {{ dbt.type_int() }}) as failures
    where 1=0
{% endif %}

{% endmacro %}

-- funcsign: (string) -> string
{% macro aggregated_unique_field(column_name) %}
  {{ return(adapter.dispatch('aggregated_unique_field', 'dbt')(column_name)) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro default__aggregated_unique_field(column_name) %}
  {{ dbt.safe_cast(column_name, dbt.type_string()) }}
{% endmacro %}
