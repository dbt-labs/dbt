{% macro exasol__any_value(expression) -%}
    {#- Exasol has no ANY_VALUE aggregate; min() returns one deterministic value
        per group, which satisfies any_value's "any one value" contract. -#}
    min({{ expression }})
{%- endmacro %}
