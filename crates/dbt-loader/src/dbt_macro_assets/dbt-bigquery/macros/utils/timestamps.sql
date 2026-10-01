{% macro bigquery__current_timestamp() -%}
  current_timestamp()
{%- endmacro %}

{% macro bigquery__snapshot_string_as_time(timestamp) -%}
    {%- set result = 'TIMESTAMP("' ~ timestamp ~ '")' -%}
    {{ return(result) }}
{%- endmacro %}

{% macro bigquery__current_timestamp_backcompat() -%}
  current_timestamp
{%- endmacro %}
