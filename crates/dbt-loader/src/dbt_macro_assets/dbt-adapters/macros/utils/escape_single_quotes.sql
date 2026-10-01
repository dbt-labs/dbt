-- funcsign: (string) -> string
{% macro escape_single_quotes(expression) %}
      {{ return(adapter.dispatch('escape_single_quotes', 'dbt') (expression)) }}
{% endmacro %}

{# /*Default to replacing a single apostrophe with two apostrophes: they're -> they''re*/ #}
-- funcsign: (string) -> string
{% macro default__escape_single_quotes(expression) -%}
{{ expression | replace("'","''") }}
{%- endmacro %}
