{#- Extracts a scalar value from a JSON string as text. `string_path` is a list of keys
    that leads to the value, so nested values can be reached. -#}
{% macro json_parse(string, string_path) -%}
    {{ return(adapter.dispatch('json_parse', 'dbt') (string, string_path)) }}
{% endmacro %}

{% macro default__json_parse(string, string_path) -%}

    json_extract_path_text({{ string }}, {%- for s in string_path -%}'{{ s }}'{%- if not loop.last -%},{%- endif -%}{%- endfor -%})

{%- endmacro %}
