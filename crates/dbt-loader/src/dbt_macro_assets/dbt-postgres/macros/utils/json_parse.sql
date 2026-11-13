{% macro postgres__json_parse(string, string_path) -%}

    {{ string }}::json #>> '{ {%- for s in string_path -%}{{ s }}{%- if not loop.last -%},{%- endif -%}{%- endfor -%} }'

{%- endmacro %}
