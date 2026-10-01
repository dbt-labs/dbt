{% macro snowflake__array_construct(inputs, data_type) -%}
    array_construct( {{ inputs|join(' , ') }} )
{%- endmacro %}
