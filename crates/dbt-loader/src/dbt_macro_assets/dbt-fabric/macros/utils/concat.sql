{% macro fabric__concat(fields) -%}
    {%- if fields|length < 2 -%}
        {{ fields[0] }}
    {%- else -%}
        concat({{ fields|join(', ') }})
    {%- endif -%}
{%- endmacro %}
