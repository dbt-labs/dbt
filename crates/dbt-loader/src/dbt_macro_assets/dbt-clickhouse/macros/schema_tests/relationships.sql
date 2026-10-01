{% macro clickhouse__test_relationships(model, column_name, to, field) %}

with child as (
    select {{ column_name }} as from_field
    from {{ model }}
    where {{ column_name }} is not null
),

parent as (
    select {{ field }} as to_field
    from {{ to }}
)

select
    from_field

from child
left join parent
    on child.from_field = parent.to_field

where parent.to_field is null
-- end_of_sql
settings join_use_nulls = 1

{% endmacro %}
