{% macro exasol__bool_or(expression) -%}
    {# Exasol spells the boolean OR-aggregate as ANY(). #}
    any({{ expression }})
{%- endmacro %}
