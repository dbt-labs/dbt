
-- funcsign: (string, string) -> string
{% macro salesforce__safe_cast(field, type) %}
    try_cast({{field}} as {{type}})
{% endmacro %}
