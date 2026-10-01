{% macro bool_or(expression) -%}
    {{ return(adapter.dispatch('bool_or', 'dbt') (expression)) }}
{% endmacro %}

{% macro default__bool_or(expression) -%}

    bool_or({{ expression }})

{%- endmacro %}
