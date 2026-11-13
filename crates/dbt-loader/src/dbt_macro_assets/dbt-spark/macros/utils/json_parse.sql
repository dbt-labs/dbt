{#- get_json_object works on both Apache Spark and Databricks and returns null for invalid JSON. -#}
{% macro spark__json_parse(string, string_path) -%}

    get_json_object({{ string }}, '${%- for s in string_path -%}{% if s is number %}[{{ s }}]{% else %}.{{ s }}{% endif %}{%- endfor -%}')

{%- endmacro %}
