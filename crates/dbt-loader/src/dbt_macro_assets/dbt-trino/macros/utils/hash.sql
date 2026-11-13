{% macro trino__hash(field) -%}
    lower(to_hex(md5(to_utf8(cast({{field}} as varchar)))))
{%- endmacro %}
