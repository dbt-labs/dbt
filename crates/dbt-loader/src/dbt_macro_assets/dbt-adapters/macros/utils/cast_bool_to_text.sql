{% macro cast_bool_to_text(field) %}
  {{ adapter.dispatch('cast_bool_to_text', 'dbt') (field) }}
{% endmacro %}

{% macro default__cast_bool_to_text(field) %}
    cast({{ field }} as {{ api.Column.translate_type('string') }})
{% endmacro %}
