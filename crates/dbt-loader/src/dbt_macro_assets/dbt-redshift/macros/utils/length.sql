{% macro redshift__length(expression) %}

    len(
        {{ expression }}
    )

{%- endmacro -%}
