{% macro duckdb__split_part(string_text, delimiter_text, part_number) %}
    string_split({{ string_text }}, {{ delimiter_text }})[ {{ part_number }} ]
{% endmacro %}
