{% macro clickhouse__current_timestamp() -%}
  now()
{%- endmacro %}

{% macro clickhouse__snapshot_string_as_time(timestamp) -%}
  {%- set result = "toDateTime('" ~ timestamp ~ "')" -%}
  {{ return(result) }}
{%- endmacro %}
