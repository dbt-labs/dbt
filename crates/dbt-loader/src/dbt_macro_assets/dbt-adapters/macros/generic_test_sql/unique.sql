{% macro default__test_unique(model, column_name) %}

select
    {{ column_name }} as unique_field,
    count(*) as n_records

from {{ model }}
where {{ column_name }} is not null
group by {{ column_name }}
having count(*) > 1

{% endmacro %}
