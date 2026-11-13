{% macro trino__current_timestamp() -%}
    current_timestamp
{%- endmacro %}

{% macro trino__snapshot_string_as_time(timestamp) %}
    {%- set result = "timestamp '" ~ timestamp ~ "'" -%}
    {{ return(result) }}
{% endmacro %}
