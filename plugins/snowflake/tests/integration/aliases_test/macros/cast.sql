

{% macro string_literal(s) -%}
  {{ adapter.dispatch('string_literal', macro_namespace='test')(s) }}
{%- endmacro %}

{% macro default__string_literal(s) %}
    '{{ s }}'::text
{% endmacro %}

{% macro bigquery__string_literal(s) %}
    cast('{{ s }}' as string)
{% endmacro %}
