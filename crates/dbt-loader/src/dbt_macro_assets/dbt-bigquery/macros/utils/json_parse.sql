{% macro bigquery__json_parse(string, string_path) -%}

    json_extract_scalar({{ string }}, '${%- for s in string_path -%}{% if s is number %}[{{ s }}]{% else %}.{{ s }}{% endif %}{%- endfor -%}')

{%- endmacro %}
