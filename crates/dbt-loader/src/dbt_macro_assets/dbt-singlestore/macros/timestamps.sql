{% macro singlestore__current_timestamp() -%}
    current_timestamp()
{%- endmacro %}

{% macro singlestore__current_timestamp_backcompat() %}
    current_timestamp()
{% endmacro %}

{% macro singlestore__current_timestamp_in_utc_backcompat() %}
    utc_timestamp()
{% endmacro %}

{% macro singlestore__snapshot_get_time() -%}
    current_timestamp()
{%- endmacro %}

{% macro singlestore__snapshot_string_as_time(timestamp) -%}
    {%- set result = "'" ~ timestamp ~ "'" -%}
    {{ return(result) }}
{%- endmacro %}
