{% set src_schema = t_schema_name() + '_sources' %}
{% set relation = source(src_schema, 'source') %}
{% set columns = adapter.get_columns_in_relation(relation) %}
{% for column in columns %}
    {% do print(column) %}
{% endfor %}

select 1 as id