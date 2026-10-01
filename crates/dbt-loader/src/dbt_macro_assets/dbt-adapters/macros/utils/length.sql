{% macro length(expression) -%}
    {{ return(adapter.dispatch('length', 'dbt') (expression)) }}
{% endmacro %}

{% macro default__length(expression) %}

    length(
        {{ expression }}
    )

{%- endmacro -%}
