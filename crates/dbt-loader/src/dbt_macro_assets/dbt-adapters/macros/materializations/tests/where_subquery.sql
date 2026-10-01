{% macro get_where_subquery(relation) -%}
    {% do return(adapter.dispatch('get_where_subquery', 'dbt')(relation)) %}
{%- endmacro %}

-- funcsign: (relation) -> string
{% macro default__get_where_subquery(relation) -%}
    {% set where = config.get('where', '') %}
    {% if where %}
        {%- set filtered -%}
            (select * from {{ relation }} where {{ where }}) dbt_subquery
        {%- endset -%}
        {% do return(filtered) %}
    {%- else -%}
        {% do return(relation) %}
    {%- endif -%}
{%- endmacro %}
