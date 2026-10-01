-- funcsign: (string) -> string
{% macro any_value(expression) -%}
    {{ return(adapter.dispatch('any_value', 'dbt') (expression)) }}
{% endmacro %}

-- funcsign: (string) -> string
{% macro default__any_value(expression) -%}

    any_value({{ expression }})

{%- endmacro %}
