-- funcsign: (list[any], string) -> list[any]
{% macro array_construct(inputs=[], data_type=api.Column.translate_type('integer')) -%}
  {{ return(adapter.dispatch('array_construct', 'dbt')(inputs, data_type)) }}
{%- endmacro %}

{# all inputs must be the same data type to match postgres functionality #}
-- funcsign: (list[any], string) -> list[any]
{% macro default__array_construct(inputs, data_type) -%}
    {% if inputs|length > 0 %}
    array[ {{ inputs|join(' , ') }} ]
    {% else %}
    array[]::{{data_type}}[]
    {% endif %}
{%- endmacro %}
