-- funcsign: (string, string) -> string
{% macro bigquery__safe_cast(field, type) %}
{%- if type.lower().startswith('array') and field is iterable and (field is not string and field is not mapping) and field | length > 0 -%}
    {#-- Extract nested type from 'array<nested_type>' --#}
    {% set nested_type = type.lower()[6:-1] %}
    {#-- BigQuery does not support direct casts to arrays. instead, each element must be cast individually + reaggregated into an array --#}
    {%- if cast_from_string_unsupported_for(nested_type)  %}
        (select array_agg(safe_cast(i as {{ nested_type }})) from unnest([
            {%- for nested_field in field %}
                {{ nested_field.strip('"').strip("'") }}{{ ',' if not loop.last }}
            {%- endfor %}
        ]) i)
    {%- else -%}
        (select array_agg(safe_cast(i as {{nested_type}})) from unnest({{field}}) i)
    {%- endif -%}

{%- elif type.lower() == 'json' and field is mapping -%}
    safe_cast(json {{ dbt.string_literal(tojson(field)) }} as json)
{%- elif cast_from_string_unsupported_for(type) and field is string -%}
    safe_cast({{field.strip('"').strip("'")}} as {{type}})
{%- else -%}
    safe_cast({{field}} as {{type}})
{%- endif -%}
{% endmacro %}

-- funcsign: (string) -> bool
{% macro cast_from_string_unsupported_for(type) %}
    {{ return(type.lower().startswith('struct') or type.lower() == 'geography') }}
{% endmacro %}
