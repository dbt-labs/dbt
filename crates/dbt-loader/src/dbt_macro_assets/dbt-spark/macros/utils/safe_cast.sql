-- funcsign: (string, string) -> string
{% macro spark__safe_cast(field, type) %}
{%- set field_clean = field.strip('"').strip("'") if (cast_from_string_unsupported_for(type) and field is string) else field -%}
cast({{field_clean}} as {{type}})
{% endmacro %}

-- funcsign: (string) -> bool
{% macro cast_from_string_unsupported_for(type) %}
    {{ return(type.lower().startswith('struct') or type.lower().startswith('array') or type.lower().startswith('map')) }}
{% endmacro %}
