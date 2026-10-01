-- funcsign: (list[any], string) -> list[any]
{% macro salesforce__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    array[ {{ inputs|join(' , ') }} ]
    {% else %}
    array[]::array({{ data_type }})
    {% endif %}
{%- endmacro %}
