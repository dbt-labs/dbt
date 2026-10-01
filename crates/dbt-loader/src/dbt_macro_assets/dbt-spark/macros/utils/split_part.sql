{% macro spark__split_part(string_text, delimiter_text, part_number) %}

    {% set delimiter_expr %}

        -- escape if starts with a special character
        case when regexp_extract({{ delimiter_text }}, '([^A-Za-z0-9])(.*)', 1) != '_'
            then concat('\\', {{ delimiter_text }})
            else {{ delimiter_text }} end

    {% endset %}

    {% if part_number >= 0 %}

        {% set split_part_expr %}

        split(
            {{ string_text }},
            {{ delimiter_expr }}
            )[({{ part_number - 1 if part_number > 0 else part_number }})]

        {% endset %}

    {% else %}

        {% set split_part_expr %}

        split(
            {{ string_text }},
            {{ delimiter_expr }}
            )[(
                length({{ string_text }})
                - length(
                    replace({{ string_text }},  {{ delimiter_text }}, '')
                ) + 1 + {{ part_number }}
            )]

        {% endset %}

    {% endif %}

    {{ return(split_part_expr) }}

{% endmacro %}
