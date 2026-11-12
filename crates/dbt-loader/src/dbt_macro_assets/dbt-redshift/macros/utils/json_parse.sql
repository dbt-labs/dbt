{#- Returns null for invalid JSON instead of raising an error. -#}
{% macro redshift__json_parse(string, string_path) -%}

    case when is_valid_json({{ string }})
        then json_extract_path_text({{ string }}, {%- for s in string_path -%}'{{ s }}'{%- if not loop.last -%},{%- endif -%}{%- endfor -%})
        else null
    end

{%- endmacro %}
