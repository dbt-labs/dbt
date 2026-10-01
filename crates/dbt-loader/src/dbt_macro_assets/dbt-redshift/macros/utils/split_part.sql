{% macro redshift__split_part(string_text, delimiter_text, part_number) %}

  {% if part_number >= 0 %}
    {{ dbt.default__split_part(string_text, delimiter_text, part_number) }}
  {% else %}
    {{ dbt._split_part_negative(string_text, delimiter_text, part_number) }}
  {% endif %}

{% endmacro %}
