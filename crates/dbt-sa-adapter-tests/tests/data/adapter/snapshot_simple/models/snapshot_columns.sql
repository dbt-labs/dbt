{% set columns = adapter.get_columns_in_relation(ref("base_snapshot")) %}

select
{% for column in columns %}
    '{{ column.name }}' as column_{{ column.name }},
    '{{ column.data_type }}' as column_type_{{ column.name }},
{% endfor %}
