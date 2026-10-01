{% macro fabric__position(substring_text, string_text) %}

    CHARINDEX(
        {{ substring_text }},
        {{ string_text }}
    )

{%- endmacro -%}
