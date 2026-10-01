{% macro bigquery__right(string_text, length_expression) %}

    case when {{ length_expression }} = 0
        then ''
    else
        substr(
            {{ string_text }},
            -1 * ({{ length_expression }})
        )
    end

{%- endmacro -%}
