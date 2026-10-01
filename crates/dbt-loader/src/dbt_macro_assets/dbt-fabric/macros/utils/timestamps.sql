{% macro fabric__current_timestamp() -%}
  CAST(SYSDATETIME() AS DATETIME2(6))
{%- endmacro %}

{% macro fabric__snapshot_string_as_time(timestamp) -%}
    {%- set result = "CONVERT(DATETIME2(6), '" ~ timestamp ~ "')" -%}
    {{ return(result) }}
{%- endmacro %}
