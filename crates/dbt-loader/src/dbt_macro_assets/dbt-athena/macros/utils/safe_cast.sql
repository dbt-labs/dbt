-- TODO: make safe_cast supports complex structures
{% macro athena__safe_cast(field, type) -%}
    try_cast({{field}} as {{type}})
{%- endmacro %}
