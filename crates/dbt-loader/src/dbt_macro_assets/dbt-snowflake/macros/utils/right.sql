{% macro snowflake__right(string_text, length_expression) %}

    case when {{ length_expression }} = 0
        then ''
    else
        right(
            {{ string_text }},
            {{ length_expression }}
        )
    end

{%- endmacro -%}
