{% macro trino__array_construct(inputs, data_type) -%}
    {%- if not inputs -%}
    null
    {%- else -%}
    array[ {{ inputs|join(' , ') }} ]
    {%- endif -%}
{%- endmacro %}
