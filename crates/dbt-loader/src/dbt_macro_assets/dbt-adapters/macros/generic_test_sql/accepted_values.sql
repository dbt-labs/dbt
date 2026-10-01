-- funcsign: (model, string, list[string], bool) -> string
{% macro default__test_accepted_values(model, column_name, values, quote=True) %}

with all_values as (

    select
        {{ column_name }} as value_field,
        count(*) as n_records

    from {{ model }}
    group by {{ column_name }}

)

select *
from all_values
where value_field not in (
    {% for value in values -%}
        {% if quote -%}
        '{{ value }}'
        {%- else -%}
        {{ value }}
        {%- endif -%}
        {%- if not loop.last -%},{%- endif %}
    {%- endfor %}
)

{% endmacro %}
