{% macro bigquery__split_part(string_text, delimiter_text, part_number) %}

  {% if part_number >= 0 %}
    split(
        {{ string_text }},
        {{ delimiter_text }}
        )[safe_offset({{ part_number - 1 }})]
  {% else %}
    split(
        {{ string_text }},
        {{ delimiter_text }}
        )[safe_offset(
          length({{ string_text }})
          - length(
              replace({{ string_text }},  {{ delimiter_text }}, '')
          ) + 1 + {{ part_number }}
        )]
  {% endif %}

{% endmacro %}
