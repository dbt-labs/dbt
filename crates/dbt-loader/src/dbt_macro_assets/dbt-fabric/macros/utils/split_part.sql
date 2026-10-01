{% macro fabric__split_part(string_text, delimiter_text, part_number) %}
    WITH SplitData AS (
        SELECT value,
        {% if part_number > 0 %}
            , ROW_NUMBER() OVER (ORDER BY ordinal ASC) AS forward_index
        {% else %}
            , ROW_NUMBER() OVER (ORDER BY ordinal DESC) AS backward_index
        {% endif %}
        FROM string_split({{ string_text }}, {{ delimiter_text }}, 1)
    )
    SELECT value
    FROM SplitData
    WHERE
    {% if part_number > 0 %}
        forward_index = {{ part_number }}
    {% else %}
        backward_index = {{ abs(part_number) }}
    {% endif %}
{% endmacro %}
