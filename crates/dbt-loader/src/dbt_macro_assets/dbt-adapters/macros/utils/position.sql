{% macro position(substring_text, string_text) -%}
    {{ return(adapter.dispatch('position', 'dbt') (substring_text, string_text)) }}
{% endmacro %}

{% macro default__position(substring_text, string_text) %}

    position(
        {{ substring_text }} in {{ string_text }}
    )

{%- endmacro -%}
