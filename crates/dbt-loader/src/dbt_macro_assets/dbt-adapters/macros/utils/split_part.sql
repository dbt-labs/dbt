-- funcsign: (string, string, int) -> string
{% macro split_part(string_text, delimiter_text, part_number) %}
  {{ return(adapter.dispatch('split_part', 'dbt') (string_text, delimiter_text, part_number)) }}
{% endmacro %}

-- funcsign: (string, string, int) -> string
{% macro default__split_part(string_text, delimiter_text, part_number) %}

    split_part(
        {{ string_text }},
        {{ delimiter_text }},
        {{ part_number }}
        )

{% endmacro %}

-- funcsign: (string, string, int) -> string
{% macro _split_part_negative(string_text, delimiter_text, part_number) %}

    split_part(
        {{ string_text }},
        {{ delimiter_text }},
          length({{ string_text }})
          - length(
              replace({{ string_text }},  {{ delimiter_text }}, '')
          ) + 2 + {{ part_number }}
        )

{% endmacro %}
