{% macro bigquery__resolve_model_name(input_model_name) -%}
    {{ input_model_name | string | replace('`', '') | replace('"', '\"') }}
{%- endmacro -%}
