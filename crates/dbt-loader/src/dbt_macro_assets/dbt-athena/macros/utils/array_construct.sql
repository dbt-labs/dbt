{% macro athena__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    array[ {{ inputs|join(' , ') }} ]
    {% else %}
    {{ safe_cast('array[]', 'array(' ~ data_type ~ ')') }}
    {% endif %}
{%- endmacro %}
